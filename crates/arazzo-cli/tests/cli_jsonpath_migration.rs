#![forbid(unsafe_code)]

//! End-to-end CLI proof that every typed JSONPath surface now runs on the one
//! shared RFC 9535 query owner (`arazzo_expr::JsonPathQuery`).
//!
//! The three production entry points are
//! `runtime_core::criteria` (success criteria),
//! `runtime_core::payload::resolve_selector_checked` (Selector Objects) and
//! `runtime_core::payload` replacement targets. This file drives all three in
//! one workflow against a hermetic `tiny_http` fixture, then asserts what an
//! operator can actually see: the captured wire body, the workflow outputs,
//! the process exit result and the `--trace` file.
//!
//! Arazzo v1.1.0 §5.8.11.4.3 requires a `type: jsonpath` condition to be *"a
//! valid JSONPath expression conforming to [RFC9535]"* and decides it on
//! nodelist cardinality; §5.8.12 requires an implementation to apply the named
//! version's semantics. RFC 9535 §2.4.6/§2.4.7 define `match()` and `search()`,
//! whose second argument may be a literal or a value drawn from the queried
//! document — both forms appear below, in criteria and in selectors.
//!
//! Negative controls cover the two rejection classes that reach a user: an
//! explicitly declared JSONPath version this runtime does not implement, and
//! an operational or admission failure. Neither may report success and neither
//! may write anything into the request body.
//!
//! Scope note: GJSON-syntax rejection is owned by
//! `crates/arazzo-cli/tests/cli_jsonpath_gjson.rs` and
//! `crates/arazzo-runtime/tests/jsonpath_gjson_rejection.rs`; per-consumer
//! selector and cardinality semantics are owned by the `arazzo-runtime`
//! `jsonpath_semantics.rs` / `jsonpath_selectors.rs` suites. This file does not
//! repeat them.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const UNSUPPORTED_VERSION: &str = "unsupported JSONPath version";
const GOESSNER: &str = "draft-goessner-dispatch-jsonpath-00";
const STRUCTURAL_LIMIT: &str = "JSONPath query structural characters limit of 128 exceeded";

/// Response body served for `GET /catalog`. One item matches both regex
/// functions, the other matches neither, so every assertion below has a
/// negative half inside the same document.
const CATALOG: &str = r#"{
  "items": [
    {"sku": "A-101", "description": "urgent restock", "tag": "urg.nt"},
    {"sku": "B-202", "description": "routine", "tag": "zzz"}
  ]
}"#;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time must be after the Unix epoch: {err}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn repo_root() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&path).unwrap_or_else(|err| panic!("canonicalizing {}: {err}", path.display()))
}

/// Prefer the binary Cargo built for this test run; fall back to the debug
/// artifact only when the harness did not export the variable.
fn cli_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_arazzo-cli") {
        return PathBuf::from(path);
    }
    let path = repo_root().join("target/debug/arazzo-cli");
    assert!(path.exists(), "CLI binary not found at {}", path.display());
    path
}

fn write_file(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(cli_bin())
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap_or_else(|err| panic!("running arazzo-cli {args:?}: {err}"))
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "parsing JSON stdout failed: {err}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn read_json(path: &Path) -> Value {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()))
}

fn string_array(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn array_of(value: &Value, field: &str) -> Vec<Value> {
    match value.get(field).and_then(Value::as_array) {
        Some(items) => items.clone(),
        None => panic!("expected an array at {field}: {value}"),
    }
}

/// One captured request: what actually went over the loopback socket.
#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    path: String,
    body: String,
}

