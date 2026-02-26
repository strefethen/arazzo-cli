use std::collections::BTreeMap;

use arazzo_runtime::{DryRunRequest, TraceStepRecord};
use arazzo_spec::{ArazzoSpec, Workflow};
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
    pub errors: Vec<String>,
}

/// Error response emitted by the `run` command on failure.
#[derive(Debug, Serialize, JsonSchema)]
pub struct RunError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Combined schema type for the `run` command output.
///
/// The actual shape depends on flags and exit code:
/// - `Success`: workflow outputs (exit 0, no `--dry-run`). Keys are workflow-defined.
/// - `Error`: error response (non-zero exit).
/// - `DryRun`: planned requests (exit 0, `--dry-run`).
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)] // Used only for schema generation via schema_for!()
pub enum RunOutput {
    Success(BTreeMap<String, Value>),
    Error(RunError),
    DryRun(Vec<DryRunRequest>),
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

pub fn emit_validate_error(path: &str, err: &str, json: bool) -> Result<(), String> {
    if json {
        return output_json(&ValidateResult {
            valid: false,
            file: path.to_string(),
            version: None,
            title: None,
            workflows: None,
            sources: None,
            errors: vec![err.to_string()],
        });
    }
    Err(format!("validation failed: {err}"))
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

pub fn emit_run_error(json: bool, err: &str, code: Option<&str>) -> Result<(), String> {
    if json {
        output_json(&RunError {
            error: err.to_string(),
            code: code.map(String::from),
        })?;
        // Return Err so main() exits with code 1. Empty string signals
        // that the error message was already written to stdout as JSON.
        return Err(String::new());
    }
    Err(err.to_string())
}

pub fn emit_dry_run_requests(json: bool, reqs: Vec<DryRunRequest>) -> Result<(), String> {
    if json {
        return output_json(&reqs);
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

pub fn emit_run_outputs(outputs: &BTreeMap<String, Value>, json: bool) -> Result<(), String> {
    if json {
        return output_json(outputs);
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
