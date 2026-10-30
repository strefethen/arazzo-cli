#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../examples/runtime_workload/catalog.rs"]
#[allow(dead_code)]
mod catalog;
#[path = "../examples/runtime_workload/fixtures.rs"]
#[allow(dead_code)]
mod fixtures;
#[path = "../examples/runtime_workload/source_check.rs"]
mod source_check;

use catalog::{ControlKind, ExpectedValue, Family, HandleMode, OutputPolicy, Transfer};
use fixtures::{FileKind, Preflight, PreparedManifest, ValidationReceipt};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const ENDPOINT: &str = "http://127.0.0.1:32123";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "arazzo-fixtures-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn write_prepared(dir: &Path, case: &catalog::CaseSpec) {
    let base = dir.join(&case.case_id);
    fs::create_dir(&base).unwrap();
    fs::write(dir.join("cli-version.stdout"), b"arazzo 0.9.0\n").unwrap();
    fs::write(dir.join("cli-version.stderr"), b"").unwrap();
    fs::write(dir.join("cli-version.status"), b"0\n").unwrap();
    let files = fixtures::generated_files(case, ENDPOINT).unwrap();
    for (file, bytes) in &files {
        fs::write(base.join(&file.path), bytes).unwrap();
    }
    let cycle = case.control == Some(ControlKind::Cycle);
    let stdout = if cycle {
        json!({"kind":"error","code":"VALIDATE_SPEC_VALIDATION","file":base.join("root.json"),"valid":false,"errors":[{"kind":"dependencyCycle","source":"validation","path":"workflow \"workload\".steps","message":"cycle"}]})
    } else {
        json!({"valid":true,"file":base.join("root.json"),"version":"1.1.0","title":"Runtime workload","workflows":1,"sources":case.dimensions.source_files})
    };
    fs::write(
        base.join("validate.stdout"),
        serde_json::to_vec(&stdout).unwrap(),
    )
    .unwrap();
    fs::write(base.join("validate.stderr"), b"").unwrap();
    let source = match source_check::check_sources(case, &base, ENDPOINT) {
        Ok(receipt) => receipt,
        Err(e) => source_check::SourceReceipt {
            valid: false,
            code: Some(e.code.clone()),
            failure: Some(e),
            files: 0,
            operations: 0,
        },
    };
    fs::write(
        base.join("source-receipt.json"),
        serde_json::to_vec(&source).unwrap(),
    )
    .unwrap();
    let manifest = PreparedManifest {
        format_version: 1,
        catalog_version: 1,
        case_id: case.case_id.clone(),
        manifest_id: format!("catalog-1/{}", case.case_id),
        endpoint: ENDPOINT.into(),
        dimensions: case.dimensions.clone(),
        settings: case.settings.clone(),
        files: files.into_iter().map(|(f, _)| f).collect(),
        expected_preflight: if cycle {
            Preflight::RootRejected
        } else if source.valid {
            Preflight::Valid
        } else {
            Preflight::SourceRejected
        },
        validation: ValidationReceipt {
            cli_path: std::env::current_exe().unwrap(),
            cli_version: "arazzo 0.9.0".into(),
            exit_code: if cycle { 1 } else { 0 },
            stdout_file: "validate.stdout".into(),
            stderr_file: "validate.stderr".into(),
            arazzo_version: if cycle { None } else { Some("1.1.0".into()) },
            workflow_count: if cycle { None } else { Some(1) },
            source_count: if cycle {
                None
            } else {
                Some(case.dimensions.source_files)
            },
            warnings: 0,
        },
        source_receipt_file: "source-receipt.json".into(),
    };
    fs::write(
        base.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn mutate(path: &Path, action: impl FnOnce(&mut Value)) {
    let mut value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    action(&mut value);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn nodes(value: &Value) -> usize {
    1 + match value {
        Value::Array(a) => a.iter().map(nodes).sum(),
        Value::Object(o) => o.values().map(nodes).sum(),
        _ => 0,
    }
}

#[test]
fn catalog_covers_complete_frozen_matrix_and_independent_expectations() {
    let all = catalog::catalog();
    assert_eq!(all.len(), 125);
    assert_eq!(
        all.iter()
            .map(|c| &c.case_id)
            .collect::<BTreeSet<_>>()
            .len(),
        all.len()
    );
    for (family, count) in [
        (Family::Sources, 4),
        (Family::Expressions, 4),
        (Family::Dag, 20),
        (Family::Bodies, 32),
        (Family::Retries, 24),
        (Family::Cancellation, 8),
    ] {
        assert_eq!(
            all.iter()
                .filter(|c| c.family == family && c.control.is_none())
                .count(),
            count
        );
    }
    assert_eq!(
        all.iter()
            .filter(|c| c.control == Some(ControlKind::RetryExhaustion))
            .count(),
        24
    );
    for c in all {
        assert_eq!(c.settings.tokio_threads, 2);
        assert_eq!(c.settings.http_timeout_ms, 5000);
        assert_eq!(c.settings.channel_capacity, 1024);
        assert_eq!(c.settings.burst, 20);
        assert!(!c.settings.observer && !c.settings.trace_hook);
        assert_eq!(c.dimensions.json_nodes, 21);
        assert_eq!(
            c.expected.operations.len(),
            c.dimensions.source_files * c.dimensions.operations_per_file
        );
        if c.control.is_none() {
            assert_eq!(c.settings.response_cap, 10_485_760);
            assert_eq!(c.settings.trace, c.family == Family::Expressions);
            assert_eq!(
                c.settings.capture,
                matches!(c.family, Family::Dag | Family::Retries)
            );
            if c.family != Family::Bodies && c.family != Family::Cancellation {
                assert_eq!(c.settings.handle_mode, HandleMode::Collect);
            }
        }
        if c.control == Some(ControlKind::RetryExhaustion) {
            assert_eq!(c.response_policy.exhaust_path.as_deref(), Some("/s0/op0"));
            assert_eq!(
                c.expected.required_paths.len(),
                if c.settings.parallel {
                    c.dimensions.steps
                } else {
                    1
                }
            );
        }
    }
    assert!(catalog::case("missing").is_err());
}

#[test]
fn every_generated_case_has_exact_deterministic_bytes_and_typed_root() {
    for case in catalog::catalog() {
        let first = fixtures::generated_files(&case, ENDPOINT).unwrap();
        assert_eq!(
            first,
            fixtures::generated_files(&case, ENDPOINT).unwrap(),
            "{}",
            case.case_id
        );
        let root = &first
            .iter()
            .find(|(f, _)| f.kind == FileKind::Root)
            .unwrap()
            .1;
        let typed: arazzo_spec::ArazzoSpec = serde_json::from_slice(root).unwrap();
        assert_eq!(typed.arazzo, "1.1.0");
        assert_eq!(typed.workflows.len(), 1);
        for step in &typed.workflows[0].steps {
            assert!(!step.success_criteria.is_empty());
            assert!(step
                .success_criteria
                .iter()
                .all(|criterion| criterion.type_.is_none()));
        }
        for (file, bytes) in first.iter().filter(|(f, _)| f.kind == FileKind::Payload) {
            assert_eq!(file.bytes, case.dimensions.serialized_body_bytes as u64);
            assert_eq!(bytes.len(), case.dimensions.serialized_body_bytes);
            let actual: Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(nodes(&actual), case.dimensions.json_nodes);
        }
        for (key, value) in &case.expected.outputs {
            let i: usize = key.split('_').nth(1).unwrap().parse().unwrap();
            let payload: Value = serde_json::from_slice(
                &first
                    .iter()
                    .find(|(f, _)| f.path == format!("payload_{i}.json"))
                    .unwrap()
                    .1,
            )
            .unwrap();
            match value {
                ExpectedValue::FullBody { .. } => {
                    assert_eq!(fixtures::materialize_expected(value).unwrap(), payload)
                }
                ExpectedValue::Scalar { value } if key.contains("_value_") => {
                    let field: usize = key.rsplit('_').next().unwrap().parse().unwrap();
                    assert_eq!(payload.pointer(&fixtures::pointer(12, field)), Some(value));
                }
                ExpectedValue::Scalar { value } => {
                    let predecessor: usize = key.rsplit('_').next().unwrap().parse().unwrap();
                    assert_eq!(
                        *value,
                        catalog::scalar(catalog::step_operation(&case, predecessor).operation, 0)
                    );
                }
            }
        }
    }
}

#[test]
fn source_checker_enumerates_every_operation_and_rejects_restricted_shape_mutations() {
    let case = catalog::case("sources_f8_o100").unwrap();
    let dir = Directory::new();
    write_prepared(&dir.0, &case);
    let base = dir.0.join(&case.case_id);
    assert_eq!(
        source_check::check_sources(&case, &base, ENDPOINT)
            .unwrap()
            .operations,
        800
    );
    let last = catalog::step_operation(&case, 9);
    assert_eq!((last.source, last.operation), (7, 99));
    let path = base.join("source_7.json");
    let original = fs::read(&path).unwrap();
    for mode in 0..7 {
        mutate(&path, |value| match mode {
            0 => value["openapi"] = json!("3.0.0"),
            1 => value["info"]["version"] = json!("2"),
            2 => value["servers"][0]["url"] = json!("https://example.com"),
            3 => value["paths"]["/s7/op99"]["get"]["operationId"] = json!("wrong"),
            4 => {
                value["paths"]["/s7/op99"]
                    .as_object_mut()
                    .unwrap()
                    .remove("get");
            }
            5 => value["paths"]["/s7/op99"]["get"]["responses"] = json!({}),
            _ => value["extra"] = json!(true),
        });
        assert!(source_check::check_sources(&case, &base, ENDPOINT).is_err());
        fs::write(&path, &original).unwrap();
    }
    for bad in [
        "http://example.com:80",
        "https://127.0.0.1:1234",
        "http://127.0.0.1:1234/x",
        "http://127.0.0.1:1234/?x=1",
    ] {
        assert!(source_check::endpoint_check(bad).is_err());
    }
}

#[test]
fn admission_rejects_root_metadata_version_targets_criteria_dimensions_and_receipt_mutations() {
    let case = catalog::case("expressions_s8_c16").unwrap();
    let dir = Directory::new();
    write_prepared(&dir.0, &case);
    let base = dir.0.join(&case.case_id);
    let prepared = fixtures::load_prepared(&dir.0, &case.case_id).unwrap();
    assert_eq!(prepared.root.workflows[0].steps.len(), 8);
    assert_eq!(prepared.manifest.settings, case.settings);
    assert_eq!(prepared.case, case);
    assert_eq!(prepared.source_base_dir, base);
    let root_path = base.join("root.json");
    let original_root = fs::read(&root_path).unwrap();
    for mode in 0..5 {
        mutate(&root_path, |v| match mode {
            0 => v["info"]["title"] = json!("Wrong"),
            1 => v["arazzo"] = json!("1.0.1"),
            2 => v["workflows"][0]["steps"][0]["operationId"] = json!("operation_0_0"),
            3 => v["workflows"][0]["steps"][0]["successCriteria"] = json!([]),
            _ => v["workflows"][0]["steps"][0]["outputs"]["value_0"] = json!("$response.body.d0"),
        });
        assert!(fixtures::load_prepared(&dir.0, &case.case_id).is_err());
        fs::write(&root_path, &original_root).unwrap();
    }
    let path = base.join("manifest.json");
    let original = fs::read(&path).unwrap();
    for mode in 0..8 {
        mutate(&path, |v| match mode {
            0 => v["dimensions"]["steps"] = json!(7),
            1 => v["settings"]["trace"] = json!(false),
            2 => v["files"][0]["bytes"] = json!(1),
            3 => v["files"][0]["path"] = json!("../root.json"),
            4 => v["expected_preflight"] = json!("root_rejected"),
            5 => v["validation"]["warnings"] = json!(1),
            6 => v["validation"]["cli_version"] = json!("arazzo invented"),
            _ => v["unknown"] = json!(true),
        });
        assert!(fixtures::load_prepared(&dir.0, &case.case_id).is_err());
        fs::write(&path, &original).unwrap();
    }
    let out_path = base.join("validate.stdout");
    let original = fs::read(&out_path).unwrap();
    mutate(&out_path, |v| v["workflows"] = json!(2));
    assert!(fixtures::load_prepared(&dir.0, &case.case_id).is_err());
    fs::write(&out_path, original).unwrap();
    let payload = base.join("payload_0.json");
    let mut bytes = fs::read(&payload).unwrap();
    let index = bytes.iter().rposition(|b| *b == b'x').unwrap();
    bytes[index] = b'y';
    fs::write(&payload, bytes).unwrap();
    assert_eq!(
        fixtures::load_prepared(&dir.0, &case.case_id)
            .err()
            .unwrap()
            .code,
        "byte_identity"
    );
}

#[test]
fn named_negative_admission_preserves_exact_defect_and_rejects_other_mutations() {
    for name in [
        "control_missing_source",
        "control_malformed_source",
        "control_cycle",
    ] {
        let case = catalog::case(name).unwrap();
        let dir = Directory::new();
        write_prepared(&dir.0, &case);
        let loaded = fixtures::load_prepared(&dir.0, name).unwrap();
        assert_ne!(loaded.manifest.expected_preflight, Preflight::Valid);
        assert_eq!(loaded.root.arazzo, "1.1.0");
        if case.control == Some(ControlKind::Cycle) {
            assert_eq!(loaded.root.workflows[0].steps[0].depends_on, vec!["step_1"]);
            mutate(&dir.0.join(name).join("validate.stdout"), |v| {
                v["errors"] = json!([{"kind":"OtherFailure"}])
            });
        } else {
            fs::write(dir.0.join(name).join("source_0.json"), b"{}").unwrap();
        }
        assert!(fixtures::load_prepared(&dir.0, name).is_err());
    }
    let case = catalog::case("sources_f1_o10").unwrap();
    let dir = Directory::new();
    write_prepared(&dir.0, &case);
    fs::remove_file(dir.0.join(&case.case_id).join("source_0.json")).unwrap();
    assert!(fixtures::load_prepared(&dir.0, &case.case_id).is_err());
}

#[test]
fn preparation_refuses_unavailable_or_wrong_cli_and_nonfresh_destination() {
    let dir = Directory::new();
    let options = fixtures::PrepareOptions {
        output_dir: dir.0.join("out"),
        data_endpoint: ENDPOINT.into(),
        cli_path: PathBuf::from("/no/such/prebuilt/arazzo"),
    };
    assert_eq!(fixtures::prepare(&options).unwrap_err().code, "cli_path");
    let options = fixtures::PrepareOptions {
        cli_path: std::env::current_exe().unwrap(),
        ..options
    };
    assert_eq!(fixtures::prepare(&options).unwrap_err().code, "cli_version");
    assert_eq!(
        fixtures::prepare(&options).unwrap_err().code,
        "fresh_directory"
    );
    assert!(fixtures::payload_bytes(0, 10, 12).is_err());
    assert!(fixtures::payload_bytes(0, 1024, 11).is_err());
    assert_eq!(
        catalog::case("control_over_cap_fixed")
            .unwrap()
            .settings
            .response_cap,
        1023
    );
    assert_eq!(
        catalog::case("control_over_cap_chunked")
            .unwrap()
            .response_policy
            .transfer,
        Transfer::Chunked
    );
    assert_eq!(
        catalog::case("bodies_s1_b1024_fullbody_resultonly_fixed")
            .unwrap()
            .settings
            .output_policy,
        OutputPolicy::FullBody
    );
}

#[cfg(unix)]
#[test]
fn setup_command_deadline_kills_and_reaps_the_owned_process() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Directory::new();
    let script = dir.0.join("prebuilt-probe");
    fs::write(
        &script,
        b"#!/bin/sh\nprintf '%s' \"$$\" > \"$0.pid\"\nexec /bin/sleep 30\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let options = fixtures::PrepareOptions {
        output_dir: dir.0.join("out"),
        data_endpoint: ENDPOINT.into(),
        cli_path: script.clone(),
    };
    let started = std::time::Instant::now();
    assert_eq!(
        fixtures::prepare(&options).unwrap_err().code,
        "cli_deadline"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(6));
    let pid = fs::read_to_string(script.with_extension("pid")).unwrap();
    let status = std::process::Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success(), "timed-out setup process is still alive");
}

#[test]
fn dag_settings_variants_preserve_identical_documents_and_all_dependency_edges() {
    for shape in ["w1", "w8", "w40", "w128", "diamond"] {
        let reference = catalog::case(&format!("dag_{shape}_pfalse_r0")).unwrap();
        let expected = fixtures::generated_files(&reference, ENDPOINT).unwrap();
        for parallel in [false, true] {
            for rate in [0, 10] {
                let other = catalog::case(&format!("dag_{shape}_p{parallel}_r{rate}")).unwrap();
                assert_eq!(
                    fixtures::generated_files(&other, ENDPOINT).unwrap(),
                    expected
                );
                let root: arazzo_spec::ArazzoSpec = serde_json::from_slice(&expected[0].1).unwrap();
                for (index, step) in root.workflows[0].steps.iter().enumerate() {
                    assert_eq!(
                        step.depends_on,
                        catalog::predecessors(&other, index)
                            .into_iter()
                            .map(|p| format!("step_{p}"))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }
}
