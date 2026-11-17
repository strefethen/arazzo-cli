use std::io::{BufRead, Read, Write};

use serde_json::Value;

const MAX_HEADER_LINES: usize = 16;
const CONTENT_LENGTH: &[u8] = b"Content-Length:";

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum FrameError {
    Oversized,
    Malformed,
    Closed,
}

impl FrameError {
    pub(crate) fn rejection(&self) -> Option<&'static str> {
        match self {
            Self::Oversized => Some("Request exceeds the server frame limit"),
            Self::Malformed => Some("Invalid request frame"),
            Self::Closed => None,
        }
    }
}

pub(crate) fn read_frame<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, FrameError> {
    let first = loop {
        let Some(line) = read_line(reader, max_bytes)? else {
            return Ok(None);
        };
        if !line.trim_ascii().is_empty() {
            break line;
        }
    };
    let first = first.trim_ascii();
    if first.starts_with(b"{") {
        return Ok(Some(first.to_vec()));
    }

    let mut declared = content_length(first)?;
    let mut header_lines = 1;
    loop {
        let line = read_line(reader, max_bytes)?.ok_or(FrameError::Closed)?;
        let line = line.trim_ascii_end();
        if line.is_empty() {
            break;
        }
        header_lines += 1;
        if header_lines > MAX_HEADER_LINES {
            return Err(FrameError::Oversized);
        }
        if let Some(length) = content_length(line)? {
            declared = Some(length);
        }
    }

    let declared = declared.ok_or(FrameError::Malformed)?;
    if declared > max_bytes {
        return Err(FrameError::Oversized);
    }
    let mut payload = Vec::with_capacity(declared);
    Read::take(&mut *reader, byte_bound(declared))
        .read_to_end(&mut payload)
        .map_err(|_| FrameError::Closed)?;
    if payload.len() < declared {
        return Err(FrameError::Closed);
    }
    Ok(Some(payload))
}

pub(crate) fn write_frame<W: Write>(writer: &mut W, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|err| format!("writing payload: {err}"))?;
    writer
        .write_all(b"\n")
        .map_err(|err| format!("writing newline: {err}"))?;
    writer
        .flush()
        .map_err(|err| format!("flushing output: {err}"))
}

fn read_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> Result<Option<Vec<u8>>, FrameError> {
    let mut line = Vec::new();
    Read::take(&mut *reader, byte_bound(max_bytes).saturating_add(1))
        .read_until(b'\n', &mut line)
        .map_err(|_| FrameError::Closed)?;
    if line.is_empty() {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') && line.len() > max_bytes {
        return Err(FrameError::Oversized);
    }
    Ok(Some(line))
}

fn content_length(line: &[u8]) -> Result<Option<usize>, FrameError> {
    let Some(raw) = line.strip_prefix(CONTENT_LENGTH) else {
        return Ok(None);
    };
    let digits = raw.trim_ascii();
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(FrameError::Malformed);
    }
    std::str::from_utf8(digits)
        .map_err(|_| FrameError::Malformed)?
        .parse::<usize>()
        .map(Some)
        .map_err(|_| FrameError::Oversized)
}

fn byte_bound(bytes: usize) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "Assertions")]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

    fn read(input: &str) -> Result<Option<Vec<u8>>, FrameError> {
        read_frame(&mut Cursor::new(input.as_bytes().to_vec()), 1024)
    }

    #[test]
    fn both_framings_round_trip_and_eof_is_none() {
        let framed = format!("Content-Length: {}\r\n\r\n{PING}", PING.len());
        for input in [framed, format!("{PING}\n"), PING.to_string()] {
            assert_eq!(read(&input), Ok(Some(PING.as_bytes().to_vec())));
        }
        assert_eq!(read(""), Ok(None));

        let message = json!({"jsonrpc":"2.0","id":1,"result":{}});
        let mut output = Vec::new();
        write_frame(&mut output, &message).unwrap();
        assert_eq!(output.last(), Some(&b'\n'));
        assert_eq!(serde_json::from_slice::<Value>(&output).unwrap(), message);
    }

    #[test]
    fn blank_lines_before_a_frame_are_skipped() {
        let framed = format!("\n \r\n\nContent-Length: {}\r\n\r\n{PING}", PING.len());
        for input in [framed, format!("\n\r\n  \n{PING}\n")] {
            assert_eq!(read(&input), Ok(Some(PING.as_bytes().to_vec())));
        }
        assert_eq!(read("\n\n\r\n"), Ok(None));
    }

    #[test]
    fn header_count_is_bounded() {
        let headers = |count: usize| {
            let extra = "X-Pad: 1\r\n".repeat(count - 1);
            format!("Content-Length: {}\r\n{extra}\r\n{PING}", PING.len())
        };
        assert_eq!(
            read(&headers(MAX_HEADER_LINES)),
            Ok(Some(PING.as_bytes().to_vec()))
        );
        assert_eq!(
            read(&headers(MAX_HEADER_LINES + 1)),
            Err(FrameError::Oversized)
        );
        assert_eq!(read("X-Pad: 1\r\n\r\n{}"), Err(FrameError::Malformed));
        assert_eq!(
            read("Content-Length: 99999999999999999999999\r\n\r\n"),
            Err(FrameError::Oversized)
        );
    }
}
