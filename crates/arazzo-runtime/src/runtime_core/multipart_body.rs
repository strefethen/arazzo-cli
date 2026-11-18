use super::request_body::BodyNode;
use super::*;

pub(super) const CONTENT_TYPE: &str = "Content-Type";
const PAYLOAD: &str = "requestBody.payload";
const JSON_PART: &str = "application/json";
const BYTES_PART: &str = "application/octet-stream";
const UNREADABLE: &str =
    "request body is multipart/form-data, whose parts have no single value to read";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Form {
    content_type: String,
    parts: Vec<Part>,
    value: Value,
    blobs: BTreeMap<String, Blob>,
}

#[derive(Debug, Clone, PartialEq)]
struct Part {
    name: String,
    content: Content,
}

#[derive(Debug, Clone, PartialEq)]
enum Content {
    Text(String),
    Json(String),
    Blob(Blob),
}

pub(super) struct Wire {
    pub(super) content_type: String,
    pub(super) bytes: Vec<u8>,
}

impl Form {
    pub(super) fn new(
        content_type: &str,
        entries: &[(String, BodyNode)],
    ) -> Result<Self, RuntimeError> {
        let mut form = Self {
            content_type: content_type.to_owned(),
            parts: Vec::new(),
            value: Value::Null,
            blobs: BTreeMap::new(),
        };
        let mut logical = serde_json::Map::new();
        for (name, node) in entries {
            let source = payload::child_pointer("", name);
            let value = match node {
                BodyNode::Null => continue,
                BodyNode::Array(items) => {
                    let mut values = Vec::with_capacity(items.len());
                    for (index, item) in items.iter().enumerate() {
                        if matches!(item, BodyNode::Null) {
                            continue;
                        }
                        let logical = payload::child_pointer(&source, &values.len().to_string());
                        let source = payload::child_pointer(&source, &index.to_string());
                        values.push(form.part(name, item, &source, logical)?);
                    }
                    if values.is_empty() {
                        continue;
                    }
                    Value::Array(values)
                }
                node => form.part(name, node, &source, source.clone())?,
            };
            logical.insert(name.clone(), value);
        }
        form.value = Value::Object(logical);
        Ok(form)
    }

    fn part(
        &mut self,
        name: &str,
        node: &BodyNode,
        source: &str,
        logical: String,
    ) -> Result<Value, RuntimeError> {
        let (content, value) = match node {
            BodyNode::String(text) | BodyNode::Template(text) => {
                (Content::Text(text.clone()), Value::String(text.clone()))
            }
            BodyNode::Number(number) => (Content::Text(number.to_string()), json!(number)),
            BodyNode::Bool(flag) => (Content::Text(flag.to_string()), Value::Bool(*flag)),
            BodyNode::Blob(blob) => {
                if let Some(media_type) = blob
                    .media_type()
                    .filter(|media_type| media_type.chars().any(char::is_control))
                {
                    return Err(invalid(format!(
                        "{PAYLOAD}{source}: bytes media type {media_type:?} holds a control character, so it cannot be a part Content-Type"
                    )));
                }
                self.blobs.insert(logical, blob.clone());
                (Content::Blob(blob.clone()), Value::Null)
            }
            BodyNode::Null | BodyNode::Array(_) | BodyNode::Map(_) => {
                match node.clone().into_json() {
                    Ok(value) => (Content::Json(value.to_string()), value),
                    Err(_) => {
                        return Err(invalid(format!(
                            "{PAYLOAD}{source}{}: multipart/form-data part {name:?} is sent as {JSON_PART}, which cannot carry bytes; give the bytes a part of their own",
                            node.bytes_pointer().unwrap_or_default()
                        )))
                    }
                }
            }
        };
        self.parts.push(Part {
            name: name.to_owned(),
            content,
        });
        Ok(value)
    }

    pub(super) fn value(&self) -> &Value {
        &self.value
    }

    pub(super) fn content_type(&self) -> &str {
        &self.content_type
    }

