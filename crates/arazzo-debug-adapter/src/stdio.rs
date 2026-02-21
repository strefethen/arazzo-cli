use std::io::{BufRead, Write};

use crate::Session;

/// Runs a newline-delimited JSON protocol loop over stdio streams.
pub fn run_stdio<R, W>(reader: &mut R, writer: &mut W) -> Result<(), String>
where
    R: BufRead,
    W: Write,
{
    let mut session = Session::new();
    let mut line = String::new();
    let mut parse_error_id: u64 = 1;

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|err| format!("reading debug request line: {err}"))?;
        if bytes == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let request = match session.parse_request_or_protocol_error(trimmed, parse_error_id) {
            Ok(request) => request,
            Err(err_response) => {
                parse_error_id = parse_error_id.saturating_add(1);
                write_response_line(&session, writer, &err_response)?;
                continue;
            }
        };

        let should_shutdown = request.method == "shutdown";
        let response = session.handle_request(request);
        write_response_line(&session, writer, &response)?;

        if should_shutdown {
            break;
        }
    }

    Ok(())
}

fn write_response_line<W>(
    session: &Session,
    writer: &mut W,
    response: &arazzo_debug_protocol::ResponseEnvelope,
) -> Result<(), String>
where
    W: Write,
{
    let line = session.encode_response_line(response)?;
    writer
        .write_all(line.as_bytes())
        .map_err(|err| format!("writing debug response line: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("flushing debug response output: {err}"))
}
