#![forbid(unsafe_code)]

mod dap_test_support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Response, Server};

const DEADLINE: Duration = Duration::from_secs(5);
struct Fixture {
    path: PathBuf,
    criterion_line: usize,
    output_line: usize,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new() -> Self {
        let server = Server::http("127.0.0.1:0")
            .unwrap_or_else(|err| panic!("bind loopback fixture: {err:?}"));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = Arc::clone(&stop);
        let base = format!("http://{}", server.server_addr());
        let worker = thread::spawn(move || {
            while !stop_worker.load(Ordering::Acquire) {
                match server.recv_timeout(Duration::from_millis(20)) {
                    Ok(Some(request)) => {
                        let _ = request.respond(Response::from_string(r#"{"value":42}"#));
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        });
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("clock: {err:?}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-lifecycle-{}-{nonce}.yaml",
            std::process::id()
        ));
        let doc = dap_test_support::write_openapi_document(
            &path,
            &base,
            &[("/data", "get", Some("fetch"))],
        )
        .unwrap_or_else(|err| panic!("OpenAPI fixture: {err:?}"));
        let text = format!("arazzo: 1.1.0\ninfo:\n  title: Lifecycle\n  version: '1'\nsourceDescriptions:\n  - name: api\n    url: {doc}\n    type: openapi\nworkflows:\n  - workflowId: lifecycle\n    steps:\n      - stepId: fetch\n        operationId: fetch\n        successCriteria:\n          - condition: $statusCode == 200\n        outputs:\n          value: $response.body#/value\n");
        let criterion_line = text
            .lines()
            .position(|l| l.contains("- condition:"))
            .unwrap_or_else(|| panic!("criterion"))
            + 1;
        let output_line = text
            .lines()
            .position(|l| l.contains("value: $response"))
            .unwrap_or_else(|| panic!("output"))
            + 1;
        std::fs::write(&path, text).unwrap_or_else(|err| panic!("Arazzo fixture: {err:?}"));
        Self {
            path,
            criterion_line,
            output_line,
            stop,
            server: Some(worker),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.server.take() {
            worker
                .join()
                .unwrap_or_else(|err| panic!("server worker: {err:?}"));
        }
        dap_test_support::remove_temp_spec(&self.path);
    }
}

struct Adapter {
    child: Child,
    input: Option<ChildStdin>,
    events: mpsc::Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
    messages: Vec<Value>,
    sequence: u64,
}
impl Adapter {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_arazzo-debug-adapter"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|err| panic!("spawn adapter: {err:?}"));
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap_or_else(|| panic!("stdout"));
        let (tx, events) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(raw) = line.strip_prefix("Content-Length:") {
                        length = raw.trim().parse::<usize>().ok();
                    }
                }
                let mut payload = vec![0; length.unwrap_or_else(|| panic!("length"))];
                if std::io::Read::read_exact(&mut reader, &mut payload).is_err() {
                    return;
                }
                if tx
                    .send(
                        serde_json::from_slice(&payload)
                            .unwrap_or_else(|err| panic!("DAP JSON: {err:?}")),
                    )
                    .is_err()
                {
                    return;
                }
            }
        });
        Self {
            child,
            input,
            events,
            reader: Some(reader),
            messages: vec![],
            sequence: 0,
        }
    }
    fn send(&mut self, command: &str, arguments: Value) -> u64 {
        self.sequence += 1;
        let bytes = dap_test_support::encode_dap_stream(&[
            json!({"seq": self.sequence, "type": "request", "command": command, "arguments": arguments}),
        ]);
        self.input
            .as_mut()
            .unwrap_or_else(|| panic!("stdin open"))
            .write_all(&bytes)
            .unwrap_or_else(|err| panic!("send request: {err:?}"));
        self.sequence
    }
    fn until(&mut self, predicate: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + DEADLINE;
        loop {
            let event = self
                .events
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| {
                    panic!("DAP handshake failed: {e}; messages: {:#?}", self.messages)
                });
            self.messages.push(event.clone());
            if predicate(&event) {
                return event;
            }
        }
    }
    fn response(&mut self, sequence: u64) -> Value {
        let value = self.until(|v| v["request_seq"] == sequence);
        assert_eq!(value["success"], true, "{value}");
        value
    }
    fn paused(&mut self, fixture: &Fixture, later_output: bool, entry: bool) {
        let seq = self.send("initialize", json!({}));
        self.response(seq);
        let seq = self.send(
            "launch",
            json!({"spec": fixture.path, "workflowId":"lifecycle", "stopOnEntry":entry}),
        );
        self.response(seq);
        let mut lines = vec![fixture.criterion_line];
        if later_output {
            lines.push(fixture.output_line);
        }
        let breakpoints: Vec<_> = lines.iter().map(|line| json!({"line":line})).collect();
        let seq = self.send(
            "setBreakpoints",
            json!({"source":{"path":fixture.path}, "breakpoints":breakpoints}),
        );
        let response = self.response(seq);
        let accepted = response["body"]["breakpoints"]
            .as_array()
            .unwrap_or_else(|| panic!("breakpoints"));
        assert_eq!(accepted.len(), lines.len());
        for (accepted, line) in accepted.iter().zip(lines) {
            assert_eq!(accepted["verified"], true, "{accepted}");
            assert_eq!(accepted["line"], line, "{accepted}");
        }
        let seq = self.send("configurationDone", json!({}));
        self.response(seq);
        self.until(|v| v["event"] == "stopped");
        let seq = self.send("stackTrace", json!({"threadId":1}));
        let stack = self.response(seq);
        let line = stack["body"]["stackFrames"][0]["line"]
            .as_u64()
            .unwrap_or_else(|| panic!("frame line")) as usize;
        if !entry {
            assert_eq!(
                line, fixture.criterion_line,
                "must stop at criterion: {stack}"
            );
        } else {
            assert!(line < fixture.criterion_line, "entry stop: {stack}");
        }
    }
    fn exited(&mut self) {
        let end = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .unwrap_or_else(|err| panic!("poll adapter: {err:?}"))
            {
                assert!(status.success(), "adapter status {status}");
                return;
            }
            assert!(
                Instant::now() < end,
                "adapter failed to exit within {DEADLINE:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Adapter {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn criterion_then_output_disconnect_reaps_adapter() {
    let fixture = Fixture::new();
    let mut adapter = Adapter::new();
    adapter.paused(&fixture, true, false);
    let seq = adapter.send("disconnect", json!({}));
    adapter.response(seq);
    adapter.until(|v| v["event"] == "terminated");
    adapter.exited();
}
#[test]
fn paused_eof_reaps_adapter() {
    let fixture = Fixture::new();
    let mut adapter = Adapter::new();
    adapter.paused(&fixture, true, true);
    adapter.input.take();
    adapter.exited();
}
#[test]
fn single_breakpoint_disconnect_control() {
    let fixture = Fixture::new();
    let mut adapter = Adapter::new();
    adapter.paused(&fixture, false, false);
    let seq = adapter.send("disconnect", json!({}));
    adapter.response(seq);
    adapter.until(|v| v["event"] == "terminated");
    adapter.exited();
}
#[test]
fn resume_and_completion_before_eof_control() {
    let fixture = Fixture::new();
    let mut adapter = Adapter::new();
    adapter.paused(&fixture, false, false);
    let seq = adapter.send("continue", json!({"threadId":1}));
    adapter.response(seq);
    let exited = adapter.until(|v| v["event"] == "exited");
    assert_eq!(exited["body"]["exitCode"], 0);
    adapter.until(|v| v["event"] == "terminated");
    adapter.input.take();
    adapter.exited();
}
struct InjectedProcess(Child);

impl Drop for InjectedProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn injected_transport_failures_are_preserved_after_cleanup() {
    for mode in [
        "reader",
        "dispatch-writer",
        "event-writer",
        "reader-channel-loss",
    ] {
        let mut child = InjectedProcess(
            Command::new(
                std::env::current_exe().unwrap_or_else(|err| panic!("test executable: {err:?}")),
            )
            .args(["--exact", "injected_failure_child", "--nocapture"])
            .env("ARAZZO_LIFECYCLE_INJECTION", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap_or_else(|err| panic!("spawn injection helper: {err:?}")),
        );
        let end = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = child
                .0
                .try_wait()
                .unwrap_or_else(|err| panic!("poll injection helper: {err:?}"))
            {
                break Some(status);
            }
            if Instant::now() >= end {
                break None;
            }
            thread::sleep(Duration::from_millis(10));
        };
        drop(child);
        assert!(
            status.is_some_and(|status| status.success()),
            "injected {mode} failed or exceeded {DEADLINE:?}: {status:?}"
        );
    }
}

enum Input {
    Bytes(Vec<u8>),
    Failure,
    ChannelLoss,
}
struct InjectedReader {
    bytes: std::io::Cursor<Vec<u8>>,
    input: mpsc::Receiver<Input>,
}
impl std::io::Read for InjectedReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let available = self.fill_buf()?;
        let count = bytes.len().min(available.len());
        bytes[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}
impl BufRead for InjectedReader {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.bytes.position() as usize == self.bytes.get_ref().len() {
            match self
                .input
                .recv_timeout(DEADLINE)
                .unwrap_or_else(|err| panic!("injected reader handshake: {err:?}"))
            {
                Input::Bytes(bytes) => self.bytes = std::io::Cursor::new(bytes),
                Input::Failure => {
                    return Err(std::io::Error::other("distinctive injected reader failure"))
                }
                Input::ChannelLoss => panic!("intentional reader-channel-loss injection"),
            }
        }
        self.bytes.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.bytes.consume(amount);
    }
}
struct InjectedWriter {
    bytes: Vec<u8>,
    input: mpsc::Sender<Input>,
    mode: String,
    criterion_line: usize,
    observed_pause: bool,
    observed_diagnostics: usize,
}
impl InjectedWriter {
    fn request(&self, sequence: u64, command: &str, arguments: Value) {
        self.input
            .send(Input::Bytes(dap_test_support::encode_dap_stream(&[
                json!({"seq":sequence,"type":"request","command":command,"arguments":arguments}),
            ])))
            .unwrap_or_else(|err| panic!("inject request: {err:?}"));
    }
}
impl Write for InjectedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let messages = dap_test_support::decode_dap_stream(&self.bytes);
        let message = messages
            .last()
            .unwrap_or_else(|| panic!("flushed DAP message"));
        if message["event"] == "stopped" && !self.observed_pause {
            self.request(5, "stackTrace", json!({"threadId":1}));
        }
        if message["command"] == "setBreakpoints" {
            let accepted = message["body"]["breakpoints"]
                .as_array()
                .unwrap_or_else(|| panic!("accepted breakpoints"));
            assert_eq!(accepted.len(), 2);
            assert!(accepted.iter().all(|bp| bp["verified"] == true));
            assert_eq!(accepted[0]["line"], self.criterion_line);
        }
        if message["command"] == "stackTrace" {
            assert_eq!(message["success"], true);
            assert_eq!(
                message["body"]["stackFrames"][0]["line"],
                self.criterion_line
            );
            self.observed_pause = true;
            match self.mode.as_str() {
                "reader" => self
                    .input
                    .send(Input::Failure)
                    .unwrap_or_else(|err| panic!("inject reader failure: {err:?}")),
                "reader-channel-loss" => self
                    .input
                    .send(Input::ChannelLoss)
                    .unwrap_or_else(|err| panic!("inject channel loss: {err:?}")),
                "dispatch-writer" => self.request(6, "threads", json!({})),
                "event-writer" => self.request(6, "continue", json!({"threadId":1})),
                _ => panic!("unknown injection mode"),
            }
        }
        if message["event"] == "output" {
            self.observed_diagnostics += 1;
        }
        let fail = (self.mode == "dispatch-writer" && message["command"] == "threads")
            || (self.mode == "event-writer" && self.observed_diagnostics == 2);
        if fail {
            assert!(
                self.observed_pause,
                "failure requires a verified active pause"
            );
            self.input
                .send(Input::Bytes(vec![]))
                .unwrap_or_else(|err| panic!("finish injected reader: {err:?}"));
            return Err(std::io::Error::other(format!(
                "distinctive injected {} failure",
                self.mode
            )));
        }
        Ok(())
    }
}

