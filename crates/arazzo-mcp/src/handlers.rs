//! Tool handler implementations for the MCP server.

use std::collections::BTreeMap;
use std::time::Duration;

use arazzo_runtime::{redacted_dry_run_request, ClientConfig, EngineBuilder};
use arazzo_spec::{ArazzoSpec, Step, StepTarget, Workflow};
use serde_json::{json, Value};

use crate::state::ServerState;

const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 30;
const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// Tool result helpers
// ---------------------------------------------------------------------------

fn tool_ok(value: &Value) -> Result<Value, String> {
    let text =
        serde_json::to_string_pretty(value).map_err(|err| format!("serializing result: {err}"))?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false,
    }))
}

fn tool_err(message: &str) -> Result<Value, String> {
    Ok(json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    }))
}

// ---------------------------------------------------------------------------
// Spec introspection helpers (adapted from arazzo-cli/src/output.rs)
// ---------------------------------------------------------------------------

fn build_workflow_info(wf: &Workflow, file: &str) -> Value {
    let inputs: Vec<String> = wf
        .inputs
        .as_ref()
        .map(|schema| schema.properties.keys().cloned().collect())
        .unwrap_or_default();
    let outputs: Vec<String> = wf.outputs.keys().cloned().collect();
    json!({
        "id": wf.workflow_id,
        "summary": wf.summary,
        "description": wf.description,
        "inputs": inputs,
        "outputs": outputs,
        "file": file,
    })
}

fn build_sources(spec: &ArazzoSpec) -> Vec<Value> {
    spec.source_descriptions
        .iter()
        .map(|src| {
            json!({
                "name": src.name,
                "url": src.url,
                "type": src.type_.to_string(),
            })
        })
        .collect()
}

fn build_step_summary(step: &Step) -> Value {
    let target = parse_step_target(step);
    let mut summary = json!({
        "stepId": step.step_id,
        "description": step.description,
        "method": target.method,
        "url": target.url,
        "operationId": target.operation_id,
        "referencedWorkflow": target.referenced_workflow,
    });
    if let Some(fields) = summary.as_object_mut() {
        if let Some(channel_path) = target.channel_path {
            fields.insert("channelPath".to_string(), Value::String(channel_path));
        }
        if let Some(action) = step.action {
            fields.insert("action".to_string(), Value::String(action.to_string()));
        }
        if let Some(timeout) = step.timeout {
            fields.insert("timeout".to_string(), Value::from(timeout));
        }
        if let Some(correlation_id) = &step.correlation_id {
            fields.insert(
                "correlationId".to_string(),
                Value::String(correlation_id.clone()),
            );
        }
        if !step.depends_on.is_empty() {
            fields.insert("dependsOn".to_string(), json!(step.depends_on));
        }
    }
    summary
}

#[derive(Default)]
struct ParsedStepTarget {
    method: Option<String>,
    url: Option<String>,
    operation_id: Option<String>,
    channel_path: Option<String>,
    referenced_workflow: Option<String>,
}

fn parse_step_target(step: &Step) -> ParsedStepTarget {
    match &step.target {
        Some(StepTarget::OperationPath(path)) => {
            let (method, url) = parse_operation_path(path, step.request_body.is_some());
            ParsedStepTarget {
                method: Some(method),
                url: Some(url),
                ..ParsedStepTarget::default()
            }
        }
        Some(StepTarget::OperationId(id)) => ParsedStepTarget {
            operation_id: Some(id.clone()),
            ..ParsedStepTarget::default()
        },
        Some(StepTarget::ChannelPath(path)) => ParsedStepTarget {
            channel_path: Some(path.clone()),
            ..ParsedStepTarget::default()
        },
        Some(StepTarget::WorkflowId(id)) => ParsedStepTarget {
            referenced_workflow: Some(id.clone()),
            ..ParsedStepTarget::default()
        },
        None => ParsedStepTarget::default(),
    }
}

fn parse_operation_path(path: &str, has_body: bool) -> (String, String) {
    let known = [
        "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
    ];
    if let Some((first, rest)) = path.split_once(' ') {
        let upper = first.to_uppercase();
        if known.contains(&upper.as_str()) {
            return (upper, rest.to_string());
        }
    }
    let method = if has_body { "POST" } else { "GET" };
    (method.to_string(), path.to_string())
}

// ---------------------------------------------------------------------------
// Handler: list_workflows
// ---------------------------------------------------------------------------

pub fn list_workflows(state: &ServerState) -> Result<Value, String> {
    let workflows: Vec<Value> = state
        .all_workflows()
        .iter()
        .map(|(loaded, wf)| build_workflow_info(wf, &loaded.file_path))
        .collect();
    tool_ok(&json!(workflows))
}

// ---------------------------------------------------------------------------
// Handler: describe_workflow
// ---------------------------------------------------------------------------

