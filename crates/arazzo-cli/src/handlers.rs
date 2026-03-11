use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use arazzo_runtime::{ClientConfig, EngineBuilder, EngineEvent, TraceStepRecord};
use arazzo_spec::ArazzoSpec;
use arazzo_validate::Error as ValidateError;
use serde_json::Value;

use crate::cli::ExpressionDiagnosticsMode;
use crate::output::{self, CatalogEntry};
use crate::run_context::{GlobalOptions, RunContext};
use crate::trace::{
    build_trace_file, prepare_trace_for_write, write_trace_file_atomic, TraceRunMetadata,
};

pub async fn run_workflow(ctx: RunContext) -> Result<(), String> {
    let _trace_pipeline_version = crate::trace::INTERNAL_TRACE_PIPELINE_VERSION;
    let run = ctx.run;
    let global = ctx.global;
    let trace_enabled = run.trace.is_some()
        || global.verbose
        || run.expr_diagnostics != ExpressionDiagnosticsMode::Off;

    let spec = match arazzo_validate::parse(&run.spec_path) {
        Ok(spec) => spec,
        Err(err) => {
            return if global.json {
                output::emit_run_error(
                    true,
                    &err.to_string(),
                    Some(run_parse_error_code(&err)),
                    &[],
                )
            } else {
                Err(format!("parsing spec: {err}"))
            };
        }
    };

    let mut inputs = BTreeMap::<String, Value>::new();
    for item in run.input_flags {
        let (key, raw_value) = parse_input_kv(&item)?;
        inputs.insert(key, parse_input_value(raw_value));
    }
    for item in run.input_json_flags {
        let (key, raw_value) = parse_input_kv(&item)?;
        let value = serde_json::from_str::<Value>(raw_value).map_err(|err| {
            format!("invalid JSON input format for \"{key}\": {err} (expected key=<json>)")
        })?;
        inputs.insert(key, value);
    }

    if global.verbose {
        eprintln!("Executing workflow: {}", run.workflow_id);
        eprintln!("Inputs: {inputs:?}");
    }

    let mut cfg = ClientConfig {
        timeout: run.http_timeout,
        ..ClientConfig::default()
    };
    for header in run.header_flags {
        if let Some((k, v)) = header.split_once('=') {
            cfg.default_headers.insert(k.to_string(), v.to_string());
        }
    }

    let mut builder = EngineBuilder::new(spec)
        .client_config(cfg)
        .parallel(run.parallel)
        .dry_run(run.dry_run)
        .strict_inputs(run.strict_inputs)
        .trace(trace_enabled);

    for openapi_path in &run.openapi_flags {
        let bytes = match fs::read(openapi_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let msg = format!("reading OpenAPI file \"{openapi_path}\": {err}");
                return if global.json {
                    output::emit_run_error(true, &msg, Some("RUN_OPENAPI_READ_FILE"), &[])
                } else {
                    Err(msg)
                };
            }
        };
        builder = builder.openapi_spec(bytes);
    }

    let engine = builder
        .build()
        .map_err(|err| format!("creating runtime engine: {err}"))?;

    let execution_timeout = run.execution_timeout;

    let run_started_at = SystemTime::now();
    let run_started_inst = std::time::Instant::now();
    let exec_result = if let Some(step_id) = &run.step_id {
        let handle = engine.execute_step(&run.workflow_id, step_id, inputs.clone(), run.no_deps);
        let cancel = handle.cancel_token().clone();
        let timeout_flag = handle.timeout_flag().clone();
        tokio::spawn(async move {
            tokio::time::sleep(execution_timeout).await;
            timeout_flag.store(true, std::sync::atomic::Ordering::Release);
            cancel.cancel();
        });
        handle.collect().await
    } else {
        engine
            .execute_with_timeout(&run.workflow_id, inputs.clone(), execution_timeout)
            .collect()
            .await
    };
    let run_finished_at = SystemTime::now();
    let run_duration = run_started_inst.elapsed();

    let run_error_text = exec_result.outputs.as_ref().err().map(ToString::to_string);
    let run_error_code = exec_result
        .outputs
        .as_ref()
        .err()
        .map(|e| e.kind.code().to_string());

    let trace_steps: Vec<TraceStepRecord> = exec_result
        .events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::TraceStep(r) => Some(r.clone()),
            _ => None,
        })
        .collect();

    let dry_run_requests: Vec<arazzo_runtime::DryRunRequest> = exec_result
        .events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::DryRunRequest(r) => Some(r.clone()),
            _ => None,
        })
        .collect();

    let expression_warnings = if run.expr_diagnostics == ExpressionDiagnosticsMode::Off {
        Vec::new()
    } else {
        collect_expression_warnings(&trace_steps)
    };
    let mut trace_write_error: Option<String> = None;

    if let Some(trace_path) = &run.trace {
        let mut trace_file = build_trace_file(
            TraceRunMetadata {
                spec_path: run.spec_path.clone(),
                workflow_id: run.workflow_id.clone(),
                parallel: run.parallel,
                dry_run: run.dry_run,
                timeout_ms: u64::try_from(execution_timeout.as_millis()).unwrap_or(u64::MAX),
                started_at: run_started_at,
                finished_at: run_finished_at,
                duration_ms: u64::try_from(run_duration.as_millis()).unwrap_or(u64::MAX),
                run_error: run_error_text.clone(),
            },
            inputs,
            trace_steps.clone(),
        );
        prepare_trace_for_write(&mut trace_file, run.trace_max_body_bytes);
        if let Err(err) = write_trace_file_atomic(Path::new(trace_path), &trace_file) {
            trace_write_error = Some(err);
        }
    }

    if let Some(run_error) = run_error_text {
        if let Some(trace_error) = trace_write_error {
            return Err(format!("{run_error}; writing trace: {trace_error}"));
        }
        return output::emit_run_error(
            global.json,
            &run_error,
            run_error_code.as_deref(),
            &expression_warnings,
        );
    }

    if let Some(trace_error) = trace_write_error {
        return Err(format!("writing trace: {trace_error}"));
    }

    if !expression_warnings.is_empty() {
        match run.expr_diagnostics {
            ExpressionDiagnosticsMode::Off => {}
            ExpressionDiagnosticsMode::Warn => {
                if !global.json {
                    emit_expression_warnings(&expression_warnings);
                }
            }
            ExpressionDiagnosticsMode::Error => {
                return output::emit_run_error(
                    global.json,
                    &format!(
                        "expression diagnostics reported {} warning(s)",
                        expression_warnings.len()
                    ),
                    Some("RUNTIME_EXPRESSION_DIAGNOSTICS"),
                    &expression_warnings,
                );
            }
        }
    }

    if run.dry_run {
        return output::emit_dry_run_requests(global.json, dry_run_requests, &expression_warnings);
    }

    let outputs = exec_result.outputs.unwrap_or_default();
    if global.verbose && !global.json {
        output::emit_run_steps(&trace_steps);
    }
    output::emit_run_outputs(&outputs, global.json, &expression_warnings)
}