    pub(super) fn descriptors(&self) -> BTreeMap<String, BytesDescriptor> {
        super::blob::descriptors(&self.blobs)
    }

    /// The body as sent under a fresh boundary; `None` when a part's bytes
    /// were restored from a trace and hold no content.
    pub(super) fn wire(&self) -> Result<Option<Wire>, RuntimeError> {
        let boundary = boundary()?;
        let mut bytes = Vec::new();
        for part in &self.parts {
            bytes.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            bytes.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{}\"",
                    escape(&part.name)
                )
                .as_bytes(),
            );
            let (media_type, content) = match &part.content {
                Content::Text(text) => (None, text.as_bytes()),
                Content::Json(text) => (Some(JSON_PART), text.as_bytes()),
                Content::Blob(blob) => {
                    let Some(content) = blob.content() else {
                        return Ok(None);
                    };
                    bytes.extend_from_slice(
                        format!(
                            "; filename=\"{}\"",
                            escape(blob.filename().unwrap_or(&part.name))
                        )
                        .as_bytes(),
                    );
                    (Some(blob.media_type().unwrap_or(BYTES_PART)), content)
                }
            };
            bytes.extend_from_slice(b"\r\n");
            if let Some(media_type) = media_type {
                bytes.extend_from_slice(format!("{CONTENT_TYPE}: {media_type}\r\n").as_bytes());
            }
            bytes.extend_from_slice(b"\r\n");
            bytes.extend_from_slice(content);
            bytes.extend_from_slice(b"\r\n");
        }
        bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        Ok(Some(Wire {
            content_type: format!("{}; boundary={boundary}", self.content_type),
            bytes,
        }))
    }
}

pub(super) fn unreadable() -> arazzo_expr::BodySlot {
    arazzo_expr::BodySlot::Unavailable {
        reason: UNREADABLE.to_owned(),
    }
}

pub(super) fn is_multipart(request: &TraceRequest) -> bool {
    request
        .headers
        .get(CONTENT_TYPE)
        .and_then(|content_type| arazzo_spec::media_type::MediaType::parse(content_type))
        .is_some_and(|media| media.is_multipart_form_data())
}

/// The dry-run projection of the parts: entries and items that send no part
/// are dropped, as they are from the wire.
pub(super) fn project(projection: Value) -> Value {
    let Value::Object(entries) = projection else {
        return projection;
    };
    Value::Object(
        entries
            .into_iter()
            .filter_map(|(name, value)| match value {
                Value::Null => None,
                Value::Array(items) => {
                    let items: Vec<Value> =
                        items.into_iter().filter(|item| !item.is_null()).collect();
                    (!items.is_empty()).then_some((name, Value::Array(items)))
                }
                value => Some((name, value)),
            })
            .collect(),
    )
}

