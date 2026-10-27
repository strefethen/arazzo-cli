use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use arazzo_runtime::{
    redact_inputs, sensitive_input_names, ClientConfig, EngineBuilder, EngineEvent,
    SequentialFallback, TraceStepRecord, TransportWarning,
};
use serde_json::Value;

use crate::cli::ExpressionDiagnosticsMode;
use crate::cli_error::{self, CliError, SpecErrorCodes, CLI_OUTPUT_WRITE};
use crate::handlers::{parse_input_kv, parse_input_value};
use crate::message_transport::{ArmedLinePrinter, RUN_MESSAGE_CODES};
use crate::output;
use crate::run_context::RunContext;
use crate::run_export::{self, RunMetadata};
use crate::trace::{
    build_trace_file, prepare_trace_for_write, write_trace_file_atomic, TraceRunMetadata,
};
use crate::transport;

const RUN_SPEC_CODES: SpecErrorCodes = SpecErrorCodes {
    read_file: "RUN_SPEC_READ_FILE",
    parse_yaml: "RUN_SPEC_PARSE_YAML",
    validation: "RUN_SPEC_VALIDATION",
    component_resolution: "RUN_SPEC_COMPONENT_RESOLUTION",
};

pub async fn run_workflow(ctx: RunContext) -> Result<(), CliError> {
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
            return Err(output::run_error("RUN_EXPORT_PATH", err, &[], &[]));
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

    // Kafka flags are validated (and the CA file read) before the spec is
    // parsed, so a malformed flag never reaches an engine or a broker.
    let message_options = run
        .message_transport
        .into_options(&RUN_MESSAGE_CODES)
        .map_err(|err| {
            output::run_error(
                err.code(),
                err.message().to_string(),
                &[],
                &transport_warnings,
            )
        })?;

    let spec = match arazzo_validate::parse(&run.spec_path) {
        Ok(spec) => spec,
        Err(err) => {
            let code = cli_error::spec_error(&err, &RUN_SPEC_CODES).code();
            let message = if global.json {
                err.to_string()
            } else {
                format!("parsing spec: {err}")
            };
            return Err(output::run_error(code, message, &[], &transport_warnings));
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
            return Err(output::run_error(
                "RUN_EXPORT_PATH",
                err,
                &[],
                &transport_warnings,
            ));
        }
    }

    let input_error =
        |message: String| output::run_error("RUN_INPUT_FORMAT", message, &[], &transport_warnings);
    let mut inputs = BTreeMap::<String, Value>::new();
    for item in run.input_flags {
        let (key, raw_value) = parse_input_kv(&item).map_err(input_error)?;
        inputs.insert(key, parse_input_value(raw_value));
    }
    for item in run.input_json_flags {
        let (key, raw_value) = parse_input_kv(&item).map_err(input_error)?;
        let value = serde_json::from_str::<Value>(raw_value).map_err(|err| {
            input_error(format!(
                "invalid JSON input format for \"{key}\": {err} (expected key=<json>)"
            ))
        })?;
        inputs.insert(key, value);
    }

    // One sensitive input-name set feeds both the `--verbose` echo and the
    // trace file, so the two sinks cannot disagree about what is a secret.
    let sensitive_inputs = sensitive_input_names(
        spec.workflows
            .iter()
            .find(|workflow| workflow.workflow_id == run.workflow_id)
            .and_then(|workflow| workflow.inputs.as_ref()),
        inputs.keys().map(String::as_str),
    );

    if global.verbose {
        eprintln!("Executing workflow: {}", run.workflow_id);
        // Same redaction as `trace.inputs`, nested key names included.
        let mut redacted = inputs.clone();
        redact_inputs(&mut redacted, &sensitive_inputs);
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
        .message_transport(message_options)
        .parallel(run.parallel)
        .dry_run(run.dry_run)
        .strict_inputs(run.strict_inputs)
        .trace(trace_enabled)
        .capture_run(run.export_run.is_some());

    // `collect()` drains events at the end; the armed line must be live so a
    // user knows when producing is safe.
    if global.verbose {
        builder = builder.observer(ArmedLinePrinter::shared());
    }

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
                return Err(output::run_error(
                    "RUN_OPENAPI_READ_FILE",
                    format!("reading OpenAPI file \"{openapi_path}\": {err}"),
                    &[],
                    &transport_warnings,
                ));
            }
        };
        builder = builder.openapi_spec(bytes, Some(PathBuf::from(openapi_path)));
    }

    let engine = builder.build().map_err(|err| {
        output::run_error(
            err.code(),
            format!("creating runtime engine: {err}"),
            &[],
            &transport_warnings,
        )
    })?;

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
    let run_error_code = exec_result.outputs.as_ref().err().map(|e| e.kind.code());

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
        prepare_trace_for_write(&mut trace_file, run.trace_max_body_bytes, &sensitive_inputs);
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
        return Err(output::run_error(
            "RUN_EXPORT_WRITE",
            causes.join("; "),
            &expression_warnings,
            &transport_warnings,
        ));
    }

    if let Some(trace_error) = trace_write_error {
        let message = match &run_error_text {
            Some(run_error) => format!("{run_error}; writing trace: {trace_error}"),
            None => format!("writing trace: {trace_error}"),
        };
        return Err(output::run_error(
            "RUN_TRACE_WRITE",
            message,
            &expression_warnings,
            &transport_warnings,
        ));
    }

    if let (Some(run_error), Some(code)) = (run_error_text, run_error_code) {
        return Err(output::run_error(
            code,
            run_error,
            &expression_warnings,
            &transport_warnings,
        ));
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
                return Err(output::run_error(
                    "RUNTIME_EXPRESSION_DIAGNOSTICS",
                    format!(
                        "expression diagnostics reported {} warning(s)",
                        expression_warnings.len()
                    ),
                    &expression_warnings,
                    &transport_warnings,
                ));
            }
        }
    }

    if run.dry_run {
        return output::emit_dry_run_requests(
            global.json,
            dry_run_requests,
            &expression_warnings,
            &transport_warnings,
        )
        .map_err(|err| CliError::new(CLI_OUTPUT_WRITE, err));
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
    .map_err(|err| CliError::new(CLI_OUTPUT_WRITE, err))
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