struct FixtureServer {
    base_url: String,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn captured(&self) -> Vec<CapturedRequest> {
        match self.captured.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Hermetic `tiny_http` fixture on an ephemeral loopback port. `GET /catalog`
/// answers with [`CATALOG`]; `POST /orders` records the received body and
/// echoes it back under `received`, so the wire body is provable twice — once
/// from the capture list and once through a downstream selector.
fn start_fixture_server() -> FixtureServer {
    let server = tiny_http::Server::http("127.0.0.1:0")
        .unwrap_or_else(|err| panic!("bind hermetic tiny_http server: {err}"));
    let base_url = format!("http://{}", server.server_addr());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match server.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(mut request)) => {
                    let method = request.method().as_str().to_string();
                    let path = request.url().to_string();
                    let mut body = String::new();
                    let _ = request.as_reader().read_to_string(&mut body);
                    let payload = if method == "POST" {
                        let received: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                        json!({"accepted": true, "received": received}).to_string()
                    } else {
                        CATALOG.to_string()
                    };
                    if let Ok(mut guard) = sink.lock() {
                        guard.push(CapturedRequest { method, path, body });
                    }
                    let response = tiny_http::Response::from_string(payload)
                        .with_status_code(tiny_http::StatusCode(200))
                        .with_header(
                            tiny_http::Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"application/json"[..],
                            )
                            .unwrap_or_else(|()| panic!("building the content-type header")),
                        );
                    let _ = request.respond(response);
                }
                Ok(None) => {}
                Err(_) => break,
            }
        }
    });
    FixtureServer {
        base_url,
        captured,
        stop,
        handle: Some(handle),
    }
}

