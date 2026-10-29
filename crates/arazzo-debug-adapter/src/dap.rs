//! DAP adapter root: orchestrates the stdin reader thread, the engine event
//! monitor thread, and the request dispatcher. All transport framing, runtime
//! session lifecycle, YAML source indexing, debug-view construction, and
//! per-command handling live in focused submodules.

use std::io::{BufRead, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[path = "dap/events.rs"]
mod events;
#[path = "dap/handlers.rs"]
mod handlers;
#[path = "dap/requests.rs"]
mod requests;
#[path = "dap/responses.rs"]
mod responses;
#[path = "dap/session.rs"]
mod session;
#[path = "dap/source_index.rs"]
mod source_index;
#[path = "dap/transport.rs"]
mod transport;
#[path = "dap/variables.rs"]
mod variables;

use handlers::{dispatch_request, DispatchOutcome};
use session::{handle_engine_event, EngineEvent, SessionState};
use transport::{spawn_reader_thread, DapCommand, OutboundSequence};

/// Runs a runtime-backed DAP loop over stdio using Content-Length framing.
///
/// Decouples stdin reading, engine event monitoring, and command processing
/// across three threads to prevent deadlocks when HTTP requests exceed any
/// single polling timeout.
pub fn run_dap_stdio<R, W>(reader: R, writer: &mut W) -> Result<(), String>
where
    R: BufRead + Read + Send + 'static,
    W: Write,
{
    let mut state = SessionState::default();
    let mut outbound = OutboundSequence::new();
    let (cmd_tx, cmd_rx) = mpsc::channel::<DapCommand>();
    let (event_tx, event_rx) = mpsc::channel::<EngineEvent>();

    // Thread A: reads DAP commands from stdin and sends them to the coordinator.
    spawn_reader_thread(reader, cmd_tx);

    let result = (|| {
        // Coordinator loop (Thread B / main thread): multiplexes commands and engine
        // events. Neither channel blocks the other—engine events arrive via Thread C
        // regardless of whether stdin is readable.
        loop {
            // Drain any pending engine events first.
            while let Ok(event) = event_rx.try_recv() {
                handle_engine_event(event, &mut state, writer, &mut outbound)?;
            }

            // Check for the next command.
            let cmd = match cmd_rx.try_recv() {
                Ok(cmd) => Some(cmd),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => break,
            };

            let Some(cmd) = cmd else {
                thread::sleep(Duration::from_millis(5));
                continue;
            };

            match cmd {
                DapCommand::Eof => break,
                DapCommand::ReadError(err) => {
                    return Err(err);
                }
                DapCommand::Request(request) => match dispatch_request(
                    request,
                    &mut state,
                    writer,
                    &mut outbound,
                    &event_tx,
                    &event_rx,
                )? {
                    DispatchOutcome::Continue => {}
                    DispatchOutcome::Break => break,
                },
            }
        }

        Ok(())
    })();

    session::cleanup_runtime(&mut state);
    drop(event_rx);
    result
}
