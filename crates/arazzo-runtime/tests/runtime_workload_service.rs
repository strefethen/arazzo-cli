#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../examples/runtime_workload/catalog.rs"]
#[allow(dead_code)]
mod catalog;
#[path = "../examples/runtime_workload/fixtures.rs"]
#[allow(dead_code)]
mod fixtures;
#[path = "../examples/runtime_workload/protocol.rs"]
#[allow(dead_code)]
mod protocol;
#[path = "../examples/runtime_workload/service.rs"]
#[allow(dead_code)]
mod service;
#[path = "../examples/runtime_workload/source_check.rs"]
#[allow(dead_code)]
mod source_check;

use catalog::{HarnessError, Stall};
use protocol::{ControlClient, Identity, ServiceReadiness, Snapshot};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[test]
fn sibling_service() {
    let Ok(budget) = std::env::var("ARAZZO_WORKLOAD_TEST_SERVICE_MS") else {
        return;
    };
    if let Err(error) = service::serve(service::ServiceOptions {
        lifetime_budget: Duration::from_millis(budget.parse().unwrap()),
    }) {
        println!("service_error={}", serde_json::to_string(&error).unwrap());
        panic!("{error}");
    }
}

struct Driver {
    child: Child,
    reader: Option<JoinHandle<String>>,
    ready: ServiceReadiness,
    finished: bool,
}

impl Driver {
    fn start(budget_ms: u64) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "sibling_service", "--nocapture"])
            .env("ARAZZO_WORKLOAD_TEST_SERVICE_MS", budget_ms.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        let reader = thread::spawn(move || {
            let mut log = String::new();
            let mut delivered = false;
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                log.push_str(&line);
                log.push('\n');
                if !delivered {
                    if let Some(start) = line.find('{') {
                        if let Ok(ready) = serde_json::from_str::<ServiceReadiness>(&line[start..])
                        {
                            send.send(ready).unwrap();
                            delivered = true;
                        }
                    }
                }
            }
            log
        });
        let ready = match receive.recv_timeout(Duration::from_secs(5)) {
            Ok(ready) => ready,
            Err(e) => {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("readiness failed: {e}; {}", reader.join().unwrap());
            }
        };
        assert_eq!(ready.protocol_version, 1);
        assert_eq!(ready.lifetime_budget_ms, budget_ms);
        Self {
            child,
            reader: Some(reader),
            ready,
            finished: false,
        }
    }
    fn client(&self) -> ControlClient {
        ControlClient::new(self.ready.control_endpoint)
    }
    fn identity(&self, case_id: &str, invocation: &str) -> Identity {
        Identity {
            catalog_version: 1,
            case_id: case_id.into(),
            invocation_id: invocation.into(),
            response_policy: catalog::case(case_id).unwrap().response_policy,
        }
    }
    fn endpoint(&self) -> std::net::SocketAddr {
        self.ready
            .data_endpoint
            .strip_prefix("http://")
            .unwrap()
            .parse()
            .unwrap()
    }
    fn wait(&mut self, expected_success: bool) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                self.finished = true;
                let output = self.reader.take().unwrap().join().unwrap();
                assert_eq!(status.success(), expected_success, "{output}");
                if !expected_success {
                    assert!(
                        output.contains("\"code\":\"deadline\""),
                        "unexpected service failure: {output}"
                    );
                }
                return status;
            }
            if Instant::now() >= deadline {
                self.child.kill().unwrap();
                self.child.wait().unwrap();
                self.finished = true;
                panic!(
                    "sibling exceeded teardown deadline; {}",
                    self.reader.take().unwrap().join().unwrap()
                );
            }
            thread::park_timeout(Duration::from_millis(1));
        }
    }
    fn stop(mut self) {
        self.client().stop().unwrap();
        self.wait(true);
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            self.child.wait().unwrap();
        }
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

fn request(driver: &Driver, path: &str, invocation: Option<&str>) -> TcpStream {
    let mut stream = TcpStream::connect(driver.endpoint()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let identity = invocation
        .map(|id| format!("{}: {id}\r\n", catalog::INVOCATION_HEADER))
        .unwrap_or_default();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\n{identity}\r\n"
    )
    .unwrap();
    stream
}

fn read_http(mut stream: TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    bytes
}

fn wait_snapshot(
    driver: &Driver,
    id: &Identity,
    condition: impl Fn(&Snapshot) -> bool,
) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = driver.client().snapshot(id).unwrap();
        if condition(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "snapshot barrier not reached: {snapshot:?}"
        );
        thread::yield_now();
    }
}

