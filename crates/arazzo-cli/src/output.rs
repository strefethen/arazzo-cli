use std::collections::BTreeMap;

use arazzo_runtime::DryRunRequest;
use arazzo_spec::{ArazzoSpec, Workflow};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub file: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub version: String,
    pub sources: Vec<SourceInfo>,
    pub workflows: Vec<WorkflowInfo>,
}

#[derive(Debug, Serialize)]
pub struct SourceInfo {
    pub name: String,
    pub url: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowInfo {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct InputDetail {
    #[serde(rename = "type")]
    pub type_: String,
    pub required: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowDetail {
    pub id: String,
    pub file: String,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    pub steps: usize,
    pub inputs: BTreeMap<String, InputDetail>,
    pub outputs: Vec<String>,
    pub sources: Vec<SourceInfo>,
}

#[derive(Debug, Serialize)]
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
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
                    println!("      - {}: {}{}", name, prop.type_, required);
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
                        type_: prop.type_.clone(),
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
                println!("  --input {}=<{}>{required}{desc}", name, prop.type_);
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

pub fn emit_run_error(json: bool, err: &str) -> Result<(), String> {
    if json {
        return output_json(&serde_json::json!({ "error": err }));
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

pub fn emit_run_outputs(outputs: &BTreeMap<String, Value>) -> Result<(), String> {
    output_json(outputs)
}

pub fn build_sources(spec: &ArazzoSpec) -> Vec<SourceInfo> {
    spec.source_descriptions
        .iter()
        .map(|src| SourceInfo {
            name: src.name.clone(),
            url: src.url.clone(),
            type_: src.type_.clone(),
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