pub fn describe_workflow(state: &ServerState, args: &Value) -> Result<Value, String> {
    let workflow_id = args
        .get("workflow_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing required argument: workflow_id".to_string())?;

    let (loaded, wf) = match state.find_workflow(workflow_id) {
        Ok(found) => found,
        Err(err) => return tool_err(&err),
    };

    let mut inputs = BTreeMap::new();
    if let Some(schema) = &wf.inputs {
        for (name, prop) in &schema.properties {
            let required = schema.required.iter().any(|r| r == name);
            inputs.insert(
                name.clone(),
                json!({
                    "type": prop.type_.map_or("unknown".to_string(), |t| t.to_string()),
                    "required": required,
                    "description": prop.description,
                }),
            );
        }
    }

    let steps: Vec<Value> = wf.steps.iter().map(build_step_summary).collect();
    let output_names: Vec<String> = wf.outputs.keys().cloned().collect();
    let sources = build_sources(&loaded.spec);

    let detail = json!({
        "id": wf.workflow_id,
        "file": loaded.file_path,
        "title": loaded.spec.info.title,
        "summary": wf.summary,
        "description": wf.description,
        "stepCount": wf.steps.len(),
        "steps": steps,
        "inputs": inputs,
        "outputs": output_names,
        "sources": sources,
    });

    tool_ok(&detail)
}

// ---------------------------------------------------------------------------
// Handler: run_workflow
// ---------------------------------------------------------------------------

