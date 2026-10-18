#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);

struct TempFile(PathBuf);

impl TempFile {
    fn write(document: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "arazzo-jentic-negative-{}-{}.arazzo.json",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes =
            serde_json::to_vec(document).unwrap_or_else(|err| panic!("serializing fixture: {err}"));
        fs::write(&path, bytes).unwrap_or_else(|err| panic!("writing temporary fixture: {err}"));
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn cli_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_arazzo-cli")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/arazzo-cli")
        })
}

fn validate(document: &Value, strict: bool) -> (bool, Value) {
    let file = TempFile::write(document);
    validate_path(&file.0, strict)
}

fn validate_path(path: &Path, strict: bool) -> (bool, Value) {
    let mut command = Command::new(cli_bin());
    command.arg("--json");
    if strict {
        command.arg("--strict");
    }
    let output = command
        .arg("validate")
        .arg(path)
        .output()
        .unwrap_or_else(|err| panic!("running validate: {err}"));
    let body = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "validate stdout must be JSON: {err}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    (output.status.success(), body)
}

#[test]
fn jentic_negative_fixtures() {
    struct Case {
        file: &'static str,
        kind: &'static str,
        path: &'static str,
        repair: fn(&mut Value),
    }

    let cases = [
        Case {
            file: "operation-path.arazzo.json",
            kind: "unsupportedOperationPath",
            path: "step \"request\".operationPath",
            repair: |doc| {
                doc["workflows"][0]["steps"][0]["operationPath"] =
                    "{$sourceDescriptions.openapi_source.url}#/paths/~1probe/get".into();
            },
        },
        Case {
            file: "output-key.arazzo.json",
            kind: "invalidIdentifier",
            path: "step \"request\".outputs.items[0].id",
            repair: |doc| {
                let outputs = doc["workflows"][0]["steps"][0]["outputs"]
                    .as_object_mut()
                    .unwrap_or_else(|| panic!("step outputs object"));
                let value = outputs
                    .remove("items[0].id")
                    .unwrap_or_else(|| panic!("defect output key"));
                outputs.insert("item_id".to_string(), value);
            },
        },
        Case {
            file: "response-pointer.arazzo.json",
            kind: "invalidExpression",
            path: "step \"request\".outputs.token",
            repair: |doc| {
                doc["workflows"][0]["steps"][0]["outputs"]["token"] =
                    "$response.body#/access_token".into();
            },
        },
        Case {
            file: "duplicate-step.arazzo.json",
            kind: "duplicateIdentifier",
            path: "step \"request\".stepId",
            repair: |doc| {
                doc["workflows"][0]["steps"][1]["stepId"] = "request_again".into();
            },
        },
    ];

    for case in cases {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../testdata/jentic-negative")
            .join(case.file);
        let source =
            fs::read(&fixture).unwrap_or_else(|err| panic!("reading {}: {err}", fixture.display()));
        let original: Value = serde_json::from_slice(&source)
            .unwrap_or_else(|err| panic!("parsing {}: {err}", fixture.display()));
        assert_eq!(original["arazzo"], "1.0.1", "{}", case.file);

        for version in ["1.0.1", "1.1.0"] {
            let mut negative = original.clone();
            negative["arazzo"] = version.into();
            let mut control = negative.clone();
            (case.repair)(&mut control);
            assert_ne!(
                negative, control,
                "{}: repair must change defect",
                case.file
            );

            for strict in [false, true] {
                let context = format!("{} {version} strict={strict}", case.file);
                let (success, body) = if version == "1.0.1" {
                    validate_path(&fixture, strict)
                } else {
                    validate(&negative, strict)
                };
                assert!(!success, "{context}: invalid fixture passed: {body}");
                assert_eq!(body["kind"], "error", "{context}: {body}");
                assert_eq!(body["valid"], false, "{context}: {body}");
                assert_eq!(
                    body["code"], "VALIDATE_SPEC_VALIDATION",
                    "{context}: {body}"
                );
                assert!(body["error"].is_string(), "{context}: {body}");
                let errors = body["errors"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{context}: structured errors missing: {body}"));
                assert_eq!(errors.len(), 1, "{context}: isolated defect: {body}");
                assert_eq!(errors[0]["source"], "validation", "{context}: {body}");
                assert_eq!(errors[0]["kind"], case.kind, "{context}: {body}");
                let path = errors[0]["path"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{context}: diagnostic path missing: {body}"));
                assert!(
                    path.starts_with("workflow \"probe\" > "),
                    "{context}: {body}"
                );
                assert!(path.ends_with(case.path), "{context}: {body}");

                let (success, body) = validate(&control, strict);
                assert!(success, "{context}: repaired control failed: {body}");
                assert_eq!(body["valid"], true, "{context}: {body}");
                assert_eq!(body["version"], version, "{context}: {body}");
                assert!(body.get("errors").is_none(), "{context}: {body}");
            }
        }
    }
}