pub fn generate_workflow(
    spec_path: &str,
    scenario: &str,
    output_path: Option<&str>,
    global: GlobalOptions,
) -> Result<(), String> {
    let bytes = fs::read(spec_path)
        .map_err(|err| format!("reading OpenAPI spec \"{spec_path}\": {err}"))?;

    let openapi: openapiv3::OpenAPI =
        serde_yml::from_slice(&bytes).map_err(|err| format!("parsing OpenAPI spec: {err}"))?;

    if scenario != "crud" {
        return Err(format!("unknown scenario \"{scenario}\"; available: crud"));
    }

    let result = crate::generate::generate_crud(&openapi, spec_path)?;

    let yaml = serde_yml::to_string(&result.spec)
        .map_err(|err| format!("serializing Arazzo spec: {err}"))?;

    for warning in &result.warnings {
        eprintln!("warning: {warning}");
    }

    if let Some(path) = output_path {
        fs::write(path, &yaml).map_err(|err| format!("writing output file \"{path}\": {err}"))?;
        output::emit_generate_result(path, &result, global.json)
    } else {
        print!("{yaml}");
        Ok(())
    }
}

pub fn validate_spec(path: &str, global: GlobalOptions) -> Result<(), String> {
    match arazzo_validate::parse(path) {
        Ok(spec) => output::emit_validate_result(path, &spec, global.json),
        Err(err) => output::emit_validate_error(path, &err, global.json),
    }
}

