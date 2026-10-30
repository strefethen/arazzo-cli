use super::catalog::{self, CaseSpec, HarnessError, Stall, Transfer, INVOCATION_HEADER};
use super::fixtures::payload_bytes;
use super::protocol::{
    self, BeginReceipt, Identity, ReadyReceipt, Refusal, ReleaseReceipt, Request, RequestRecord,
    Response, ServiceReadiness, Snapshot, Success,
};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::{Duration, Instant};

const HEADER_CAP: usize = 16 * 1024;
const CONNECTION_CAP: usize = 128;
const IO_BUDGET: Duration = Duration::from_secs(5);

pub(crate) struct ServiceOptions {
    pub lifetime_budget: Duration,
}

struct Session {
    identity: Identity,
    case: CaseSpec,
    deadline: Instant,
    bodies: BTreeMap<String, Vec<u8>>,
    counts: BTreeMap<String, u32>,
    records: Vec<RequestRecord>,
    stalled: BTreeSet<String>,
    rejected: u64,
}

struct Channel {
    socket: TcpStream,
    input: Vec<u8>,
    output: Vec<u8>,
    sent: usize,
    deadline: Instant,
    input_done: bool,
}

impl Channel {
    fn new(socket: TcpStream) -> Result<Self, HarnessError> {
        socket
            .set_nonblocking(true)
            .map_err(|e| failure("socket_mode", e))?;
        Ok(Self {
            socket,
            input: Vec::new(),
            output: Vec::new(),
            sent: 0,
            deadline: Instant::now() + IO_BUDGET,
            input_done: false,
        })
    }
    fn close(&self) -> Result<(), HarnessError> {
        match self.socket.shutdown(Shutdown::Both) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotConnected => Ok(()),
            Err(e) => Err(failure("socket_close", e)),
        }
    }
}

struct DataConnection {
    channel: Channel,
    record: Option<usize>,
    body_ranges: Vec<(usize, usize)>,
    release_at: usize,
    stall_at: Option<usize>,
    path: Option<String>,
}

fn failure(code: &str, e: impl std::fmt::Display) -> HarnessError {
    HarnessError::new("service", code, e)
}

fn encoded<T: Serialize>(value: &T) -> Result<Vec<u8>, HarnessError> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| failure("serialize", e))?;
    bytes.push(b'\n');
    if bytes.len() > protocol::RESPONSE_CAP {
        return Err(failure("response_cap", bytes.len()));
    }
    Ok(bytes)
}

fn success<T: Serialize>(
    op: &str,
    identity: Option<Identity>,
    payload: T,
) -> Result<Response, HarnessError> {
    Ok(Response::Success(Success {
        op: op.into(),
        identity,
        ok: true,
        payload: serde_json::to_value(payload).map_err(|e| failure("serialize", e))?,
    }))
}

fn refusal(op: &str, code: &str, message: impl std::fmt::Display) -> Response {
    Response::Refusal(Refusal {
        op: op.into(),
        ok: false,
        code: code.into(),
        message: message.to_string(),
    })
}

fn start_session(
    identity: Identity,
    budget: u64,
    used: &mut BTreeSet<String>,
) -> Result<Session, HarnessError> {
    if identity.catalog_version != catalog::CATALOG_VERSION
        || identity.invocation_id.is_empty()
        || identity.invocation_id.len() > 128
        || !identity
            .invocation_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        || budget == 0
        || budget > 75_000
    {
        return Err(failure(
            "identity_or_budget",
            "invalid identity or session budget",
        ));
    }
    if used.contains(&identity.invocation_id) {
        return Err(failure(
            "stale_identity",
            "invocation ID has already been used",
        ));
    }
    if used.len() >= 10_000 {
        return Err(failure("session_cap", "service session limit reached"));
    }
    let case = catalog::case(&identity.case_id)?;
    if identity.response_policy != case.response_policy {
        return Err(failure("policy", "response policy differs from catalog"));
    }
    let mut bodies = BTreeMap::new();
    for i in 0..case.dimensions.steps {
        let op = catalog::step_operation(&case, i);
        bodies.insert(
            op.path,
            payload_bytes(
                op.operation,
                case.dimensions.serialized_body_bytes,
                case.dimensions.pointer_depth,
            )?,
        );
    }
    used.insert(identity.invocation_id.clone());
    Ok(Session {
        identity,
        case,
        deadline: Instant::now() + Duration::from_millis(budget),
        bodies,
        counts: BTreeMap::new(),
        records: Vec::new(),
        stalled: BTreeSet::new(),
        rejected: 0,
    })
}

