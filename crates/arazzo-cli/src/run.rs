use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use arazzo_runtime::{
    is_sensitive_key, ClientConfig, EngineBuilder, EngineEvent, SequentialFallback,
    TraceStepRecord, TransportWarning, REDACTED,
};
use arazzo_validate::Error as ValidateError;
use serde_json::Value;

use crate::cli::ExpressionDiagnosticsMode;
use crate::handlers::{parse_input_kv, parse_input_value};
use crate::output;
use crate::run_context::RunContext;
use crate::run_export::{self, RunMetadata};
use crate::trace::{
    build_trace_file, prepare_trace_for_write, write_trace_file_atomic, TraceRunMetadata,
};
use crate::transport;

pub async fn run_workflow(ctx: RunContext) -> Result<(), String> {
    let _trace_pipeline_version = crate::trace::INTERNAL_TRACE_PIPELINE_VERSION;
    let run = ctx.run;
    let global = ctx.global;
    if let Some(export_path) = &run.export_run {
        let mut sources = vec![Path::new(&run.spec_path)];
        sources.extend(
            run.openapi_flags
                .iter()
                .map(|path| Path::new(path.as_str())),
        );
        if let Err(err) = run_export::validate_destination(
            Path::new(export_path),
            run.trace.as_deref().map(Path::new),
            &sources,
        ) {
            return output::emit_run_error(global.json, &err, Some("RUN_EXPORT_PATH"), &[], &[]);
        }
    }
    let trace_enabled = run.trace.is_some()
        || global.verbose
        || run.expr_diagnostics != ExpressionDiagnosticsMode::Off;

    // Startup insecure-exception notice: live runs only (run is always
    // live; replay has no transport flags). Stderr line unless
    // squelched; the structured entry persists regardless.
    let squelched = run.transport.no_transport_warnings;
    let mut transport_warnings: Vec<TransportWarning> = Vec::new();
    if let Some(warning) = run.transport.startup_warning() {
        transport::eprint_warning(&warning, squelched);
        transport_warnings.push(warning);
    }

    let spec = match arazzo_validate::parse(&run.spec_path) {
        Ok(spec) => spec,
        Err(err) => {
            return if global.json {
                output::emit_run_error(
                    true,
                    &err.to_string(),
                    Some(run_parse_error_code(&err)),
                    &[],
                    &transport_warnings,
                )
            } else {
                Err(format!("parsing spec: {err}"))
            };
        }
    };

    if let Some(export_path) = &run.export_run {
        let base_dir = Path::new(&run.spec_path).parent().unwrap_or(Path::new("."));
        let local_sources = arazzo_runtime::relative_openapi_source_paths(&spec, base_dir);
        let mut sources = vec![PathBuf::from(&run.spec_path)];
        sources.extend(run.openapi_flags.iter().map(PathBuf::from));
        sources.extend(local_sources.into_iter().map(|(_, path)| path));
        let source_refs: Vec<&Path> = sources.iter().map(PathBuf::as_path).collect();
        if let Err(err) = run_export::validate_destination(
            Path::new(export_path),
            run.trace.as_deref().map(Path::new),
            &source_refs,
        ) {
            return output::emit_run_error(
                global.json,
                &err,
                Some("RUN_EXPORT_PATH"),
                &[],
                &transport_warnings,
            );
        }
    }

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
        let redacted: BTreeMap<&str, _> = inputs
            .iter()
            .map(|(k, v)| {
                if is_sensitive_key(k) {
                    (k.as_str(), serde_json::Value::String(REDACTED.into()))
                } else {
                    (k.as_str(), v.clone())
                }
            })
            .collect();
        eprintln!("Inputs: {redacted:?}");
    }

    let mut cfg = ClientConfig {
        timeout: run.http_timeout,
        ..ClientConfig::default()
    };
    run.transport.apply(&mut cfg, true);
    for header in run.header_flags {
        let parsed = header
            .split_once(':')
            .map(|(k, v)| (k.trim(), v.trim_start()))
            .or_else(|| header.split_once('=').map(|(k, v)| (k.trim(), v)));
        if let Some((k, v)) = parsed {
            cfg.default_headers.insert(k.to_string(), v.to_string());
        } else {
            eprintln!(
                "warning: ignoring malformed header (expected 'Name: value' or 'Name=value'): {header}"
            );
        }
    }

    let mut builder = EngineBuilder::new(spec)
        .client_config(cfg)
        .parallel(run.parallel)
        .dry_run(run.dry_run)
        .strict_inputs(run.strict_inputs)
        .trace(trace_enabled)
        .capture_run(run.export_run.is_some());

    if let Some(dir) = Path::new(&run.spec_path).parent() {
        builder = builder.source_base_dir(dir);
    }

    if let Some(max_bytes) = run.max_response_size {
        builder = builder.max_response_bytes(max_bytes);
    }

    for openapi_path in &run.openapi_flags {
        let bytes = match fs::read(openapi_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let msg = format!("reading OpenAPI file \"{openapi_path}\": {err}");
                return if global.json {
                    output::emit_run_error(
                        true,
                        &msg,
                        Some("RUN_OPENAPI_READ_FILE"),
                        &[],
                        &transport_warnings,
                    )
                } else {
                    Err(msg)
                };
            }
        };
        builder = builder.openapi_spec(bytes, Some(PathBuf::from(openapi_path)));
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

    // Engine-emitted transport warnings (already on stderr when not
    // squelched) join the structured list for --json output and traces.
    transport_warnings.extend(exec_result.events.iter().filter_map(|e| match e {
        EngineEvent::TransportWarning(w) => Some(w.clone()),
        _ => None,
    }));
    // End-of-run unused-exception note. Skipped for dry runs: nothing
    // was sent, so "no request targeted it" would be vacuous noise.
    if !run.dry_run {
        if let Some(warning) = transport::unused_warning(engine.unused_insecure_hosts()) {
            transport::eprint_warning(&warning, squelched);
            transport_warnings.push(warning);
        }
    }

    // A workflow invoked more than once reports the same fallback each time.
    let mut sequential_fallbacks = Vec::<SequentialFallback>::new();
    for fallback in exec_result.sequential_fallbacks() {
        if !sequential_fallbacks.contains(fallback) {
            sequential_fallbacks.push(fallback.clone());
        }
    }
    if global.verbose {
        for fallback in &sequential_fallbacks {
            eprintln!("Parallel mode: {}", fallback.message);
        }
    }

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
                transport_warnings: transport_warnings.clone(),
                sequential_fallbacks,
            },
            inputs.clone(),
            trace_steps.clone(),
        );
        prepare_trace_for_write(&mut trace_file, run.trace_max_body_bytes);
        if let Err(err) = write_trace_file_atomic(Path::new(trace_path), &trace_file) {
            trace_write_error = Some(err);
        }
    }

    let mut export_write_error = None;
    if let Some(export_path) = &run.export_run {
        let export = run_export::build(
            RunMetadata {
                spec_path: &run.spec_path,
                workflow_id: &run.workflow_id,
                step_id: run.step_id.as_deref(),
                parallel: run.parallel,
                dry_run: run.dry_run,
                started_at: run_started_at,
                finished_at: run_finished_at,
                duration_ms: u64::try_from(run_duration.as_millis()).unwrap_or(u64::MAX),
            },
            &inputs,
            &exec_result,
        );
        let written =
            export.and_then(|artifact| run_export::write_atomic(Path::new(export_path), &artifact));
        if let Err(err) = written {
            eprintln!("writing run export: {err}");
            export_write_error = Some(err);
        }
    }

    if let Some(export_error) = export_write_error {
        let mut causes = Vec::new();
        if let Some(run_error) = &run_error_text {
            causes.push(run_error.clone());
        }
        if let Some(trace_error) = &trace_write_error {
            causes.push(format!("writing trace: {trace_error}"));
        }
        causes.push(format!("writing run export: {export_error}"));
        return output::emit_run_error(
            global.json,
            &causes.join("; "),
            Some("RUN_EXPORT_WRITE"),
            &expression_warnings,
            &transport_warnings,
        );
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
            &transport_warnings,
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
                    &transport_warnings,
                );
            }
        }
    }

    if run.dry_run {
        return output::emit_dry_run_requests(
            global.json,
            dry_run_requests,
            &expression_warnings,
            &transport_warnings,
        );
    }

    let outputs = exec_result.outputs.unwrap_or_default();
    if global.verbose && !global.json {
        output::emit_run_steps(&trace_steps);
    }
    output::emit_run_outputs(
        &outputs,
        global.json,
        &expression_warnings,
        &transport_warnings,
    )
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