pub fn list_workflows(path: &str, global: GlobalOptions) -> Result<(), String> {
    let spec = arazzo_validate::parse(path).map_err(|err| err.to_string())?;
    output::emit_workflow_list(&spec, global.json)
}

pub fn catalog_workflows(dir: &str, global: GlobalOptions) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("reading directory \"{dir}\": {err}"))?;
    let mut paths = Vec::<PathBuf>::new();
    for entry in entries {
        let entry = match entry {
            Ok(v) => v,
            Err(err) => {
                if global.verbose {
                    eprintln!("skipping entry: {err}");
                }
                continue;
            }
        };
        paths.push(entry.path());
    }
    paths.sort_unstable();

    let mut catalog = Vec::<CatalogEntry>::new();
    for path in paths {
        if !is_arazzo_yaml_path(&path) {
            continue;
        }

        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let spec = match arazzo_validate::parse(&path) {
            Ok(spec) => spec,
            Err(err) => {
                if global.verbose {
                    eprintln!("skipping {file_name}: {err}");
                }
                continue;
            }
        };
        let workflows = spec
            .workflows
            .iter()
            .map(output::build_workflow_info)
            .collect::<Vec<_>>();
        catalog.push(CatalogEntry {
            file: file_name,
            title: spec.info.title.clone(),
            description: spec.info.description.clone(),
            version: spec.info.version.clone(),
            sources: output::build_sources(&spec),
            workflows,
        });
    }
    catalog.sort_unstable_by(|a, b| a.file.cmp(&b.file));

    output::emit_catalog(&catalog, global.json)
}

pub fn list_steps(path: &str, workflow_id: &str, global: GlobalOptions) -> Result<(), String> {
    let spec = arazzo_validate::parse(path).map_err(|err| err.to_string())?;
    let workflow = spec
        .workflows
        .iter()
        .find(|wf| wf.workflow_id == workflow_id)
        .ok_or_else(|| format!("workflow \"{workflow_id}\" not found in {path}"))?;
    output::emit_step_list(path, workflow, global.json)
}

pub fn show_workflow(workflow_id: &str, dir: &str, global: GlobalOptions) -> Result<(), String> {
    let (spec, file) = find_workflow(dir, workflow_id)?;
    let workflow = spec
        .workflows
        .iter()
        .find(|wf| wf.workflow_id == workflow_id)
        .ok_or_else(|| format!("workflow \"{workflow_id}\" not found in {dir}"))?;
    output::emit_workflow_detail(&spec, workflow, file, global.json)
}

fn find_workflow(dir: &str, workflow_id: &str) -> Result<(ArazzoSpec, String), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("reading directory \"{dir}\": {err}"))?;
    let mut paths = Vec::<PathBuf>::new();
    for entry in entries {
        let entry = match entry {
            Ok(v) => v,
            Err(_) => continue,
        };
        paths.push(entry.path());
    }
    paths.sort_unstable();

    let mut matches = Vec::<String>::new();
    let mut match_spec: Option<ArazzoSpec> = None;
    let mut match_file = String::new();

    for path in paths {
        if !is_arazzo_yaml_path(&path) {
            continue;
        }
        let spec = match arazzo_validate::parse(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for wf in &spec.workflows {
            if wf.workflow_id == workflow_id {
                let file = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                matches.push(file.clone());
                match_spec = Some(spec.clone());
                match_file = file;
            }
        }
    }
    matches.sort_unstable();

    if matches.is_empty() {
        return Err(format!("workflow \"{workflow_id}\" not found in {dir}"));
    }
    if matches.len() > 1 {
        return Err(format!(
            "workflow \"{workflow_id}\" found in multiple files: {matches:?}"
        ));
    }

    match match_spec {
        Some(spec) => Ok((spec, match_file)),
        None => Err(format!("workflow \"{workflow_id}\" not found in {dir}")),
    }
}

fn is_arazzo_yaml_path(path: &Path) -> bool {
    match path.extension().and_then(|s| s.to_str()) {
        Some(ext) => {
            let ext = ext.to_ascii_lowercase();
            ext == "yaml" || ext == "yml"
        }
        None => false,
    }
}