#[test]
fn injected_failure_child() {
    let Ok(mode) = std::env::var("ARAZZO_LIFECYCLE_INJECTION") else {
        return;
    };
    let fixture = Fixture::new();
    let text = std::fs::read_to_string(&fixture.path).unwrap_or_else(|err| panic!("read injection fixture: {err:?}"))
        .replace("        outputs:\n", "        outputs:\n          aaa: $response.body#/absent_one\n          bbb: $response.body#/absent_two\n");
    std::fs::write(&fixture.path, text)
        .unwrap_or_else(|err| panic!("write injection fixture: {err:?}"));
    let (tx, rx) = mpsc::channel();
    let input = dap_test_support::encode_dap_stream(&[
        json!({"seq":1,"type":"request","command":"initialize","arguments":{}}),
        json!({"seq":2,"type":"request","command":"launch","arguments":{"spec":fixture.path,"workflowId":"lifecycle","stopOnEntry":false}}),
        json!({"seq":3,"type":"request","command":"setBreakpoints","arguments":{"source":{"path":fixture.path},"breakpoints":[{"line":fixture.criterion_line},{"line":fixture.output_line+2}]}}),
        json!({"seq":4,"type":"request","command":"configurationDone","arguments":{}}),
    ]);
    let reader = InjectedReader {
        bytes: std::io::Cursor::new(input),
        input: rx,
    };
    let mut writer = InjectedWriter {
        bytes: vec![],
        input: tx,
        mode: mode.clone(),
        criterion_line: fixture.criterion_line,
        observed_pause: false,
        observed_diagnostics: 0,
    };
    let result = arazzo_debug_adapter::run_dap_stdio(reader, &mut writer);
    assert!(
        writer.observed_pause,
        "must verify active criterion pause before injecting failure"
    );
    if mode == "reader-channel-loss" {
        assert_eq!(result, Ok(()));
    } else {
        match result {
            Err(err) => assert!(err.contains(&format!("distinctive injected {mode} failure"))),
            Ok(()) => panic!("injected transport remains an error"),
        }
    }
}
