use std::collections::BTreeSet;
use std::path::Path;

use arazzo_spec::{ArazzoSpec, Info, SourceDescription, SourceType, Step, StepTarget, Workflow};
use arazzo_validate::{
    parse_bytes_with_diagnostics, Diagnostic, Error, Severity, ValidationErrorKind,
};
use serde_yaml_ng::{Mapping, Value};

pub const ROOT_BYTE_CAP: usize = 2 * 1024 * 1024;
pub const SOURCE_BYTE_CAP: usize = 4 * 1024 * 1024;
pub const DUPLICATE_PATH: &str = "workflow \"root_load\" > step \"step_0000\".stepId";
pub type ParseResult = Result<(ArazzoSpec, Vec<Diagnostic>), Error>;

pub struct RootCase {
    pub name: &'static str,
    pub steps: usize,
    pub bytes: Vec<u8>,
    expected: ArazzoSpec,
}

impl RootCase {
    pub fn verify(&self, result: &ParseResult) -> Result<(), String> {
        let (actual, diagnostics) = result
            .as_ref()
            .map_err(|error| format!("{}: unexpected parser failure: {error}", self.name))?;
        if !diagnostics.is_empty() {
            return Err(format!(
                "{}: expected zero diagnostics, got {diagnostics:?}",
                self.name
            ));
        }
        if actual != &self.expected {
            return Err(format!(
                "{}: complete model differs from independently constructed {}-step expectation",
                self.name, self.steps
            ));
        }
        Ok(())
    }
}

pub struct Fixtures {
    pub roots: [RootCase; 3],
    pub source: Value,
    pub source_bytes: Vec<u8>,
    pub malformed: Vec<u8>,
    pub duplicate: Vec<u8>,
}

impl Fixtures {
    pub fn load() -> Result<Self, String> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/root_load");
        let source_bytes = read_fixture(&directory, "openapi.yaml", SOURCE_BYTE_CAP)?;
        let source = verify_source(&source_bytes)?;
        let prepare = |name, steps| -> Result<RootCase, String> {
            let bytes = read_fixture(&directory, name, ROOT_BYTE_CAP)?;
            let expected = expected_root(steps);
            verify_operation_resolution(&source, &expected)?;
            Ok(RootCase {
                name,
                steps,
                bytes,
                expected,
            })
        };
        Ok(Self {
            roots: [
                prepare("steps_10.yaml", 10)?,
                prepare("steps_100.yaml", 100)?,
                prepare("steps_1000.yaml", 1000)?,
            ],
            source,
            source_bytes,
            malformed: read_fixture(&directory, "malformed.yaml", ROOT_BYTE_CAP)?,
            duplicate: read_fixture(&directory, "duplicate_step.yaml", ROOT_BYTE_CAP)?,
        })
    }

    pub fn preflight(&self) -> Result<(), String> {
        eprintln!(
            "root_load preparation PASS source=openapi.yaml bytes={} operations=10",
            self.source_bytes.len()
        );
        let mut successful = 0;
        for root in &self.roots {
            let result = parse_bytes_with_diagnostics(&root.bytes);
            root.verify(&result)
                .and_then(|()| match &result {
                    Ok((spec, _)) => verify_operation_resolution(&self.source, spec),
                    Err(error) => Err(error.to_string()),
                })
                .map_err(|error| preflight_failure(root.name, successful, error))?;
            successful += 1;
            eprintln!(
                "root_load preflight PASS case={} steps={} bytes={} requested=5 successful={successful} failed=0",
                root.name, root.steps, root.bytes.len()
            );
        }
        for (name, bytes, verify) in [
            (
                "malformed.yaml",
                self.malformed.as_slice(),
                verify_malformed as fn(&ParseResult) -> Result<(), String>,
            ),
            (
                "duplicate_step.yaml",
                self.duplicate.as_slice(),
                verify_duplicate,
            ),
        ] {
            verify(&parse_bytes_with_diagnostics(bytes))
                .map_err(|error| preflight_failure(name, successful, error))?;
            successful += 1;
            eprintln!(
                "root_load control PASS case={name} bytes={} requested=5 successful={successful} failed=0",
                bytes.len()
            );
        }
        Ok(())
    }
}

fn preflight_failure(name: &str, successful: usize, error: String) -> String {
    format!(
        "preflight case={name} requested=5 successful={successful} failed=1 unattempted={}: {error}",
        4 - successful
    )
}