fn raw_control(driver: &Driver, bytes: &[u8]) -> Value {
    let mut socket = TcpStream::connect(driver.ready.control_endpoint).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket.write_all(bytes).unwrap();
    socket.shutdown(Shutdown::Write).unwrap();
    let mut output = Vec::new();
    socket.read_to_end(&mut output).unwrap();
    protocol::decode_frame(&output, protocol::RESPONSE_CAP).unwrap()
}

fn decode_chunked(wire: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    let mut rest = wire;
    loop {
        let end = rest.windows(2).position(|w| w == b"\r\n").unwrap();
        let length = usize::from_str_radix(std::str::from_utf8(&rest[..end]).unwrap(), 16).unwrap();
        rest = &rest[end + 2..];
        if length == 0 {
            assert_eq!(rest, b"\r\n");
            break;
        }
        decoded.extend_from_slice(&rest[..length]);
        assert_eq!(&rest[length..length + 2], b"\r\n");
        rest = &rest[length + 2..];
    }
    decoded
}

#[test]
fn fixed_chunked_delivery_exact_lengths_markers_and_quiescent_release() {
    for case_id in [
        "bodies_s1_b1024_scalar_collect_fixed",
        "control_complete_chunked",
    ] {
        let driver = Driver::start(30_000);
        let id = driver.identity(case_id, "delivery");
        driver.client().begin(&id, Duration::from_secs(10)).unwrap();
        assert!(driver.client().readiness(&id).unwrap().ready);
        let wire = read_http(request(&driver, "/s0/op0", Some(&id.invocation_id)));
        let offset = wire.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let body = if case_id.contains("chunked") {
            decode_chunked(&wire[offset..])
        } else {
            wire[offset..].to_vec()
        };
        assert_eq!(body, fixtures::payload_bytes(0, 1024, 12).unwrap());
        let snap = wait_snapshot(&driver, &id, |s| s.active_connections == 0);
        assert_eq!(snap.requests.len(), 1);
        let r = &snap.requests[0];
        assert_eq!(r.body_bytes_written, 1024);
        assert!(r.complete);
        assert_eq!(r.status, 200);
        assert!(r.final_response_release_ns.unwrap() >= r.header_arrival_ns);
        driver.client().release(&id).unwrap();
        assert!(driver.client().snapshot(&id).is_err());
        driver.stop();
    }
}

