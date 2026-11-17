use arazzo_expr::{BlobSlot, BodySlot};
use arazzo_spec::{media_type, RequestBody};
use serde::ser::{Error as _, Serialize, Serializer};
use serde_yaml_ng::Value as Yaml;

use super::request_admission::RequestAdmission;
use super::*;

const PAYLOAD: &str = "requestBody.payload";
const BYTES_BODY: &str = "request body is bytes, which only a whole output can carry";

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BodyNode {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<BodyNode>),
    Map(Vec<(String, BodyNode)>),
    Blob(Blob),
}

struct BytesLeaf;

impl From<Value> for BodyNode {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(value) => Self::Bool(value),
            Value::Number(value) => Self::Number(value),
            Value::String(value) => Self::String(value),
            Value::Array(items) => Self::Array(items.into_iter().map(Self::from).collect()),
            Value::Object(fields) => Self::Map(
                fields
                    .into_iter()
                    .map(|(name, value)| (name, Self::from(value)))
                    .collect(),
            ),
        }
    }
}

impl BodyNode {
    fn bytes_pointer(&self) -> Option<String> {
        match self {
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) => None,
            Self::Array(items) => items.iter().enumerate().find_map(|(index, item)| {
                item.bytes_pointer().map(|rest| format!("/{index}{rest}"))
            }),
            Self::Map(entries) => entries.iter().find_map(|(name, item)| {
                item.bytes_pointer()
                    .map(|rest| format!("{}{rest}", payload::child_pointer("", name)))
            }),
            Self::Blob(_) => Some(String::new()),
        }
    }

    fn into_json(self) -> Result<Value, BytesLeaf> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::Number(value),
            Self::String(value) => Value::String(value),
            Self::Array(items) => Value::Array(
                items
                    .into_iter()
                    .map(Self::into_json)
                    .collect::<Result<_, _>>()?,
            ),
            Self::Map(entries) => Value::Object(
                entries
                    .into_iter()
                    .map(|(name, item)| item.into_json().map(|value| (name, value)))
                    .collect::<Result<_, _>>()?,
            ),
            Self::Blob(_) => return Err(BytesLeaf),
        })
    }

    fn presentation(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(*value),
            Self::Number(value) => Value::Number(value.clone()),
            Self::String(value) => Value::String(value.clone()),
            Self::Array(items) => Value::Array(items.iter().map(Self::presentation).collect()),
            Self::Map(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), item.presentation()))
                    .collect(),
            ),
            Self::Blob(blob) => json!(blob.descriptor()),
        }
    }
}

struct Json<'a>(&'a BodyNode);

impl Serialize for Json<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            BodyNode::Null => serializer.serialize_unit(),
            BodyNode::Bool(value) => serializer.serialize_bool(*value),
            BodyNode::Number(value) => value.serialize(serializer),
            BodyNode::String(value) => serializer.serialize_str(value),
            BodyNode::Array(items) => serializer.collect_seq(items.iter().map(Json)),
            BodyNode::Map(entries) => {
                serializer.collect_map(entries.iter().map(|(name, item)| (name, Json(item))))
            }
            BodyNode::Blob(_) => Err(S::Error::custom("bytes have no JSON form")),
        }
    }
}

fn invalid(message: String) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::InvalidRequestBody, message)
}

fn no_text_form(content_type: &str, pointer: &str) -> RuntimeError {
    invalid(format!(
        "{PAYLOAD}{pointer}: contentType {content_type:?} cannot carry bytes; send bytes as the whole payload under a contentType that is neither JSON nor application/x-www-form-urlencoded"
    ))
}

fn encode_json(content_type: &str, body: &BodyNode) -> Result<Vec<u8>, RuntimeError> {
    serde_json::to_vec(&Json(body)).map_err(|error| match body.bytes_pointer() {
        Some(pointer) if media_type::is_json(content_type) => no_text_form(content_type, &pointer),
        Some(pointer) => invalid(format!(
            "{PAYLOAD}{pointer}: bytes inside an object or array cannot be sent under contentType {content_type:?}; only a payload that is the bytes value itself can be"
        )),
        None => invalid(format!("{PAYLOAD}: {error}")),
    })
}