pub fn verify_cap(name: &str, bytes: &[u8], cap: usize) -> Result<(), String> {
    if bytes.len() > cap {
        return Err(format!(
            "{name}: {} bytes exceeds experimental fixture cap {cap}",
            bytes.len()
        ));
    }
    Ok(())
}

fn read_fixture(directory: &Path, name: &str, cap: usize) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(directory.join(name))
        .map_err(|error| format!("{name}: reading fixture: {error}"))?;
    verify_cap(name, &bytes, cap)?;
    Ok(bytes)
}

fn expected_root(count: usize) -> ArazzoSpec {
    ArazzoSpec {
        arazzo: "1.1.0".to_owned(),
        info: Info {
            title: "Root load benchmark".to_owned(),
            version: "1.0.0".to_owned(),
            ..Info::default()
        },
        source_descriptions: vec![SourceDescription {
            name: "api".to_owned(),
            url: "openapi.yaml".to_owned(),
            type_: SourceType::OpenApi,
            ..SourceDescription::default()
        }],
        workflows: vec![Workflow {
            workflow_id: "root_load".to_owned(),
            steps: (0..count)
                .map(|index| Step {
                    step_id: format!("step_{index:04}"),
                    target: Some(StepTarget::OperationId(format!("op_{:02}", index % 10))),
                    ..Step::default()
                })
                .collect(),
            ..Workflow::default()
        }],
        ..ArazzoSpec::default()
    }
}

fn mapping<const N: usize>(entries: [(&str, Value); N]) -> Value {
    Value::Mapping(
        entries
            .into_iter()
            .map(|(key, value)| (Value::String(key.to_owned()), value))
            .collect(),
    )
}

fn expected_source() -> Value {
    let paths: Mapping = (0..10)
        .map(|index| {
            (
                Value::String(format!("/op_{index:02}")),
                mapping([(
                    "get",
                    mapping([
                        ("operationId", Value::String(format!("op_{index:02}"))),
                        (
                            "responses",
                            mapping([(
                                "200",
                                mapping([("description", Value::String("OK".to_owned()))]),
                            )]),
                        ),
                    ]),
                )]),
            )
        })
        .collect();
    mapping([
        ("openapi", Value::String("3.2.0".to_owned())),
        (
            "info",
            mapping([
                ("title", Value::String("Root load API".to_owned())),
                ("version", Value::String("1.0.0".to_owned())),
            ]),
        ),
        ("paths", Value::Mapping(paths)),
    ])
}

pub fn verify_source(bytes: &[u8]) -> Result<Value, String> {
    verify_cap("openapi.yaml", bytes, SOURCE_BYTE_CAP)?;
    let actual: Value = serde_yaml_ng::from_slice(bytes)
        .map_err(|error| format!("openapi.yaml: parsing source fixture: {error}"))?;
    if actual != expected_source() {
        return Err(
            "openapi.yaml: complete source shape differs from independent expectation".into(),
        );
    }
    Ok(actual)
}

pub fn verify_operation_resolution(source: &Value, root: &ArazzoSpec) -> Result<(), String> {
    let mut operations = BTreeSet::new();
    for index in 0..10 {
        let path = format!("/op_{index:02}");
        let id = source["paths"][&path]["get"]["operationId"]
            .as_str()
            .ok_or_else(|| format!("openapi.yaml: missing operation at {path}"))?;
        if !operations.insert(id) {
            return Err(format!("openapi.yaml: duplicate operationId {id}"));
        }
    }
    for workflow in &root.workflows {
        for step in &workflow.steps {
            match &step.target {
                Some(StepTarget::OperationId(id)) if operations.contains(id.as_str()) => {}
                target => {
                    return Err(format!(
                        "workflow {} step {}: unresolved fixture operation {target:?}",
                        workflow.workflow_id, step.step_id
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn verify_malformed(result: &ParseResult) -> Result<(), String> {
    match result {
        Err(Error::ParseYaml(_)) => Ok(()),
        other => Err(format!("malformed.yaml: expected ParseYaml, got {other:?}")),
    }
}

pub fn verify_duplicate(result: &ParseResult) -> Result<(), String> {
    match result {
        Err(Error::Validation(report))
            if report.errors.len() == 1
                && report.warnings.is_empty()
                && report.errors[0].severity == Severity::Error
                && report.errors[0].kind == ValidationErrorKind::DuplicateIdentifier
                && report.errors[0].path == DUPLICATE_PATH =>
        {
            Ok(())
        }
        other => Err(format!(
            "duplicate_step.yaml: expected one Error DuplicateIdentifier at {DUPLICATE_PATH} and zero warnings, got {other:?}"
        )),
    }
}
