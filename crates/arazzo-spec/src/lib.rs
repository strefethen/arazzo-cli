#![forbid(unsafe_code)]

//! Core Arazzo specification types for the Rust implementation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

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
    pub success_actions: BTreeMap<String, Vec<OnAction>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub failure_actions: BTreeMap<String, Vec<OnAction>>,
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
}

/// Source description type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    OpenApi,
    Arazzo,
}

/// API source descriptor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceDescription {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "type")]
    pub type_: SourceType,
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
}

/// Schema-like object used for workflow inputs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaObject {
    #[serde(rename = "type", default)]
    pub type_: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, PropertyDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
}

/// Schema property definition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyDef {
    #[serde(rename = "type", default)]
    pub type_: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_yaml::Value>,
}

/// Workflow step definition.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    #[serde(default)]
    pub step_id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub operation_id: String,
    #[serde(default)]
    pub operation_path: String,
    #[serde(default)]
    pub workflow_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBody>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_criteria: Vec<SuccessCriterion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_success: Vec<OnAction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_failure: Vec<OnAction>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, String>,
}

/// Operation parameter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameter {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "in", default)]
    pub in_: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub reference: String,
}

/// Request body metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBody {
    #[serde(default)]
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_yaml::Value>,
    #[serde(default)]
    pub reference: String,
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

/// Action for `onSuccess` / `onFailure`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnAction {
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub type_: String,
    #[serde(default)]
    pub workflow_id: String,
    #[serde(default)]
    pub step_id: String,
    #[serde(default)]
    pub retry_after: i64,
    #[serde(default)]
    pub retry_limit: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<SuccessCriterion>,
}

/// Parses raw YAML bytes into an unvalidated specification model.
pub fn parse_unvalidated_bytes(data: &[u8]) -> Result<ArazzoSpec, serde_yaml::Error> {
    serde_yaml::from_slice::<ArazzoSpec>(data)
}
