use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use arazzo_runtime::{EngineBuilder, EngineEvent, TraceStepRecord};
use arazzo_spec::ArazzoSpec;
use arazzo_validate::Error as ValidateError;
use serde_json::Value;

use crate::cli::{ExpressionDiagnosticsMode, TestFormat};
use crate::output::{self, CatalogEntry};
use crate::run_context::{GlobalOptions, RunContext};
use crate::trace::read_trace_file;
use crate::transport::{self, TransportFlags};

pub async fn run_workflow(ctx: RunContext) -> Result<(), String> {
    crate::run::run_workflow(ctx).await
}

pub async fn replay_trace(
    trace_path: &str,
    spec_override: Option<&str>,
    workflow_id_override: Option<&str>,
    openapi_flags: &[String],
    execution_timeout: std::time::Duration,
    global: GlobalOptions,
) -> Result<(), String> {
    let trace_path_ref = Path::new(trace_path);
    let trace = match read_trace_file(trace_path_ref) {
        Ok(trace) => trace,
        Err(err) => {
            return output::emit_replay_error(global.json, &err, Some("REPLAY_TRACE_LOAD"));
        }
    };

    let workflow_id = workflow_id_override
        .unwrap_or(&trace.run.workflow_id)
        .to_string();
    let spec_path =
        resolve_replay_spec_path(trace_path_ref, spec_override, trace.run.spec_path.as_str());
    let spec = match arazzo_validate::parse(&spec_path) {
        Ok(spec) => spec,
        Err(err) => {
            return output::emit_replay_error(
                global.json,
                &err.to_string(),
                Some(replay_parse_error_code(&err)),
            );
        }
    };

    let mut builder = EngineBuilder::new(spec)
        .parallel(trace.run.parallel)
        .trace(true)
        .replay_trace_steps(trace.steps.clone());

    if let Some(dir) = Path::new(&spec_path).parent() {
        builder = builder.source_base_dir(dir);
    }

    for openapi_path in openapi_flags {
        let bytes = match fs::read(openapi_path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let msg = format!("reading OpenAPI file \"{openapi_path}\": {err}");
                return output::emit_replay_error(
                    global.json,
                    &msg,
                    Some("REPLAY_OPENAPI_READ_FILE"),
                );
            }
        };
        builder = builder.openapi_spec(bytes, Some(PathBuf::from(openapi_path)));
    }

    let engine = match builder.build() {
        Ok(engine) => engine,
        Err(err) => {
            return output::emit_replay_error(global.json, &err.to_string(), Some(err.kind.code()));
        }
    };

    if global.verbose {
        eprintln!("Replaying trace: {trace_path}");
        eprintln!("Spec: {spec_path}");
        eprintln!("Workflow: {workflow_id}");
    }

    let expected_request_count = trace.steps.iter().filter(|s| s.request.is_some()).count();
    let exec_result = engine
        .execute_with_timeout(&workflow_id, trace.inputs.clone(), execution_timeout)
        .collect()
        .await;

    let replay_steps: Vec<TraceStepRecord> = exec_result
        .events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::TraceStep(r) => Some(r.clone()),
            _ => None,
        })
        .collect();
    let actual_request_count = replay_steps.iter().filter(|s| s.request.is_some()).count();

    let outputs = match exec_result.outputs {
        Ok(outputs) => outputs,
        Err(err) => {
            return output::emit_replay_error(global.json, &err.to_string(), Some(err.kind.code()));
        }
    };

    if actual_request_count != expected_request_count {
        return output::emit_replay_error(
            global.json,
            &format!(
                "replay request drift: expected {expected_request_count} recorded request(s) but replay executed {actual_request_count}"
            ),
            Some("REPLAY_REQUEST_COUNT_MISMATCH"),
        );
    }

    output::emit_replay_success(&outputs, actual_request_count, global.json)
}

pub fn generate_workflow(
    spec_path: &str,
    scenario: &str,
    output_path: Option<&str>,
    global: GlobalOptions,
) -> Result<(), String> {
    let bytes = fs::read(spec_path)
        .map_err(|err| format!("reading OpenAPI spec \"{spec_path}\": {err}"))?;

    crate::generate::ensure_supported_openapi_version(&bytes)?;

    let openapi: openapiv3::OpenAPI =
        serde_yaml_ng::from_slice(&bytes).map_err(|err| format!("parsing OpenAPI spec: {err}"))?;

    if scenario != "crud" {
        return Err(format!("unknown scenario \"{scenario}\"; available: crud"));
    }

    // `sourceDescriptions[].url` must resolve, via the runtime's document-
    // semantics loader, from the generated Arazzo document's own directory —
    // an `--output` file's parent, or the current working directory when
    // writing to stdout — never an absolute filesystem path.
    let cwd = std::env::current_dir()
        .map_err(|err| format!("determining current working directory: {err}"))?;
    let output_dir: PathBuf = match output_path {
        Some(path) => Path::new(path)
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
        None => PathBuf::from("."),
    };
    let document_url = crate::generate::relative_document_url(spec_path, &output_dir, &cwd)?;

    let result = crate::generate::generate_crud(&openapi, spec_path, &document_url)?;

    let yaml = serde_yaml_ng::to_string(&result.spec)
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
    match arazzo_validate::parse_with_diagnostics(path) {
        Ok((_, warnings)) if global.strict && !warnings.is_empty() => {
            output::emit_validate_error(path, &promote_warnings(Vec::new(), warnings), global.json)
        }
        Ok((spec, warnings)) => output::emit_validate_result(path, &spec, &warnings, global.json),
        Err(arazzo_validate::Error::Validation(report))
            if global.strict && !report.warnings.is_empty() =>
        {
            output::emit_validate_error(
                path,
                &promote_warnings(report.errors, report.warnings),
                global.json,
            )
        }
        Err(err) => output::emit_validate_error(path, &err, global.json),
    }
}