fn encode_form(content_type: &str, body: &BodyNode) -> Result<Vec<u8>, RuntimeError> {
    match body.clone().into_json() {
        Ok(value) => form_body::encode(&value),
        Err(BytesLeaf) => Err(no_text_form(
            content_type,
            &body.bytes_pointer().unwrap_or_default(),
        )),
    }
}

fn encode(content_type: &str, body: &BodyNode) -> Result<Vec<u8>, RuntimeError> {
    if content_type.is_empty() {
        if let Some(pointer) = body.bytes_pointer() {
            return Err(invalid(format!(
                "{PAYLOAD}{pointer}: the payload carries bytes, so an explicit contentType is required"
            )));
        }
    }
    if form_body::is_form(content_type) {
        return encode_form(content_type, body);
    }
    if media_type::is_json(content_type) {
        return encode_json(content_type, body);
    }
    match body {
        BodyNode::Blob(blob) => Ok(blob.bytes().to_vec()),
        BodyNode::String(text) => Ok(text.as_bytes().to_vec()),
        BodyNode::Null
        | BodyNode::Bool(_)
        | BodyNode::Number(_)
        | BodyNode::Array(_)
        | BodyNode::Map(_) => encode_json(content_type, body),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum LogicalBody {
    #[default]
    Absent,
    Json(Value),
    Bytes(Blob),
}

impl LogicalBody {
    pub(crate) fn json(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Absent | Self::Bytes(_) => None,
        }
    }

    pub(super) fn blob(&self) -> Option<&Blob> {
        match self {
            Self::Bytes(blob) => Some(blob),
            Self::Absent | Self::Json(_) => None,
        }
    }

    pub(super) fn slot(&self) -> BodySlot {
        match self {
            Self::Absent => BodySlot::Absent,
            Self::Json(value) => BodySlot::present(value.clone()),
            Self::Bytes(_) => bytes_slot(),
        }
    }

    pub(super) fn blobs(&self) -> BTreeMap<String, BytesDescriptor> {
        match self {
            Self::Absent | Self::Json(_) => BTreeMap::new(),
            Self::Bytes(blob) => BTreeMap::from([(String::new(), blob.descriptor())]),
        }
    }
}

fn bytes_slot() -> BodySlot {
    BodySlot::Unavailable {
        reason: BYTES_BODY.to_owned(),
    }
}

pub(super) fn recorded_bytes(request: &TraceRequest) -> Option<BodySlot> {
    (!request.body_blobs.is_empty()).then(bytes_slot)
}

#[derive(Debug, Default)]
pub(super) struct PreparedBody {
    pub(super) wire: Option<Vec<u8>>,
    pub(super) logical: LogicalBody,
    pub(super) projection: Option<Value>,
    pub(super) warnings: Vec<String>,
}

struct Resolved {
    tree: BodyNode,
    projection: Value,
    warnings: Vec<String>,
}

pub(super) fn prepare(
    step: &Step,
    vars: &VarStore,
    eval: &ExpressionEvaluator,
    admission: &RequestAdmission<'_>,
    dry_run: bool,
) -> Result<PreparedBody, RuntimeError> {
    let Some(request_body) = &step.request_body else {
        return Ok(PreparedBody::default());
    };
    let Some(payload) = &request_body.payload else {
        return Ok(PreparedBody::default());
    };
    let content_type = request_body.content_type.as_str();
    let carries_bytes = references_bytes(payload, eval);
    let refused = |error: RuntimeError| {
        if carries_bytes {
            RuntimeError::new(
                error.kind,
                format!(
                    "step {:?}: {}; request not sent",
                    step.step_id, error.message
                ),
            )
        } else {
            error
        }
    };
    let Resolved {
        tree,
        projection,
        warnings,
    } = if carries_bytes {
        BytesPayload {
            step_id: &step.step_id,
            vars,
            eval,
            admission,
            warnings: Vec::new(),
        }
        .resolve(payload, request_body)?
    } else {
        resolve_json(request_body, payload, eval, admission, dry_run)?
    };
    let wire = encode(content_type, &tree).map_err(refused)?;
    let logical = match tree {
        BodyNode::Blob(blob) => LogicalBody::Bytes(blob),
        tree => LogicalBody::Json(tree.into_json().map_err(|BytesLeaf| {
            RuntimeError::new(
                RuntimeErrorKind::InternalError,
                format!(
                    "step {:?}: {PAYLOAD}: an encoded request body holds bytes that have no logical form",
                    step.step_id
                ),
            )
        })?),
    };
    Ok(PreparedBody {
        wire: Some(wire),
        logical,
        projection: Some(projection),
        warnings,
    })
}

fn resolve_json(
    request_body: &RequestBody,
    payload: &ValueSource,
    eval: &ExpressionEvaluator,
    admission: &RequestAdmission<'_>,
    dry_run: bool,
) -> Result<Resolved, RuntimeError> {
    let resolution = admission.resolve(payload, eval, PAYLOAD)?;
    let mut projection = admission.project(&resolution, PAYLOAD)?;
    let mut body = resolution.value;
    let mut warnings: Vec<String> = resolution
        .warnings
        .into_iter()
        .map(|warning| format!("{PAYLOAD}: {warning}"))
        .collect();
    if !request_body.replacements.is_empty() {
        let content_type = request_body.content_type.as_str();
        if dry_run {
            let (mutated, display, replaced) = payload::apply_replacements_projected(
                body,
                projection,
                content_type,
                &request_body.replacements,
                eval,
                admission,
            )?;
            body = mutated;
            projection = display;
            warnings.extend(replaced);
        } else {
            let (mutated, replaced) = apply_replacements(
                body,
                content_type,
                &request_body.replacements,
                eval,
                admission,
            )?;
            body = mutated;
            projection = body.clone();
            warnings.extend(replaced);
        }
    }
    Ok(Resolved {
        tree: BodyNode::from(body),
        projection,
        warnings,
    })
}

fn references_bytes(source: &ValueSource, eval: &ExpressionEvaluator) -> bool {
    if eval.context().blob_slots.is_empty() {
        return false;
    }
    match source {
        ValueSource::Literal(Yaml::String(text)) => eval.blob_slot(text).is_some(),
        ValueSource::Literal(Yaml::Sequence(items)) => items
            .iter()
            .any(|item| references_bytes(&item.clone().into(), eval)),
        ValueSource::Literal(Yaml::Mapping(fields)) => fields
            .values()
            .any(|item| references_bytes(&item.clone().into(), eval)),
        ValueSource::Literal(_) | ValueSource::Selector(_) => false,
    }
}

struct BytesPayload<'a> {
    step_id: &'a str,
    vars: &'a VarStore,
    eval: &'a ExpressionEvaluator,
    admission: &'a RequestAdmission<'a>,
    warnings: Vec<String>,
}