#[test]
fn retries_have_predetermined_status_attempt_bounds_and_reset_counters() {
    let driver = Driver::start(30_000);
    let id = driver.identity("retries_s1_l3_pfalse_d0", "attempts");
    driver.client().begin(&id, Duration::from_secs(10)).unwrap();
    for expected in [503, 503, 503, 200] {
        let reply = read_http(request(&driver, "/s0/op0", Some(&id.invocation_id)));
        assert!(reply.starts_with(format!("HTTP/1.1 {expected}").as_bytes()));
    }
    let reply = read_http(request(&driver, "/s0/op0", Some(&id.invocation_id)));
    assert!(reply.ends_with(b"excess_attempts"));
    let snap = wait_snapshot(&driver, &id, |s| s.active_connections == 0);
    assert_eq!(
        snap.requests.iter().map(|r| r.attempt).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(snap.rejected_requests, 1);
    driver.client().release(&id).unwrap();
    assert!(driver.client().begin(&id, Duration::from_secs(10)).is_err());
    let next = Identity {
        invocation_id: "next".into(),
        ..id
    };
    driver
        .client()
        .begin(&next, Duration::from_secs(10))
        .unwrap();
    assert!(driver.client().snapshot(&next).unwrap().requests.is_empty());
    let reply = read_http(request(&driver, "/s0/op0", Some(&next.invocation_id)));
    assert!(reply.starts_with(b"HTTP/1.1 503"));
    driver.client().release(&next).unwrap();
    driver.stop();
}

#[test]
fn identity_overlap_early_reset_unknown_routes_and_configuration_fail_explicitly() {
    let driver = Driver::start(30_000);
    let id = driver.identity("expressions_s8_c1", "identity");
    let client = driver.client();
    assert!(client.begin(&id, Duration::ZERO).is_err());
    assert!(client.begin(&id, Duration::from_millis(75_001)).is_err());
    let mut wrong = id.clone();
    wrong.response_policy.body_bytes += 1;
    assert!(client.begin(&wrong, Duration::from_secs(10)).is_err());
    wrong = id.clone();
    wrong.catalog_version = 2;
    assert!(client.begin(&wrong, Duration::from_secs(10)).is_err());
    client.begin(&id, Duration::from_secs(10)).unwrap();
    assert_eq!(
        client
            .begin(&id, Duration::from_secs(10))
            .err()
            .unwrap()
            .code,
        "overlap"
    );
    wrong = id.clone();
    wrong.invocation_id = "stale".into();
    assert!(client.readiness(&wrong).is_err());
    assert!(client.snapshot(&wrong).is_err());
    assert!(client.release(&wrong).is_err());
    for (path, invocation, code) in [
        ("/s0/op0", None, "missing_identity"),
        ("/s0/op0", Some("stale"), "stale_identity"),
        ("/unknown", Some("identity"), "unknown_route"),
    ] {
        assert!(read_http(request(&driver, path, invocation)).ends_with(code.as_bytes()));
    }
    assert_eq!(client.snapshot(&id).unwrap().rejected_requests, 3);
    client.release(&id).unwrap();
    driver.stop();
}

#[test]
fn stalls_use_complete_barrier_and_release_closes_every_handler_before_reset() {
    for phase in ["headers", "betweenchunks"] {
        for width in [1, 8] {
            let driver = Driver::start(30_000);
            let case_id = format!("cancellation_w{width}_{phase}_collect");
            let id = driver.identity(&case_id, "stalled");
            driver.client().begin(&id, Duration::from_secs(10)).unwrap();
            assert!(!driver.client().readiness(&id).unwrap().ready);
            let streams: Vec<_> = (0..width)
                .map(|i| request(&driver, &format!("/s0/op{i}"), Some(&id.invocation_id)))
                .collect();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let ready = driver.client().readiness(&id).unwrap();
                if ready.ready {
                    assert_eq!(ready.stalled_paths.len(), width);
                    assert_eq!(ready.arrived_paths.len(), width);
                    break;
                }
                assert!(Instant::now() < deadline, "missing stall barrier");
                thread::yield_now();
            }
            let snap = driver.client().snapshot(&id).unwrap();
            assert_eq!(snap.active_connections, width);
            assert!(snap
                .requests
                .iter()
                .all(|r| !r.complete && r.final_response_release_ns.is_none()));
            for r in &snap.requests {
                assert_eq!(
                    r.body_bytes_written,
                    if id.response_policy.stall == Stall::Headers {
                        0
                    } else {
                        512
                    }
                );
            }
            assert!(driver.client().begin(&id, Duration::from_secs(10)).is_err());
            let receipt = driver.client().release(&id).unwrap();
            assert!(receipt.joined && receipt.active_connections == 0);
            for stream in streams {
                let body = read_http(stream);
                if phase == "headers" {
                    assert!(body.is_empty());
                } else {
                    assert!(!body.ends_with(b"0\r\n\r\n"));
                }
            }
            let next = Identity {
                invocation_id: "after-release".into(),
                ..id
            };
            driver
                .client()
                .begin(&next, Duration::from_secs(10))
                .unwrap();
            assert_eq!(
                driver.client().snapshot(&next).unwrap().active_connections,
                0
            );
            driver.client().release(&next).unwrap();
            driver.stop();
        }
    }
}

#[test]
fn header_connection_and_control_frame_limits_are_bounded() {
    let driver = Driver::start(30_000);
    let id = driver.identity("cancellation_w1_headers_collect", "limits");
    driver.client().begin(&id, Duration::from_secs(15)).unwrap();
    let mut large = TcpStream::connect(driver.endpoint()).unwrap();
    large
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    large
        .write_all(
            format!(
                "GET /s0/op0 HTTP/1.1\r\nHuge: {}\r\n\r\n",
                "a".repeat(16 * 1024)
            )
            .as_bytes(),
        )
        .unwrap();
    let mut buffer = Vec::new();
    let _ = large.read_to_end(&mut buffer);
    assert!(driver.client().snapshot(&id).unwrap().rejected_requests >= 1);
    let mut sockets = Vec::new();
    for _ in 0..128 {
        sockets.push(TcpStream::connect(driver.endpoint()).unwrap());
    }
    wait_snapshot(&driver, &id, |s| s.active_connections == 128);
    let mut excess = TcpStream::connect(driver.endpoint()).unwrap();
    excess
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut byte = [0];
    assert!(matches!(excess.read(&mut byte), Ok(0) | Err(_)));
    assert!(driver.client().snapshot(&id).unwrap().rejected_requests >= 2);
    let over = vec![b'x'; protocol::REQUEST_CAP + 1];
    assert_eq!(raw_control(&driver, &over)["ok"], false);
    driver.client().release(&id).unwrap();
    drop(sockets);
    driver.stop();
}