/// The integrated cutover workflow.
///
/// Step `catalog` decides three `type: jsonpath` criteria — a literal-pattern
/// `match()`, a `search()` whose pattern is a sibling node, and a
/// descent-plus-slice query that the retired handwritten subset rejected
/// outright — then projects four Selector Objects, one of which legitimately
/// selects nothing.
///
/// Step `order` feeds those selections into three JSONPath replacement
/// targets, including a descent target and a nested Selector Object value, and
/// posts the result.
#[test]
fn cli_workflow_runs_criteria_selectors_and_replacements_on_the_shared_engine() {
    let server = start_fixture_server();
    let temp = TempDir::new("arazzo-cli-jsonpath-migration");
    let spec_path = temp.path().join("migration.arazzo.yaml");
    let trace_path = temp.path().join("trace.json");
    write_file(
        &spec_path,
        &format!(
            r#"arazzo: 1.1.0
info:
  title: RFC 9535 integrated cutover
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: {base}
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: catalog
        operationPath: GET /catalog
        successCriteria:
          - context: $response.body
            condition: '$.items[?match(@.sku, "A-[0-9]{{3}}")]'
            type: jsonpath
          - context: $response.body
            condition: '$.items[?search(@.description, @.tag)]'
            type:
              type: jsonpath
              version: rfc9535
          - context: $response.body
            condition: '$..items[0:1]'
            type: jsonpath
        outputs:
          urgentSku:
            context: $response.body
            selector: '$.items[?search(@.description, @.tag)].sku'
            type:
              type: jsonpath
              version: rfc9535
          everySku:
            context: $response.body
            selector: '$..sku'
            type: jsonpath
          absentSku:
            context: $response.body
            selector: '$.items[?match(@.sku, "Z-[0-9]{{3}}")].sku'
            type: jsonpath
          catalogBody:
            context: $response.body
            selector: '$'
            type: jsonpath
      - stepId: order
        operationPath: POST /orders
        requestBody:
          contentType: application/json
          payload:
            order:
              sku: unset
              note: unset
            audit:
              skus: []
          replacements:
            - target: $.order.sku
              targetSelectorType: jsonpath
              value: $steps.catalog.outputs.urgentSku
            - target: $.audit.skus
              targetSelectorType: jsonpath
              value: $steps.catalog.outputs.everySku
            - target: '$..note'
              targetSelectorType:
                type: jsonpath
                version: rfc9535
              value:
                context: $steps.catalog.outputs.catalogBody
                selector: '$.items[?match(@.sku, "A-101")].description'
                type:
                  type: jsonpath
                  version: rfc9535
        successCriteria:
          - context: $response.body
            condition: '$.accepted'
            type: jsonpath
        outputs:
          echoedSku:
            context: $response.body
            selector: '$.received.order.sku'
            type: jsonpath
    outputs:
      urgentSku: $steps.catalog.outputs.urgentSku
      everySku: $steps.catalog.outputs.everySku
      absentSku: $steps.catalog.outputs.absentSku
      echoedSku: $steps.order.outputs.echoedSku
"#,
            base = server.base_url
        ),
    );

    let spec = spec_path.to_string_lossy().to_string();
    let trace = trace_path.to_string_lossy().to_string();
    let output = run(&[
        "--json",
        "run",
        &spec,
        "wf",
        "--trace",
        &trace,
        "--expr-diagnostics",
        "warn",
    ]);
    assert!(
        output.status.success(),
        "the integrated workflow must succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let body = stdout_json(&output);
    assert_eq!(body.get("kind").and_then(Value::as_str), Some("success"));

    // --- the wire body ----------------------------------------------------
    let captured = server.captured();
    assert_eq!(captured.len(), 2, "{captured:?}");
    assert_eq!(captured[0].method, "GET", "{captured:?}");
    assert_eq!(captured[0].path, "/catalog", "{captured:?}");
    assert_eq!(captured[1].method, "POST", "{captured:?}");
    assert_eq!(captured[1].path, "/orders", "{captured:?}");
    let posted: Value = serde_json::from_str(&captured[1].body)
        .unwrap_or_else(|err| panic!("parsing the captured POST body: {err}; {captured:?}"));
    assert_eq!(
        posted,
        json!({
            "order": {
                // one match collapses to the scalar
                "sku": "A-101",
                // descent target, nested Selector Object value
                "note": "urgent restock"
            },
            "audit": {
                // many matches keep query order as an array
                "skus": ["A-101", "B-202"]
            }
        }),
        "every replacement, and nothing else, reached the wire"
    );

    // --- the outputs ------------------------------------------------------
    let outputs = match body.get("outputs") {
        Some(outputs) => outputs,
        None => panic!("expected run outputs: {body}"),
    };
    assert_eq!(outputs.get("urgentSku"), Some(&json!("A-101")), "{outputs}");
    assert_eq!(
        outputs.get("everySku"),
        Some(&json!(["A-101", "B-202"])),
        "{outputs}"
    );
    // A selector that matches nothing is a legitimate null, not a failure:
    // the run still succeeds and the only signal is the no-match warning.
    assert_eq!(outputs.get("absentSku"), Some(&Value::Null), "{outputs}");
    assert_eq!(
        outputs.get("echoedSku"),
        Some(&json!("A-101")),
        "the server saw the replaced value: {outputs}"
    );

    let warnings = string_array(&body, "warnings");
    let no_match: Vec<&String> = warnings
        .iter()
        .filter(|warning| warning.contains("selector matched no values"))
        .collect();
    assert_eq!(no_match.len(), 1, "{warnings:?}");
    assert!(
        warnings.iter().all(|warning| {
            !warning.contains("invalid JSONPath syntax")
                && !warning.contains(UNSUPPORTED_VERSION)
                && !warning.contains("JSONPath evaluation failed")
        }),
        "no JSONPath rejection may appear on the sound path: {warnings:?}"
    );

    // --- the trace --------------------------------------------------------
    let trace_file = read_json(&trace_path);
    let steps = array_of(&trace_file, "steps");
    assert_eq!(steps.len(), 2, "{trace_file}");

    let criteria = array_of(&steps[0], "criteria");
    assert_eq!(criteria.len(), 3, "{trace_file}");
    for criterion in &criteria {
        assert_eq!(
            criterion.get("type"),
            Some(&json!("jsonpath")),
            "{criterion}"
        );
        assert_eq!(criterion.get("result"), Some(&json!(true)), "{criterion}");
        assert!(
            string_array(criterion, "warnings").is_empty(),
            "{criterion}"
        );
    }

    let request = match steps[1].get("request") {
        Some(request) => request,
        None => panic!("expected a traced request for the order step: {trace_file}"),
    };
    assert_eq!(
        request.get("body"),
        Some(&posted),
        "the trace and the wire must agree: {trace_file}"
    );
    assert_eq!(
        steps[1]
            .get("response")
            .and_then(|response| response.get("statusCode")),
        Some(&json!(200)),
        "{trace_file}"
    );
    let traced_outputs = match steps[1].get("outputs") {
        Some(outputs) => outputs,
        None => panic!("expected traced step outputs: {trace_file}"),
    };
    assert_eq!(
        traced_outputs.get("echoedSku"),
        Some(&json!("A-101")),
        "{trace_file}"
    );
}

/// Negative control on the resolution half of the run. A dry run resolves
/// parameters, payloads and replacement targets without sending anything, so
/// it is where an unsupported declared version and an admission-limit
/// rejection become observable — as `--json` warnings with the resolved body
/// untouched. The sound control replacement in the same list still applies,
/// which is what proves the rejections were targeted rather than a blanket
/// bail-out.
#[test]
fn dry_run_rejects_goessner_and_over_budget_targets_without_writing() {
    let temp = TempDir::new("arazzo-cli-jsonpath-migration-dry-run");
    let spec_path = temp.path().join("rejected.arazzo.yaml");
    // 200 `.` bytes: over the 128 structural-character admission budget, and
    // rejected before the upstream parser ever sees the expression.
    let over_budget = format!("${}", ".a".repeat(200));
    write_file(
        &spec_path,
        &format!(
            r#"arazzo: 1.1.0
info:
  title: Rejected typed JSONPath surfaces
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        document:
          type: object
    steps:
      - stepId: update
        operationPath: POST /items
        requestBody:
          contentType: application/json
          payload:
            keep: old
            versioned: old
            budgeted: old
            valued: old
          replacements:
            - target: $.keep
              targetSelectorType: jsonpath
              value: new
            - target: $.versioned
              targetSelectorType:
                type: jsonpath
                version: {goessner}
              value: new
            - target: '{over_budget}'
              targetSelectorType: jsonpath
              value: new
            - target: $.valued
              targetSelectorType: jsonpath
              value:
                context: $inputs.document
                selector: '$.items[?match(@.sku, "A-101")].sku'
                type:
                  type: jsonpath
                  version: {goessner}
"#,
            goessner = GOESSNER,
            over_budget = over_budget
        ),
    );

    let spec = spec_path.to_string_lossy().to_string();
    let document = r#"document={"items":[{"sku":"A-101"}]}"#;
    let output = run(&["--json", "run", &spec, "wf", "--dry-run", "-i", document]);
    assert!(
        output.status.success(),
        "a rejected target warns and continues; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let body = stdout_json(&output);
    let requests = array_of(&body, "requests");
    assert_eq!(requests.len(), 1, "{body}");
    assert_eq!(
        requests[0].get("body"),
        Some(&json!({
            "keep": "new",
            "versioned": "old",
            "budgeted": "old",
            "valued": "old"
        })),
        "only the sound replacement may write: {body}"
    );

    let warnings = string_array(&requests[0], "warnings");
    let indexed = |prefix: &str| -> Vec<String> {
        warnings
            .iter()
            .filter(|warning| warning.starts_with(prefix))
            .cloned()
            .collect()
    };

    assert!(
        indexed("requestBody.replacements[0]:").is_empty(),
        "the sound replacement must not warn: {warnings:?}"
    );

    let version_warnings = indexed("requestBody.replacements[1]:");
    assert_eq!(version_warnings.len(), 1, "{warnings:?}");
    assert!(
        version_warnings[0].contains(UNSUPPORTED_VERSION) && version_warnings[0].contains(GOESSNER),
        "{warnings:?}"
    );

    let budget_warnings = indexed("requestBody.replacements[2]:");
    assert_eq!(budget_warnings.len(), 1, "{warnings:?}");
    assert!(
        budget_warnings[0].contains(STRUCTURAL_LIMIT),
        "{warnings:?}"
    );

    // A rejected replacement *value* reports twice: the selector diagnostic
    // and the notice that nothing was written.
    let value_warnings = indexed("requestBody.replacements[3]:");
    assert_eq!(value_warnings.len(), 2, "{warnings:?}");
    assert!(
        value_warnings[0].contains(UNSUPPORTED_VERSION) && value_warnings[0].contains(GOESSNER),
        "{warnings:?}"
    );
    assert!(
        value_warnings[1].contains("value did not resolve; replacement skipped and body unchanged"),
        "{warnings:?}"
    );
}

/// Negative control on the criterion half of the run. A dry run never
/// evaluates success criteria, so each rejection class is proven against the
/// live fixture, one workflow apiece because a failing criterion ends its
/// step. Both must fail the run with the ordinary criteria failure and record
/// the diagnostic on the criterion itself.
#[test]
fn live_criteria_fail_on_unsupported_version_and_on_operational_failure() {
    let server = start_fixture_server();
    let temp = TempDir::new("arazzo-cli-jsonpath-migration-criteria");
    let spec_path = temp.path().join("criteria.arazzo.yaml");
    write_file(
        &spec_path,
        &format!(
            r#"arazzo: 1.1.0
info:
  title: Rejected typed JSONPath criteria
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: {base}
    type: openapi
workflows:
  - workflowId: versioned
    steps:
      - stepId: check
        operationPath: GET /catalog
        successCriteria:
          - context: $response.body
            condition: '$.items[0].sku'
            type:
              type: jsonpath
              version: {goessner}
  - workflowId: operational
    steps:
      - stepId: check
        operationPath: GET /catalog
        successCriteria:
          - context: $response.body
            condition: '$.items[?match(@.sku, "a{{1000000000}}")]'
            type: jsonpath
"#,
            base = server.base_url,
            goessner = GOESSNER
        ),
    );

    let spec = spec_path.to_string_lossy().to_string();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (workflow, needle) in [
        ("versioned", UNSUPPORTED_VERSION),
        ("operational", "JSONPath evaluation failed"),
    ] {
        let trace_path = temp.path().join(format!("{workflow}-trace.json"));
        let trace = trace_path.to_string_lossy().to_string();
        let output = run(&[
            "--json",
            "run",
            &spec,
            workflow,
            "--trace",
            &trace,
            "--expr-diagnostics",
            "warn",
        ]);
        assert!(
            !output.status.success(),
            "{workflow}: a rejected condition must fail the step; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );

        let body = stdout_json(&output);
        assert_eq!(
            body.get("code").and_then(Value::as_str),
            Some("RUNTIME_SUCCESS_CRITERIA_FAILED"),
            "{workflow}: {body}"
        );

        let trace_file = read_json(&trace_path);
        let steps = array_of(&trace_file, "steps");
        assert_eq!(steps.len(), 1, "{workflow}: {trace_file}");
        // The request was issued and answered: the rejection is a criterion
        // decision, not a refusal to send.
        assert_eq!(
            steps[0]
                .get("response")
                .and_then(|response| response.get("statusCode")),
            Some(&json!(200)),
            "{workflow}: {trace_file}"
        );
        let criteria = array_of(&steps[0], "criteria");
        assert_eq!(criteria.len(), 1, "{workflow}: {trace_file}");
        assert_eq!(
            criteria[0].get("result"),
            Some(&json!(false)),
            "{workflow}: {trace_file}"
        );
        let criterion_warnings = string_array(&criteria[0], "warnings");
        assert_eq!(criterion_warnings.len(), 1, "{criterion_warnings:?}");
        assert!(
            criterion_warnings[0].contains(needle),
            "{workflow}: {criterion_warnings:?}"
        );
        // A failed step records no outputs, so nothing downstream can consume
        // a value the engine never selected.
        assert!(
            steps[0].get("outputs").is_none(),
            "{workflow}: {trace_file}"
        );
        seen.insert(workflow.to_string(), criterion_warnings[0].clone());
    }

    assert_eq!(seen.len(), 2, "{seen:?}");
    let versioned = &seen["versioned"];
    assert!(
        versioned.contains(GOESSNER) && versioned.contains("only rfc9535 is supported"),
        "the diagnostic names the rejected version and the supported one: {versioned}"
    );
}
