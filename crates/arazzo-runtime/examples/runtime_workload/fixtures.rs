use super::catalog::{
    self, CaseSpec, ControlKind, ExpectedValue, Family, HarnessError, OutputPolicy, Settings,
};
use super::source_check::{check_sources, endpoint_check, SourceReceipt};
use arazzo_spec::ArazzoSpec;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub(crate) struct PrepareOptions {
    pub output_dir: PathBuf,
    pub data_endpoint: String,
    pub cli_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Preflight {
    Valid,
    RootRejected,
    SourceRejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileKind {
    Root,
    Source,
    Payload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedFile {
    pub path: String,
    pub kind: FileKind,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationReceipt {
    pub cli_path: PathBuf,
    pub cli_version: String,
    pub exit_code: i32,
    pub stdout_file: String,
    pub stderr_file: String,
    pub arazzo_version: Option<String>,
    pub workflow_count: Option<usize>,
    pub source_count: Option<usize>,
    pub warnings: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedManifest {
    pub format_version: u32,
    pub catalog_version: u32,
    pub case_id: String,
    pub manifest_id: String,
    pub endpoint: String,
    pub dimensions: catalog::Dimensions,
    pub settings: Settings,
    pub files: Vec<PreparedFile>,
    pub expected_preflight: Preflight,
    pub validation: ValidationReceipt,
    pub source_receipt_file: String,
}

pub(crate) struct PreparedCase {
    pub manifest: PreparedManifest,
    pub case: CaseSpec,
    pub root: ArazzoSpec,
    pub source_base_dir: PathBuf,
}

fn error(code: &str, e: impl std::fmt::Display) -> HarnessError {
    HarnessError::new("fixtures", code, e)
}

pub(crate) fn pointer(depth: usize, field: usize) -> String {
    format!(
        "/{}/v{field}",
        (0..depth - 1)
            .map(|i| format!("d{i}"))
            .collect::<Vec<_>>()
            .join("/")
    )
}

pub(crate) fn payload_bytes(
    index: usize,
    size: usize,
    depth: usize,
) -> Result<Vec<u8>, HarnessError> {
    if depth != 12 || size > 8_388_608 {
        return Err(error(
            "payload_dimensions",
            "unsupported depth or byte count",
        ));
    }
    let leaf = (0..8)
        .map(|f| format!("\"v{f}\":{}", (index + 1) * 100 + f))
        .collect::<Vec<_>>()
        .join(",");
    let mut body = String::from("{");
    for d in 0..depth - 1 {
        body.push_str(&format!("\"d{d}\":{{"));
    }
    body.push_str(&leaf);
    for _ in 0..depth - 1 {
        body.push('}');
    }
    body.push_str(",\"padding\":\"");
    let overhead = body.len() + 2;
    if overhead > size {
        return Err(error("payload_size", "payload exceeds declared length"));
    }
    body.extend(std::iter::repeat_n('x', size - overhead));
    body.push_str("\"}");
    Ok(body.into_bytes())
}

pub(crate) fn materialize_expected(value: &ExpectedValue) -> Result<Value, HarnessError> {
    match value {
        ExpectedValue::Scalar { value } => Ok(value.clone()),
        ExpectedValue::FullBody {
            payload_index,
            bytes,
            pointer_depth,
        } => {
            let mut leaf = Map::new();
            for f in 0..8 {
                leaf.insert(format!("v{f}"), catalog::scalar(*payload_index, f));
            }
            let mut nested = Value::Object(leaf);
            for d in (0..pointer_depth - 1).rev() {
                nested = json!({format!("d{d}"): nested});
            }
            let object = nested
                .as_object_mut()
                .ok_or_else(|| error("expected_body", "expected object"))?;
            object.insert("padding".into(), json!(""));
            let overhead = serde_json::to_vec(&nested)
                .map_err(|e| error("expected_body", e))?
                .len();
            if overhead > *bytes {
                return Err(error("expected_body", "invalid expected size"));
            }
            nested["padding"] = json!("x".repeat(bytes - overhead));
            Ok(nested)
        }
    }
}

pub(crate) fn root_document(case: &CaseSpec) -> Value {
    let sources: Vec<_> = (0..case.dimensions.source_files).map(|s| json!({"name":format!("source_{s}"), "url":format!("source_{s}.json"), "type":"openapi"})).collect();
    let mut workflow_outputs = Map::new();
    let steps: Vec<_> = (0..case.dimensions.steps).map(|i| {
        let op = catalog::step_operation(case, i);
        let mut outputs = Map::new();
        for f in 0..case.dimensions.scalar_outputs_per_step {
            outputs.insert(format!("value_{f}"), json!(format!("$response.body#{}", pointer(case.dimensions.pointer_depth, f))));
            workflow_outputs.insert(format!("step_{i}_value_{f}"), json!(format!("$steps.step_{i}.outputs.value_{f}")));
        }
        if case.settings.output_policy == OutputPolicy::FullBody {
            outputs.insert("body".into(), json!("$response.body"));
            workflow_outputs.insert(format!("step_{i}_body"), json!(format!("$steps.step_{i}.outputs.body")));
        }
        for p in catalog::output_predecessors(case, i) {
            outputs.insert(format!("predecessor_{p}"), json!(format!("$steps.step_{p}.outputs.value_0")));
            workflow_outputs.insert(format!("step_{i}_predecessor_{p}"), json!(format!("$steps.step_{i}.outputs.predecessor_{p}")));
        }
        let criteria: Vec<_> = (0..case.dimensions.criteria_per_step).map(|j| {
            let condition = if case.control == Some(ControlKind::FalseCriterion) && i == 0 && j == 0 { "$statusCode == 999".into() }
            else if case.family == Family::Expressions {
                format!("$statusCode == 200 && $response.body#{} == {}", pointer(case.dimensions.pointer_depth, j % 8), (op.operation + 1) * 100 + j % 8)
            } else { "$statusCode == 200".into() };
                json!({"condition":condition})
        }).collect();
        let mut step = json!({"stepId":format!("step_{i}"), "operationId":format!("$sourceDescriptions.source_{}.{}", op.source, op.operation_id), "successCriteria":criteria, "outputs":outputs});
        let mut dependencies = catalog::predecessors(case, i);
        if case.control == Some(ControlKind::Cycle) && i == 0 { dependencies.push(1); }
        if !dependencies.is_empty() { step["dependsOn"] = json!(dependencies.iter().map(|p| format!("step_{p}")).collect::<Vec<_>>()); }
        if case.family == Family::Retries {
            step["onFailure"] = json!([{"name":format!("retry_{i}"), "type":"retry", "retryLimit":case.dimensions.retry_limit,
                "retryAfter":case.dimensions.retry_after_ms as f64 / 1000.0, "criteria":[{"condition":"$statusCode == 503"}]}]);
        }
        if case.control == Some(ControlKind::SuccessEnd) && i == 0 {
            step["onSuccess"] = json!([{"name":"end_workflow", "type":"end"}]);
        }
        step
    }).collect();
    if case.control == Some(ControlKind::SuccessEnd) {
        workflow_outputs.retain(|name, _| name.starts_with("step_0_"));
    }
    json!({"arazzo":"1.1.0", "info":{"title":"Runtime workload", "version":"1"},
        "sourceDescriptions":sources, "workflows":[{"workflowId":case.workflow_id,"steps":steps,"outputs":workflow_outputs}]})
}

pub(crate) fn generated_files(
    case: &CaseSpec,
    endpoint: &str,
) -> Result<Vec<(PreparedFile, Vec<u8>)>, HarnessError> {
    endpoint_check(endpoint)?;
    let root = serde_json::to_vec(&root_document(case)).map_err(|e| error("serialize", e))?;
    if root.len() > 2 * 1024 * 1024 {
        return Err(error("root_cap", root.len()));
    }
    let mut files = vec![(
        PreparedFile {
            path: "root.json".into(),
            kind: FileKind::Root,
            bytes: root.len() as u64,
        },
        root,
    )];
    let mut source_bytes = 0;
    for s in 0..case.dimensions.source_files {
        if case.control == Some(ControlKind::MissingSource) && s == 0 {
            continue;
        }
        let mut paths = Map::new();
        for o in 0..case.dimensions.operations_per_file {
            let op = catalog::operation(s, o);
            paths.insert(op.path, json!({"get":{"operationId":op.operation_id,"responses":{
                "200":{"description":"Successful workload response"},"503":{"description":"Predetermined retry response"}}}}));
        }
        let bytes = if case.control == Some(ControlKind::MalformedSource) && s == 0 {
            b"{invalid openapi".to_vec()
        } else {
            serde_json::to_vec(&json!({"openapi":"3.2.0","info":{"title":format!("source_{s}"),"version":"1"},"servers":[{"url":endpoint}],"paths":paths})).map_err(|e| error("serialize", e))?
        };
        source_bytes += bytes.len();
        if source_bytes > 4 * 1024 * 1024 {
            return Err(error("source_cap", source_bytes));
        }
        files.push((
            PreparedFile {
                path: format!("source_{s}.json"),
                kind: FileKind::Source,
                bytes: bytes.len() as u64,
            },
            bytes,
        ));
    }
    for i in 0..case.dimensions.steps {
        let bytes = payload_bytes(
            catalog::step_operation(case, i).operation,
            case.dimensions.serialized_body_bytes,
            case.dimensions.pointer_depth,
        )?;
        files.push((
            PreparedFile {
                path: format!("payload_{i}.json"),
                kind: FileKind::Payload,
                bytes: bytes.len() as u64,
            },
            bytes,
        ));
    }
    Ok(files)
}

fn bounded_command(
    cli: &Path,
    args: &[&std::ffi::OsStr],
    dir: &Path,
    label: &str,
) -> Result<i32, HarnessError> {
    let stdout =
        File::create(dir.join(format!("{label}.stdout"))).map_err(|e| error("receipt_write", e))?;
    let stderr =
        File::create(dir.join(format!("{label}.stderr"))).map_err(|e| error("receipt_write", e))?;
    let mut child = Command::new(cli)
        .args(args)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| error("cli_spawn", e))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code().ok_or_else(|| error("cli_signal", status)),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(2)),
            other => {
                let killed = child.kill();
                let reaped = child.wait();
                return Err(error(
                    "cli_deadline",
                    format!("status={other:?}; kill={killed:?}; reap={reaped:?}"),
                ));
            }
        }
    }
}

fn expected_preflight(case: &CaseSpec) -> Preflight {
    match case.control {
        Some(ControlKind::MissingSource | ControlKind::MalformedSource) => {
            Preflight::SourceRejected
        }
        Some(ControlKind::Cycle) => Preflight::RootRejected,
        _ => Preflight::Valid,
    }
}

fn checked_receipt(
    case: &CaseSpec,
    dir: &Path,
    receipt: &ValidationReceipt,
) -> Result<(), HarnessError> {
    if receipt.stdout_file != "validate.stdout"
        || receipt.stderr_file != "validate.stderr"
        || receipt.cli_version.trim().is_empty()
        || !receipt.cli_path.is_absolute()
    {
        return Err(error("cli_receipt", "incorrect receipt identity"));
    }
    let bytes = read_regular(dir, &receipt.stdout_file, 4 * 1024 * 1024)?;
    let actual: Value = serde_json::from_slice(&bytes).map_err(|e| error("cli_json", e))?;
    if actual["file"] != dir.join("root.json").to_string_lossy().as_ref() {
        return Err(error(
            "cli_receipt",
            "validation receipt names a different root",
        ));
    }
    let count = |key| actual[key].as_u64().and_then(|v| usize::try_from(v).ok());
    let version = actual["version"].as_str().map(str::to_owned);
    let warnings = actual
        .get("warnings")
        .map_or(Some(0), |w| w.as_array().map(Vec::len));
    if warnings != Some(receipt.warnings)
        || version != receipt.arazzo_version
        || count("workflows") != receipt.workflow_count
        || count("sources") != receipt.source_count
    {
        return Err(error(
            "cli_receipt",
            "recorded metadata differs from stdout",
        ));
    }
    read_regular(dir, &receipt.stderr_file, 4 * 1024 * 1024)?;
    if expected_preflight(case) == Preflight::RootRejected {
        if receipt.exit_code == 0
            || actual["valid"] != false
            || actual["kind"] != "error"
            || actual["code"] != "VALIDATE_SPEC_VALIDATION"
            || receipt.warnings != 0
            || !actual["errors"].as_array().is_some_and(|es| {
                es.len() == 1
                    && es[0]["kind"] == "dependencyCycle"
                    && es[0]["source"] == "validation"
                    && es[0]["path"] == format!("workflow {:?}.steps", case.workflow_id)
            })
        {
            return Err(error(
                "cycle_receipt",
                "cycle must have validator cycle rejection",
            ));
        }
    } else if receipt.exit_code != 0
        || actual["valid"] != true
        || version.as_deref() != Some("1.1.0")
        || receipt.workflow_count != Some(1)
        || receipt.source_count != Some(case.dimensions.source_files)
        || receipt.warnings != 0
        || actual
            .get("errors")
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(error("cli_validation", actual));
    }
    Ok(())
}