fn close_data(data: &mut Vec<DataConnection>) -> Result<(), HarnessError> {
    let mut failure = None;
    for connection in data.drain(..) {
        if let Err(e) = connection.channel.close() {
            failure = Some(e);
        }
    }
    failure.map_or(Ok(()), Err)
}

fn control(
    request: Request,
    session: &mut Option<Session>,
    data: &mut Vec<DataConnection>,
    used: &mut BTreeSet<String>,
    stopping: &mut bool,
) -> Result<Response, HarnessError> {
    let op = request.op();
    if *stopping {
        return Ok(refusal(op, "stopping", "service shutdown has begun"));
    }
    if let Request::Begin {
        identity,
        session_budget_ms,
    } = request
    {
        if session.is_some() || !data.is_empty() {
            return Ok(refusal(
                op,
                "overlap",
                "begin requires released, quiescent session",
            ));
        }
        return match start_session(identity.clone(), session_budget_ms, used) {
            Ok(active) => {
                *session = Some(active);
                success(op, Some(identity), BeginReceipt { session_budget_ms })
            }
            Err(e) => Ok(refusal(op, &e.code, e.message)),
        };
    }
    if matches!(request, Request::Stop {}) {
        close_data(data)?;
        *session = None;
        *stopping = true;
        return success(op, None, json!({"stopped":true}));
    }
    let identity = match request {
        Request::Ready { identity }
        | Request::Snapshot { identity }
        | Request::Release { identity } => identity,
        _ => return Err(failure("dispatch", "unreachable operation")),
    };
    let Some(active) = session.as_ref() else {
        return Ok(refusal(op, "no_session", "no active session"));
    };
    if active.identity != identity {
        return Ok(refusal(
            op,
            "stale_identity",
            "identity does not match active session",
        ));
    }
    match op {
        "ready" => {
            let arrived: BTreeSet<_> = active.records.iter().map(|r| r.path.clone()).collect();
            let ready = active.identity.response_policy.stall == Stall::None
                || active
                    .case
                    .expected
                    .required_paths
                    .iter()
                    .all(|p| active.stalled.contains(p));
            success(
                op,
                Some(identity),
                ReadyReceipt {
                    ready,
                    arrived_paths: arrived.into_iter().collect(),
                    stalled_paths: active.stalled.iter().cloned().collect(),
                },
            )
        }
        "snapshot" => success(
            op,
            Some(identity),
            Snapshot {
                requests: active.records.clone(),
                active_connections: data.len(),
                rejected_requests: active.rejected,
            },
        ),
        "release" => {
            close_data(data)?;
            *session = None;
            success(
                op,
                Some(identity),
                ReleaseReceipt {
                    active_connections: 0,
                    joined: true,
                },
            )
        }
        _ => Err(failure("dispatch", op)),
    }
}

