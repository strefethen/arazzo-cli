use serde_json::Value;

pub fn encode_dap_stream(messages: &[Value]) -> Vec<u8> {
    let mut out = Vec::<u8>::new();
    for message in messages {
        let payload = match serde_json::to_vec(message) {
            Ok(value) => value,
            Err(err) => panic!("serializing DAP test message: {err}"),
        };
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(&payload);
    }
    out
}

pub fn decode_dap_stream(bytes: &[u8]) -> Vec<Value> {
    let mut cursor = 0usize;
    let mut messages = Vec::<Value>::new();
    while cursor < bytes.len() {
        let rest = &bytes[cursor..];
        let Some(header_end) = find_header_end(rest) else {
            break;
        };
        let header_bytes = &rest[..header_end];
        let header = match String::from_utf8(header_bytes.to_vec()) {
            Ok(value) => value,
            Err(err) => panic!("decoding DAP header utf8: {err}"),
        };
        let content_length = parse_content_length(&header);

        let payload_start = cursor + header_end + 4;
        let payload_end = payload_start + content_length;
        if payload_end > bytes.len() {
            panic!(
                "payload end {} exceeds stream length {}",
                payload_end,
                bytes.len()
            );
        }
        let payload = &bytes[payload_start..payload_end];
        let value = match serde_json::from_slice::<Value>(payload) {
            Ok(value) => value,
            Err(err) => panic!("decoding DAP payload json: {err}"),
        };
        messages.push(value);
        cursor = payload_end;
    }
    messages
}

fn parse_content_length(header: &str) -> usize {
    for line in header.split("\r\n") {
        let Some(raw) = line.strip_prefix("Content-Length:") else {
            continue;
        };
        return match raw.trim().parse::<usize>() {
            Ok(value) => value,
            Err(err) => panic!("parsing Content-Length {raw:?}: {err}"),
        };
    }
    panic!("Content-Length header missing");
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|w| w == b"\r\n\r\n")
}