pub(crate) fn prepare(options: &PrepareOptions) -> Result<Vec<PreparedManifest>, HarnessError> {
    endpoint_check(&options.data_endpoint)?;
    let cli = fs::canonicalize(&options.cli_path).map_err(|e| error("cli_path", e))?;
    fs::create_dir(&options.output_dir).map_err(|e| error("fresh_directory", e))?;
    let version_status = bounded_command(
        &cli,
        &["--version".as_ref()],
        &options.output_dir,
        "cli-version",
    )?;
    let version = String::from_utf8(read_regular(
        &options.output_dir,
        "cli-version.stdout",
        4096,
    )?)
    .map_err(|e| error("cli_version", e))?;
    fs::write(
        options.output_dir.join("cli-version.status"),
        format!("{version_status}\n"),
    )
    .map_err(|e| error("receipt_write", e))?;
    if version_status != 0 || !version.starts_with("arazzo ") {
        return Err(error("cli_version", version));
    }
    let mut manifests = Vec::new();
    for case in catalog::catalog() {
        let dir = options.output_dir.join(&case.case_id);
        fs::create_dir(&dir).map_err(|e| error("case_directory", e))?;
        let files = generated_files(&case, &options.data_endpoint)?;
        for (file, bytes) in &files {
            fs::write(dir.join(&file.path), bytes).map_err(|e| error("fixture_write", e))?;
        }
        let root = dir.join("root.json");
        let exit_code = bounded_command(
            &cli,
            &[
                "validate".as_ref(),
                root.as_os_str(),
                "--strict".as_ref(),
                "--json".as_ref(),
            ],
            &dir,
            "validate",
        )?;
        let out: Value =
            serde_json::from_slice(&read_regular(&dir, "validate.stdout", 4 * 1024 * 1024)?)
                .map_err(|e| error("cli_json", e))?;
        let validation = ValidationReceipt {
            cli_path: cli.clone(),
            cli_version: version.trim().into(),
            exit_code,
            stdout_file: "validate.stdout".into(),
            stderr_file: "validate.stderr".into(),
            arazzo_version: out["version"].as_str().map(str::to_owned),
            workflow_count: out["workflows"].as_u64().map(|v| v as usize),
            source_count: out["sources"].as_u64().map(|v| v as usize),
            warnings: out["warnings"].as_array().map_or(0, Vec::len),
        };
        checked_receipt(&case, &dir, &validation)?;
        let source_receipt = match check_sources(&case, &dir, &options.data_endpoint) {
            Ok(receipt) if expected_preflight(&case) != Preflight::SourceRejected => receipt,
            Err(e)
                if (case.control == Some(ControlKind::MissingSource)
                    && e.code == "missing_source")
                    || (case.control == Some(ControlKind::MalformedSource)
                        && e.code == "malformed_source") =>
            {
                SourceReceipt {
                    valid: false,
                    code: Some(e.code.clone()),
                    failure: Some(e),
                    files: 0,
                    operations: 0,
                }
            }
            Ok(_) => {
                return Err(error(
                    "negative_admission",
                    "negative source unexpectedly valid",
                ))
            }
            Err(e) => return Err(e),
        };
        fs::write(
            dir.join("source-receipt.json"),
            serde_json::to_vec(&source_receipt).map_err(|e| error("serialize", e))?,
        )
        .map_err(|e| error("receipt_write", e))?;
        let manifest = PreparedManifest {
            format_version: 1,
            catalog_version: catalog::CATALOG_VERSION,
            case_id: case.case_id.clone(),
            manifest_id: format!("catalog-1/{}", case.case_id),
            endpoint: options.data_endpoint.clone(),
            dimensions: case.dimensions.clone(),
            settings: case.settings.clone(),
            files: files.into_iter().map(|(f, _)| f).collect(),
            expected_preflight: expected_preflight(&case),
            validation,
            source_receipt_file: "source-receipt.json".into(),
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec(&manifest).map_err(|e| error("serialize", e))?,
        )
        .map_err(|e| error("manifest_write", e))?;
        load_prepared(&options.output_dir, &case.case_id)?;
        manifests.push(manifest);
    }
    Ok(manifests)
}