pub fn run_workflow(
    state: &ServerState,
    args: &Value,
    runtime: &tokio::runtime::Runtime,
) -> Result<Value, String> {
    let workflow_id = args
        .get("workflow_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing required argument: workflow_id".to_string())?;

    let (loaded, _wf) = match state.find_workflow(workflow_id) {
        Ok(found) => found,
        Err(err) => return tool_err(&err),
    };

    let inputs: BTreeMap<String, Value> = args
        .get("inputs")
        .and_then(Value::as_object)
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let parallel = args
        .get("parallel")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let http_timeout = Duration::from_secs(
        args.get("http_timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_HTTP_TIMEOUT_SECS),
    );

    let execution_timeout = Duration::from_secs(
        args.get("execution_timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_EXECUTION_TIMEOUT_SECS),
    );

    // Clone the spec because EngineBuilder takes ownership.
    let spec = loaded.spec.clone();

    let cfg = ClientConfig {
        timeout: http_timeout,
        ..ClientConfig::default()
    };

    let engine = match EngineBuilder::new(spec)
        .client_config(cfg)
        .parallel(parallel)
        .dry_run(dry_run)
        .strict_inputs(true)
        .build()
    {
        Ok(e) => e,
        Err(err) => return tool_err(&format!("building engine: {err}")),
    };

    let result = runtime.block_on(async {
        engine
            .execute_with_timeout(workflow_id, inputs, execution_timeout)
            .collect()
            .await
    });

    match result.outputs {
        Ok(ref outputs) => {
            if dry_run {
                let dry_run_reqs: Vec<_> = result
                    .dry_run_requests()
                    .into_iter()
                    .cloned()
                    .map(redacted_dry_run_request)
                    .collect();
                let reqs_json: Vec<Value> = dry_run_reqs
                    .iter()
                    .map(|r| {
                        json!({
                            "stepId": r.step_id,
                            "method": r.method,
                            "url": r.url,
                            "headers": r.headers,
                            "body": r.body,
                        })
                    })
                    .collect();
                tool_ok(&json!({ "kind": "dryRun", "requests": reqs_json }))
            } else {
                tool_ok(&json!({ "kind": "success", "outputs": outputs }))
            }
        }
        Err(err) => {
            let error_json = json!({
                "error": err.message,
                "code": err.code(),
            });
            let text = serde_json::to_string(&error_json)
                .unwrap_or_else(|_| format!("{{\"error\":\"{}\"}}", err.code()));
            tool_err(&text)
        }
    }
}

// ---------------------------------------------------------------------------
// Handler: validate_spec
// ---------------------------------------------------------------------------

pub fn validate_spec(state: &ServerState, args: &Value) -> Result<Value, String> {
    let file_path = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing required argument: file_path".to_string())?;

    if let Err(err) = state.check_path_allowed(file_path) {
        return tool_err(&err);
    }

    match arazzo_validate::parse(file_path) {
        Ok(spec) => tool_ok(&json!({
            "valid": true,
            "file": file_path,
            "version": spec.arazzo,
            "title": spec.info.title,
            "workflows": spec.workflows.len(),
            "sources": spec.source_descriptions.len(),
        })),
        Err(err) => {
            let errors = build_validate_errors(&err);
            tool_ok(&json!({
                "valid": false,
                "file": file_path,
                "errors": errors,
            }))
        }
    }
}

fn build_validate_errors(err: &arazzo_validate::Error) -> Vec<Value> {
    match err {
        arazzo_validate::Error::Validation(report) => report
            .errors
            .iter()
            .map(|item| {
                json!({
                    "source": "validation",
                    "path": if item.path.is_empty() { Value::Null } else { Value::String(item.path.clone()) },
                    "message": item.message,
                })
            })
            .collect(),
        arazzo_validate::Error::ReadFile(inner) => {
            vec![json!({ "source": "readFile", "message": format!("{inner}") })]
        }
        arazzo_validate::Error::ParseYaml(inner) => {
            vec![json!({ "source": "parseYaml", "message": format!("{inner}") })]
        }
        arazzo_validate::Error::ComponentResolution(msg) => {
            vec![json!({ "source": "componentResolution", "message": msg })]
        }
    }
}

// ---------------------------------------------------------------------------
// Handler: generate_workflow
// ---------------------------------------------------------------------------

pub fn generate_workflow(state: &ServerState, args: &Value) -> Result<Value, String> {
    let file_path = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing required argument: file_path".to_string())?;

    if let Err(err) = state.check_path_allowed(file_path) {
        return tool_err(&err);
    }

    let openapi = match arazzo_generate::parse_openapi_file(file_path) {
        Ok(spec) => spec,
        Err(err) => return tool_err(&err),
    };

    let result = match arazzo_generate::crud::generate_crud(&openapi, file_path) {
        Ok(r) => r,
        Err(err) => return tool_err(&err),
    };

    let yaml = serde_yaml_ng::to_string(&result.spec)
        .map_err(|err| format!("serializing Arazzo spec: {err}"))?;

    tool_ok(&json!({
        "yaml": yaml,
        "warnings": result.warnings,
        "resources": result.resources,
        "auth_type": result.auth_type,
    }))
}

// ---------------------------------------------------------------------------
// Handler: describe_openapi
// ---------------------------------------------------------------------------

pub fn describe_openapi(state: &ServerState, args: &Value) -> Result<Value, String> {
    let file_path = args
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing required argument: file_path".to_string())?;

    if let Err(err) = state.check_path_allowed(file_path) {
        return tool_err(&err);
    }

    let openapi = match arazzo_generate::parse_openapi_file(file_path) {
        Ok(spec) => spec,
        Err(err) => return tool_err(&err),
    };

    let description = arazzo_generate::openapi_describe::describe(&openapi);
    tool_ok(&description)
}

// ---------------------------------------------------------------------------
// Handler: generate_example
// ---------------------------------------------------------------------------

pub fn generate_example(args: &Value) -> Result<Value, String> {
    let schema = args
        .get("schema")
        .ok_or_else(|| "missing required argument: schema".to_string())?;

    let field_name = args
        .get("field_name")
        .and_then(Value::as_str)
        .unwrap_or("value");

    let example =
        arazzo_generate::standalone_example::generate_from_json_schema(schema, field_name);
    tool_ok(&example)
}

#[cfg(test)]
mod tests {
    use super::{build_sources, build_step_summary};

    #[test]
    fn step_summary_exposes_async_metadata_without_http_defaults() {
        let yaml = r#"
arazzo: "1.1.0"
info:
  title: Async Summary
  version: "1.0.0"
sourceDescriptions:
  - name: events
    type: asyncapi
    url: https://example.com/asyncapi.yaml
workflows:
  - workflowId: wait-for-event
    steps:
      - stepId: receive-event
        channelPath: '{$sourceDescriptions.events.url}#/channels/events'
        action: receive
        timeout: 5000
        correlationId: $inputs.eventId
        dependsOn:
          - publish-event
"#;
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("async summary fixture should parse: {err}"),
        };

        let summary = build_step_summary(&spec.workflows[0].steps[0]);
        assert_eq!(
            summary
                .get("channelPath")
                .and_then(serde_json::Value::as_str),
            Some("{$sourceDescriptions.events.url}#/channels/events")
        );
        assert_eq!(
            summary.get("action").and_then(serde_json::Value::as_str),
            Some("receive")
        );
        assert_eq!(
            summary.get("timeout").and_then(serde_json::Value::as_u64),
            Some(5000)
        );
        assert_eq!(
            summary
                .get("correlationId")
                .and_then(serde_json::Value::as_str),
            Some("$inputs.eventId")
        );
        assert_eq!(
            summary.get("dependsOn"),
            Some(&serde_json::json!(["publish-event"]))
        );
        assert_eq!(summary.get("method"), Some(&serde_json::Value::Null));
        assert_eq!(summary.get("url"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn source_summaries_preserve_exact_source_type_wire_values() {
        let yaml = r#"
arazzo: "1.1.0"
info:
  title: Source Summary
  version: "1.0.0"
sourceDescriptions:
  - name: openapiSource
    type: openapi
    url: https://example.com/openapi.yaml
  - name: arazzoSource
    type: arazzo
    url: https://example.com/child.arazzo.yaml
  - name: asyncapiSource
    type: asyncapi
    url: https://example.com/asyncapi.yaml
workflows:
  - workflowId: inspect-sources
    steps:
      - stepId: inspect
        operationPath: /inspect
"#;
        let spec = match arazzo_validate::parse_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("source summary fixture should validate: {err}"),
        };

        let sources = build_sources(&spec);
        let source_types = sources
            .iter()
            .map(|source| {
                source
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_else(|| panic!("source summary should contain a string type"))
            })
            .collect::<Vec<_>>();

        assert_eq!(source_types, ["openapi", "arazzo", "asyncapi"]);
    }
}
