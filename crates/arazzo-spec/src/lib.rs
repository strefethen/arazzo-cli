#![forbid(unsafe_code)]

//! Core Arazzo specification types for the Rust implementation.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Raw `x-*` Specification Extension fields preserved on Arazzo objects.
pub type VendorExtensions = BTreeMap<String, serde_yaml_ng::Value>;

fn is_vendor_extension_key(key: &str) -> bool {
    key.starts_with("x-")
}

fn vendor_extensions_is_empty(extensions: &VendorExtensions) -> bool {
    extensions.keys().all(|key| !is_vendor_extension_key(key))
}

fn serialize_vendor_extensions<S>(
    extensions: &VendorExtensions,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    extensions
        .iter()
        .filter(|(key, _)| is_vendor_extension_key(key))
        .collect::<BTreeMap<_, _>>()
        .serialize(serializer)
}

fn deserialize_vendor_extensions<'de, D>(deserializer: D) -> Result<VendorExtensions, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = BTreeMap::<String, serde_yaml_ng::Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter(|(key, _)| is_vendor_extension_key(key))
        .collect())
}

/// Root Arazzo specification document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArazzoSpec {
    #[serde(default)]
    pub arazzo: String,
    #[serde(default)]
    pub info: Info,
    #[serde(default)]
    pub source_descriptions: Vec<SourceDescription>,
    #[serde(default)]
    pub workflows: Vec<Workflow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// Reusable component collections referenced via `$components.*`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Components {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, SchemaObject>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Parameter>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub success_actions: BTreeMap<String, OnAction>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub failure_actions: BTreeMap<String, OnAction>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// Metadata about the specification.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// Source description type discriminator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    #[default]
    OpenApi,
    Arazzo,
}

/// API source descriptor.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescription {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "type")]
    pub type_: SourceType,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// A single workflow in the document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    #[serde(default)]
    pub workflow_id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<SchemaObject>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_actions: Vec<OnAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_actions: Vec<OnAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// JSON Schema type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonSchemaType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
}

impl std::fmt::Display for JsonSchemaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String => write!(f, "string"),
            Self::Integer => write!(f, "integer"),
            Self::Number => write!(f, "number"),
            Self::Boolean => write!(f, "boolean"),
            Self::Array => write!(f, "array"),
            Self::Object => write!(f, "object"),
        }
    }
}

/// Schema-like object used for workflow inputs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObject {
    #[serde(rename = "$ref", default, skip_serializing_if = "String::is_empty")]
    pub ref_: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<JsonSchemaType>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, PropertyDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// Schema property definition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyDef {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<JsonSchemaType>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_yaml_ng::Value>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// Step target discriminator — exactly one of operationId, operationPath, or workflowId.
#[derive(Debug, Clone, PartialEq)]
pub enum StepTarget {
    OperationId(String),
    OperationPath(String),
    WorkflowId(String),
}

/// Workflow step definition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    pub step_id: String,
    pub description: String,
    pub target: Option<StepTarget>,
    pub parameters: Vec<Parameter>,
    pub request_body: Option<RequestBody>,
    pub success_criteria: Vec<SuccessCriterion>,
    pub on_success: Vec<OnAction>,
    pub on_failure: Vec<OnAction>,
    pub outputs: BTreeMap<String, String>,
    pub extensions: VendorExtensions,
}

/// Serde helper that mirrors the flat YAML/JSON shape of a Step.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepSerde {
    #[serde(default)]
    step_id: String,
    #[serde(default)]
    description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    operation_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    operation_path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    workflow_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    parameters: Vec<Parameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    request_body: Option<RequestBody>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    success_criteria: Vec<SuccessCriterion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    on_success: Vec<OnAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    on_failure: Vec<OnAction>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    outputs: BTreeMap<String, String>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    extensions: VendorExtensions,
}