impl BytesPayload<'_> {
    fn resolve(
        mut self,
        payload: &ValueSource,
        request_body: &RequestBody,
    ) -> Result<Resolved, RuntimeError> {
        let tree = self.node(payload, "")?;
        if !request_body.replacements.is_empty() {
            return Err(RuntimeError::new(
                RuntimeErrorKind::ValueUnavailable,
                format!(
                    "step {:?}: requestBody.replacements[0].target: {PAYLOAD}{} is bytes, which a replacement target cannot read; request not sent",
                    self.step_id,
                    tree.bytes_pointer().unwrap_or_default()
                ),
            ));
        }
        Ok(Resolved {
            projection: tree.presentation(),
            tree,
            warnings: self.warnings,
        })
    }

    fn node(&mut self, source: &ValueSource, pointer: &str) -> Result<BodyNode, RuntimeError> {
        match source {
            ValueSource::Literal(Yaml::String(text)) => match self.eval.blob_slot(text) {
                Some(slot) => self.blob(&slot, pointer),
                None => self.leaf(source, pointer),
            },
            ValueSource::Literal(Yaml::Sequence(items)) => {
                let mut nodes = Vec::with_capacity(items.len());
                for (index, item) in items.iter().enumerate() {
                    let pointer = payload::child_pointer(pointer, &index.to_string());
                    nodes.push(self.node(&item.clone().into(), &pointer)?);
                }
                Ok(BodyNode::Array(nodes))
            }
            ValueSource::Literal(Yaml::Mapping(fields)) => {
                let mut entries = Vec::<(String, BodyNode)>::with_capacity(fields.len());
                for (key, item) in fields {
                    let name = key.as_str().unwrap_or_default().to_owned();
                    let pointer = payload::child_pointer(pointer, &name);
                    let node = self.node(&item.clone().into(), &pointer)?;
                    match entries.iter_mut().find(|(existing, _)| *existing == name) {
                        Some(entry) => entry.1 = node,
                        None => entries.push((name, node)),
                    }
                }
                Ok(BodyNode::Map(entries))
            }
            ValueSource::Literal(_) | ValueSource::Selector(_) => self.leaf(source, pointer),
        }
    }

    fn blob(&self, slot: &BlobSlot, pointer: &str) -> Result<BodyNode, RuntimeError> {
        self.vars.blob(slot).map(BodyNode::Blob).ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorKind::InternalError,
                format!(
                    "step {:?}: {PAYLOAD}{pointer}: {slot} is a bytes slot with no bytes behind it; request not sent",
                    self.step_id
                ),
            )
        })
    }

    fn leaf(&mut self, source: &ValueSource, pointer: &str) -> Result<BodyNode, RuntimeError> {
        let location = format!("{PAYLOAD}{pointer}");
        let resolution = self.admission.resolve(source, self.eval, &location)?;
        self.admission.check(&resolution, &location)?;
        self.warnings.extend(
            resolution
                .warnings
                .iter()
                .map(|warning| format!("{PAYLOAD}: {warning}")),
        );
        Ok(BodyNode::from(resolution.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg() -> Blob {
        Blob::new(vec![0xFF, 0xD8, 0xFF, 0xE0]).with_media_type("image/jpeg")
    }

    fn refusal(content_type: &str, body: &BodyNode) -> String {
        match encode(content_type, body) {
            Ok(wire) => panic!("{content_type:?} must refuse bytes, sent {wire:?}"),
            Err(error) => {
                assert_eq!(error.code(), "RUNTIME_INVALID_REQUEST_BODY");
                error.message
            }
        }
    }

    #[test]
    fn a_whole_blob_is_sent_raw_only_under_an_explicit_type_that_is_not_json_or_form() {
        let body = BodyNode::Blob(jpeg());
        for content_type in [
            "image/jpeg",
            "application/octet-stream",
            "text/plain",
            "multipart/form-data",
        ] {
            assert_eq!(
                encode(content_type, &body).unwrap_or_else(|error| panic!("{error}")),
                [0xFF, 0xD8, 0xFF, 0xE0],
                "{content_type}"
            );
        }
        assert_eq!(
            refusal("", &body),
            "requestBody.payload: the payload carries bytes, so an explicit contentType is required"
        );
        for content_type in [
            "application/json",
            "application/problem+json",
            "Application/JSON; charset=utf-8",
            "application/x-www-form-urlencoded",
        ] {
            assert_eq!(
                refusal(content_type, &body),
                format!("requestBody.payload: contentType {content_type:?} cannot carry bytes; send bytes as the whole payload under a contentType that is neither JSON nor application/x-www-form-urlencoded")
            );
        }
    }

    #[test]
    fn a_blob_leaf_inside_a_map_or_array_reaches_no_wire_form() {
        let map = BodyNode::Map(vec![
            ("caption".to_owned(), BodyNode::String("cat".to_owned())),
            (
                "a/b".to_owned(),
                BodyNode::Array(vec![BodyNode::Null, BodyNode::Blob(jpeg())]),
            ),
        ]);
        assert_eq!(map.bytes_pointer().as_deref(), Some("/a~1b/1"));
        assert_eq!(
            refusal("", &map),
            "requestBody.payload/a~1b/1: the payload carries bytes, so an explicit contentType is required"
        );
        for content_type in ["application/json", "application/x-www-form-urlencoded"] {
            assert!(
                refusal(content_type, &map).starts_with(&format!(
                    "requestBody.payload/a~1b/1: contentType {content_type:?} cannot carry bytes"
                )),
                "{content_type}"
            );
        }
        assert_eq!(
            refusal("multipart/form-data", &map),
            "requestBody.payload/a~1b/1: bytes inside an object or array cannot be sent under contentType \"multipart/form-data\"; only a payload that is the bytes value itself can be"
        );
        assert!(map.clone().into_json().is_err());
        assert_eq!(
            map.presentation()["a/b"][1]["kind"],
            json!("bytes"),
            "a dry run shows the descriptor"
        );
    }

    #[test]
    fn json_trees_encode_exactly_as_their_values_did() {
        let value = json!({"z": 1, "a": [true, null, 1.5, "é\"\n"], "n": {"k": -7}});
        let tree = BodyNode::from(value.clone());
        assert_eq!(tree.bytes_pointer(), None);
        for content_type in ["application/json", "", "application/xml", "text/plain"] {
            assert_eq!(
                encode(content_type, &tree).unwrap_or_else(|error| panic!("{error}")),
                serde_json::to_vec(&value).unwrap_or_else(|error| panic!("{error}")),
                "{content_type:?}"
            );
        }
        assert_eq!(
            encode(
                "application/x-www-form-urlencoded",
                &BodyNode::from(json!({"b": "1 2", "a": [3, 4]}))
            )
            .unwrap_or_else(|error| panic!("{error}")),
            b"b=1+2&a=3&a=4"
        );
        for content_type in ["", "text/plain", "application/xml"] {
            assert_eq!(
                encode(content_type, &BodyNode::from(json!("<a/>")))
                    .unwrap_or_else(|error| panic!("{error}")),
                b"<a/>"
            );
        }
        assert_eq!(
            encode("application/json", &BodyNode::from(json!("text")))
                .unwrap_or_else(|error| panic!("{error}")),
            b"\"text\""
        );
        assert_eq!(tree.clone().into_json().ok(), Some(value.clone()));
        assert_eq!(tree.presentation(), value);
    }

    #[test]
    fn a_bytes_body_is_unreadable_as_request_body_and_recorded_out_of_band() {
        let bytes = LogicalBody::Bytes(jpeg());
        assert_eq!(bytes.json(), None);
        assert_eq!(
            bytes.slot(),
            BodySlot::Unavailable {
                reason: "request body is bytes, which only a whole output can carry".to_owned()
            }
        );
        assert_eq!(
            json!(bytes.blobs()),
            json!({"": {"kind": "bytes", "size": 4, "mediaType": "image/jpeg",
                "sha256": "ba4f25bf16ba4be6bc7d3276fafeb67f9eb3c5df042bc3a405e1af15b921eed7"}})
        );
        let json = LogicalBody::Json(json!({"a": 1}));
        assert_eq!(json.slot(), BodySlot::present(json!({"a": 1})));
        assert!(json.blobs().is_empty());
        assert_eq!(LogicalBody::Absent.slot(), BodySlot::Absent);
    }
}