fn read_regular(dir: &Path, relative: &str, cap: u64) -> Result<Vec<u8>, HarnessError> {
    if Path::new(relative)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
        || Path::new(relative).components().count() != 1
    {
        return Err(error("relative_path", relative));
    }
    let path = dir.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|e| error("file_metadata", format!("{}: {e}", path.display())))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > cap {
        return Err(error("file_kind_or_cap", path.display()));
    }
    fs::read(path).map_err(|e| error("file_read", e))
}

pub(crate) fn load_prepared(dir: &Path, case_id: &str) -> Result<PreparedCase, HarnessError> {
    let case = catalog::case(case_id)?;
    let base = dir.join(case_id);
    if fs::symlink_metadata(&base)
        .map_err(|e| error("case_directory", e))?
        .file_type()
        .is_symlink()
    {
        return Err(error("case_directory", "symlink refused"));
    }
    let manifest: PreparedManifest =
        serde_json::from_slice(&read_regular(&base, "manifest.json", 2 * 1024 * 1024)?)
            .map_err(|e| error("manifest_parse", e))?;
    if manifest.format_version != 1
        || manifest.catalog_version != catalog::CATALOG_VERSION
        || manifest.case_id != case_id
        || manifest.manifest_id != format!("catalog-1/{case_id}")
        || manifest.dimensions != case.dimensions
        || manifest.settings != case.settings
        || manifest.expected_preflight != expected_preflight(&case)
        || manifest.source_receipt_file != "source-receipt.json"
    {
        return Err(error("manifest_identity", case_id));
    }
    let generated = generated_files(&case, &manifest.endpoint)?;
    if manifest.files != generated.iter().map(|(f, _)| f.clone()).collect::<Vec<_>>() {
        return Err(error("file_manifest", case_id));
    }
    for (file, bytes) in generated {
        if read_regular(&base, &file.path, file.bytes)? != bytes {
            return Err(error("byte_identity", file.path));
        }
    }
    if case.control == Some(ControlKind::MissingSource) && base.join("source_0.json").exists() {
        return Err(error("negative_identity", "missing source was supplied"));
    }
    let version = read_regular(dir, "cli-version.stdout", 4096)?;
    if std::str::from_utf8(&version)
        .map_err(|e| error("cli_version", e))?
        .trim()
        != manifest.validation.cli_version
        || !manifest.validation.cli_version.starts_with("arazzo ")
    {
        return Err(error(
            "cli_version",
            "manifest CLI version differs from retained executable receipt",
        ));
    }
    read_regular(dir, "cli-version.stderr", 4096)?;
    if read_regular(dir, "cli-version.status", 32)? != b"0\n" {
        return Err(error(
            "cli_version_status",
            "CLI version command did not succeed",
        ));
    }
    checked_receipt(&case, &base, &manifest.validation)?;
    let recorded: SourceReceipt =
        serde_json::from_slice(&read_regular(&base, "source-receipt.json", 4096)?)
            .map_err(|e| error("source_receipt", e))?;
    let expected = match check_sources(&case, &base, &manifest.endpoint) {
        Ok(receipt) => receipt,
        Err(e)
            if (case.control == Some(ControlKind::MissingSource) && e.code == "missing_source")
                || (case.control == Some(ControlKind::MalformedSource)
                    && e.code == "malformed_source") =>
        {
            SourceReceipt {
                valid: false,
                code: Some(e.code.clone()),
                failure: Some(e),
                files: 0,
                operations: 0,
            }
        }
        Err(e) => return Err(e),
    };
    if recorded != expected {
        return Err(error("source_receipt", "receipt mismatch"));
    }
    let root = serde_json::from_slice(&read_regular(&base, "root.json", 2 * 1024 * 1024)?)
        .map_err(|e| error("root_parse", e))?;
    Ok(PreparedCase {
        manifest,
        case,
        root,
        source_base_dir: base,
    })
}