impl Serialize for Step {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (operation_id, operation_path, workflow_id) = match &self.target {
            Some(StepTarget::OperationId(v)) => (v.clone(), String::new(), String::new()),
            Some(StepTarget::OperationPath(v)) => (String::new(), v.clone(), String::new()),
            Some(StepTarget::WorkflowId(v)) => (String::new(), String::new(), v.clone()),
            None => (String::new(), String::new(), String::new()),
        };
        StepSerde {
            step_id: self.step_id.clone(),
            description: self.description.clone(),
            operation_id,
            operation_path,
            workflow_id,
            parameters: self.parameters.clone(),
            request_body: self.request_body.clone(),
            success_criteria: self.success_criteria.clone(),
            on_success: self.on_success.clone(),
            on_failure: self.on_failure.clone(),
            outputs: self.outputs.clone(),
            extensions: self.extensions.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = StepSerde::deserialize(deserializer)?;
        let has_operation_id = !raw.operation_id.is_empty();
        let has_operation_path = !raw.operation_path.is_empty();
        let has_workflow_id = !raw.workflow_id.is_empty();
        let target_count = usize::from(has_operation_id)
            + usize::from(has_operation_path)
            + usize::from(has_workflow_id);

        if target_count > 1 {
            return Err(serde::de::Error::custom(
                "step must specify exactly one of operationId, operationPath, or workflowId",
            ));
        }

        let target = if has_workflow_id {
            Some(StepTarget::WorkflowId(raw.workflow_id))
        } else if has_operation_path {
            Some(StepTarget::OperationPath(raw.operation_path))
        } else if has_operation_id {
            Some(StepTarget::OperationId(raw.operation_id))
        } else {
            None
        };
        Ok(Step {
            step_id: raw.step_id,
            description: raw.description,
            target,
            parameters: raw.parameters,
            request_body: raw.request_body,
            success_criteria: raw.success_criteria,
            on_success: raw.on_success,
            on_failure: raw.on_failure,
            outputs: raw.outputs,
            extensions: raw.extensions,
        })
    }
}

/// Parameter location discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
    Cookie,
}

/// Operation parameter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub in_: Option<ParamLocation>,
    #[serde(default)]
    pub value: serde_yaml_ng::Value,
    #[serde(default)]
    pub reference: String,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

impl Parameter {
    /// Returns the value as a string suitable for expression evaluation.
    pub fn value_as_str(&self) -> String {
        match &self.value {
            serde_yaml_ng::Value::String(s) => s.clone(),
            serde_yaml_ng::Value::Number(n) => {
                if let Some(u) = n.as_u64() {
                    u.to_string()
                } else if let Some(i) = n.as_i64() {
                    i.to_string()
                } else if let Some(f) = n.as_f64() {
                    f.to_string()
                } else {
                    String::new()
                }
            }
            serde_yaml_ng::Value::Bool(b) => b.to_string(),
            serde_yaml_ng::Value::Null => String::new(),
            other => {
                // Convert YAML mappings/sequences to JSON strings so that
                // downstream expression evaluation (which operates on JSON)
                // receives a valid representation.
                let json_val: serde_json::Value =
                    serde_json::to_value(other).unwrap_or(serde_json::Value::Null);
                serde_json::to_string(&json_val).unwrap_or_default()
            }
        }
    }

    /// Returns true if the value is empty (null or empty string).
    pub fn is_value_empty(&self) -> bool {
        match &self.value {
            serde_yaml_ng::Value::Null => true,
            serde_yaml_ng::Value::String(s) => s.is_empty(),
            _ => false,
        }
    }
}

/// Request body metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBody {
    #[serde(default)]
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_yaml_ng::Value>,
    #[serde(default)]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replacements: Vec<Replacement>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// A single payload replacement targeting a JSON Pointer (RFC 6901) or XPath
/// 1.0 location inside the request body. Per Arazzo 1.0.1 §4.6.14.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Replacement {
    /// RFC 6901 JSON Pointer (for JSON bodies) or XPath 1.0 expression (for
    /// XML/text bodies). Required.
    #[serde(default)]
    pub target: String,

    /// Value to inject. May be a literal scalar, mapping, sequence, or an
    /// Arazzo runtime expression string resolved at send time.
    #[serde(default)]
    pub value: serde_yaml_ng::Value,
}

