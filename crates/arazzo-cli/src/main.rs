#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use arazzo_runtime::{ClientConfig, Engine, ExecutionOptions, TraceStepRecord};
use arazzo_spec::{ArazzoSpec, Workflow};
use clap::{Parser, Subcommand};
use humantime::format_rfc3339;
use serde::Serialize;
use serde_json::Value;

const TRACE_SCHEMA_VERSION: &str = "trace.v1";
const TRACE_REDACTED: &str = "[REDACTED]";
const TRACE_BODY_PREVIEW_DEFAULT_BYTES: usize = 2048;
const TRACE_MAX_BODY_BYTES_LIMIT: usize = 1024 * 1024;
const TRACE_SENSITIVE_KEYS: [&str; 18] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "apikey",
    "token",
    "access_token",
    "refresh_token",
    "id_token",
    "secret",
    "client_secret",
    "password",
    "passwd",
    "pwd",
    "session",
    "sessionid",
];

#[derive(Parser, Debug)]
#[command(name = "arazzo")]
#[command(about = "Execute Arazzo 1.0 workflows")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    #[arg(short = 'v', long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run {
        spec: String,
        workflow_id: String,

        #[arg(short = 'i', long = "input")]
        input: Vec<String>,

        #[arg(
            short = 't',
            long = "timeout",
            default_value = "30s",
            value_parser = parse_duration_value
        )]
        timeout: Duration,

        #[arg(short = 'H', long = "header")]
        header: Vec<String>,

        #[arg(long)]
        parallel: bool,

        #[arg(long = "dry-run")]
        dry_run: bool,

        #[arg(long = "trace")]
        trace: Option<String>,

        #[arg(
            long = "trace-max-body-bytes",
            default_value_t = TRACE_BODY_PREVIEW_DEFAULT_BYTES,
            value_parser = parse_trace_max_body_bytes
        )]
        trace_max_body_bytes: usize,
    },
    Validate {
        spec: String,
    },
    List {
        spec: String,
    },
    Catalog {
        dir: String,
    },
    Show {
        workflow_id: String,
        #[arg(long = "dir", default_value = ".")]
        dir: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogEntry {
    file: String,
    title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    version: String,
    sources: Vec<SourceInfo>,
    workflows: Vec<WorkflowInfo>,
}

#[derive(Debug, Serialize)]
struct SourceInfo {
    name: String,
    url: String,
    #[serde(rename = "type")]
    type_: String,
}

#[derive(Debug, Serialize)]
struct WorkflowInfo {
    id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    summary: String,
    inputs: Vec<String>,
    outputs: Vec<String>,
}

#[derive(Debug, Serialize)]
struct InputDetail {
    #[serde(rename = "type")]
    type_: String,
    required: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
}

#[derive(Debug, Serialize)]
struct WorkflowDetail {
    id: String,
    file: String,
    title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    summary: String,
    steps: usize,
    inputs: BTreeMap<String, InputDetail>,
    outputs: Vec<String>,
    sources: Vec<SourceInfo>,
}

#[derive(Debug, Serialize)]
struct ValidateResult {
    valid: bool,
    file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sources: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
}

#[derive(Debug)]
struct RunOptions {
    spec_path: String,
    workflow_id: String,
    input_flags: Vec<String>,
    timeout: Duration,
    header_flags: Vec<String>,
    parallel: bool,
    dry_run: bool,
    trace: Option<String>,
    trace_max_body_bytes: usize,
    verbose: bool,
    json: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceFile {
    schema_version: String,
    tool: TraceTool,
    run: TraceRun,
    inputs: BTreeMap<String, Value>,
    steps: Vec<TraceStepRecord>,
}

#[derive(Debug, Serialize)]
struct TraceTool {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceRun {
    spec_path: String,
    workflow_id: String,
    parallel: bool,
    dry_run: bool,
    timeout_ms: u64,
    started_at: String,
    finished_at: String,
    duration_ms: u64,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn main() {
    load_env_file(".env");
    let cli = Cli::parse();
    if let Err(err) = run(cli) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Run {
            spec,
            workflow_id,
            input,
            timeout,
            header,
            parallel,
            dry_run,
            trace,
            trace_max_body_bytes,
        } => run_workflow(RunOptions {
            spec_path: spec,
            workflow_id,
            input_flags: input,
            timeout,
            header_flags: header,
            parallel,
            dry_run,
            trace,
            trace_max_body_bytes,
            verbose: cli.verbose,
            json: cli.json,
        }),
        Commands::Validate { spec } => validate_spec(&spec, cli.json),
        Commands::List { spec } => list_workflows(&spec, cli.json),
        Commands::Catalog { dir } => catalog_workflows(&dir, cli.json, cli.verbose),
        Commands::Show { workflow_id, dir } => show_workflow(&workflow_id, &dir, cli.json),
    }
}

fn run_workflow(opts: RunOptions) -> Result<(), String> {
    let RunOptions {
        spec_path,
        workflow_id,
        input_flags,
        timeout,
        header_flags,
        parallel,
        dry_run,
        trace,
        trace_max_body_bytes,
        verbose,
        json,
    } = opts;

    let spec = match arazzo_validate::parse(&spec_path) {
        Ok(spec) => spec,
        Err(err) => {
            if json {
                return output_json(&serde_json::json!({ "error": err.to_string() }));
            }
            return Err(format!("parsing spec: {err}"));
        }
    };

    let mut inputs = BTreeMap::<String, Value>::new();
    for item in input_flags {
        let Some((k, v)) = item.split_once('=') else {
            return Err(format!(
                "invalid input format: \"{item}\" (expected key=value)"
            ));
        };
        inputs.insert(k.to_string(), parse_input_value(v));
    }

    if verbose {
        eprintln!("Executing workflow: {workflow_id}");
        eprintln!("Inputs: {inputs:?}");
    }

    let mut cfg = ClientConfig {
        timeout,
        ..ClientConfig::default()
    };
    for header in header_flags {
        if let Some((k, v)) = header.split_once('=') {
            cfg.default_headers.insert(k.to_string(), v.to_string());
        }
    }
    let mut engine = Engine::with_client_config(spec, cfg)
        .map_err(|err| format!("creating runtime engine: {err}"))?;
    engine.set_parallel_mode(parallel);
    engine.set_dry_run_mode(dry_run);
    engine.set_trace_enabled(trace.is_some());

    let execution_timeout = timeout
        .checked_mul(10)
        .unwrap_or_else(|| Duration::from_secs(u64::MAX));

    let run_started_at = SystemTime::now();
    let run_started_inst = std::time::Instant::now();
    let outputs_result = engine.execute_with_options(
        &workflow_id,
        inputs.clone(),
        ExecutionOptions::with_timeout(execution_timeout),
    );
    let run_finished_at = SystemTime::now();
    let run_duration = run_started_inst.elapsed();

    let run_error_text = outputs_result.as_ref().err().map(ToString::to_string);
    let mut trace_write_error: Option<String> = None;
    if let Some(trace_path) = trace {
        let mut trace_file = TraceFile {
            schema_version: TRACE_SCHEMA_VERSION.to_string(),
            tool: TraceTool {
                name: "arazzo".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            run: TraceRun {
                spec_path: spec_path.clone(),
                workflow_id: workflow_id.clone(),
                parallel,
                dry_run,
                timeout_ms: u64::try_from(execution_timeout.as_millis()).unwrap_or(u64::MAX),
                started_at: format_rfc3339(run_started_at).to_string(),
                finished_at: format_rfc3339(run_finished_at).to_string(),
                duration_ms: u64::try_from(run_duration.as_millis()).unwrap_or(u64::MAX),
                status: if run_error_text.is_some() {
                    "failure".to_string()
                } else {
                    "success".to_string()
                },
                error: run_error_text.clone(),
            },
            inputs,
            steps: engine.trace_steps(),
        };

        redact_trace_file(&mut trace_file, trace_max_body_bytes);
        if let Err(err) = write_trace_file_atomic(Path::new(&trace_path), &trace_file) {
            trace_write_error = Some(err);
        }
    }

    if let Some(run_error) = run_error_text {
        if let Some(trace_error) = trace_write_error {
            return Err(format!("{run_error}; writing trace: {trace_error}"));
        }
        if json {
            return output_json(&serde_json::json!({ "error": run_error }));
        }
        return Err(run_error);
    }

    if let Some(trace_error) = trace_write_error {
        return Err(format!("writing trace: {trace_error}"));
    }

    let outputs = outputs_result.unwrap_or_default();

    if dry_run {
        let reqs = engine.dry_run_requests();
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
        return Ok(());
    }

    output_json(&outputs)
}

fn validate_spec(path: &str, json: bool) -> Result<(), String> {
    match arazzo_validate::parse(path) {
        Ok(spec) => {
            if json {
                return output_json(&ValidateResult {
                    valid: true,
                    file: path.to_string(),
                    version: Some(spec.arazzo),
                    title: Some(spec.info.title),
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
        Err(err) => {
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
    }
}

fn list_workflows(path: &str, json: bool) -> Result<(), String> {
    let spec = arazzo_validate::parse(path).map_err(|err| err.to_string())?;
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

fn catalog_workflows(dir: &str, json: bool, verbose: bool) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|err| format!("reading directory \"{dir}\": {err}"))?;

    let mut catalog = Vec::<CatalogEntry>::new();
    for entry in entries {
        let entry = match entry {
            Ok(v) => v,
            Err(err) => {
                if verbose {
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
                if verbose {
                    eprintln!("skipping {file_name}: {err}");
                }
                continue;
            }
        };
        let workflows = spec
            .workflows
            .iter()
            .map(build_workflow_info)
            .collect::<Vec<_>>();
        catalog.push(CatalogEntry {
            file: file_name,
            title: spec.info.title.clone(),
            description: spec.info.description.clone(),
            version: spec.info.version.clone(),
            sources: build_sources(&spec),
            workflows,
        });
    }

    if json {
        return output_json(&catalog);
    }

    println!("File                 Workflow ID   Summary");
    println!("-------------------  ------------  ----------------------------------------");
    for row in &catalog {
        for wf in &row.workflows {
            println!("{:<20} {:<12} {}", row.file, wf.id, wf.summary);
        }
    }
    Ok(())
}

fn show_workflow(workflow_id: &str, dir: &str, json: bool) -> Result<(), String> {
    let (spec, file) = find_workflow(dir, workflow_id)?;
    let wf = spec
        .workflows
        .iter()
        .find(|wf| wf.workflow_id == workflow_id)
        .ok_or_else(|| format!("workflow \"{workflow_id}\" not found in {dir}"))?;

    if json {
        let mut inputs = BTreeMap::<String, InputDetail>::new();
        if let Some(schema) = &wf.inputs {
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
            id: wf.workflow_id.clone(),
            file,
            title: spec.info.title.clone(),
            summary: wf.summary.clone(),
            steps: wf.steps.len(),
            inputs,
            outputs: wf.outputs.keys().cloned().collect(),
            sources: build_sources(&spec),
        });
    }

    println!("Workflow: {}", wf.workflow_id);
    println!("File:     {file}");
    println!("Title:    {}", spec.info.title);
    if !wf.summary.is_empty() {
        println!("Summary:  {}", wf.summary);
    }
    println!("Steps:    {}", wf.steps.len());
    println!();

    if let Some(schema) = &wf.inputs {
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

    if !wf.outputs.is_empty() {
        println!("Outputs:");
        for name in wf.outputs.keys() {
            println!("  {name}");
        }
    }

    Ok(())
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

fn build_sources(spec: &ArazzoSpec) -> Vec<SourceInfo> {
    spec.source_descriptions
        .iter()
        .map(|src| SourceInfo {
            name: src.name.clone(),
            url: src.url.clone(),
            type_: src.type_.clone(),
        })
        .collect()
}

fn build_workflow_info(wf: &Workflow) -> WorkflowInfo {
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

fn parse_duration_value(raw: &str) -> Result<Duration, String> {
    if let Ok(seconds) = raw.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    humantime::parse_duration(raw).map_err(|err| format!("invalid timeout \"{raw}\": {err}"))
}

fn parse_trace_max_body_bytes(raw: &str) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|err| format!("invalid trace max body bytes \"{raw}\": {err}"))?;
    if value > TRACE_MAX_BODY_BYTES_LIMIT {
        return Err(format!(
            "trace max body bytes must be <= {TRACE_MAX_BODY_BYTES_LIMIT}"
        ));
    }
    Ok(value)
}

fn redact_trace_file(trace: &mut TraceFile, max_body_bytes: usize) {
    redact_json_object(&mut trace.inputs);
    for step in &mut trace.steps {
        if let Some(request) = &mut step.request {
            redact_headers(&mut request.headers);
            redact_url_query(&mut request.url);
            if let Some(body) = &mut request.body {
                redact_json_value(body);
            }
        }

        if let Some(response) = &mut step.response {
            redact_headers(&mut response.headers);
            if response.content_type == "json" {
                if let Some(preview) = &mut response.body_preview {
                    if let Ok(mut value) = serde_json::from_str::<Value>(preview) {
                        redact_json_value(&mut value);
                        if let Ok(serialized) = serde_json::to_string(&value) {
                            *preview = serialized;
                        }
                    }
                }
            }
            if let Some(preview) = &mut response.body_preview {
                let mut bytes = preview.as_bytes().to_vec();
                if bytes.len() > max_body_bytes {
                    bytes.truncate(max_body_bytes);
                    let mut text = String::from_utf8_lossy(&bytes).to_string();
                    text.push_str("...");
                    *preview = text;
                }
            }
        }

        redact_json_object(&mut step.outputs);
    }
}

fn redact_headers(headers: &mut BTreeMap<String, String>) {
    for (name, value) in headers {
        if is_sensitive_key(name) {
            *value = TRACE_REDACTED.to_string();
        }
    }
}

fn redact_url_query(url: &mut String) {
    let mut parsed = match url::Url::parse(url) {
        Ok(value) => value,
        Err(_) => return,
    };

    let mut pairs = Vec::<(String, String)>::new();
    for (key, value) in parsed.query_pairs() {
        if is_sensitive_key(&key) {
            pairs.push((key.to_string(), TRACE_REDACTED.to_string()));
        } else {
            pairs.push((key.to_string(), value.to_string()));
        }
    }
    if pairs.is_empty() {
        return;
    }

    parsed
        .query_pairs_mut()
        .clear()
        .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    *url = parsed.to_string();
}

fn redact_json_object(map: &mut BTreeMap<String, Value>) {
    for (key, value) in map {
        if is_sensitive_key(key) {
            *value = Value::String(TRACE_REDACTED.to_string());
        } else {
            redact_json_value(value);
        }
    }
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                if is_sensitive_key(key) {
                    *nested = Value::String(TRACE_REDACTED.to_string());
                } else {
                    redact_json_value(nested);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        _ => {}
    }
}

fn is_sensitive_key(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    TRACE_SENSITIVE_KEYS.iter().any(|key| lower == *key)
}

fn write_trace_file_atomic(path: &Path, trace: &TraceFile) -> Result<(), String> {
    let parent = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    fs::create_dir_all(&parent)
        .map_err(|err| format!("creating trace directory {}: {err}", parent.display()))?;

    let stamp = match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        Ok(delta) => delta.as_nanos(),
        Err(_) => 0,
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("trace.json");
    let tmp_name = format!(".{file_name}.tmp-{}-{stamp}", std::process::id());
    let tmp_path = parent.join(tmp_name);

    {
        let mut file = fs::File::create(&tmp_path)
            .map_err(|err| format!("creating temp trace file {}: {err}", tmp_path.display()))?;
        serde_json::to_writer_pretty(&mut file, trace).map_err(|err| {
            format!(
                "serializing trace JSON to temp file {}: {err}",
                tmp_path.display()
            )
        })?;
        use std::io::Write;
        writeln!(file)
            .map_err(|err| format!("writing trace newline to {}: {err}", tmp_path.display()))?;
        file.sync_all()
            .map_err(|err| format!("syncing temp trace file {}: {err}", tmp_path.display()))?;
    }

    fs::rename(&tmp_path, path).map_err(|err| {
        let _ = fs::remove_file(&tmp_path);
        format!(
            "renaming temp trace file {} to {}: {err}",
            tmp_path.display(),
            path.display()
        )
    })
}

fn output_json<T: Serialize>(value: &T) -> Result<(), String> {
    serde_json::to_writer_pretty(std::io::stdout(), value)
        .map_err(|err| format!("writing JSON: {err}"))?;
    println!();
    Ok(())
}

fn load_env_file(path: impl AsRef<Path>) {
    let file = match fs::File::open(path.as_ref()) {
        Ok(file) => file,
        Err(_) => return,
    };

    let reader = io::BufReader::new(file);
    for line in reader.lines() {
        let line = match line {
            Ok(v) => v,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        std::env::set_var(key, value);
    }
}
