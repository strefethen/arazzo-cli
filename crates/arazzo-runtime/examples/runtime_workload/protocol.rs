use super::catalog::{HarnessError, ResponsePolicy};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

pub(crate) const REQUEST_CAP: usize = 65_536;
pub(crate) const RESPONSE_CAP: usize = 4 * 1024 * 1024;
pub(crate) const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub catalog_version: u32,
    pub case_id: String,
    pub invocation_id: String,
    pub response_policy: ResponsePolicy,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    Begin {
        identity: Identity,
        session_budget_ms: u64,
    },
    Ready {
        identity: Identity,
    },
    Snapshot {
        identity: Identity,
    },
    Release {
        identity: Identity,
    },
    Stop {},
}

impl Request {
    pub fn op(&self) -> &'static str {
        match self {
            Self::Begin { .. } => "begin",
            Self::Ready { .. } => "ready",
            Self::Snapshot { .. } => "snapshot",
            Self::Release { .. } => "release",
            Self::Stop {} => "stop",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Response {
    Success(Success),
    Refusal(Refusal),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Success {
    pub op: String,
    pub identity: Option<Identity>,
    pub ok: bool,
    pub payload: Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Refusal {
    pub op: String,
    pub ok: bool,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct BeginReceipt {
    pub session_budget_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadyReceipt {
    pub ready: bool,
    pub arrived_paths: Vec<String>,
    pub stalled_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestRecord {
    pub path: String,
    pub attempt: u32,
    pub status: u16,
    pub header_arrival_ns: u64,
    pub final_response_release_ns: Option<u64>,
    pub body_bytes_written: u64,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub requests: Vec<RequestRecord>,
    pub active_connections: usize,
    pub rejected_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseReceipt {
    pub active_connections: usize,
    pub joined: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceReadiness {
    pub protocol_version: u32,
    pub data_endpoint: String,
    pub control_endpoint: SocketAddr,
    pub lifetime_budget_ms: u64,
}

pub(crate) fn decode_frame<T: DeserializeOwned>(
    bytes: &[u8],
    cap: usize,
) -> Result<T, HarnessError> {
    if bytes.len() > cap
        || bytes.last() != Some(&b'\n')
        || bytes[..bytes.len().saturating_sub(1)].contains(&b'\n')
    {
        return Err(HarnessError::new(
            "protocol",
            "frame",
            "expected exactly one bounded newline-terminated frame",
        ));
    }
    serde_json::from_slice(&bytes[..bytes.len() - 1])
        .map_err(|e| HarnessError::new("protocol", "json", e))
}

#[derive(Clone)]
pub(crate) struct ControlClient {
    endpoint: SocketAddr,
}

impl ControlClient {
    pub fn new(endpoint: SocketAddr) -> Self {
        Self { endpoint }
    }

    fn exchange<T: DeserializeOwned>(
        &self,
        request: Request,
        expected: Option<&Identity>,
    ) -> Result<T, HarnessError> {
        let op = request.op();
        let mut bytes =
            serde_json::to_vec(&request).map_err(|e| HarnessError::new("protocol", "encode", e))?;
        bytes.push(b'\n');
        if bytes.len() > REQUEST_CAP {
            return Err(HarnessError::new("protocol", "request_cap", bytes.len()));
        }
        let deadline = Instant::now() + CONTROL_TIMEOUT;
        let mut stream = TcpStream::connect_timeout(&self.endpoint, CONTROL_TIMEOUT)
            .map_err(|e| HarnessError::new("protocol", "connect", e))?;
        stream
            .set_nonblocking(true)
            .map_err(|e| HarnessError::new("protocol", "socket_mode", e))?;
        let check_deadline = || {
            if Instant::now() >= deadline {
                Err(HarnessError::new(
                    "protocol",
                    "deadline",
                    "control exchange exceeded five seconds",
                ))
            } else {
                Ok(())
            }
        };
        let mut sent = 0;
        while sent < bytes.len() {
            check_deadline()?;
            match stream.write(&bytes[sent..]) {
                Ok(0) => return Err(HarnessError::new("protocol", "write", "zero-length write")),
                Ok(count) => sent += count,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::park_timeout(Duration::from_millis(1))
                }
                Err(e) => return Err(HarnessError::new("protocol", "write", e)),
            }
        }
        stream
            .shutdown(Shutdown::Write)
            .map_err(|e| HarnessError::new("protocol", "shutdown", e))?;
        let mut reply = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            check_deadline()?;
            let count = match stream.read(&mut buffer) {
                Ok(count) => count,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::park_timeout(Duration::from_millis(1));
                    continue;
                }
                Err(e) => return Err(HarnessError::new("protocol", "read", e)),
            };
            if count == 0 {
                break;
            }
            reply.extend_from_slice(&buffer[..count]);
            if reply.len() > RESPONSE_CAP {
                return Err(HarnessError::new("protocol", "response_cap", reply.len()));
            }
        }
        match decode_frame::<Response>(&reply, RESPONSE_CAP)? {
            Response::Success(success)
                if success.ok && success.op == op && success.identity.as_ref() == expected =>
            {
                serde_json::from_value(success.payload)
                    .map_err(|e| HarnessError::new("protocol", "payload", e))
            }
            Response::Success(_) => Err(HarnessError::new(
                "protocol",
                "echo",
                "operation, identity or success mismatch",
            )),
            Response::Refusal(refusal) if !refusal.ok && refusal.op == op => {
                Err(HarnessError::new("service", &refusal.code, refusal.message))
            }
            Response::Refusal(_) => Err(HarnessError::new(
                "protocol",
                "echo",
                "refusal operation or ok mismatch",
            )),
        }
    }

    pub fn begin(
        &self,
        identity: &Identity,
        budget: Duration,
    ) -> Result<BeginReceipt, HarnessError> {
        let session_budget_ms = u64::try_from(budget.as_millis())
            .map_err(|e| HarnessError::new("protocol", "budget", e))?;
        let receipt: BeginReceipt = self.exchange(
            Request::Begin {
                identity: identity.clone(),
                session_budget_ms,
            },
            Some(identity),
        )?;
        if receipt.session_budget_ms != session_budget_ms {
            return Err(HarnessError::new(
                "protocol",
                "budget_echo",
                "session budget mismatch",
            ));
        }
        Ok(receipt)
    }
    pub fn readiness(&self, identity: &Identity) -> Result<ReadyReceipt, HarnessError> {
        self.exchange(
            Request::Ready {
                identity: identity.clone(),
            },
            Some(identity),
        )
    }
    pub fn snapshot(&self, identity: &Identity) -> Result<Snapshot, HarnessError> {
        self.exchange(
            Request::Snapshot {
                identity: identity.clone(),
            },
            Some(identity),
        )
    }
    pub fn release(&self, identity: &Identity) -> Result<ReleaseReceipt, HarnessError> {
        let receipt: ReleaseReceipt = self.exchange(
            Request::Release {
                identity: identity.clone(),
            },
            Some(identity),
        )?;
        if receipt.active_connections != 0 || !receipt.joined {
            return Err(HarnessError::new(
                "protocol",
                "release",
                "session did not quiesce",
            ));
        }
        Ok(receipt)
    }
    pub fn stop(&self) -> Result<(), HarnessError> {
        let _: Value = self.exchange(Request::Stop {}, None)?;
        Ok(())
    }
}