fn run_parse_error_code(err: &ValidateError) -> &'static str {
    match err {
        ValidateError::ReadFile(_) => "RUN_SPEC_READ_FILE",
        ValidateError::ParseYaml(_) => "RUN_SPEC_PARSE_YAML",
        ValidateError::Validation(_) => "RUN_SPEC_VALIDATION",
        ValidateError::ComponentResolution(_) => "RUN_SPEC_COMPONENT_RESOLUTION",
    }
}

fn collect_expression_warnings(steps: &[TraceStepRecord]) -> Vec<String> {
    let mut warnings = Vec::new();
    for step in steps {
        for warning in &step.warnings {
            warnings.push(format!(
                "workflow \"{}\" step \"{}\": {}",
                step.workflow_id, step.step_id, warning
            ));
        }
    }
    warnings
}

fn emit_expression_warnings(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}

fn parse_input_kv(raw: &str) -> Result<(String, &str), String> {
    let Some((key, value)) = raw.split_once('=') else {
        return Err(format!(
            "invalid input format: \"{raw}\" (expected key=value)"
        ));
    };
    Ok((key.to_string(), value))
}

pub fn schema(command: Option<&str>) -> Result<(), String> {
    use schemars::schema_for;

    use crate::output::{
        CatalogEntry, GenerateResult, RunOutput, StepInfo, ValidateResult, WorkflowDetail,
        WorkflowInfo,
    };

    match command {
        Some("validate") => output::output_json(&schema_for!(ValidateResult)),
        Some("list") => output::output_json(&schema_for!(Vec<WorkflowInfo>)),
        Some("catalog") => output::output_json(&schema_for!(Vec<CatalogEntry>)),
        Some("show") => output::output_json(&schema_for!(WorkflowDetail)),
        Some("steps") => output::output_json(&schema_for!(Vec<StepInfo>)),
        Some("run") => output::output_json(&schema_for!(RunOutput)),
        Some("generate") => output::output_json(&schema_for!(GenerateResult)),
        Some(other) => Err(format!(
            "unknown command: \"{other}\". Available: validate, list, catalog, show, steps, run, generate"
        )),
        None => output::output_json(&[
            "validate", "list", "catalog", "show", "steps", "run", "generate",
        ]),
    }
}

fn parse_input_value(raw: &str) -> Value {
    // Single-quoted values bypass coercion: 'true' → String("true")
    if let Some(inner) = raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return Value::String(inner.to_string());
    }

    let mut value = raw.to_string();
    if value.starts_with('$') {
        let var_name = value
            .trim_start_matches('$')
            .trim_matches(|c| c == '{' || c == '}');
        if let Ok(found) = std::env::var(var_name) {
            value = found;
        }
    }

    if value == "true" {
        return Value::Bool(true);
    }
    if value == "false" {
        return Value::Bool(false);
    }
    if value == "null" {
        return Value::Null;
    }
    if let Ok(v) = value.parse::<i64>() {
        return serde_json::json!(v);
    }
    if let Ok(v) = value.parse::<u64>() {
        return serde_json::json!(v);
    }
    if let Ok(v) = value.parse::<f64>() {
        return serde_json::json!(v);
    }
    Value::String(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_input_value_coerces_bool() {
        assert_eq!(parse_input_value("true"), Value::Bool(true));
        assert_eq!(parse_input_value("false"), Value::Bool(false));
    }

    #[test]
    fn parse_input_value_coerces_number() {
        assert_eq!(parse_input_value("123"), serde_json::json!(123));
    }

    #[test]
    fn parse_input_value_single_quotes_bypass_coercion() {
        assert_eq!(
            parse_input_value("'true'"),
            Value::String("true".to_string())
        );
        assert_eq!(
            parse_input_value("'false'"),
            Value::String("false".to_string())
        );
        assert_eq!(parse_input_value("'123'"), Value::String("123".to_string()));
        assert_eq!(
            parse_input_value("'null'"),
            Value::String("null".to_string())
        );
    }

    #[test]
    fn parse_input_value_plain_string_unchanged() {
        assert_eq!(
            parse_input_value("hello"),
            Value::String("hello".to_string())
        );
    }
}