fn read_control(channel: &mut Channel) -> Result<bool, HarnessError> {
    let mut buffer = [0; 4096];
    loop {
        match channel.socket.read(&mut buffer) {
            Ok(0) => {
                channel.input_done = true;
                return Ok(true);
            }
            Ok(n) => {
                channel.input.extend_from_slice(&buffer[..n]);
                if channel.input.len() > protocol::REQUEST_CAP {
                    return Err(failure("frame_cap", channel.input.len()));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) => return Err(failure("control_read", e)),
        }
    }
}

fn write_control(channel: &mut Channel) -> Result<bool, HarnessError> {
    match channel.socket.write(&channel.output[channel.sent..]) {
        Ok(0) => Err(failure("control_write", "zero-length write")),
        Ok(n) => {
            channel.sent += n;
            Ok(channel.sent == channel.output.len())
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
        Err(e) => Err(failure("control_write", e)),
    }
}

fn parse_http(bytes: &[u8]) -> Result<(String, String), HarnessError> {
    let text = std::str::from_utf8(bytes).map_err(|e| failure("header_encoding", e))?;
    let mut lines = text.split("\r\n");
    let first = lines
        .next()
        .ok_or_else(|| failure("request_line", "missing request line"))?;
    let parts: Vec<_> = first.split(' ').collect();
    if parts.len() != 3
        || parts[0] != "GET"
        || parts[2] != "HTTP/1.1"
        || !parts[1].starts_with('/')
        || parts[1].contains('?')
    {
        return Err(failure(
            "request_line",
            "expected exact GET path and HTTP/1.1",
        ));
    }
    let mut invocation = None;
    let mut names = BTreeSet::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| failure("header", "invalid header"))?;
        let name = name.to_ascii_lowercase();
        if !names.insert(name.clone()) {
            return Err(failure("header", "duplicate header"));
        }
        if name == INVOCATION_HEADER {
            invocation = Some(value.trim().to_string());
        }
        if name == "transfer-encoding" || (name == "content-length" && value.trim() != "0") {
            return Err(failure("request_body", "request body refused"));
        }
    }
    Ok((
        parts[1].into(),
        invocation.ok_or_else(|| failure("missing_identity", "missing invocation header"))?,
    ))
}

fn response(
    connection: &mut DataConnection,
    session: &mut Session,
    now: u64,
) -> Result<(), HarnessError> {
    let (path, invocation) = parse_http(&connection.channel.input)?;
    if invocation != session.identity.invocation_id {
        return Err(failure("stale_identity", invocation));
    }
    let body = session
        .bodies
        .get(&path)
        .ok_or_else(|| failure("unknown_route", &path))?;
    let attempt = session.counts.get(&path).copied().unwrap_or(0) + 1;
    if attempt > session.case.dimensions.retry_limit + 1 {
        return Err(failure("excess_attempts", &path));
    }
    session.counts.insert(path.clone(), attempt);
    let policy = &session.identity.response_policy;
    let status =
        if attempt <= policy.failure_attempts || policy.exhaust_path.as_deref() == Some(&path) {
            503
        } else {
            200
        };
    let mut wire = format!(
        "HTTP/1.1 {status} Workload\r\nContent-Type: application/json\r\nConnection: close\r\n"
    )
    .into_bytes();
    let mut ranges = Vec::new();
    match policy.transfer {
        Transfer::Fixed => {
            wire.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
            let start = wire.len();
            wire.extend_from_slice(body);
            ranges.push((start, wire.len()));
        }
        Transfer::Chunked => {
            wire.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
            for chunk in body.chunks(body.len().div_ceil(2)) {
                wire.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                let start = wire.len();
                wire.extend_from_slice(chunk);
                ranges.push((start, wire.len()));
                wire.extend_from_slice(b"\r\n");
                if connection.stall_at.is_none() && policy.stall == Stall::BetweenChunks {
                    connection.stall_at = Some(wire.len());
                }
            }
            wire.extend_from_slice(b"0\r\n\r\n");
        }
    }
    connection.release_at = ranges
        .last()
        .map(|(_, end)| end - 1)
        .ok_or_else(|| failure("body", "empty body"))?;
    if policy.stall == Stall::Headers {
        connection.stall_at = Some(0);
    }
    connection.body_ranges = ranges;
    connection.path = Some(path.clone());
    connection.record = Some(session.records.len());
    connection.channel.output = wire;
    connection.channel.input_done = true;
    connection.channel.deadline = if policy.stall == Stall::Headers {
        session.deadline
    } else {
        session.deadline.min(Instant::now() + IO_BUDGET)
    };
    session.records.push(RequestRecord {
        path,
        attempt,
        status,
        header_arrival_ns: now,
        final_response_release_ns: None,
        body_bytes_written: 0,
        complete: false,
    });
    Ok(())
}