#[test]
fn malformed_truncated_multiple_unknown_and_missing_identity_protocol_are_refused() {
    let driver = Driver::start(30_000);
    for bytes in [
        b"{}\n".as_slice(),
        b"{\n",
        b"{\"op\":\"stop\"}",
        b"{\"op\":\"stop\"}\n{\"op\":\"stop\"}\n",
        b"{\"op\":\"other\"}\n",
        b"{\"op\":\"ready\"}\n",
        b"{\"op\":\"stop\",\"extra\":true}\n",
    ] {
        assert_eq!(raw_control(&driver, bytes)["ok"], false);
    }
    assert!(protocol::decode_frame::<Value>(b"", 10).is_err());
    assert!(protocol::decode_frame::<Value>(b"{}\n", 2).is_err());
    driver.stop();
}

fn fake_control(reply: Value, call: impl FnOnce(ControlClient) -> HarnessError) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut incoming = Vec::new();
        socket.read_to_end(&mut incoming).unwrap();
        let mut bytes = serde_json::to_vec(&reply).unwrap();
        bytes.push(b'\n');
        socket.write_all(&bytes).unwrap();
    });
    let error = call(ControlClient::new(address));
    assert!(matches!(
        error.code.as_str(),
        "echo" | "payload" | "release" | "budget_echo"
    ));
    server.join().unwrap();
}

