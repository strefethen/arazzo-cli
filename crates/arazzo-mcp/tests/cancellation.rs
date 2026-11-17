#![allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]

use std::fs;
use std::io::{self, BufReader, PipeWriter, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arazzo_mcp::protocol;
use arazzo_mcp::state::ServerState;
use serde_json::{json, Value};

const WAIT: Duration = Duration::from_secs(10);

struct StalledApi {
    base_url: String,
    arrivals: Receiver<()>,
    release: Sender<()>,
    next_step_requests: Arc<AtomicUsize>,
}

impl StalledApi {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", server.server_addr());
        let (arrived, arrivals) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let next_step_requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&next_step_requests);
        thread::spawn(move || {
            for request in server.incoming_requests() {
                if request.url() == "/slow" {
                    let _ = arrived.send(());
                    let _ = released.recv();
                } else {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                let _ = request.respond(tiny_http::Response::from_string("{}"));
            }
        });
        Self {
            base_url,
            arrivals,
            release,
            next_step_requests,
        }
    }

    fn await_slow_step(&self) {
        self.arrivals.recv_timeout(WAIT).unwrap();
    }

    fn next_steps(&self) -> usize {
        self.next_step_requests.load(Ordering::SeqCst)
    }

    fn state(&self) -> ServerState {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "arazzo-mcp-cancellation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let ok = json!({"200": {"description": "OK"}});
        let openapi = json!({"openapi": "3.0.3", "info": {"title": "Stall", "version": "1"},
            "servers": [{"url": self.base_url}],
            "paths": {"/slow": {"get": {"operationId": "getSlow", "responses": ok}},
                      "/next": {"get": {"operationId": "getNext", "responses": ok}}}});
        fs::write(root.join("api.json"), openapi.to_string()).unwrap();
        let spec = root.join("stall.arazzo.yaml");
        fs::write(&spec, "arazzo: 1.1.0\ninfo: {title: Stall, version: '1'}\nsourceDescriptions: [{name: api, url: ./api.json, type: openapi}]\nworkflows: [{workflowId: stall, steps: [{stepId: slow, operationId: getSlow}, {stepId: next, operationId: getNext}]}]\n").unwrap();
        ServerState::load(&[spec.to_string_lossy().into_owned()], None).unwrap()
    }
}

struct ResponseStream {
    pending: Vec<u8>,
    sent: Sender<Value>,
}

impl Write for ResponseStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buffer);
        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let response = serde_json::from_slice(&line).map_err(io::Error::other)?;
            let _ = self.sent.send(response);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Session {
    input: Option<PipeWriter>,
    responses: Receiver<Value>,
    served: Receiver<Result<(), String>>,
}

impl Session {
    fn start(state: ServerState) -> Self {
        let (reader, input) = io::pipe().unwrap();
        let (sent, responses) = mpsc::channel();
        let (done, served) = mpsc::channel();
        thread::spawn(move || {
            let mut output = ResponseStream {
                pending: Vec::new(),
                sent,
            };
            let _ = done.send(protocol::serve(BufReader::new(reader), &mut output, &state));
        });
        Self {
            input: Some(input),
            responses,
            served,
        }
    }

    fn send(&mut self, message: &Value) {
        let payload = message.to_string();
        let input = self.input.as_mut().unwrap();
        write!(input, "Content-Length: {}\r\n\r\n{payload}", payload.len()).unwrap();
        input.flush().unwrap();
    }

    fn next_id(&self) -> Value {
        self.responses.recv_timeout(WAIT).unwrap()["id"].clone()
    }

    fn close_and_collect(mut self, within: Duration) -> Vec<Value> {
        self.input.take();
        self.served.recv_timeout(within).unwrap().unwrap();
        self.responses.iter().collect()
    }
}

fn run(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": "run_workflow", "arguments": {"workflow_id": "stall"}}})
}

fn cancelled(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": id, "reason": "user abort"}})
}

fn ping(id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "ping"})
}

#[test]
fn ping_is_answered_while_a_workflow_is_stalled() {
    let api = StalledApi::start();
    let mut session = Session::start(api.state());
    session.send(&run(1));
    api.await_slow_step();
    session.send(&ping(2));
    assert_eq!(session.next_id(), 2);
    api.release.send(()).unwrap();
    let finished = session.responses.recv_timeout(WAIT).unwrap();
    assert_eq!(finished["id"], 1);
    assert_eq!(finished["result"]["isError"], false, "{finished}");
    assert_eq!(api.next_steps(), 1);
    assert!(session.close_and_collect(WAIT).is_empty());
}

#[test]
fn cancellation_notice_stops_the_run_and_writes_no_response_for_it() {
    let api = StalledApi::start();
    let mut session = Session::start(api.state());
    session.send(&run(1));
    api.await_slow_step();
    session.send(&cancelled(1));
    session.send(&ping(2));
    assert_eq!(session.next_id(), 2);
    api.release.send(()).unwrap();
    session.send(&run(3));
    api.await_slow_step();
    assert_eq!(api.next_steps(), 0);
    api.release.send(()).unwrap();
    assert_eq!(session.next_id(), 3);
    assert_eq!(api.next_steps(), 1);
    assert!(session.close_and_collect(WAIT).is_empty());
}

#[test]
fn closing_input_mid_run_cancels_it_and_returns_without_a_response() {
    let api = StalledApi::start();
    let mut session = Session::start(api.state());
    session.send(&run(1));
    api.await_slow_step();
    assert!(session.close_and_collect(Duration::from_secs(5)).is_empty());
    assert_eq!(api.next_steps(), 0);
}

#[test]
fn a_second_tool_call_runs_after_the_active_one_in_order() {
    let api = StalledApi::start();
    let mut session = Session::start(api.state());
    session.send(&run(1));
    api.await_slow_step();
    session.send(&run(2));
    session.send(&ping(3));
    assert_eq!(session.next_id(), 3);
    api.release.send(()).unwrap();
    assert_eq!(session.next_id(), 1);
    api.await_slow_step();
    api.release.send(()).unwrap();
    assert_eq!(session.next_id(), 2);
    assert_eq!(api.next_steps(), 2);
    assert!(session.close_and_collect(WAIT).is_empty());
}
