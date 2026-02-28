use std::collections::BTreeMap;

use arazzo_runtime::{DryRunRequest, TraceStepRecord};
use arazzo_spec::{ArazzoSpec, Workflow};
use arazzo_validate::{Error as ValidateError, ValidationErrorKind};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub file: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub version: String,
    pub sources: Vec<SourceInfo>,
    pub workflows: Vec<WorkflowInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SourceInfo {
    pub name: String,
    pub url: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkflowInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InputDetail {
    #[serde(rename = "type")]
    pub type_: String,
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WorkflowDetail {
    pub id: String,
    pub file: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub steps: usize,
    pub inputs: BTreeMap<String, InputDetail>,
    pub outputs: Vec<String>,
    pub sources: Vec<SourceInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ValidateResult {
    pub valid: bool,
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ValidateIssue>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ValidateIssue {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

/// Combined schema type for the `run` command output.
///
/// The actual shape depends on flags and exit code:
/// - `Success`: `{"kind":"success","outputs":{...},"warnings":[...]?}` (exit 0, no `--dry-run`)
/// - `Error`: `{"kind":"error","error":"...","code":"...","warnings":[...]?}` (non-zero exit)
/// - `DryRun`: `{"kind":"dryRun","requests":[...],"warnings":[...]?}` (exit 0, `--dry-run`)
#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[allow(dead_code)] // Used only for schema generation via schema_for!()
pub enum RunOutput {
    Success {
        outputs: BTreeMap<String, Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
    },
    Error {
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
    },
    DryRun {
        requests: Vec<DryRunRequest>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
    },
}

pub fn output_json<T: Serialize + ?Sized>(value: &T) -> Result<(), String> {
    serde_json::to_writer_pretty(std::io::stdout(), value)
        .map_err(|err| format!("writing JSON: {err}"))?;
    println!();
    Ok(())
}

pub fn emit_validate_result(path: &str, spec: &ArazzoSpec, json: bool) -> Result<(), String> {
    if json {
        return output_json(&ValidateResult {
            valid: true,
            file: path.to_string(),
            version: Some(spec.arazzo.clone()),
            title: Some(spec.info.title.clone()),
            workflows: Some(spec.workflows.len()),
            sources: Some(spec.source_descriptions.len()),
            errors: Vec::new(),
        });
    }

    println!("Valid Arazzo {} spec: {}", spec.arazzo, spec.info.title);
    println!("  Version: {}", spec.info.version);
    println!("  Workflows: {}", spec.workflows.len());
    println!("  Sources: {}", spec.source_descriptions.len());
    Ok(())
}

pub fn emit_validate_error(path: &str, err: &ValidateError, json: bool) -> Result<(), String> {
    if json {
        return output_json(&ValidateResult {
            valid: false,
            file: path.to_string(),
            version: None,
            title: None,
            workflows: None,
            sources: None,
            errors: build_validate_issues(err),
        });
    }
    Err(format!("validation failed: {err}"))
}

fn build_validate_issues(err: &ValidateError) -> Vec<ValidateIssue> {
    match err {
        ValidateError::Validation(report) => report
            .errors
            .iter()
            .map(|item| ValidateIssue {
                source: "validation".to_string(),
                kind: Some(validation_error_kind_name(&item.kind).to_string()),
                path: if item.path.is_empty() {
                    None
                } else {
                    Some(item.path.clone())
                },
                message: item.message.clone(),
            })
            .collect(),
        ValidateError::ReadFile(inner) => vec![ValidateIssue {
            source: "readFile".to_string(),
            kind: None,
            path: None,
            message: format!("reading arazzo file: {inner}"),
        }],
        ValidateError::ParseYaml(inner) => vec![ValidateIssue {
            source: "parseYaml".to_string(),
            kind: None,
            path: None,
            message: format!("parsing arazzo yaml: {inner}"),
        }],
        ValidateError::ComponentResolution(message) => vec![ValidateIssue {
            source: "componentResolution".to_string(),
            kind: None,
            path: None,
            message: message.clone(),
        }],
    }
}

fn validation_error_kind_name(kind: &ValidationErrorKind) -> &'static str {
    match kind {
        ValidationErrorKind::MissingRequiredField => "missingRequiredField",
        ValidationErrorKind::DuplicateIdentifier => "duplicateIdentifier",
        ValidationErrorKind::InvalidStepTarget => "invalidStepTarget",
        ValidationErrorKind::UnsupportedVersion => "unsupportedVersion",
        ValidationErrorKind::InvalidParameterLocation => "invalidParameterLocation",
        ValidationErrorKind::MissingParameterValue => "missingParameterValue",
        ValidationErrorKind::InvalidExpression => "invalidExpression",
        ValidationErrorKind::InvalidReference => "invalidReference",
        ValidationErrorKind::InvalidRetryField => "invalidRetryField",
        ValidationErrorKind::InvalidCriterionType => "invalidCriterionType",
        _ => "unknown",
    }
}

pub fn emit_workflow_list(spec: &ArazzoSpec, json: bool) -> Result<(), String> {
    if json {
        let rows = spec
            .workflows
            .iter()
            .map(build_workflow_info)
            .collect::<Vec<_>>();
        return output_json(&rows);
    }

    println!("Workflows in {}:\n", spec.info.title);
    for wf in &spec.workflows {
        println!("  {}", wf.workflow_id);
        if !wf.summary.is_empty() {
            println!("    Summary: {}", wf.summary);
        }
        if let Some(inputs) = &wf.inputs {
            if !inputs.properties.is_empty() {
                println!("    Inputs:");
                for (name, prop) in &inputs.properties {
                    let required = if inputs.required.iter().any(|r| r == name) {
                        " (required)"
                    } else {
                        ""
                    };
                    let type_str = prop.type_.map_or("unknown".to_string(), |t| t.to_string());
                    println!("      - {name}: {type_str}{required}");
                }
            }
        }
        if !wf.outputs.is_empty() {
            let out = wf.outputs.keys().cloned().collect::<Vec<_>>();
            println!("    Outputs: {out:?}");
        }
        println!();
    }
    Ok(())
}

pub fn emit_catalog(entries: &[CatalogEntry], json: bool) -> Result<(), String> {
    if json {
        return output_json(entries);
    }

    println!("File                 Workflow ID   Summary");
    println!("-------------------  ------------  ----------------------------------------");
    for row in entries {
        for wf in &row.workflows {
            println!("{:<20} {:<12} {}", row.file, wf.id, wf.summary);
        }
    }
    Ok(())
}

pub fn emit_workflow_detail(
    spec: &ArazzoSpec,
    workflow: &Workflow,
    file: String,
    json: bool,
) -> Result<(), String> {
    if json {
        let mut inputs = BTreeMap::<String, InputDetail>::new();
        if let Some(schema) = &workflow.inputs {
            for (name, prop) in &schema.properties {
                let required = schema.required.iter().any(|r| r == name);
                inputs.insert(
                    name.clone(),
                    InputDetail {
                        type_: prop.type_.map_or(String::new(), |t| t.to_string()),
                        required,
                        description: prop.description.clone(),
                    },
                );
            }
        }

        return output_json(&WorkflowDetail {
            id: workflow.workflow_id.clone(),
            file,
            title: spec.info.title.clone(),
            summary: workflow.summary.clone(),
            steps: workflow.steps.len(),
            inputs,
            outputs: workflow.outputs.keys().cloned().collect(),
            sources: build_sources(spec),
        });
    }

    println!("Workflow: {}", workflow.workflow_id);
    println!("File:     {file}");
    println!("Title:    {}", spec.info.title);
    if !workflow.summary.is_empty() {
        println!("Summary:  {}", workflow.summary);
    }
    println!("Steps:    {}", workflow.steps.len());
    println!();

    if let Some(schema) = &workflow.inputs {
        if !schema.properties.is_empty() {
            println!("Inputs:");
            for (name, prop) in &schema.properties {
                let required = if schema.required.iter().any(|r| r == name) {
                    " (required)"
                } else {
                    ""
                };
                let desc = if prop.description.is_empty() {
                    String::new()
                } else {
                    format!(" - {}", prop.description)
                };
                let type_str = prop.type_.map_or("unknown".to_string(), |t| t.to_string());
                println!("  --input {name}=<{type_str}>{required}{desc}");
            }
            println!();
        }
    }

    if !workflow.outputs.is_empty() {
        println!("Outputs:");
        for name in workflow.outputs.keys() {
            println!("  {name}");
        }
    }
    Ok(())
}

pub fn emit_run_error(
    json: bool,
    err: &str,
    code: Option<&str>,
    warnings: &[String],
) -> Result<(), String> {
    if json {
        output_json(&RunOutput::Error {
            error: err.to_string(),
            code: code.map(String::from),
            warnings: warnings.to_vec(),
        })?;
        // Return Err so main() exits with code 1. Empty string signals
        // that the error message was already written to stdout as JSON.
        return Err(String::new());
    }
    Err(err.to_string())
}

pub fn emit_dry_run_requests(
    json: bool,
    reqs: Vec<DryRunRequest>,
    warnings: &[String],
) -> Result<(), String> {
    if json {
        return output_json(&RunOutput::DryRun {
            requests: reqs,
            warnings: warnings.to_vec(),
        });
    }
    for r in reqs {
        println!("{} {}", r.method, r.url);
        for (k, v) in r.headers {
            println!("  {k}: {v}");
        }
        if let Some(body) = r.body {
            println!("  Body: {body}");
        }
        println!();
    }
    Ok(())
}

pub fn emit_run_steps(steps: &[TraceStepRecord]) {
    for step in steps {
        let status_code = step
            .response
            .as_ref()
            .map(|r| r.status_code.to_string())
            .unwrap_or_else(|| "---".to_string());

        let method = step
            .request
            .as_ref()
            .map(|r| r.method.as_str())
            .unwrap_or("???");

        let url = step
            .request
            .as_ref()
            .map(|r| r.url.as_str())
            .unwrap_or(&step.operation_path);

        let retry_suffix = if step.attempt > 1 {
            format!(" (attempt {})", step.attempt)
        } else {
            String::new()
        };

        println!(
            "  [{status_code}] {method} {url}  ({duration}ms){retry_suffix}",
            duration = step.duration_ms,
        );
    }
    println!();
}

pub fn emit_run_outputs(
    outputs: &BTreeMap<String, Value>,
    json: bool,
    warnings: &[String],
) -> Result<(), String> {
    if json {
        return output_json(&RunOutput::Success {
            outputs: outputs.clone(),
            warnings: warnings.to_vec(),
        });
    }

    if outputs.is_empty() {
        println!("Workflow completed (no outputs)");
        return Ok(());
    }

    println!("Outputs:\n");
    for (key, value) in outputs {
        let display = format_value(value);
        println!("  {key}: {display}");
    }
    Ok(())
}

fn format_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        // For arrays/objects, fall back to compact JSON
        other => other.to_string(),
    }
}

pub fn build_sources(spec: &ArazzoSpec) -> Vec<SourceInfo> {
    spec.source_descriptions
        .iter()
        .map(|src| SourceInfo {
            name: src.name.clone(),
            url: src.url.clone(),
            type_: format!("{:?}", src.type_).to_lowercase(),
        })
        .collect()
}

pub fn build_workflow_info(wf: &Workflow) -> WorkflowInfo {
    let inputs = wf
        .inputs
        .as_ref()
        .map(|schema| schema.properties.keys().cloned().collect())
        .unwrap_or_default();
    let outputs = wf.outputs.keys().cloned().collect::<Vec<_>>();
    WorkflowInfo {
        id: wf.workflow_id.clone(),
        summary: wf.summary.clone(),
        inputs,
        outputs,
    }
}