fn escape(text: &str) -> String {
    text.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn boundary() -> Result<String, RuntimeError> {
    let mut random = [0u8; 16];
    getrandom::getrandom(&mut random).map_err(|error| {
        RuntimeError::new(
            RuntimeErrorKind::InternalError,
            format!("choosing a multipart boundary: {error}"),
        )
    })?;
    Ok(random
        .iter()
        .fold(String::from("arazzo-"), |mut boundary, byte| {
            boundary.push_str(&format!("{byte:02x}"));
            boundary
        }))
}

fn invalid(message: String) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::InvalidRequestBody, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(value: Value) -> Form {
        match BodyNode::from(value) {
            BodyNode::Map(entries) => {
                Form::new("multipart/form-data", &entries).unwrap_or_else(|error| panic!("{error}"))
            }
            node => panic!("not a map: {node:?}"),
        }
    }

    #[test]
    fn names_and_filenames_escape_quote_cr_and_lf_only() {
        assert_eq!(escape("a\"b\rc\nd é;="), "a%22b%0Dc%0Ad é;=");
    }

    #[test]
    fn each_wire_gets_a_fresh_boundary_named_only_in_its_content_type() {
        let form = form(json!({"caption": "cat"}));
        let wires: Vec<Wire> = (0..2)
            .map(|_| {
                form.wire()
                    .unwrap_or_else(|error| panic!("{error}"))
                    .unwrap_or_else(|| panic!("text parts always have a wire form"))
            })
            .collect();
        assert_ne!(wires[0].content_type, wires[1].content_type);
        for wire in &wires {
            let boundary = wire
                .content_type
                .strip_prefix("multipart/form-data; boundary=arazzo-")
                .unwrap_or_else(|| panic!("{}", wire.content_type));
            assert_eq!(boundary.len(), 32);
            assert_eq!(
                String::from_utf8_lossy(&wire.bytes),
                format!("--arazzo-{boundary}\r\nContent-Disposition: form-data; name=\"caption\"\r\n\r\ncat\r\n--arazzo-{boundary}--\r\n")
            );
        }
    }

    #[test]
    fn the_logical_body_keeps_sent_parts_with_blob_placeholders_marked_out_of_band() {
        let photo = Blob::new(b"jpeg".to_vec()).with_media_type("image/jpeg");
        let entries = vec![
            ("none".to_owned(), BodyNode::Null),
            ("empty".to_owned(), BodyNode::Array(Vec::new())),
            ("nulls".to_owned(), BodyNode::Array(vec![BodyNode::Null])),
            (
                "files".to_owned(),
                BodyNode::Array(vec![
                    BodyNode::Null,
                    BodyNode::String("a".to_owned()),
                    BodyNode::Blob(photo.clone()),
                ]),
            ),
            ("photo".to_owned(), BodyNode::Blob(photo.clone())),
        ];
        let form = Form::new("multipart/form-data", &entries).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(form.value(), &json!({"files": ["a", null], "photo": null}));
        assert_eq!(
            form.descriptors().keys().collect::<Vec<_>>(),
            ["/files/1", "/photo"]
        );
        assert_eq!(form.parts.len(), 3);
        let recorded = Form::new(
            "multipart/form-data",
            &[(
                "photo".to_owned(),
                BodyNode::Blob(Blob::from_descriptor(photo.descriptor())),
            )],
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert!(matches!(recorded.wire(), Ok(None)));
        assert_eq!(
            project(
                json!({"none": null, "empty": [], "nulls": [null], "files": [null, "a"], "n": 0})
            ),
            json!({"files": ["a"], "n": 0})
        );
    }

    #[test]
    fn bytes_inside_a_json_part_or_a_control_character_media_type_are_refused() {
        let photo = Blob::new(b"jpeg".to_vec());
        for (entries, message) in [
            (
                vec![(
                    "meta".to_owned(),
                    BodyNode::Map(vec![("file".to_owned(), BodyNode::Blob(photo.clone()))]),
                )],
                "requestBody.payload/meta/file: multipart/form-data part \"meta\" is sent as application/json, which cannot carry bytes; give the bytes a part of their own",
            ),
            (
                vec![(
                    "tags".to_owned(),
                    BodyNode::Array(vec![
                        BodyNode::Null,
                        BodyNode::Array(vec![BodyNode::Blob(photo.clone())]),
                    ]),
                )],
                "requestBody.payload/tags/1/0: multipart/form-data part \"tags\" is sent as application/json, which cannot carry bytes; give the bytes a part of their own",
            ),
            (
                vec![(
                    "photo".to_owned(),
                    BodyNode::Blob(photo.clone().with_media_type("image/png\r\nX-Injected: 1")),
                )],
                "requestBody.payload/photo: bytes media type \"image/png\\r\\nX-Injected: 1\" holds a control character, so it cannot be a part Content-Type",
            ),
        ] {
            let error = Form::new("multipart/form-data", &entries)
                .err()
                .unwrap_or_else(|| panic!("{entries:?} must be refused"));
            assert_eq!(error.code(), "RUNTIME_INVALID_REQUEST_BODY");
            assert_eq!(error.message, message);
        }
    }
}
