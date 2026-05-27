use std::io::{BufRead, Read, Write};
use std::sync::mpsc;
use std::thread;

use serde_json::Value;

use super::requests::DapRequest;

/// Inbound command from the DAP reader thread to the coordinator loop.
pub(super) enum DapCommand {
    Request(DapRequest),
    Eof,
    ReadError(String),
}

/// Allocates monotonically increasing outbound DAP `seq` values.
#[derive(Debug)]
pub(super) struct OutboundSequence {
    next: u64,
}

impl OutboundSequence {
    pub(super) fn new() -> Self {
        Self { next: 1 }
    }

    pub(super) fn alloc(&mut self) -> u64 {
        let seq = self.next;
        self.next = self.next.saturating_add(1);
        seq
    }
}

/// Spawns the dedicated stdin reader thread. Decoupling stdin reading from the
/// coordinator loop prevents the editor's request stream from blocking engine
/// event handling when HTTP requests exceed any single polling timeout.
pub(super) fn spawn_reader_thread<R>(reader: R, cmd_tx: mpsc::Sender<DapCommand>)
where
    R: BufRead + Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = reader;
        loop {
            match read_dap_message(&mut reader) {
                Ok(Some(payload)) => match serde_json::from_str::<DapRequest>(&payload) {
                    Ok(request) => {
                        if cmd_tx.send(DapCommand::Request(request)).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        // Intentional: reader thread is exiting; if main loop already
                        // dropped the receiver, this send failing is harmless.
                        if cmd_tx
                            .send(DapCommand::ReadError(format!(
                                "parsing DAP request JSON: {err}"
                            )))
                            .is_err()
                        {
                            // Coordinator is gone; nothing left to do in reader thread.
                        }
                        break;
                    }
                },
                Ok(None) => {
                    // Intentional: EOF on stdin; receiver may already be dropped.
                    if cmd_tx.send(DapCommand::Eof).is_err() {
                        // Coordinator already exited.
                    }
                    break;
                }
                Err(err) => {
                    // Intentional: reader thread is exiting; receiver may already be dropped.
                    if cmd_tx.send(DapCommand::ReadError(err)).is_err() {
                        // Coordinator already exited.
                    }
                    break;
                }
            }
        }
    });
}

pub(super) fn read_dap_message<R>(reader: &mut R) -> Result<Option<String>, String>
where
    R: BufRead + Read,
{
    let mut line = String::new();
    let mut content_length: Option<usize> = None;

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|err| format!("reading DAP header line: {err}"))?;
        if bytes == 0 {
            return Ok(None);
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(raw) = trimmed.strip_prefix("Content-Length:") {
            let parsed = raw
                .trim()
                .parse::<usize>()
                .map_err(|err| format!("parsing DAP Content-Length: {err}"))?;
            content_length = Some(parsed);
        }
    }

    let Some(content_length) = content_length else {
        return Err("missing DAP Content-Length header".to_string());
    };
    let mut buf = vec![0u8; content_length];
    reader
        .read_exact(&mut buf)
        .map_err(|err| format!("reading DAP payload: {err}"))?;
    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| format!("decoding DAP payload utf8: {err}"))
}

pub(super) fn write_dap_message<W>(writer: &mut W, value: &Value) -> Result<(), String>
where
    W: Write,
{
    let payload =
        serde_json::to_vec(value).map_err(|err| format!("serializing DAP JSON: {err}"))?;
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    writer
        .write_all(header.as_bytes())
        .map_err(|err| format!("writing DAP header: {err}"))?;
    writer
        .write_all(&payload)
        .map_err(|err| format!("writing DAP payload: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("flushing DAP output: {err}"))
}
