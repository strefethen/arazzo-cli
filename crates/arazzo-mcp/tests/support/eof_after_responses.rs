use std::io::{self, BufRead, Cursor, Read, Write};
use std::ops::Deref;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;

const HANG_GUARD: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Answers {
    written: Mutex<usize>,
    changed: Condvar,
}

impl Answers {
    fn record(&self, responses: usize) {
        let mut written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        *written += responses;
        self.changed.notify_all();
    }

    fn wait_for(&self, expected: usize) {
        let written = self.written.lock().unwrap_or_else(PoisonError::into_inner);
        let _answered = self
            .changed
            .wait_timeout_while(written, HANG_GUARD, |written| *written < expected);
    }
}

pub struct AnsweredOutput {
    bytes: Vec<u8>,
    answers: Arc<Answers>,
}

impl Write for AnsweredOutput {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buffer);
        let responses = buffer.iter().filter(|&&byte| byte == b'\n').count();
        if responses > 0 {
            self.answers.record(responses);
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Deref for AnsweredOutput {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

pub struct EofAfterResponses {
    input: Cursor<Vec<u8>>,
    answers: Arc<Answers>,
    expected: usize,
}

impl EofAfterResponses {
    fn hold_eof(&mut self) -> io::Result<()> {
        if self.input.fill_buf()?.is_empty() {
            self.answers.wait_for(self.expected);
        }
        Ok(())
    }
}

impl Read for EofAfterResponses {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.hold_eof()?;
        self.input.read(buffer)
    }
}

impl BufRead for EofAfterResponses {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.hold_eof()?;
        self.input.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.input.consume(amount);
    }
}

pub fn eof_after_responses(input: Vec<u8>) -> (EofAfterResponses, AnsweredOutput) {
    let answers = Arc::new(Answers::default());
    let expected = payloads(&input)
        .filter(|payload| {
            serde_json::from_slice::<Value>(payload)
                .is_ok_and(|request| request.get("id").is_some())
        })
        .count();
    let reader = EofAfterResponses {
        input: Cursor::new(input),
        answers: Arc::clone(&answers),
        expected,
    };
    let output = AnsweredOutput {
        bytes: Vec::new(),
        answers,
    };
    (reader, output)
}

fn payloads(mut input: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        input = input.trim_ascii_start();
        if input.is_empty() {
            return None;
        }
        let payload;
        if let Some(header) = input.strip_prefix(b"Content-Length:") {
            let header_end = header.windows(4).position(|window| window == b"\r\n\r\n")?;
            let length: usize = std::str::from_utf8(&header[..header_end])
                .ok()?
                .trim()
                .parse()
                .ok()?;
            let body = header.get(header_end + 4..)?;
            (payload, input) = body.split_at(length.min(body.len()));
        } else {
            let end = input
                .iter()
                .position(|&b| b == b'\n')
                .unwrap_or(input.len());
            (payload, input) = input.split_at(end);
        }
        Some(payload)
    })
}
