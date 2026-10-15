//! Canonical targets for actual operations in the supported OpenAPI model.

use std::collections::HashMap;

use arazzo_spec::{classify_operation_id, OperationIdTarget, StepTarget};
use openapiv3::{OpenAPI, Operation, ReferenceOr};

/// Reject duplicate IDs before CRUD selection can hide an invalid operation.
pub(crate) fn validate_operation_ids(openapi: &OpenAPI) -> Result<(), String> {
    let mut locations = HashMap::new();
    for (path, item) in &openapi.paths.paths {
        let ReferenceOr::Item(item) = item else {
            continue;
        };
        for (method, operation) in item.iter() {
            let Some(id) = operation.operation_id.as_deref() else {
                continue;
            };
            let location = format!("{} {path}", method.to_ascii_uppercase());
            if let Some(previous) = locations.insert(id, location.clone()) {
                return Err(format!(
                    "duplicate operationId {id:?} at {previous} and {location}; operation IDs must be unique"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn operation_target(
    source_name: &str,
    path: &str,
    method: &str,
    operation: &Operation,
) -> Result<StepTarget, String> {
    // Source names have one grammar for both standard target forms. Refuse an
    // invalid derived name rather than emitting an unresolvable reference.
    let source_probe = format!("$sourceDescriptions.{source_name}.url");
    if !matches!(
        classify_operation_id(&source_probe),
        OperationIdTarget::SourceQualified { source_name: parsed, operation_id: "url" }
            if parsed == source_name
    ) {
        return Err(format!(
            "generated source description name {source_name:?} cannot be represented in an operation target"
        ));
    }

    if let Some(id) = &operation.operation_id {
        let qualified = format!("$sourceDescriptions.{source_name}.{id}");
        if matches!(
            classify_operation_id(&qualified),
            OperationIdTarget::SourceQualified { source_name: parsed_source, operation_id: parsed_id }
                if parsed_source == source_name && parsed_id == id
        ) {
            return Ok(StepTarget::OperationId(qualified));
        }
    }

    let token = path.replace('~', "~0").replace('/', "~1");
    let pointer = format!("/paths/{token}/{}", method.to_ascii_lowercase());
    Ok(StepTarget::OperationPath(format!(
        "{{$sourceDescriptions.{source_name}.url}}#{}",
        encode_fragment(&pointer)
    )))
}

/// Encode UTF-8 bytes after JSON Pointer token escaping. Keep only unreserved
/// URI bytes and pointer separators literal; notably '%' must be encoded so a
/// path containing a percent sequence survives exactly one fragment decoding.
fn encode_fragment(pointer: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(pointer.len());
    for byte in pointer.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    encoded
}