/// `--strict` makes every warning fatal, so promoted findings travel the
/// existing error path and render exactly like validation errors. No finding is
/// left reported as non-fatal.
fn promote_warnings(
    mut errors: Vec<arazzo_validate::Diagnostic>,
    warnings: Vec<arazzo_validate::Diagnostic>,
) -> arazzo_validate::Error {
    errors.extend(
        warnings
            .into_iter()
            .map(arazzo_validate::Diagnostic::into_error),
    );
    arazzo_validate::Error::Validation(arazzo_validate::ValidationReport {
        errors,
        warnings: Vec::new(),
    })
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

fn replay_parse_error_code(err: &ValidateError) -> &'static str {
    match err {
        ValidateError::ReadFile(_) => "REPLAY_SPEC_READ_FILE",
        ValidateError::ParseYaml(_) => "REPLAY_SPEC_PARSE_YAML",
        ValidateError::Validation(_) => "REPLAY_SPEC_VALIDATION",
        ValidateError::ComponentResolution(_) => "REPLAY_SPEC_COMPONENT_RESOLUTION",
    }
}

fn resolve_replay_spec_path(
    trace_path: &Path,
    spec_override: Option<&str>,
    trace_spec_path: &str,
) -> String {
    if let Some(spec) = spec_override {
        return spec.to_string();
    }

    let trace_spec = PathBuf::from(trace_spec_path);
    if trace_spec.is_absolute() {
        return trace_spec.to_string_lossy().to_string();
    }

    // Prefer trace-relative path over CWD to avoid picking up
    // a same-named spec in the current directory.
    let trace_parent = trace_path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let candidate = trace_parent.join(&trace_spec);
    if candidate.exists() {
        return candidate.to_string_lossy().to_string();
    }

    // Fall back to CWD-relative
    if trace_spec.exists() {
        return trace_spec.to_string_lossy().to_string();
    }

    trace_spec.to_string_lossy().to_string()
}

pub(crate) fn parse_input_kv(raw: &str) -> Result<(String, &str), String> {
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
        CatalogEntry, GenerateResult, ReplayOutput, RunOutput, StepInfo, TestOutput,
        ValidateResult, WorkflowDetail, WorkflowInfo,
    };

    match command {
        Some("validate") => output::output_json(&schema_for!(ValidateResult)),
        Some("list") => output::output_json(&schema_for!(Vec<WorkflowInfo>)),
        Some("catalog") => output::output_json(&schema_for!(Vec<CatalogEntry>)),
        Some("show") => output::output_json(&schema_for!(WorkflowDetail)),
        Some("steps") => output::output_json(&schema_for!(Vec<StepInfo>)),
        Some("run") => output::output_json(&schema_for!(RunOutput)),
        Some("export-run") => output::output_json(&schema_for!(crate::run_export::RunExport)),
        Some("replay") => output::output_json(&schema_for!(ReplayOutput)),
        Some("generate") => output::output_json(&schema_for!(GenerateResult)),
        Some("test") => output::output_json(&schema_for!(TestOutput)),
        Some(other) => Err(format!(
            "unknown command: \"{other}\". Available: validate, list, catalog, show, steps, run, export-run, replay, generate, test"
        )),
        None => output::output_json(&[
            "validate", "list", "catalog", "show", "steps", "run", "export-run", "replay", "generate", "test",
        ]),
    }
}

pub(crate) fn parse_input_value(raw: &str) -> Value {
    // Single-quoted values bypass coercion: 'true' → String("true")
    if let Some(inner) = raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        return Value::String(inner.to_string());
    }

    let value = raw.to_string();
    // Env var values are always strings in the OS — return them as-is
    // to avoid surprising coercion (e.g., MY_VAR=true → Bool(true)).
    if value.starts_with('$') {
        let var_name = value
            .strip_prefix('$')
            .unwrap_or(&value)
            .trim_matches(|c: char| c == '{' || c == '}');
        if let Ok(found) = std::env::var(var_name) {
            return Value::String(found);
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
        if v.is_finite() {
            return serde_json::json!(v);
        }
    }
    Value::String(value)
}

