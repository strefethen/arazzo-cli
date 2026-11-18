use arazzo_expr::{BlobSlot, BodySlot};
use arazzo_spec::{media_type::MediaType, RequestBody};
use encoding_rs::{Encoding, UTF_8};
use serde::ser::{Error as _, Serialize, Serializer};
use serde_yaml_ng::Value as Yaml;

use super::multipart_body::Form;
use super::request_admission::RequestAdmission;
use super::*;

const PAYLOAD: &str = "requestBody.payload";
const FORM: &str = "application/x-www-form-urlencoded";
const OMITTED_CONTENT_TYPE: &str = "application/json";
const BYTES_BODY: &str = "request body is bytes, which only a whole output can carry";

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BodyNode {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Template(String),
    Array(Vec<BodyNode>),
    Map(Vec<(String, BodyNode)>),
    Blob(Blob),
}

pub(super) struct BytesLeaf;

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
    fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) | Self::Template(_) => "string",
            Self::Array(_) => "array",
            Self::Map(_) => "object",
            Self::Blob(_) => "bytes",
        }
    }

    pub(super) fn bytes_pointer(&self) -> Option<String> {
        match self {
            Self::Null | Self::Bool(_) | Self::Number(_) | Self::String(_) | Self::Template(_) => {
                None
            }
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

    pub(super) fn into_json(self) -> Result<Value, BytesLeaf> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::Number(value),
            Self::String(value) | Self::Template(value) => Value::String(value),
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
            Self::String(value) | Self::Template(value) => Value::String(value.clone()),
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
            BodyNode::String(value) | BodyNode::Template(value) => serializer.serialize_str(value),
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

enum Rule {
    Json { omitted: bool },
    Form,
    Multipart { boundary: bool },
    Other { charset: Option<String> },
}

impl Rule {
    fn of(content_type: &str) -> Self {
        if content_type.is_empty() {
            return Self::Json { omitted: true };
        }
        let Some(media) = MediaType::parse(content_type) else {
            return Self::Other { charset: None };
        };
        if media.is_json() {
            Self::Json { omitted: false }
        } else if media.is_form_urlencoded() {
            Self::Form
        } else if media.is_multipart_form_data() {
            Self::Multipart {
                boundary: media.boundary().is_some(),
            }
        } else {
            Self::Other {
                charset: media.charset().map(str::to_owned),
            }
        }
    }
}

#[derive(Debug)]
enum Encoded {
    Body(Option<Vec<u8>>),
    Multipart(Form),
}

fn encode(content_type: &str, body: &BodyNode) -> Result<Encoded, RuntimeError> {
    match Rule::of(content_type) {
        Rule::Json { omitted } => encode_json(content_type, omitted, body).map(Some),
        Rule::Form => encode_form(content_type, body).map(Some),
        Rule::Multipart { boundary: false } => match body {
            BodyNode::Map(entries) => {
                return Form::new(content_type, entries).map(Encoded::Multipart)
            }
            body => Err(multipart_refusal(content_type, false, body)),
        },
        Rule::Multipart { boundary: true } => Err(multipart_refusal(content_type, true, body)),
        Rule::Other { charset } => encode_other(content_type, charset.as_deref(), body),
    }
    .map(Encoded::Body)
}

fn encode_json(
    content_type: &str,
    omitted: bool,
    body: &BodyNode,
) -> Result<Vec<u8>, RuntimeError> {
    if let Some(pointer) = body.bytes_pointer() {
        return Err(if omitted {
            invalid(format!(
                "{PAYLOAD}{pointer}: the payload carries bytes, so an explicit contentType is required"
            ))
        } else {
            no_text_form(content_type, &pointer)
        });
    }
    match body {
        BodyNode::Template(text) => Ok(text.as_bytes().to_vec()),
        body => {
            serde_json::to_vec(&Json(body)).map_err(|error| invalid(format!("{PAYLOAD}: {error}")))
        }
    }
}

fn encode_form(content_type: &str, body: &BodyNode) -> Result<Vec<u8>, RuntimeError> {
    match body.clone().into_json() {
        Ok(Value::Object(fields)) => Ok(form_body::encode(&fields)),
        Ok(Value::String(text)) => Ok(text.into_bytes()),
        Ok(_) => Err(invalid(format!(
            "{FORM} request body must be an object or string; got {}",
            body.kind()
        ))),
        Err(BytesLeaf) => Err(no_text_form(
            content_type,
            &body.bytes_pointer().unwrap_or_default(),
        )),
    }
}

fn multipart_refusal(content_type: &str, boundary: bool, body: &BodyNode) -> RuntimeError {
    invalid(match body {
        _ if boundary => format!(
            "{PAYLOAD}: contentType {content_type:?} carries a boundary parameter; the runtime chooses the multipart boundary, so remove it from contentType"
        ),
        body => format!(
            "{PAYLOAD}: contentType {content_type:?} needs an object payload, one form-data part per entry; got {}",
            body.kind()
        ),
    })
}

fn encode_other(
    content_type: &str,
    charset: Option<&str>,
    body: &BodyNode,
) -> Result<Option<Vec<u8>>, RuntimeError> {
    let text = match body {
        BodyNode::Blob(blob) => return Ok(blob.content().map(<[u8]>::to_vec)),
        BodyNode::String(text) | BodyNode::Template(text) => text.clone(),
        BodyNode::Number(value) => value.to_string(),
        BodyNode::Bool(value) => value.to_string(),
        BodyNode::Null | BodyNode::Array(_) | BodyNode::Map(_) => {
            return Err(invalid(match body.bytes_pointer() {
                Some(pointer) => format!(
                    "{PAYLOAD}{pointer}: bytes inside an object or array cannot be sent under contentType {content_type:?}; only a payload that is the bytes value itself can be"
                ),
                None => format!(
                    "{PAYLOAD}: contentType {content_type:?} sends only a string, number, boolean or bytes payload; got {}; use a JSON contentType for structured data",
                    body.kind()
                ),
            }))
        }
    };
    match charset {
        Some(label) if Encoding::for_label(label.as_bytes()) != Some(UTF_8) => Err(invalid(format!(
            "{PAYLOAD}: contentType {content_type:?} declares charset {label:?}; a {} payload is sent as UTF-8 and never transcoded, so declare charset=utf-8 or omit it",
            body.kind()
        ))),
        Some(_) | None => Ok(Some(text.into_bytes())),
    }
}

pub(super) fn refuse_content_type_parameter(step: &Step) -> Result<(), RuntimeError> {
    if step.request_body.is_none() {
        return Ok(());
    }
    match step.parameters.iter().enumerate().find(|(_, parameter)| {
        parameter.in_ == Some(ParamLocation::Header)
            && parameter.name.eq_ignore_ascii_case("content-type")
    }) {
        Some((index, parameter)) => Err(invalid(format!(
            "{}: the runtime sets Content-Type from requestBody.contentType alone, so a step with a requestBody cannot also send it as a header parameter; request not sent",
            request_admission::RequestAdmission::parameter_location(index, parameter)
        ))),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum LogicalBody {
    #[default]
    Absent,
    Json(Value),
    Bytes(Blob),
    Multipart(Form),
}

impl LogicalBody {
    pub(crate) fn json(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Multipart(form) => Some(form.value()),
            Self::Absent | Self::Bytes(_) => None,
        }
    }

    pub(super) fn blob(&self) -> Option<&Blob> {
        match self {
            Self::Bytes(blob) => Some(blob),
            Self::Absent | Self::Json(_) | Self::Multipart(_) => None,
        }
    }

    pub(super) fn slot(&self) -> BodySlot {
        match self {
            Self::Absent => BodySlot::Absent,
            Self::Json(value) => BodySlot::present(value.clone()),
            Self::Bytes(_) => bytes_slot(),
            Self::Multipart(_) => multipart_body::unreadable(),
        }
    }

    pub(super) fn blobs(&self) -> BTreeMap<String, BytesDescriptor> {
        match self {
            Self::Absent | Self::Json(_) => BTreeMap::new(),
            Self::Bytes(blob) => BTreeMap::from([(String::new(), blob.descriptor())]),
            Self::Multipart(form) => form.descriptors(),
        }
    }

    pub(super) fn form(&self) -> Option<&Form> {
        match self {
            Self::Multipart(form) => Some(form),
            Self::Absent | Self::Json(_) | Self::Bytes(_) => None,
        }
    }
}

fn bytes_slot() -> BodySlot {
    BodySlot::Unavailable {
        reason: BYTES_BODY.to_owned(),
    }
}

pub(super) fn recorded_bytes(request: &TraceRequest) -> Option<BodySlot> {
    if multipart_body::is_multipart(request) {
        return Some(multipart_body::unreadable());
    }
    (!request.body_blobs.is_empty()).then(bytes_slot)
}

#[derive(Debug, Default)]
pub(super) struct PreparedBody {
    pub(super) wire: Option<Vec<u8>>,
    pub(super) content_type: Option<String>,
    pub(super) wire_content_type: Option<String>,
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
    let header = if content_type.is_empty() {
        OMITTED_CONTENT_TYPE
    } else {
        content_type
    };
    let wire = match encode(content_type, &tree).map_err(refused)? {
        Encoded::Body(wire) => wire,
        Encoded::Multipart(form) => {
            let sent = form.wire()?;
            return Ok(PreparedBody {
                wire_content_type: sent
                    .as_ref()
                    .filter(|_| !dry_run)
                    .map(|sent| sent.content_type.clone()),
                wire: sent.map(|sent| sent.bytes),
                content_type: Some(header.to_owned()),
                logical: LogicalBody::Multipart(form),
                projection: Some(multipart_body::project(projection)),
                warnings,
            });
        }
    };
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
        wire,
        content_type: Some(header.to_owned()),
        wire_content_type: None,
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
    let template = matches!(payload, ValueSource::Literal(Yaml::String(_)))
        && !resolution
            .expressions
            .iter()
            .any(|occurrence| occurrence.direct && occurrence.pointer.is_empty());
    let mut projection = admission.project(&resolution, PAYLOAD)?;
    let mut body = resolution.value;
    let mut warnings: Vec<String> = resolution
        .warnings
        .into_iter()
        .map(|warning| format!("{PAYLOAD}: {warning}"))
        .collect();
    if !request_body.replacements.is_empty() {
        let declared = request_body.content_type.as_str();
        let json_text = template && matches!(Rule::of(declared), Rule::Json { .. });
        let content_type = if json_text && declared.is_empty() {
            OMITTED_CONTENT_TYPE
        } else {
            declared
        };
        if json_text {
            body = parse_json_text(body)?;
            projection = body.clone();
        }
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
        if json_text {
            body = Value::String(body.to_string());
            projection = Value::String(projection.to_string());
        }
    }
    let tree = match body {
        Value::String(text) if template => BodyNode::Template(text),
        body => BodyNode::from(body),
    };
    Ok(Resolved {
        tree,
        projection,
        warnings,
    })
}

fn parse_json_text(body: Value) -> Result<Value, RuntimeError> {
    let Value::String(text) = body else {
        return Ok(body);
    };
    serde_json::from_str(&text).map_err(|error| {
        invalid(format!(
            "{PAYLOAD}: replacements apply to a literal JSON payload by parsing its interpolated text, which is not JSON: {error}"
        ))
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

    fn sent(content_type: &str, body: &BodyNode) -> Vec<u8> {
        match encode(content_type, body) {
            Ok(Encoded::Body(Some(wire))) => wire,
            Ok(encoded) => panic!("{content_type:?}: {body:?} has no raw wire form: {encoded:?}"),
            Err(error) => panic!("{content_type:?}: {error}"),
        }
    }

    fn refusal(content_type: &str, body: &BodyNode) -> String {
        match encode(content_type, body) {
            Ok(wire) => panic!("{content_type:?} must refuse {body:?}, sent {wire:?}"),
            Err(error) => {
                assert_eq!(error.code(), "RUNTIME_INVALID_REQUEST_BODY");
                error.message
            }
        }
    }

    #[test]
    fn content_types_classify_by_essence_case_insensitively() {
        for content_type in [FORM, "Application/X-WWW-Form-Urlencoded; charset=UTF-8"] {
            assert!(
                matches!(Rule::of(content_type), Rule::Form),
                "{content_type}"
            );
        }
        for content_type in [
            "application/x-www-form-urlencoded-extra",
            "text/application/x-www-form-urlencoded",
        ] {
            assert!(
                matches!(Rule::of(content_type), Rule::Other { .. }),
                "{content_type}"
            );
        }
        assert!(matches!(Rule::of(""), Rule::Json { omitted: true }));
        for content_type in [
            "application/json",
            "Application/Problem+JSON; charset=utf-8",
        ] {
            assert!(
                matches!(Rule::of(content_type), Rule::Json { omitted: false }),
                "{content_type}"
            );
        }
        assert!(matches!(
            Rule::of("Multipart/Form-Data"),
            Rule::Multipart { boundary: false }
        ));
        assert!(matches!(
            Rule::of("multipart/form-data; Boundary=x"),
            Rule::Multipart { boundary: true }
        ));
    }

    #[test]
    fn a_whole_blob_is_sent_raw_only_under_an_explicit_type_that_is_not_json_form_or_multipart() {
        let body = BodyNode::Blob(jpeg());
        for content_type in [
            "image/jpeg",
            "application/octet-stream",
            "text/plain",
            "text/plain; charset=iso-8859-1",
        ] {
            assert_eq!(
                sent(content_type, &body),
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
            FORM,
        ] {
            assert_eq!(
                refusal(content_type, &body),
                format!("requestBody.payload: contentType {content_type:?} cannot carry bytes; send bytes as the whole payload under a contentType that is neither JSON nor application/x-www-form-urlencoded")
            );
        }
        assert_eq!(
            refusal("multipart/form-data", &body),
            "requestBody.payload: contentType \"multipart/form-data\" needs an object payload, one form-data part per entry; got bytes"
        );
    }

    #[test]
    fn a_recorded_blob_has_no_wire_form_and_keeps_every_refusal() {
        let body = BodyNode::Blob(Blob::from_descriptor(jpeg().descriptor()));
        for content_type in ["image/jpeg", "application/octet-stream"] {
            assert!(
                matches!(encode(content_type, &body), Ok(Encoded::Body(None))),
                "{content_type}"
            );
        }
        for content_type in ["", "application/json", FORM, "multipart/form-data"] {
            assert!(encode(content_type, &body).is_err(), "{content_type:?}");
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
        for content_type in ["application/json", FORM] {
            assert!(
                refusal(content_type, &map).starts_with(&format!(
                    "requestBody.payload/a~1b/1: contentType {content_type:?} cannot carry bytes"
                )),
                "{content_type}"
            );
        }
        assert_eq!(
            refusal("application/octet-stream", &map),
            "requestBody.payload/a~1b/1: bytes inside an object or array cannot be sent under contentType \"application/octet-stream\"; only a payload that is the bytes value itself can be"
        );
        assert!(map.clone().into_json().is_err());
        assert_eq!(
            map.presentation()["a/b"][1]["kind"],
            json!("bytes"),
            "a dry run shows the descriptor"
        );
    }

    #[test]
    fn json_rows_send_a_literal_template_verbatim_and_serialize_everything_else() {
        let value = json!({"z": 1, "a": [true, null, 1.5, "é\"\n"], "n": {"k": -7}});
        let tree = BodyNode::from(value.clone());
        assert_eq!(tree.bytes_pointer(), None);
        let template = BodyNode::Template("{\"id\": \"7\"}\n".to_owned());
        for content_type in ["application/json", "", "application/vnd.api+json"] {
            assert_eq!(
                sent(content_type, &tree),
                serde_json::to_vec(&value).unwrap_or_else(|error| panic!("{error}")),
                "{content_type:?}"
            );
            assert_eq!(sent(content_type, &template), b"{\"id\": \"7\"}\n");
            assert_eq!(
                sent(content_type, &BodyNode::from(json!("text"))),
                b"\"text\""
            );
            assert_eq!(sent(content_type, &BodyNode::Null), b"null");
        }
        assert_eq!(tree.clone().into_json().ok(), Some(value.clone()));
        assert_eq!(tree.presentation(), value);
        assert_eq!(template.presentation(), json!("{\"id\": \"7\"}\n"));
    }

    #[test]
    fn form_rows_encode_a_map_or_send_a_string_and_refuse_other_shapes() {
        assert_eq!(
            sent(FORM, &BodyNode::from(json!({"b": "1 2", "a": [3, 4]}))),
            b"b=1+2&a=3&a=4"
        );
        for body in [
            BodyNode::from(json!("x=a+b%20c")),
            BodyNode::Template("x=a+b%20c".to_owned()),
        ] {
            assert_eq!(sent(FORM, &body), b"x=a+b%20c");
        }
        for (value, kind) in [
            (json!([]), "array"),
            (json!(["SECRET"]), "array"),
            (json!(42), "number"),
            (json!(true), "boolean"),
            (Value::Null, "null"),
        ] {
            assert_eq!(
                refusal(FORM, &BodyNode::from(value)),
                format!("application/x-www-form-urlencoded request body must be an object or string; got {kind}")
            );
        }
    }

    #[test]
    fn multipart_rows_assemble_a_map_and_refuse_every_other_shape() {
        let map = BodyNode::from(json!({"caption": "cat"}));
        for content_type in ["multipart/form-data", "Multipart/Form-Data; charset=utf-8"] {
            match encode(content_type, &map) {
                Ok(Encoded::Multipart(form)) => {
                    assert_eq!(form.content_type(), content_type);
                    assert_eq!(form.value(), &json!({"caption": "cat"}));
                }
                other => panic!("{content_type:?}: {other:?}"),
            }
        }
        for (body, kind) in [
            (BodyNode::from(json!("text")), "string"),
            (BodyNode::from(json!([1])), "array"),
            (BodyNode::Null, "null"),
        ] {
            assert_eq!(
                refusal("multipart/form-data", &body),
                format!("requestBody.payload: contentType \"multipart/form-data\" needs an object payload, one form-data part per entry; got {kind}")
            );
        }
        assert_eq!(
            refusal("multipart/form-data; boundary=abc", &map),
            "requestBody.payload: contentType \"multipart/form-data; boundary=abc\" carries a boundary parameter; the runtime chooses the multipart boundary, so remove it from contentType"
        );
    }

    #[test]
    fn other_rows_send_text_and_scalars_as_utf8_and_refuse_structure_and_other_charsets() {
        for content_type in [
            "text/plain",
            "application/xml",
            "text/plain; charset=UTF-8",
            "text/plain; charset=utf8",
            "not a media type",
        ] {
            assert_eq!(
                sent(content_type, &BodyNode::from(json!("<a/> é"))),
                "<a/> é".as_bytes()
            );
            assert_eq!(
                sent(content_type, &BodyNode::Template("t".to_owned())),
                b"t"
            );
            assert_eq!(sent(content_type, &BodyNode::from(json!(42))), b"42");
            assert_eq!(sent(content_type, &BodyNode::from(json!(-1.5))), b"-1.5");
            assert_eq!(sent(content_type, &BodyNode::from(json!(true))), b"true");
        }
        for (value, kind) in [
            (json!({"key": "val"}), "object"),
            (json!(["a"]), "array"),
            (Value::Null, "null"),
        ] {
            assert_eq!(
                refusal("application/xml", &BodyNode::from(value)),
                format!("requestBody.payload: contentType \"application/xml\" sends only a string, number, boolean or bytes payload; got {kind}; use a JSON contentType for structured data")
            );
        }
        for (content_type, label) in [
            ("text/plain; charset=iso-8859-1", "iso-8859-1"),
            ("text/plain; charset=utf-16", "utf-16"),
            ("text/plain; charset=bogus", "bogus"),
        ] {
            assert_eq!(
                refusal(content_type, &BodyNode::from(json!("x"))),
                format!("requestBody.payload: contentType {content_type:?} declares charset {label:?}; a string payload is sent as UTF-8 and never transcoded, so declare charset=utf-8 or omit it")
            );
        }
        assert!(
            refusal("text/plain; charset=latin1", &BodyNode::from(json!(1)))
                .contains("a number payload is sent as UTF-8")
        );
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
        let form = match encode("multipart/form-data", &BodyNode::from(json!({"a": "b"}))) {
            Ok(Encoded::Multipart(form)) => LogicalBody::Multipart(form),
            other => panic!("{other:?}"),
        };
        assert_eq!(form.json(), Some(&json!({"a": "b"})));
        assert_eq!(form.blob(), None);
        assert_eq!(
            form.slot(),
            BodySlot::Unavailable {
                reason:
                    "request body is multipart/form-data, whose parts have no single value to read"
                        .to_owned()
            }
        );
        let recorded = TraceRequest {
            headers: BTreeMap::from([(
                "Content-Type".to_owned(),
                "multipart/form-data".to_owned(),
            )]),
            body: Some(json!({"a": "b"})),
            ..TraceRequest::default()
        };
        assert_eq!(recorded_bytes(&recorded), Some(form.slot()));
    }
}
