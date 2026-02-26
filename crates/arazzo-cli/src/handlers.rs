use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use arazzo_runtime::{ClientConfig, Engine, ExecutionOptions};
use arazzo_spec::ArazzoSpec;
use serde_json::Value;

use crate::output::{self, CatalogEntry};
use crate::run_context::{GlobalOptions, RunContext};
use crate::trace::{
    build_trace_file, prepare_trace_for_write, write_trace_file_atomic, TraceRunMetadata,
};

pub fn run_workflow(ctx: RunContext) -> Result<(), String> {
    let _trace_pipeline_version = crate::trace::INTERNAL_TRACE_PIPELINE_VERSION;
    let run = ctx.run;
    let global = ctx.global;
    let trace_enabled = run.trace.is_some() || global.verbose;

    let spec = match arazzo_validate::parse(&run.spec_path) {
        Ok(spec) => spec,
        Err(err) => {
            return if global.json {
                output::emit_run_error(true, &err.to_string(), None)
            } else {
                Err(format!("parsing spec: {err}"))
            };
        }
    };

    let mut inputs = BTreeMap::<String, Value>::new();
    for item in run.input_flags {
        let Some((k, v)) = item.split_once('=') else {
            return Err(format!(
                "invalid input format: \"{item}\" (expected key=value)"
            ));
        };
        inputs.insert(k.to_string(), parse_input_value(v));
    }

    if global.verbose {
        eprintln!("Executing workflow: {}", run.workflow_id);
        eprintln!("Inputs: {inputs:?}");
    }

    let mut cfg = ClientConfig {
        timeout: run.timeout,
        ..ClientConfig::default()
    };
    for header in run.header_flags {
        if let Some((k, v)) = header.split_once('=') {
            cfg.default_headers.insert(k.to_string(), v.to_string());
        }
    }

    let mut engine = Engine::with_client_config(spec, cfg)
        .map_err(|err| format!("creating runtime engine: {err}"))?;
    engine.set_parallel_mode(run.parallel);
    engine.set_dry_run_mode(run.dry_run);
    engine.set_trace_enabled(trace_enabled);

    let execution_timeout = run
        .timeout
        .checked_mul(10)
        .unwrap_or_else(|| std::time::Duration::from_secs(u64::MAX));

    let run_started_at = SystemTime::now();
    let run_started_inst = std::time::Instant::now();
    let outputs_result = engine.execute_with_options(
        &run.workflow_id,
        inputs.clone(),
        ExecutionOptions::with_timeout(execution_timeout),
    );
    let run_finished_at = SystemTime::now();
    let run_duration = run_started_inst.elapsed();

    let run_error_text = outputs_result.as_ref().err().map(ToString::to_string);
    let run_error_code = outputs_result
        .as_ref()
        .err()
        .map(|e| e.kind.code().to_string());
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
            engine.trace_steps(),
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
        return output::emit_run_error(global.json, &run_error, run_error_code.as_deref());
    }

    if let Some(trace_error) = trace_write_error {
        return Err(format!("writing trace: {trace_error}"));
    }

    if run.dry_run {
        return output::emit_dry_run_requests(global.json, engine.dry_run_requests());
    }

    let outputs = outputs_result.unwrap_or_default();
    if global.verbose && !global.json {
        output::emit_run_steps(&engine.trace_steps());
    }
    output::emit_run_outputs(&outputs, global.json)
}

pub fn validate_spec(path: &str, global: GlobalOptions) -> Result<(), String> {
    match arazzo_validate::parse(path) {
        Ok(spec) => output::emit_validate_result(path, &spec, global.json),
        Err(err) => output::emit_validate_error(path, &err.to_string(), global.json),
    }
}

pub fn list_workflows(path: &str, global: GlobalOptions) -> Result<(), String> {
    let spec = arazzo_validate::parse(path).map_err(|err| err.to_string())?;
    output::emit_workflow_list(&spec, global.json)
}

pub fn catalog_workflows(dir: &str, global: GlobalOptions) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("reading directory \"{dir}\": {err}"))?;

    let mut catalog = Vec::<CatalogEntry>::new();
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
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
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

    output::emit_catalog(&catalog, global.json)
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

    let mut matches = Vec::<String>::new();
    let mut match_spec: Option<ArazzoSpec> = None;
    let mut match_file = String::new();

    for entry in entries {
        let entry = match entry {
            Ok(v) => v,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
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

pub fn schema(command: Option<&str>) -> Result<(), String> {
    use schemars::schema_for;

    use crate::output::{CatalogEntry, RunOutput, ValidateResult, WorkflowDetail, WorkflowInfo};

    match command {
        Some("validate") => output::output_json(&schema_for!(ValidateResult)),
        Some("list") => output::output_json(&schema_for!(Vec<WorkflowInfo>)),
        Some("catalog") => output::output_json(&schema_for!(Vec<CatalogEntry>)),
        Some("show") => output::output_json(&schema_for!(WorkflowDetail)),
        Some("run") => output::output_json(&schema_for!(RunOutput)),
        Some(other) => Err(format!(
            "unknown command: \"{other}\". Available: validate, list, catalog, show, run"
        )),
        None => output::output_json(&["validate", "list", "catalog", "show", "run"]),
    }
}

fn parse_input_value(raw: &str) -> Value {
    let mut value = raw.to_string();
    if value.starts_with('$') {
        let var_name = value
            .trim_start_matches('$')
            .trim_matches(|c| c == '{' || c == '}');
        if let Ok(found) = std::env::var(var_name) {
            value = found;
        }
    }

    if let Ok(v) = value.parse::<f64>() {
        return serde_json::json!(v);
    }
    if value == "true" {
        return Value::Bool(true);
    }
    if value == "false" {
        return Value::Bool(false);
    }
    Value::String(value)
}