#[allow(clippy::too_many_arguments)]
pub async fn run_tests(
    paths: Vec<String>,
    format: TestFormat,
    input: Vec<String>,
    input_json: Vec<String>,
    http_timeout: std::time::Duration,
    execution_timeout: std::time::Duration,
    header: Vec<String>,
    openapi: Vec<String>,
    expr_diagnostics: ExpressionDiagnosticsMode,
    parallel: bool,
    strict_inputs: bool,
    max_response_size: Option<usize>,
    fail_fast: bool,
    filter: Option<String>,
    transport_flags: TransportFlags,
    global: GlobalOptions,
) -> Result<(), String> {
    use crate::test_runner::{discover_test_specs, run_test_suite, TestRunOptions};

    // Startup insecure-exception notice, before any suite runs.
    let squelched = transport_flags.no_transport_warnings;
    if let Some(warning) = transport_flags.startup_warning() {
        transport::eprint_warning(&warning, squelched);
    }

    let emit_error = |err: String| -> Result<(), String> {
        let result = output::TestOutput::Error {
            error: err.clone(),
            code: None,
        };
        if global.json || format == TestFormat::Json {
            output::output_json(&result)?;
            Err(String::new())
        } else {
            Err(err)
        }
    };

    let specs = match discover_test_specs(&paths) {
        Ok(s) => s,
        Err(err) => return emit_error(err),
    };

    // Parse inputs (same logic as `run`).
    let mut inputs = BTreeMap::<String, Value>::new();
    for item in input {
        let (key, raw_value) = parse_input_kv(&item)?;
        inputs.insert(key, parse_input_value(raw_value));
    }
    for item in input_json {
        let (key, raw_value) = parse_input_kv(&item)?;
        let value = serde_json::from_str::<Value>(raw_value).map_err(|err| {
            format!("invalid JSON input format for \"{key}\": {err} (expected key=<json>)")
        })?;
        inputs.insert(key, value);
    }

    // Parse headers.
    let mut headers = BTreeMap::new();
    for h in header {
        let parsed = h
            .split_once(':')
            .map(|(k, v)| (k.trim(), v.trim_start()))
            .or_else(|| h.split_once('=').map(|(k, v)| (k.trim(), v)));
        if let Some((k, v)) = parsed {
            headers.insert(k.to_string(), v.to_string());
        } else {
            eprintln!(
                "warning: ignoring malformed header (expected 'Name: value' or 'Name=value'): {h}"
            );
        }
    }

    // Load OpenAPI files once.
    let mut openapi_bytes = Vec::new();
    for openapi_path in &openapi {
        let bytes = fs::read(openapi_path)
            .map_err(|err| format!("reading OpenAPI file \"{openapi_path}\": {err}"))?;
        openapi_bytes.push((PathBuf::from(openapi_path), bytes));
    }

    // Compile filter regex.
    let filter_re = match filter {
        Some(pattern) => match regex::Regex::new(&pattern) {
            Ok(re) => Some(re),
            Err(err) => return emit_error(format!("invalid --filter regex: {err}")),
        },
        None => None,
    };

    let opts = TestRunOptions {
        inputs,
        http_timeout,
        execution_timeout,
        headers,
        openapi_bytes,
        expr_diagnostics,
        parallel,
        strict_inputs,
        max_response_size,
        fail_fast,
        filter: filter_re,
        transport: transport_flags,
    };

    let result = run_test_suite(&specs, &opts).await;

    // Print collected transport warning lines (cleartext, unused) once,
    // deduplicated across suites. The startup line already printed; the
    // structured entries stay in the JSON output regardless of squelch.
    if let output::TestOutput::Results {
        transport_warnings, ..
    } = &result
    {
        for warning in transport_warnings {
            if warning.kind != arazzo_runtime::TransportWarningKind::InsecureHostsActive
                && warning.kind != arazzo_runtime::TransportWarningKind::InsecureAllHosts
            {
                transport::eprint_warning(warning, squelched);
            }
        }
    }

    // Determine exit code: non-zero if any failures/errors.
    let has_failures = match &result {
        output::TestOutput::Results { summary, .. } => {
            summary.failed > 0 || summary.errors > 0 || summary.suite_errors > 0
        }
        output::TestOutput::Error { .. } => true,
    };

    use crate::test_runner::{format_junit, format_tap, print_human_summary};

    let effective_format = if global.json {
        TestFormat::Json
    } else {
        format
    };

    match effective_format {
        TestFormat::Json => {
            output::output_json(&result)?;
        }
        TestFormat::Tap => {
            print!("{}", format_tap(&result));
            print_human_summary(&result);
        }
        TestFormat::Junit => {
            print!("{}", format_junit(&result));
            print_human_summary(&result);
        }
    }

    if has_failures {
        Err(String::new())
    } else {
        Ok(())
    }
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