/// Step success criterion.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuccessCriterion {
    #[serde(default)]
    pub condition: String,
    #[serde(default)]
    pub context: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<CriterionType>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// Criterion expression type selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CriterionType {
    Name(String),
    ExpressionType(CriterionExpressionType),
}

/// Object form of criterion expression type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionExpressionType {
    #[serde(rename = "type", default)]
    pub type_: String,
    #[serde(default)]
    pub version: String,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

impl SuccessCriterion {
    /// Returns the effective criterion type name (`simple` when omitted).
    pub fn resolved_type_name(&self) -> String {
        match &self.type_ {
            None => "simple".to_string(),
            Some(CriterionType::Name(name)) => name.trim().to_lowercase(),
            Some(CriterionType::ExpressionType(expr)) => expr.type_.trim().to_lowercase(),
        }
    }

    /// Returns the declared criterion type version when object form is used.
    pub fn declared_type_version(&self) -> Option<&str> {
        match &self.type_ {
            Some(CriterionType::ExpressionType(expr)) if !expr.version.is_empty() => {
                Some(expr.version.as_str())
            }
            _ => None,
        }
    }

    /// Returns whether `type` was explicitly declared in the specification.
    pub fn has_declared_type(&self) -> bool {
        self.type_.is_some()
    }
}

/// Action type discriminator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionType {
    #[default]
    End,
    Goto,
    Retry,
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::End => write!(f, "end"),
            Self::Goto => write!(f, "goto"),
            Self::Retry => write!(f, "retry"),
        }
    }
}

/// Action for `onSuccess` / `onFailure`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnAction {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub type_: ActionType,
    #[serde(default)]
    pub workflow_id: String,
    #[serde(default)]
    pub step_id: String,
    #[serde(default)]
    pub retry_after: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<SuccessCriterion>,
    #[serde(
        flatten,
        default,
        skip_serializing_if = "vendor_extensions_is_empty",
        serialize_with = "serialize_vendor_extensions",
        deserialize_with = "deserialize_vendor_extensions"
    )]
    pub extensions: VendorExtensions,
}

/// Parses raw YAML bytes into an unvalidated specification model.
pub fn parse_unvalidated_bytes(data: &[u8]) -> Result<ArazzoSpec, serde_yaml_ng::Error> {
    serde_yaml_ng::from_slice::<ArazzoSpec>(data)
}

#[cfg(test)]
mod tests {
    use super::{Replacement, RequestBody};

    fn serialize<T: serde::Serialize>(value: &T) -> String {
        match serde_yaml_ng::to_string(value) {
            Ok(serialized) => serialized,
            Err(err) => panic!("serializing YAML: {err}"),
        }
    }

    fn deserialize_request_body(input: &str) -> RequestBody {
        match serde_yaml_ng::from_str::<RequestBody>(input) {
            Ok(body) => body,
            Err(err) => panic!("deserializing request body YAML: {err}"),
        }
    }

    #[test]
    fn requestbody_replacements_roundtrip() {
        let body = RequestBody {
            content_type: "application/json".to_string(),
            replacements: vec![
                Replacement {
                    target: "/foo".to_string(),
                    value: serde_yaml_ng::Value::String("bar".to_string()),
                },
                Replacement {
                    target: "/count".to_string(),
                    value: serde_yaml_ng::Value::Number(2.into()),
                },
            ],
            ..RequestBody::default()
        };

        let serialized = serialize(&body);
        let reparsed = deserialize_request_body(&serialized);

        assert_eq!(reparsed, body);
    }

    #[test]
    fn requestbody_default_has_empty_replacements() {
        assert!(RequestBody::default().replacements.is_empty());
    }

    #[test]
    fn replacements_skip_serializing_when_empty() {
        let serialized = serialize(&RequestBody {
            replacements: Vec::new(),
            ..RequestBody::default()
        });

        assert!(!serialized.contains("replacements:"));
    }
}