fn advance_data(
    connection: &mut DataConnection,
    session: &mut Option<Session>,
    clock: Instant,
) -> bool {
    if !connection.channel.input_done {
        let mut buf = [0; 4096];
        loop {
            match connection.channel.socket.read(&mut buf) {
                Ok(0) => return true,
                Ok(n) => {
                    connection.channel.input.extend_from_slice(&buf[..n]);
                    if connection.channel.input.len() > HEADER_CAP {
                        reject(session);
                        return true;
                    }
                    if let Some(end) = connection
                        .channel
                        .input
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                    {
                        if end + 4 != connection.channel.input.len() {
                            reject(session);
                            return true;
                        }
                        let outcome = session
                            .as_mut()
                            .ok_or_else(|| failure("no_session", "no active session"))
                            .and_then(|s| response(connection, s, nanoseconds(clock)));
                        if let Err(e) = outcome {
                            reject(session);
                            connection.channel.input_done = true;
                            connection.channel.output = format!("HTTP/1.1 400 Refused\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",e.code.len(),e.code).into_bytes();
                        }
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return false,
                Err(_) => return true,
            }
        }
    }
    let start = connection.channel.sent;
    if connection.stall_at == Some(start) {
        if let (Some(active), Some(path)) = (session.as_mut(), connection.path.as_ref()) {
            active.stalled.insert(path.clone());
            connection.channel.deadline = active.deadline;
        }
        return false;
    }
    if let (Some(active), Some(record)) = (session.as_mut(), connection.record) {
        if start >= connection.release_at
            && active.records[record].final_response_release_ns.is_none()
        {
            active.records[record].final_response_release_ns = Some(nanoseconds(clock));
        }
    }
    let mut end = connection.channel.output.len().min(start + 65_536);
    if start < connection.release_at {
        end = end.min(connection.release_at);
    }
    if let Some(stall) = connection.stall_at {
        end = end.min(stall);
    }
    match connection
        .channel
        .socket
        .write(&connection.channel.output[start..end])
    {
        Ok(0) => true,
        Ok(n) => {
            connection.channel.sent += n;
            if let (Some(active), Some(record)) = (session.as_mut(), connection.record) {
                active.records[record].body_bytes_written += connection
                    .body_ranges
                    .iter()
                    .map(|(a, b)| (start + n).min(*b).saturating_sub(start.max(*a)) as u64)
                    .sum::<u64>();
                active.records[record].complete =
                    connection.channel.sent == connection.channel.output.len();
            }
            connection.channel.sent == connection.channel.output.len()
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
        Err(_) => true,
    }
}

fn nanoseconds(clock: Instant) -> u64 {
    u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX)
}
fn reject(session: &mut Option<Session>) {
    if let Some(s) = session {
        s.rejected = s.rejected.saturating_add(1);
    }
}

pub(crate) fn serve(options: ServiceOptions) -> Result<(), HarnessError> {
    serve_with_readiness(options, |ready| {
        let stdout = io::stdout();
        let mut lock = stdout.lock();
        lock.write_all(&encoded(ready)?)
            .map_err(|e| failure("readiness_write", e))?;
        lock.flush().map_err(|e| failure("readiness_flush", e))
    })
}

pub(crate) fn serve_with_readiness<F>(
    options: ServiceOptions,
    report: F,
) -> Result<(), HarnessError>
where
    F: FnOnce(&ServiceReadiness) -> Result<(), HarnessError>,
{
    let clock = Instant::now();
    if options.lifetime_budget.is_zero() || options.lifetime_budget.as_millis() == 0 {
        return Err(failure(
            "lifetime_budget",
            "positive millisecond lifetime required",
        ));
    }
    let deadline = clock
        .checked_add(options.lifetime_budget)
        .ok_or_else(|| failure("lifetime_budget", "unrepresentable deadline"))?;
    let data_listener = TcpListener::bind("127.0.0.1:0").map_err(|e| failure("data_bind", e))?;
    let control_listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| failure("control_bind", e))?;
    data_listener
        .set_nonblocking(true)
        .map_err(|e| failure("listener_mode", e))?;
    control_listener
        .set_nonblocking(true)
        .map_err(|e| failure("listener_mode", e))?;
    report(&ServiceReadiness {
        protocol_version: 1,
        data_endpoint: format!(
            "http://{}",
            data_listener
                .local_addr()
                .map_err(|e| failure("data_address", e))?
        ),
        control_endpoint: control_listener
            .local_addr()
            .map_err(|e| failure("control_address", e))?,
        lifetime_budget_ms: u64::try_from(options.lifetime_budget.as_millis())
            .map_err(|e| failure("lifetime_budget", e))?,
    })?;
    let mut session = None;
    let mut used = BTreeSet::new();
    let mut data = Vec::new();
    let mut controls: Vec<Channel> = Vec::new();
    let mut stopping = false;
    let mut outcome = Ok(());
    loop {
        let mut progressed = false;
        if Instant::now() >= deadline
            || session
                .as_ref()
                .is_some_and(|s: &Session| Instant::now() >= s.deadline)
        {
            outcome = Err(failure(
                "deadline",
                "service lifetime or session deadline expired",
            ));
            break;
        }
        if stopping && controls.is_empty() {
            break;
        }
        if !stopping {
            for _ in 0..CONNECTION_CAP {
                match control_listener.accept() {
                    Ok((socket, _)) if controls.len() < CONNECTION_CAP => {
                        controls.push(Channel::new(socket)?);
                        progressed = true;
                    }
                    Ok((socket, _)) => {
                        socket
                            .shutdown(Shutdown::Both)
                            .map_err(|e| failure("control_limit_close", e))?;
                        progressed = true;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        outcome = Err(failure("control_accept", e));
                        break;
                    }
                }
            }
            for _ in 0..CONNECTION_CAP {
                match data_listener.accept() {
                    Ok((socket, _)) if data.len() < CONNECTION_CAP => {
                        data.push(DataConnection {
                            channel: Channel::new(socket)?,
                            record: None,
                            body_ranges: Vec::new(),
                            release_at: 0,
                            stall_at: None,
                            path: None,
                        });
                        progressed = true;
                    }
                    Ok((socket, _)) => {
                        reject(&mut session);
                        socket
                            .shutdown(Shutdown::Both)
                            .map_err(|e| failure("connection_limit_close", e))?;
                        progressed = true;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        outcome = Err(failure("data_accept", e));
                        break;
                    }
                }
            }
        }
        if outcome.is_err() {
            break;
        }
        let mut index = 0;
        while index < data.len() {
            let before = (
                data[index].channel.input.len(),
                data[index].channel.sent,
                data[index].channel.input_done,
            );
            let done = Instant::now() >= data[index].channel.deadline
                || advance_data(&mut data[index], &mut session, clock);
            progressed |= done
                || before
                    != (
                        data[index].channel.input.len(),
                        data[index].channel.sent,
                        data[index].channel.input_done,
                    );
            if done {
                data.swap_remove(index).channel.close()?;
            } else {
                index += 1;
            }
        }
        let mut index = 0;
        while index < controls.len() {
            let channel = &mut controls[index];
            let before = (channel.input.len(), channel.sent, channel.input_done);
            let mut done = Instant::now() >= channel.deadline;
            if !done && !channel.input_done {
                match read_control(channel) {
                    Ok(true) => {
                        let reply = match protocol::decode_frame::<Request>(
                            &channel.input,
                            protocol::REQUEST_CAP,
                        ) {
                            Ok(request) => {
                                control(request, &mut session, &mut data, &mut used, &mut stopping)?
                            }
                            Err(e) => refusal("invalid", &e.code, e.message),
                        };
                        channel.output = encoded(&reply)?;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        channel.input_done = true;
                        channel.output = encoded(&refusal("invalid", &e.code, e.message))?;
                    }
                }
            }
            if !done && channel.input_done {
                done = write_control(channel)?;
            }
            progressed |= done || before != (channel.input.len(), channel.sent, channel.input_done);
            if done {
                controls.swap_remove(index).close()?;
            } else {
                index += 1;
            }
        }
        if !progressed {
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }
    let cleanup = close_data(&mut data);
    for channel in controls {
        channel.close()?;
    }
    cleanup?;
    outcome
}
