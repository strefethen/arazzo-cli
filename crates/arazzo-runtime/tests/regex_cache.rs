//! Shared regex criteria preserve complete serial/parallel HTTP traces.
#![allow(clippy::expect_used)]

mod common;

use arazzo_runtime::{ExecutionResult, TraceStepRecord};
use arazzo_spec::{CriterionType, Step, StepTarget, SuccessCriterion, Workflow};
use common::{
    fixture_engine_builder, make_spec_with_base, start_server_concurrent, MockHttpResponse,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const WORKFLOW: &str = "shared-regex";
const PATTERN: &str = "^ready$";
const FIXED_DATE: &str = "Wed, 28 Oct 2026 12:00:00 GMT";
const STEPS: [&str; 3] = ["a", "b", "c"];

fn shared_regex_workflow() -> Workflow {
    Workflow {
        workflow_id: WORKFLOW.to_owned(),
        steps: STEPS
            .iter()
            .map(|step| Step {
                step_id: (*step).to_owned(),
                target: Some(StepTarget::OperationPath(format!(
                    "{{$sourceDescriptions.test.url}}#/paths/~1{step}/get"
                ))),
                success_criteria: vec![SuccessCriterion {
                    condition: PATTERN.to_owned(),
                    context: "$response.body#/text".to_owned(),
                    type_: Some(CriterionType::Name("regex".to_owned())),
                    ..SuccessCriterion::default()
                }],
                outputs: BTreeMap::from([("text".to_owned(), "$response.body#/text".into())]),
                ..Step::default()
            })
            .collect(),
        outputs: STEPS
            .iter()
            .map(|step| {
                (
                    (*step).to_owned(),
                    format!("$steps.{step}.outputs.text").into(),
                )
            })
            .collect(),
        ..Workflow::default()
    }
}

async fn execute(base_url: &str, parallel: bool) -> (ExecutionResult, Duration) {
    let spec = make_spec_with_base(base_url, vec![shared_regex_workflow()]);
    let engine = fixture_engine_builder(spec)
        .parallel(parallel)
        .trace(true)
        .build()
        .expect("build the hermetic engine");
    let start = Instant::now();
    let result = engine.execute_collect(WORKFLOW, BTreeMap::new()).await;
    (result, start.elapsed())
}

fn comparable_trace_records(result: &ExecutionResult, elapsed: Duration) -> Vec<TraceStepRecord> {
    let traces = result.trace_steps();
    assert_eq!(traces.len(), STEPS.len());
    traces
        .into_iter()
        .enumerate()
        .map(|(index, trace)| {
            assert_eq!(trace.workflow_id, WORKFLOW);
            assert_eq!(trace.step_id, STEPS[index], "source step replay order");
            assert_eq!(trace.attempt, 1, "one complete attempt per step");
            assert_eq!(trace.seq, index as u64 + 1, "unique monotonic sequence");
            assert!(trace.error.is_none(), "{trace:?}");
            assert_eq!(trace.criteria.len(), 1);
            assert_eq!(trace.criteria[0].type_, "regex");
            assert_eq!(trace.criteria[0].condition, PATTERN);
            assert!(trace.criteria[0].result, "{trace:?}");
            assert_eq!(
                trace.outputs,
                BTreeMap::from([("text".to_owned(), json!("ready"))])
            );
            let response = trace.response.as_ref().expect("record the HTTP response");
            assert_eq!(
                response.headers.get("date").map(String::as_str),
                Some(FIXED_DATE)
            );
            assert_eq!(response.status_code, 200);
            assert!(
                u128::from(trace.duration_ms) <= elapsed.as_millis() + 1,
                "raw step duration {}ms exceeds observed run {elapsed:?}",
                trace.duration_ms
            );
            let mut comparable = trace.clone();
            // Same server, stateless bodies and a fixed Date leave only the
            // elapsed duration volatile. Every other field compares below.
            comparable.duration_ms = 0;
            comparable
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_regex_has_identical_outputs_and_complete_traces_with_parallel_overlap() {
    let parallel_phase = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(AtomicUsize::new(0));
    let overlap_proven = Arc::new(AtomicBool::new(false));
    let arrivals = Arc::new((Mutex::new(0usize), Condvar::new()));
    let server = {
        let parallel_phase = Arc::clone(&parallel_phase);
        let requests = Arc::clone(&requests);
        let overlap_proven = Arc::clone(&overlap_proven);
        let arrivals = Arc::clone(&arrivals);
        start_server_concurrent(move |method, path, _headers, body| {
            assert_eq!(method, "GET");
            assert!(STEPS.iter().any(|step| path == format!("/{step}")));
            assert!(body.is_empty());
            requests.fetch_add(1, Ordering::Relaxed);
            if parallel_phase.load(Ordering::Relaxed) {
                let (count, ready) = &*arrivals;
                let mut count = count.lock().expect("arrival counter");
                *count += 1;
                if *count == STEPS.len() {
                    // The other handlers are blocked until this arrival, so
                    // this proves concurrent in-flight requests without a
                    // throughput or elapsed-time threshold.
                    overlap_proven.store(true, Ordering::Relaxed);
                    ready.notify_all();
                }
                let (count, _) = ready
                    .wait_timeout_while(count, Duration::from_secs(5), |count| *count < STEPS.len())
                    .expect("wait for the parallel level");
                if *count < STEPS.len() {
                    // A serial fallback must fail visibly, never pass by
                    // serving the expected successful response after timeout.
                    return MockHttpResponse::empty(500);
                }
            }
            let mut response = MockHttpResponse::json(200, r#"{"text":"ready"}"#);
            response
                .headers
                .insert("Date".to_owned(), FIXED_DATE.to_owned());
            response
                .headers
                .insert("X-Regex-Fixture".to_owned(), "shared".to_owned());
            response
        })
    };

    let (serial, serial_elapsed) = execute(&server.base_url, false).await;
    assert_eq!(requests.load(Ordering::Relaxed), STEPS.len());
    assert!(!overlap_proven.load(Ordering::Relaxed));
    parallel_phase.store(true, Ordering::Relaxed);
    let (parallel, parallel_elapsed) = execute(&server.base_url, true).await;
    assert_eq!(requests.load(Ordering::Relaxed), 2 * STEPS.len());
    assert!(
        overlap_proven.load(Ordering::Relaxed),
        "parallel HTTP requests must overlap"
    );
    assert!(serial.sequential_fallbacks().is_empty());
    assert!(parallel.sequential_fallbacks().is_empty());
    let expected = BTreeMap::from(STEPS.map(|step| (step.to_owned(), json!("ready"))));
    assert_eq!(serial.outputs.as_ref().expect("serial outputs"), &expected);
    assert_eq!(
        parallel.outputs.as_ref().expect("parallel outputs"),
        &expected
    );
    assert_eq!(
        comparable_trace_records(&serial, serial_elapsed),
        comparable_trace_records(&parallel, parallel_elapsed),
        "every TraceStepRecord field except separately bounded duration_ms"
    );
}