#[test]
fn clients_reject_wrong_echo_failed_readiness_and_unavailable_service() {
    let case = catalog::case("expressions_s8_c1").unwrap();
    let id = Identity {
        catalog_version: 1,
        case_id: case.case_id,
        invocation_id: "echo".into(),
        response_policy: case.response_policy,
    };
    let mut wrong = id.clone();
    wrong.invocation_id = "wrong".into();
    fake_control(
        json!({"op":"ready","identity":wrong,"ok":true,"payload":{"ready":true,"arrived_paths":[],"stalled_paths":[]}}),
        |c| c.readiness(&id).unwrap_err(),
    );
    fake_control(
        json!({"op":"ready","identity":id,"ok":true,"payload":{"ready":true,"arrived_paths":[],"stalled_paths":[],"unexpected":1}}),
        |c| c.readiness(&id).unwrap_err(),
    );
    fake_control(
        json!({"op":"release","identity":id,"ok":true,"payload":{"active_connections":1,"joined":false}}),
        |c| c.release(&id).unwrap_err(),
    );
    fake_control(
        json!({"op":"begin","identity":id,"ok":true,"payload":{"session_budget_ms":1}}),
        |c| c.begin(&id, Duration::from_secs(1)).unwrap_err(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    assert_eq!(
        ControlClient::new(address).readiness(&id).unwrap_err().code,
        "connect"
    );
}

#[test]
fn lifetime_session_and_readiness_failure_have_observable_bounded_shutdown() {
    assert!(service::serve(service::ServiceOptions {
        lifetime_budget: Duration::ZERO
    })
    .is_err());
    let error = service::serve_with_readiness(
        service::ServiceOptions {
            lifetime_budget: Duration::from_secs(1),
        },
        |_| {
            Err(HarnessError::new(
                "test",
                "readiness",
                "intentional refusal",
            ))
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "readiness");
    let mut driver = Driver::start(150);
    driver.wait(false);
    let mut driver = Driver::start(30_000);
    let id = driver.identity("cancellation_w1_headers_collect", "expiry");
    driver
        .client()
        .begin(&id, Duration::from_millis(150))
        .unwrap();
    let stream = request(&driver, "/s0/op0", Some(&id.invocation_id));
    driver.wait(false);
    assert!(read_http(stream).is_empty());
}

#[test]
fn causal_response_release_precedes_the_next_request_header_barrier() {
    let driver = Driver::start(30_000);
    let id = driver.identity("dag_diamond_ptrue_r0", "causal");
    driver.client().begin(&id, Duration::from_secs(10)).unwrap();
    for path in ["/s0/op0", "/s0/op1", "/s0/op2", "/s0/op3"] {
        assert!(
            read_http(request(&driver, path, Some(&id.invocation_id))).starts_with(b"HTTP/1.1 200")
        );
    }
    let snapshot = wait_snapshot(&driver, &id, |s| s.active_connections == 0);
    for (prerequisite, dependent) in [(0, 1), (0, 2), (1, 3), (2, 3)] {
        assert!(
            snapshot.requests[dependent].header_arrival_ns
                >= snapshot.requests[prerequisite]
                    .final_response_release_ns
                    .unwrap()
        );
    }
    driver.client().release(&id).unwrap();
    driver.stop();
}

#[test]
fn truncated_multiple_and_oversized_responses_cannot_enter_client_admission() {
    for response in [
        b"{".to_vec(),
        b"{}\n{}\n".to_vec(),
        vec![b'x'; protocol::RESPONSE_CAP + 1],
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut incoming = Vec::new();
            socket.read_to_end(&mut incoming).unwrap();
            let result = socket.write_all(&response);
            if let Err(e) = result {
                assert!(matches!(
                    e.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                ));
            }
        });
        let error = ControlClient::new(address).stop().unwrap_err();
        assert!(matches!(error.code.as_str(), "frame" | "response_cap"));
        server.join().unwrap();
    }
}

#[test]
fn an_unfinished_control_response_hits_the_absolute_exchange_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (release, hold) = mpsc::sync_channel::<()>(1);
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut incoming = Vec::new();
        socket.read_to_end(&mut incoming).unwrap();
        socket.write_all(b"{").unwrap();
        hold.recv_timeout(Duration::from_secs(6)).unwrap();
    });
    let start = Instant::now();
    assert_eq!(
        ControlClient::new(address).stop().unwrap_err().code,
        "deadline"
    );
    assert!(start.elapsed() < Duration::from_secs(6));
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn largest_fixed_chunked_and_parallel_bodies_deliver_every_declared_byte() {
    for case_id in [
        "bodies_s1_b8388608_fullbody_collect_fixed",
        "bodies_s1_b8388608_fullbody_collect_chunked",
        "bodies_s8_b1048576_scalar_collect_chunked",
    ] {
        let driver = Driver::start(30_000);
        let case = catalog::case(case_id).unwrap();
        let id = driver.identity(case_id, "large-body");
        driver.client().begin(&id, Duration::from_secs(15)).unwrap();
        let streams: Vec<_> = (0..case.dimensions.steps)
            .map(|i| request(&driver, &format!("/s0/op{i}"), Some(&id.invocation_id)))
            .collect();
        for (index, stream) in streams.into_iter().enumerate() {
            let wire = read_http(stream);
            let offset = wire.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            let body = if case.response_policy.transfer == catalog::Transfer::Chunked {
                decode_chunked(&wire[offset..])
            } else {
                wire[offset..].to_vec()
            };
            assert_eq!(
                body,
                fixtures::payload_bytes(index, case.dimensions.serialized_body_bytes, 12).unwrap()
            );
        }
        let snapshot = wait_snapshot(&driver, &id, |s| s.active_connections == 0);
        assert_eq!(snapshot.requests.len(), case.dimensions.steps);
        assert!(snapshot
            .requests
            .iter()
            .all(|r| r.complete
                && r.body_bytes_written == case.dimensions.serialized_body_bytes as u64));
        driver.client().release(&id).unwrap();
        driver.stop();
    }
}

#[test]
fn maximum_width_two_level_requests_preserve_every_causal_dependency_marker() {
    let driver = Driver::start(30_000);
    let id = driver.identity("dag_w128_ptrue_r0", "wide-dag");
    driver.client().begin(&id, Duration::from_secs(15)).unwrap();
    for start in [0, 128] {
        let streams: Vec<_> = (start..start + 128)
            .map(|i| request(&driver, &format!("/s0/op{i}"), Some(&id.invocation_id)))
            .collect();
        for stream in streams {
            assert!(read_http(stream).starts_with(b"HTTP/1.1 200"));
        }
    }
    let snapshot = wait_snapshot(&driver, &id, |s| s.active_connections == 0);
    assert_eq!(snapshot.requests.len(), 256);
    assert_eq!(snapshot.rejected_requests, 0);
    let records: std::collections::BTreeMap<_, _> = snapshot
        .requests
        .iter()
        .map(|r| (r.path.as_str(), r))
        .collect();
    for prerequisite in 0..128 {
        for dependent in 128..256 {
            let first = records[format!("/s0/op{prerequisite}").as_str()];
            let second = records[format!("/s0/op{dependent}").as_str()];
            assert!(second.header_arrival_ns >= first.final_response_release_ns.unwrap());
            assert!(first.complete && second.complete);
        }
    }
    driver.client().release(&id).unwrap();
    driver.stop();
}
