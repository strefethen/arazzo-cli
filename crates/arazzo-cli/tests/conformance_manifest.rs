#![forbid(unsafe_code)]

//! Hermetic validation for the finite Arazzo 1.1 conformance claim surface.
//!
//! The inventory is deliberate review surface. It is not inferred from current
//! tests or source files, and this gate deliberately does not consult `tkt`:
//! orchestration verifies live owners after the Cargo evidence passes.
//! Specification identity and allowed anchors are pinned in the tracked root
//! manifest, so validation never depends on ignored local specification files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

const AREAS: [(&str, &str); 5] = [
    ("model", "tests/conformance/model.json"),
    ("validation", "tests/conformance/validation.json"),
    ("expressions", "tests/conformance/expressions.json"),
    ("runtime", "tests/conformance/runtime.json"),
    ("surfaces", "tests/conformance/surfaces.json"),
];
const INVENTORY_PATH: &str = "tests/conformance/manifest.json";
const SPEC_PREFIX: &str = "spec/arazzo/v1.1.0.html#";
const SPEC_DOCUMENT: &str = "Arazzo Specification";
const SPEC_VERSION: &str = "1.1.0";
const SPEC_SOURCE: &str = "https://spec.openapis.org/arazzo/v1.1.0.html";
const SPEC_SHA256: &str = "8e2ea7d20accaef846080bca77d349b137414e66c94f015a002b290cbf79b538";
static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

type Fragments = BTreeMap<String, Value>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    Covered,
    FailClosed,
    Unsupported,
    Deviation,
}

impl Status {
    fn parse(value: &str, claim_id: &str) -> Result<Self, String> {
        match value {
            "covered" => Ok(Self::Covered),
            "failClosed" => Ok(Self::FailClosed),
            "unsupported" => Ok(Self::Unsupported),
            "deviation" => Ok(Self::Deviation),
            other => Err(claim_error(claim_id, &format!("unknown status {other:?}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Authority {
    ArazzoSpec,
    RepositoryContract,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StatusTotals {
    covered: usize,
    fail_closed: usize,
    unsupported: usize,
    deviation: usize,
}

impl StatusTotals {
    fn add(&mut self, status: Status) {
        match status {
            Status::Covered => self.covered += 1,
            Status::FailClosed => self.fail_closed += 1,
            Status::Unsupported => self.unsupported += 1,
            Status::Deviation => self.deviation += 1,
        }
    }

    fn all_covered(&self) -> bool {
        self.fail_closed == 0 && self.unsupported == 0 && self.deviation == 0
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ManifestReport {
    arazzo: StatusTotals,
    repository: StatusTotals,
    full_compliance: bool,
}

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|err| panic!("system time must be after the Unix epoch: {err}"))
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "arazzo-conformance-manifest-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("creating {}: {err}", path.display()));
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn workspace_root() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&path).unwrap_or_else(|err| panic!("canonicalizing {}: {err}", path.display()))
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> Value {
    let raw =
        fs::read_to_string(path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()))
}

fn checked_in_documents() -> (Value, Fragments) {
    let root = crate_root();
    let manifest = read_json(&root.join(INVENTORY_PATH));
    let fragments = AREAS
        .into_iter()
        .map(|(area, relative)| (area.to_string(), read_json(&root.join(relative))))
        .collect();
    (manifest, fragments)
}

fn claim_error(claim_id: &str, message: &str) -> String {
    format!("claim {claim_id}: {message}")
}

fn required_object<'a>(value: &'a Value, subject: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{subject} must be a JSON object"))
}

fn required_array<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    subject: &str,
) -> Result<&'a Vec<Value>, String> {
    object
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{subject}: {field} must be an array"))
}

fn required_text<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    subject: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{subject}: {field} must be a non-empty string"))
}

fn known_area(area: &str) -> bool {
    AREAS.into_iter().any(|(known, _)| known == area)
}

fn is_claim_id(value: &str) -> bool {
    let mut previous_separator = true;
    for byte in value.bytes() {
        if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            previous_separator = false;
        } else if matches!(byte, b'.' | b'-') && !previous_separator {
            previous_separator = true;
        } else {
            return false;
        }
    }
    !value.is_empty() && !previous_separator
}

fn is_ticket_owner(value: &str) -> bool {
    value.strip_prefix("ac-").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

fn is_normative_level(value: &str) -> bool {
    matches!(
        value,
        "REQUIRED"
            | "MUST"
            | "MUST NOT"
            | "SHALL"
            | "SHALL NOT"
            | "SHOULD"
            | "SHOULD NOT"
            | "RECOMMENDED"
            | "NOT RECOMMENDED"
            | "MAY"
            | "OPTIONAL"
    )
}

fn is_repository_contract_id(value: &str) -> bool {
    value.strip_prefix("repository.").is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            })
    })
}

fn is_rust_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn resolve_workspace_file(workspace: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path.extension().and_then(|value| value.to_str()) != Some("rs") {
        return Err(format!(
            "evidence path {relative:?} must name a Rust source file"
        ));
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!(
            "evidence path {relative:?} must be workspace-relative"
        ));
    }
    let resolved = workspace.join(path);
    if !resolved.is_file() {
        return Err(format!(
            "evidence file {} does not exist",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn is_spec_anchor_id(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn tracked_spec_anchor_ids(manifest: &Map<String, Value>) -> Result<BTreeSet<String>, String> {
    let subject = "manifest specAuthority";
    let authority = manifest
        .get("specAuthority")
        .ok_or_else(|| format!("{subject} is required"))?;
    let authority = required_object(authority, subject)?;
    let expected_fields = BTreeSet::from([
        "allowedAnchorIds",
        "document",
        "sha256",
        "source",
        "version",
    ]);
    let actual_fields = authority
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual_fields != expected_fields {
        return Err(format!(
            "{subject} fields must be exactly {expected_fields:?}, got {actual_fields:?}"
        ));
    }
    for (field, expected) in [
        ("document", SPEC_DOCUMENT),
        ("version", SPEC_VERSION),
        ("source", SPEC_SOURCE),
        ("sha256", SPEC_SHA256),
    ] {
        let actual = required_text(authority, field, subject)?;
        if actual != expected {
            return Err(format!(
                "{subject}: {field} must be the pinned value {expected:?}, got {actual:?}"
            ));
        }
    }

    let mut anchors = BTreeSet::new();
    for anchor in required_array(authority, "allowedAnchorIds", subject)? {
        let anchor = anchor
            .as_str()
            .filter(|anchor| is_spec_anchor_id(anchor))
            .ok_or_else(|| {
                format!("{subject}: allowedAnchorIds must contain non-empty HTML anchor IDs")
            })?;
        if !anchors.insert(anchor.to_string()) {
            return Err(format!("{subject}: duplicate allowed anchor ID {anchor:?}"));
        }
    }
    Ok(anchors)
}

fn scrub_non_code(source: &str) -> String {
    let input = source.as_bytes();
    let mut output = vec![b' '; input.len()];
    let mut cursor = 0;
    let mut block_depth = 0_usize;
    let mut quote = None;
    let mut raw_hashes = None;

    while cursor < input.len() {
        if let Some(hashes) = raw_hashes {
            if input[cursor] == b'"' && input[cursor + 1..].starts_with(&vec![b'#'; hashes]) {
                cursor += hashes + 1;
                raw_hashes = None;
                continue;
            }
            if input[cursor] == b'\n' {
                output[cursor] = b'\n';
            }
            cursor += 1;
            continue;
        }

        if let Some(delimiter) = quote {
            if input[cursor] == b'\\' {
                if cursor + 1 < input.len() && input[cursor + 1] == b'\n' {
                    output[cursor + 1] = b'\n';
                }
                cursor += 2;
                continue;
            }
            if input[cursor] == delimiter {
                quote = None;
            } else if input[cursor] == b'\n' {
                output[cursor] = b'\n';
            }
            cursor += 1;
            continue;
        }

        if block_depth > 0 {
            if input[cursor..].starts_with(b"/*") {
                block_depth += 1;
                cursor += 2;
            } else if input[cursor..].starts_with(b"*/") {
                block_depth -= 1;
                cursor += 2;
            } else {
                if input[cursor] == b'\n' {
                    output[cursor] = b'\n';
                }
                cursor += 1;
            }
            continue;
        }

        if input[cursor..].starts_with(b"//") {
            cursor += 2;
            while cursor < input.len() && input[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        if input[cursor..].starts_with(b"/*") {
            block_depth = 1;
            cursor += 2;
            continue;
        }
        if input[cursor] == b'"' {
            quote = Some(b'"');
            cursor += 1;
            continue;
        }
        if input[cursor] == b'\'' {
            quote = Some(b'\'');
            cursor += 1;
            continue;
        }
        if input[cursor] == b'r' {
            let mut end = cursor + 1;
            while end < input.len() && input[end] == b'#' {
                end += 1;
            }
            if end < input.len() && input[end] == b'"' {
                raw_hashes = Some(end - cursor - 1);
                cursor = end + 1;
                continue;
            }
        }
        output[cursor] = input[cursor];
        cursor += 1;
    }

    String::from_utf8(output)
        .unwrap_or_else(|err| panic!("scrubbed Rust source must remain UTF-8: {err}"))
}

fn skip_whitespace(source: &[u8], cursor: &mut usize) {
    while source
        .get(*cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        *cursor += 1;
    }
}

fn consume_word(source: &[u8], cursor: &mut usize, word: &str) -> bool {
    let word = word.as_bytes();
    if !source[*cursor..].starts_with(word) {
        return false;
    }
    let before_is_word = *cursor > 0 && source[*cursor - 1].is_ascii_alphanumeric()
        || (*cursor > 0 && source[*cursor - 1] == b'_');
    let after = *cursor + word.len();
    let after_is_word = source
        .get(after)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
    if before_is_word || after_is_word {
        return false;
    }
    *cursor = after;
    true
}

fn attribute_end(source: &[u8], mut cursor: usize) -> Option<usize> {
    let mut depth = 0_usize;
    while let Some(byte) = source.get(cursor) {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

fn is_test_attribute(attribute: &str) -> bool {
    attribute == "test" || attribute == "tokio::test" || attribute.starts_with("tokio::test(")
}

fn is_ignore_attribute(attribute: &str) -> bool {
    attribute == "ignore" || attribute.starts_with("ignore=")
}

fn skip_delimited(source: &[u8], cursor: &mut usize) -> bool {
    let Some(open) = source.get(*cursor).copied() else {
        return false;
    };
    let Some(close) = (match open {
        b'(' => Some(b')'),
        b'[' => Some(b']'),
        b'{' => Some(b'}'),
        _ => None,
    }) else {
        return false;
    };
    let mut closing = vec![close];
    *cursor += 1;
    while let Some(byte) = source.get(*cursor).copied() {
        *cursor += 1;
        match byte {
            b'(' => closing.push(b')'),
            b'[' => closing.push(b']'),
            b'{' => closing.push(b'}'),
            b')' | b']' | b'}' if closing.last() == Some(&byte) => {
                closing.pop();
                if closing.is_empty() {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn normalized_attribute(source: &[u8], start: usize, end: usize) -> String {
    source[start..end]
        .iter()
        .filter(|byte| !byte.is_ascii_whitespace())
        .map(|byte| char::from(*byte))
        .collect()
}

fn parse_outer_attributes(source: &[u8], cursor: &mut usize) -> Vec<String> {
    let mut attributes = Vec::new();
    while source
        .get(*cursor..)
        .is_some_and(|remaining| remaining.starts_with(b"#["))
    {
        let Some(end) = attribute_end(source, *cursor + 1) else {
            *cursor = source.len();
            break;
        };
        attributes.push(normalized_attribute(source, *cursor + 2, end - 1));
        *cursor = end;
        skip_whitespace(source, cursor);
    }
    attributes
}

fn parse_inner_attribute(source: &[u8], cursor: &mut usize) -> Option<String> {
    if !source
        .get(*cursor..)
        .is_some_and(|remaining| remaining.starts_with(b"#!["))
    {
        return None;
    }
    let Some(end) = attribute_end(source, *cursor + 2) else {
        *cursor = source.len();
        return Some("malformed".to_string());
    };
    let attribute = normalized_attribute(source, *cursor + 3, end - 1);
    *cursor = end;
    Some(attribute)
}

fn attributes_are_enabled_for_test(attributes: &[String]) -> bool {
    attributes.iter().all(|attribute| {
        if attribute == "cfg_attr" || attribute.starts_with("cfg_attr(") {
            return false;
        }
        if attribute == "cfg" || attribute.starts_with("cfg(") {
            return attribute == "cfg(test)";
        }
        true
    })
}

fn skip_visibility(source: &[u8], cursor: &mut usize) {
    if !consume_word(source, cursor, "pub") {
        return;
    }
    skip_whitespace(source, cursor);
    if source.get(*cursor) == Some(&b'(') {
        let _ = skip_delimited(source, cursor);
        skip_whitespace(source, cursor);
    }
}

fn skip_non_module_item(source: &[u8], cursor: &mut usize) {
    while let Some(byte) = source.get(*cursor).copied() {
        match byte {
            b';' => {
                *cursor += 1;
                return;
            }
            b'}' => return,
            b'(' | b'[' => {
                if !skip_delimited(source, cursor) {
                    return;
                }
            }
            b'{' => {
                let _ = skip_delimited(source, cursor);
                skip_whitespace(source, cursor);
                if source.get(*cursor) == Some(&b';') {
                    *cursor += 1;
                }
                return;
            }
            _ => *cursor += 1,
        }
    }
}

fn skip_remaining_item_list(source: &[u8], cursor: &mut usize, nested_module: bool) {
    if !nested_module {
        *cursor = source.len();
        return;
    }
    while let Some(byte) = source.get(*cursor).copied() {
        match byte {
            b'}' => {
                *cursor += 1;
                return;
            }
            b'(' | b'[' | b'{' => {
                if !skip_delimited(source, cursor) {
                    return;
                }
            }
            _ => *cursor += 1,
        }
    }
}

fn scan_rust_item_list(
    source: &[u8],
    cursor: &mut usize,
    nested_module: bool,
    tests: &mut BTreeSet<String>,
) {
    while *cursor < source.len() {
        skip_whitespace(source, cursor);
        while source.get(*cursor) == Some(&b';') {
            *cursor += 1;
            skip_whitespace(source, cursor);
        }
        if source.get(*cursor) == Some(&b'}') {
            *cursor += usize::from(nested_module);
            return;
        }
        if let Some(attribute) = parse_inner_attribute(source, cursor) {
            if !attributes_are_enabled_for_test(std::slice::from_ref(&attribute)) {
                skip_remaining_item_list(source, cursor, nested_module);
                return;
            }
            continue;
        }

        let attributes = parse_outer_attributes(source, cursor);
        let item_start = *cursor;
        let mut head = item_start;
        skip_visibility(source, &mut head);

        let mut module_head = head;
        if consume_word(source, &mut module_head, "mod") {
            skip_whitespace(source, &mut module_head);
            while source
                .get(module_head)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'#'))
            {
                module_head += 1;
            }
            skip_whitespace(source, &mut module_head);
            *cursor = module_head;
            if source.get(*cursor) == Some(&b'{') {
                *cursor += 1;
                if attributes_are_enabled_for_test(&attributes) {
                    scan_rust_item_list(source, cursor, true, tests);
                } else {
                    *cursor -= 1;
                    let _ = skip_delimited(source, cursor);
                }
            } else {
                skip_non_module_item(source, cursor);
            }
            continue;
        }

        let mut function_head = head;
        loop {
            let before_qualifier = function_head;
            for qualifier in ["async", "unsafe"] {
                if consume_word(source, &mut function_head, qualifier) {
                    skip_whitespace(source, &mut function_head);
                    break;
                }
            }
            if function_head == before_qualifier {
                break;
            }
        }
        if consume_word(source, &mut function_head, "fn") {
            skip_whitespace(source, &mut function_head);
            let name_start = function_head;
            while source
                .get(function_head)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                function_head += 1;
            }
            let name = std::str::from_utf8(&source[name_start..function_head])
                .unwrap_or_else(|err| panic!("scrubbed Rust identifier must be UTF-8: {err}"));
            let has_test_attribute = attributes
                .iter()
                .any(|attribute| is_test_attribute(attribute));
            let ignored = attributes
                .iter()
                .any(|attribute| is_ignore_attribute(attribute));
            if has_test_attribute
                && !ignored
                && attributes_are_enabled_for_test(&attributes)
                && is_rust_identifier(name)
            {
                tests.insert(name.to_string());
            }
            *cursor = function_head;
            skip_non_module_item(source, cursor);
            continue;
        }

        *cursor = item_start;
        skip_non_module_item(source, cursor);
        if *cursor == item_start {
            *cursor += 1;
        }
    }
}

fn recognized_non_ignored_test_items(source: &str) -> BTreeSet<String> {
    let source = scrub_non_code(source);
    let bytes = source.as_bytes();
    let mut tests = BTreeSet::new();
    let mut cursor = 0;
    scan_rust_item_list(bytes, &mut cursor, false, &mut tests);
    tests
}

fn validate_evidence_reference(
    workspace: &Path,
    reference: &str,
    claim_id: &str,
) -> Result<(), String> {
    let mut parts = reference.split('#');
    let path = parts.next().unwrap_or_default();
    let test_name = parts.next().unwrap_or_default();
    if parts.next().is_some() || path.is_empty() || !is_rust_identifier(test_name) {
        return Err(claim_error(
            claim_id,
            &format!("malformed evidence reference {reference:?}"),
        ));
    }
    let source_path = resolve_workspace_file(workspace, path)
        .map_err(|message| claim_error(claim_id, &message))?;
    let source = fs::read_to_string(&source_path).map_err(|err| {
        claim_error(
            claim_id,
            &format!("reading {}: {err}", source_path.display()),
        )
    })?;
    if !recognized_non_ignored_test_items(&source).contains(test_name) {
        return Err(claim_error(
            claim_id,
            &format!(
                "evidence reference {reference:?} does not name a non-ignored recognized Rust test item"
            ),
        ));
    }
    Ok(())
}

fn validate_evidence_list(
    workspace: &Path,
    evidence: &Map<String, Value>,
    field: &str,
    claim_id: &str,
    required: bool,
) -> Result<(), String> {
    let Some(values) = evidence.get(field) else {
        if required {
            return Err(claim_error(claim_id, &format!("missing {field} evidence")));
        }
        return Ok(());
    };
    let values = values
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| {
            claim_error(
                claim_id,
                &format!("{field} evidence must be a non-empty array"),
            )
        })?;
    for value in values {
        let reference = value.as_str().ok_or_else(|| {
            claim_error(claim_id, &format!("{field} evidence must contain strings"))
        })?;
        validate_evidence_reference(workspace, reference, claim_id)?;
    }
    Ok(())
}

fn validate_optional_evidence(
    workspace: &Path,
    entry: &Map<String, Value>,
    claim_id: &str,
) -> Result<(), String> {
    let Some(evidence) = entry.get("evidence") else {
        return Ok(());
    };
    let evidence = evidence
        .as_object()
        .ok_or_else(|| claim_error(claim_id, "evidence must be an object"))?;
    for field in ["positive", "negative", "zeroSideEffect"] {
        if evidence.contains_key(field) {
            validate_evidence_list(workspace, evidence, field, claim_id, false)?;
        }
    }
    Ok(())
}

fn validate_authority(
    workspace: &Path,
    allowed_spec_anchor_ids: &BTreeSet<String>,
    used_spec_anchor_ids: &mut BTreeSet<String>,
    entry: &Map<String, Value>,
    claim_id: &str,
) -> Result<Authority, String> {
    let authority = required_text(entry, "authority", &format!("claim {claim_id}"))?;
    match authority {
        "arazzoSpec" => {
            if entry.contains_key("contractId") || entry.contains_key("surface") {
                return Err(claim_error(
                    claim_id,
                    "arazzoSpec authority cannot include repository-contract fields",
                ));
            }
            let anchors = required_array(entry, "specAnchors", &format!("claim {claim_id}"))?;
            if anchors.is_empty() {
                return Err(claim_error(claim_id, "specAnchors must not be empty"));
            }
            for anchor in anchors {
                let anchor = anchor.as_str().ok_or_else(|| {
                    claim_error(claim_id, "specAnchors must contain vendored Arazzo anchors")
                })?;
                if !anchor.starts_with(SPEC_PREFIX) || anchor.len() == SPEC_PREFIX.len() {
                    return Err(claim_error(
                        claim_id,
                        "specAnchors must cite spec/arazzo/v1.1.0.html with a non-empty anchor",
                    ));
                }
                let fragment = &anchor[SPEC_PREFIX.len()..];
                if !allowed_spec_anchor_ids.contains(fragment) {
                    return Err(claim_error(
                        claim_id,
                        &format!(
                            "spec anchor {anchor:?} is not in the tracked specAuthority allowlist"
                        ),
                    ));
                }
                used_spec_anchor_ids.insert(fragment.to_string());
            }
            let levels = required_array(entry, "normativeLevels", &format!("claim {claim_id}"))?;
            if levels.is_empty() {
                return Err(claim_error(claim_id, "normativeLevels must not be empty"));
            }
            for level in levels {
                let level = level
                    .as_str()
                    .filter(|level| is_normative_level(level))
                    .ok_or_else(|| {
                        claim_error(claim_id, "normativeLevels contains an unknown level")
                    })?;
                let _ = level;
            }
            Ok(Authority::ArazzoSpec)
        }
        "repositoryContract" => {
            if entry.contains_key("specAnchors") || entry.contains_key("normativeLevels") {
                return Err(claim_error(
                    claim_id,
                    "repositoryContract authority cannot include Arazzo-spec fields",
                ));
            }
            let contract_id = required_text(entry, "contractId", &format!("claim {claim_id}"))?;
            if !is_repository_contract_id(contract_id) {
                return Err(claim_error(
                    claim_id,
                    "contractId must be a stable repository.* identifier",
                ));
            }
            let surface = required_text(entry, "surface", &format!("claim {claim_id}"))?;
            resolve_workspace_file(workspace, surface).map_err(|message| {
                claim_error(claim_id, &format!("invalid repository surface: {message}"))
            })?;
            Ok(Authority::RepositoryContract)
        }
        other => Err(claim_error(
            claim_id,
            &format!("unknown authority {other:?}"),
        )),
    }
}

fn validate_claim(
    workspace: &Path,
    allowed_spec_anchor_ids: &BTreeSet<String>,
    used_spec_anchor_ids: &mut BTreeSet<String>,
    inventory_area: &str,
    fragment_area: &str,
    entry: &Value,
) -> Result<(String, Authority, Status), String> {
    let entry = required_object(entry, "claim entry")?;
    let claim_id = required_text(entry, "id", "claim entry")?;
    if !is_claim_id(claim_id) {
        return Err(claim_error(
            claim_id,
            "id must be a stable lower-case dotted identifier",
        ));
    }
    let area = required_text(entry, "area", &format!("claim {claim_id}"))?;
    if area != inventory_area {
        return Err(claim_error(
            claim_id,
            &format!("owning area {area:?} does not match inventory area {inventory_area:?}"),
        ));
    }
    if area != fragment_area {
        return Err(claim_error(
            claim_id,
            &format!("owning area {area:?} does not match physical fragment {fragment_area:?}"),
        ));
    }
    let _ = required_text(entry, "claim", &format!("claim {claim_id}"))?;
    let authority = validate_authority(
        workspace,
        allowed_spec_anchor_ids,
        used_spec_anchor_ids,
        entry,
        claim_id,
    )?;
    let status = Status::parse(
        required_text(entry, "status", &format!("claim {claim_id}"))?,
        claim_id,
    )?;

    match status {
        Status::Covered => {
            let evidence = entry
                .get("evidence")
                .and_then(Value::as_object)
                .ok_or_else(|| claim_error(claim_id, "covered status requires evidence"))?;
            validate_evidence_list(workspace, evidence, "positive", claim_id, true)?;
            validate_evidence_list(workspace, evidence, "negative", claim_id, true)?;
            validate_optional_evidence(workspace, entry, claim_id)?;
        }
        Status::FailClosed => {
            let owner = required_text(entry, "owner", &format!("claim {claim_id}"))?;
            if !is_ticket_owner(owner) {
                return Err(claim_error(
                    claim_id,
                    "owner must use hermetic ac- ticket syntax",
                ));
            }
            let _ = required_text(entry, "reason", &format!("claim {claim_id}"))?;
            let evidence = entry
                .get("evidence")
                .and_then(Value::as_object)
                .ok_or_else(|| claim_error(claim_id, "failClosed status requires evidence"))?;
            validate_evidence_list(workspace, evidence, "negative", claim_id, true)?;
            validate_evidence_list(workspace, evidence, "zeroSideEffect", claim_id, true)?;
            validate_optional_evidence(workspace, entry, claim_id)?;
        }
        Status::Unsupported | Status::Deviation => {
            let owner = required_text(entry, "owner", &format!("claim {claim_id}"))?;
            if !is_ticket_owner(owner) {
                return Err(claim_error(
                    claim_id,
                    "owner must use hermetic ac- ticket syntax",
                ));
            }
            let _ = required_text(entry, "reason", &format!("claim {claim_id}"))?;
            validate_optional_evidence(workspace, entry, claim_id)?;
        }
    }

    Ok((claim_id.to_string(), authority, status))
}

fn validate_manifest(
    workspace: &Path,
    manifest: &Value,
    fragments: &Fragments,
) -> Result<ManifestReport, String> {
    let manifest = required_object(manifest, "manifest")?;
    if manifest.get("formatVersion").and_then(Value::as_u64) != Some(1) {
        return Err("manifest: formatVersion must be 1".to_string());
    }
    let allowed_spec_anchor_ids = tracked_spec_anchor_ids(manifest)?;
    let full_compliance = manifest
        .get("fullCompliance")
        .and_then(Value::as_bool)
        .ok_or_else(|| "manifest: fullCompliance must be a boolean".to_string())?;
    let inventory = required_array(manifest, "inventory", "manifest")?;
    if inventory.is_empty() {
        return Err("manifest: inventory must not be empty".to_string());
    }

    let mut inventory_by_id = BTreeMap::new();
    for row in inventory {
        let row = required_object(row, "manifest inventory entry")?;
        let id = required_text(row, "id", "manifest inventory entry")?;
        let area = required_text(row, "area", &format!("inventory claim {id}"))?;
        if !is_claim_id(id) {
            return Err(claim_error(
                id,
                "inventory id must be a stable lower-case dotted identifier",
            ));
        }
        if !known_area(area) {
            return Err(claim_error(
                id,
                &format!("inventory has unknown area {area:?}"),
            ));
        }
        if inventory_by_id
            .insert(id.to_string(), area.to_string())
            .is_some()
        {
            return Err(claim_error(id, "duplicate claim ID in canonical inventory"));
        }
    }

    let expected_areas: BTreeSet<_> = AREAS.into_iter().map(|(area, _)| area).collect();
    let actual_areas: BTreeSet<_> = fragments.keys().map(String::as_str).collect();
    if actual_areas != expected_areas {
        return Err(format!(
            "manifest fragments must be exactly {:?}, got {:?}",
            expected_areas, actual_areas
        ));
    }

    let mut used_spec_anchor_ids = BTreeSet::new();
    let mut observed_ids = BTreeSet::new();
    let mut report = ManifestReport {
        full_compliance,
        ..ManifestReport::default()
    };
    for (area, _) in AREAS {
        let fragment = fragments
            .get(area)
            .ok_or_else(|| format!("manifest fragment {area:?} is missing"))?;
        let fragment = required_object(fragment, &format!("{area} fragment"))?;
        let declared_area = required_text(fragment, "area", &format!("{area} fragment"))?;
        if declared_area != area {
            return Err(format!("{area} fragment declares area {declared_area:?}"));
        }
        let claims = required_array(fragment, "claims", &format!("{area} fragment"))?;
        for entry in claims {
            let entry_id = entry
                .as_object()
                .and_then(|entry| entry.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            if !observed_ids.insert(entry_id.to_string()) {
                return Err(claim_error(
                    entry_id,
                    "duplicate claim ID in merged fragments",
                ));
            }
            let expected_area = inventory_by_id.get(entry_id).ok_or_else(|| {
                claim_error(entry_id, "is not declared by the canonical inventory")
            })?;
            let (claim_id, authority, status) = validate_claim(
                workspace,
                &allowed_spec_anchor_ids,
                &mut used_spec_anchor_ids,
                expected_area,
                area,
                entry,
            )?;
            match authority {
                Authority::ArazzoSpec => report.arazzo.add(status),
                Authority::RepositoryContract => report.repository.add(status),
            }
            if claim_id != entry_id {
                return Err(claim_error(
                    entry_id,
                    "claim ID changed while validating the merged manifest",
                ));
            }
        }
    }

    for claim_id in inventory_by_id.keys() {
        if !observed_ids.contains(claim_id) {
            return Err(claim_error(
                claim_id,
                "is missing from the merged fragments",
            ));
        }
    }
    if used_spec_anchor_ids != allowed_spec_anchor_ids {
        let unused = allowed_spec_anchor_ids
            .difference(&used_spec_anchor_ids)
            .cloned()
            .collect::<Vec<_>>();
        return Err(format!(
            "manifest specAuthority contains allowed anchor IDs not used by any Arazzo claim: {unused:?}"
        ));
    }
    if full_compliance && !report.arazzo.all_covered() {
        return Err(
            "manifest: fullCompliance cannot be true while an Arazzo-spec claim is non-covered"
                .to_string(),
        );
    }
    Ok(report)
}

fn write_source(workspace: &Path, relative: &str, source: &str) {
    let path = workspace.join(relative);
    let parent = path
        .parent()
        .unwrap_or_else(|| panic!("test source path must have a parent: {}", path.display()));
    fs::create_dir_all(parent).unwrap_or_else(|err| panic!("creating {}: {err}", parent.display()));
    fs::write(&path, source).unwrap_or_else(|err| panic!("writing {}: {err}", path.display()));
}

fn covered_claim(id: &str, positive: &str, negative: &str) -> Value {
    json!({
        "id": id,
        "area": "model",
        "authority": "arazzoSpec",
        "specAnchors": ["spec/arazzo/v1.1.0.html#versions"],
        "normativeLevels": ["SHALL"],
        "claim": "A focused test claim.",
        "status": "covered",
        "evidence": { "positive": [positive], "negative": [negative] }
    })
}

fn non_covered_claim(id: &str, status: &str) -> Value {
    json!({
        "id": id,
        "area": "model",
        "authority": "arazzoSpec",
        "specAnchors": ["spec/arazzo/v1.1.0.html#versions"],
        "normativeLevels": ["SHALL"],
        "claim": "A focused non-covered test claim.",
        "status": status,
        "owner": "ac-abc12",
        "reason": "Owned by the test ticket."
    })
}

fn repository_claim(id: &str, status: &str) -> Value {
    json!({
        "id": id,
        "area": "surfaces",
        "authority": "repositoryContract",
        "contractId": "repository.tests.output.v1",
        "surface": "tests/evidence.rs",
        "claim": "A repository contract test claim.",
        "status": status,
        "owner": "ac-abc12",
        "reason": "Owned by the test ticket."
    })
}

fn test_spec_authority(allowed_anchor_ids: Vec<&str>) -> Value {
    json!({
        "document": SPEC_DOCUMENT,
        "version": SPEC_VERSION,
        "source": SPEC_SOURCE,
        "sha256": SPEC_SHA256,
        "allowedAnchorIds": allowed_anchor_ids
    })
}

fn documents_with_claim(claim: Value, full_compliance: bool) -> (Value, Fragments) {
    let id = claim
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("test claim must have an id"));
    let area = claim
        .get("area")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("test claim must have an area"));
    let allowed_anchor_ids = if claim.get("authority").and_then(Value::as_str) == Some("arazzoSpec")
    {
        vec!["versions"]
    } else {
        Vec::new()
    };
    let mut fragments = Fragments::new();
    for (known_area, _) in AREAS {
        let claims = if known_area == area {
            vec![claim.clone()]
        } else {
            Vec::new()
        };
        fragments.insert(
            known_area.to_string(),
            json!({ "area": known_area, "claims": claims }),
        );
    }
    (
        json!({
            "formatVersion": 1,
            "fullCompliance": full_compliance,
            "specAuthority": test_spec_authority(allowed_anchor_ids),
            "inventory": [{ "id": id, "area": area }]
        }),
        fragments,
    )
}

fn claim_mut<'a>(fragments: &'a mut Fragments, area: &str, id: &str) -> &'a mut Map<String, Value> {
    fragments
        .get_mut(area)
        .and_then(Value::as_object_mut)
        .and_then(|fragment| fragment.get_mut("claims"))
        .and_then(Value::as_array_mut)
        .and_then(|claims| {
            claims
                .iter_mut()
                .find(|claim| claim.get("id").and_then(Value::as_str) == Some(id))
        })
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("claim {id} must exist in {area} test fragment"))
}

fn expected_error(result: Result<ManifestReport, String>, context: &str) -> String {
    match result {
        Ok(report) => panic!("{context}, but validation passed with {report:?}"),
        Err(error) => error,
    }
}

#[test]
fn checked_in_manifest_has_deterministic_separate_totals() {
    let root = workspace_root();
    let (manifest, fragments) = checked_in_documents();
    let report = validate_manifest(&root, &manifest, &fragments)
        .unwrap_or_else(|err| panic!("checked-in manifest must validate: {err}"));
    assert_eq!(
        report.arazzo,
        StatusTotals {
            covered: 5,
            fail_closed: 1,
            unsupported: 3,
            deviation: 10,
        }
    );
    assert_eq!(
        report.repository,
        StatusTotals {
            covered: 0,
            fail_closed: 0,
            unsupported: 0,
            deviation: 1,
        }
    );
    assert!(!report.full_compliance);
}

#[test]
fn duplicate_inventory_ids_and_fragment_ids_fail() {
    let root = workspace_root();
    let (mut manifest, fragments) = checked_in_documents();
    manifest["inventory"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("inventory array"))
        .push(json!({
            "id": "model.version-grammar-feature-set",
            "area": "model"
        }));
    let error = expected_error(
        validate_manifest(&root, &manifest, &fragments),
        "duplicate inventory must fail",
    );
    assert!(
        error.contains("model.version-grammar-feature-set"),
        "{error}"
    );

    let (manifest, mut fragments) = checked_in_documents();
    let duplicate = fragments["model"]["claims"][0].clone();
    fragments
        .get_mut("model")
        .and_then(|fragment| fragment.get_mut("claims"))
        .and_then(Value::as_array_mut)
        .unwrap_or_else(|| panic!("model claims array"))
        .push(duplicate);
    let error = expected_error(
        validate_manifest(&root, &manifest, &fragments),
        "duplicate fragment must fail",
    );
    assert!(
        error.contains("model.version-grammar-feature-set"),
        "{error}"
    );

    let (manifest, mut fragments) = checked_in_documents();
    let moved_claim = {
        let model_claims = fragments
            .get_mut("model")
            .and_then(|fragment| fragment.get_mut("claims"))
            .and_then(Value::as_array_mut)
            .unwrap_or_else(|| panic!("model claims array"));
        let index = model_claims
            .iter()
            .position(|claim| {
                claim.get("id").and_then(Value::as_str) == Some("model.version-grammar-feature-set")
            })
            .unwrap_or_else(|| panic!("model claim must exist"));
        model_claims.remove(index)
    };
    fragments
        .get_mut("runtime")
        .and_then(|fragment| fragment.get_mut("claims"))
        .and_then(Value::as_array_mut)
        .unwrap_or_else(|| panic!("runtime claims array"))
        .push(moved_claim);
    let error = expected_error(
        validate_manifest(&root, &manifest, &fragments),
        "claim moved across physical fragments must fail",
    );
    assert!(
        error.contains("model.version-grammar-feature-set") && error.contains("physical fragment"),
        "{error}"
    );
}

#[test]
fn missing_or_unexpected_inventory_claim_and_area_mismatch_fail() {
    let root = workspace_root();
    let (manifest, mut fragments) = checked_in_documents();
    let claims = fragments
        .get_mut("model")
        .and_then(|fragment| fragment.get_mut("claims"))
        .and_then(Value::as_array_mut)
        .unwrap_or_else(|| panic!("model claims array"));
    claims.retain(|claim| {
        claim.get("id").and_then(Value::as_str) != Some("model.version-grammar-feature-set")
    });
    let error = expected_error(
        validate_manifest(&root, &manifest, &fragments),
        "missing inventory claim must fail",
    );
    assert!(
        error.contains("model.version-grammar-feature-set"),
        "{error}"
    );

    let (manifest, mut fragments) = checked_in_documents();
    fragments
        .get_mut("model")
        .and_then(|fragment| fragment.get_mut("claims"))
        .and_then(Value::as_array_mut)
        .unwrap_or_else(|| panic!("model claims array"))
        .push(non_covered_claim("model.unexpected-claim", "deviation"));
    let error = expected_error(
        validate_manifest(&root, &manifest, &fragments),
        "unexpected claim must fail",
    );
    assert!(error.contains("model.unexpected-claim"), "{error}");

    let (manifest, mut fragments) = checked_in_documents();
    claim_mut(&mut fragments, "model", "model.version-grammar-feature-set")
        .insert("area".to_string(), json!("runtime"));
    let error = expected_error(
        validate_manifest(&root, &manifest, &fragments),
        "area mismatch must fail",
    );
    assert!(
        error.contains("model.version-grammar-feature-set"),
        "{error}"
    );
}

#[test]
fn covered_evidence_requires_positive_and_negative_references() {
    let temp = TempWorkspace::new();
    write_source(temp.path(), "tests/evidence.rs", "#[test]\nfn good() {}\n");
    let (manifest, mut fragments) = documents_with_claim(
        covered_claim(
            "model.covered-evidence",
            "tests/evidence.rs#good",
            "tests/evidence.rs#good",
        ),
        true,
    );
    claim_mut(&mut fragments, "model", "model.covered-evidence")
        .get_mut("evidence")
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("evidence object"))
        .remove("positive");
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "missing positive must fail",
    );
    assert!(
        error.contains("model.covered-evidence") && error.contains("positive"),
        "{error}"
    );

    let (manifest, mut fragments) = documents_with_claim(
        covered_claim(
            "model.covered-evidence",
            "tests/evidence.rs#good",
            "tests/evidence.rs#good",
        ),
        true,
    );
    claim_mut(&mut fragments, "model", "model.covered-evidence")
        .get_mut("evidence")
        .and_then(Value::as_object_mut)
        .unwrap_or_else(|| panic!("evidence object"))
        .remove("negative");
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "missing negative must fail",
    );
    assert!(
        error.contains("model.covered-evidence") && error.contains("negative"),
        "{error}"
    );
}

#[test]
fn non_covered_claims_require_owner_and_fail_closed_evidence() {
    let temp = TempWorkspace::new();
    write_source(temp.path(), "tests/evidence.rs", "#[test]\nfn good() {}\n");
    let (manifest, fragments) =
        documents_with_claim(non_covered_claim("model.bad-status", "other"), false);
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "unknown status must fail",
    );
    assert!(
        error.contains("model.bad-status") && error.contains("unknown status"),
        "{error}"
    );

    let (manifest, mut fragments) =
        documents_with_claim(non_covered_claim("model.owner", "deviation"), false);
    claim_mut(&mut fragments, "model", "model.owner").remove("owner");
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "missing owner must fail",
    );
    assert!(
        error.contains("model.owner") && error.contains("owner"),
        "{error}"
    );

    let (manifest, mut fragments) =
        documents_with_claim(non_covered_claim("model.reason", "deviation"), false);
    claim_mut(&mut fragments, "model", "model.reason").remove("reason");
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "missing reason must fail",
    );
    assert!(
        error.contains("model.reason") && error.contains("reason"),
        "{error}"
    );

    let mut claim = non_covered_claim("model.fail-closed", "failClosed");
    claim
        .as_object_mut()
        .unwrap_or_else(|| panic!("claim object"))
        .insert(
            "evidence".to_string(),
            json!({ "negative": ["tests/evidence.rs#good"] }),
        );
    let (manifest, fragments) = documents_with_claim(claim, false);
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "incomplete failClosed must fail",
    );
    assert!(
        error.contains("model.fail-closed") && error.contains("zeroSideEffect"),
        "{error}"
    );
}

#[test]
fn evidence_references_require_existing_non_ignored_test_items() {
    let temp = TempWorkspace::new();
    write_source(
        temp.path(),
        "tests/evidence.rs",
        r#"
#[test]
fn sync_case() {}

#[tokio::test]
async fn async_case() {}

#[cfg(test)]
mod nested_tests {
    #[test]
    fn nested_sync_case() {}

    mod deeper {
        #[tokio::test]
        async fn nested_async_case() {}
    }
}

fn helper_only() {}

// #[test]
// fn commented_case() {}

#[test]
#[ignore]
fn ignored_case() {}

macro_rules! define_macro_rule_case {
    () => {
        #[test]
        fn macro_rule_case() {}
    };
}

define_tests! {
    #[test]
    fn macro_invocation_case() {}
}

#[cfg(any())]
#[test]
fn disabled_cfg_case() {}

#[cfg_attr(test, ignore)]
#[test]
fn disabled_cfg_attr_case() {}

#[cfg(any())]
mod disabled_module {
    #[test]
    fn disabled_module_case() {}
}

mod inner_disabled_module {
    #![cfg(any())]

    #[test]
    fn inner_disabled_module_case() {}
}

mod inner_cfg_attr_module {
    #![cfg_attr(test, cfg(any()))]

    #[test]
    fn inner_cfg_attr_module_case() {}
}

fn enclosing_function() {
    #[test]
    fn nested_in_function_body_case() {}
}

struct Fixture;

impl Fixture {
    #[test]
    fn nested_in_impl_body_case() {}
}

const TOKEN_BODY: () = {
    #[test]
    fn nested_in_const_body_case() {}
};
"#,
    );
    let source = fs::read_to_string(temp.path().join("tests/evidence.rs"))
        .unwrap_or_else(|err| panic!("reading evidence parser fixture: {err}"));
    let recognized = recognized_non_ignored_test_items(&source);
    for test_name in [
        "sync_case",
        "async_case",
        "nested_sync_case",
        "nested_async_case",
    ] {
        assert!(recognized.contains(test_name), "missing {test_name:?}");
    }
    for test_name in [
        "macro_rule_case",
        "macro_invocation_case",
        "disabled_cfg_case",
        "disabled_cfg_attr_case",
        "disabled_module_case",
        "inner_disabled_module_case",
        "inner_cfg_attr_module_case",
        "nested_in_function_body_case",
        "nested_in_impl_body_case",
        "nested_in_const_body_case",
    ] {
        assert!(
            !recognized.contains(test_name),
            "non-executable token item {test_name:?} must not be recognized"
        );
    }
    for reference in [
        "tests/evidence.rs#sync_case",
        "tests/evidence.rs#async_case",
        "tests/evidence.rs#nested_sync_case",
        "tests/evidence.rs#nested_async_case",
    ] {
        let (manifest, fragments) = documents_with_claim(
            covered_claim("model.test-reference", reference, reference),
            true,
        );
        validate_manifest(temp.path(), &manifest, &fragments)
            .unwrap_or_else(|err| panic!("valid reference {reference:?} must pass: {err}"));
    }
    for reference in [
        "tests/missing.rs#sync_case",
        "tests/evidence.rs#missing_case",
        "tests/evidence.rs#not-a-function",
        "tests/evidence.rs#helper_only",
        "tests/evidence.rs#commented_case",
        "tests/evidence.rs#ignored_case",
        "tests/evidence.rs#macro_rule_case",
        "tests/evidence.rs#macro_invocation_case",
        "tests/evidence.rs#disabled_cfg_case",
        "tests/evidence.rs#disabled_cfg_attr_case",
        "tests/evidence.rs#disabled_module_case",
        "tests/evidence.rs#inner_disabled_module_case",
        "tests/evidence.rs#inner_cfg_attr_module_case",
        "tests/evidence.rs#nested_in_function_body_case",
        "tests/evidence.rs#nested_in_impl_body_case",
        "tests/evidence.rs#nested_in_const_body_case",
    ] {
        let (manifest, fragments) = documents_with_claim(
            covered_claim("model.test-reference", reference, reference),
            true,
        );
        let error = expected_error(
            validate_manifest(temp.path(), &manifest, &fragments),
            "invalid evidence reference must fail",
        );
        assert!(error.contains("model.test-reference"), "{error}");
    }
}

#[test]
fn tracked_spec_authority_is_pinned_finite_and_hermetic() {
    let temp = TempWorkspace::new();
    write_source(temp.path(), "tests/evidence.rs", "#[test]\nfn good() {}\n");
    assert!(
        !temp.path().join("spec").exists(),
        "the manifest gate fixture must not depend on a vendored spec directory"
    );

    let (manifest, fragments) = documents_with_claim(
        covered_claim(
            "model.authority",
            "tests/evidence.rs#good",
            "tests/evidence.rs#good",
        ),
        true,
    );
    validate_manifest(temp.path(), &manifest, &fragments)
        .unwrap_or_else(|err| panic!("tracked authority must validate without /spec: {err}"));

    let mut missing_authority = manifest.clone();
    missing_authority
        .as_object_mut()
        .unwrap_or_else(|| panic!("manifest object"))
        .remove("specAuthority");
    let error = expected_error(
        validate_manifest(temp.path(), &missing_authority, &fragments),
        "missing specification authority must fail",
    );
    assert!(error.contains("specAuthority is required"), "{error}");

    let mut extra_field = manifest.clone();
    extra_field["specAuthority"]["unreviewed"] = json!(true);
    let error = expected_error(
        validate_manifest(temp.path(), &extra_field, &fragments),
        "extra specification authority field must fail",
    );
    assert!(error.contains("fields must be exactly"), "{error}");

    let mut changed_checksum = manifest.clone();
    changed_checksum["specAuthority"]["sha256"] = json!("not-the-pinned-checksum");
    let error = expected_error(
        validate_manifest(temp.path(), &changed_checksum, &fragments),
        "changed specification checksum must fail",
    );
    assert!(
        error.contains("sha256") && error.contains("pinned"),
        "{error}"
    );

    let mut duplicate_anchor = manifest.clone();
    duplicate_anchor["specAuthority"]["allowedAnchorIds"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("allowed anchors array"))
        .push(json!("versions"));
    let error = expected_error(
        validate_manifest(temp.path(), &duplicate_anchor, &fragments),
        "duplicate allowed anchor must fail",
    );
    assert!(error.contains("duplicate allowed anchor"), "{error}");

    let mut malformed_anchor = manifest.clone();
    malformed_anchor["specAuthority"]["allowedAnchorIds"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("allowed anchors array"))
        .push(json!("not an anchor"));
    let error = expected_error(
        validate_manifest(temp.path(), &malformed_anchor, &fragments),
        "malformed allowed anchor must fail",
    );
    assert!(error.contains("non-empty HTML anchor IDs"), "{error}");

    let mut unused_anchor = manifest;
    unused_anchor["specAuthority"]["allowedAnchorIds"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("allowed anchors array"))
        .push(json!("unused-anchor"));
    let error = expected_error(
        validate_manifest(temp.path(), &unused_anchor, &fragments),
        "unused allowed anchor must fail",
    );
    assert!(error.contains("not used"), "{error}");
}

#[test]
fn authority_forms_are_exclusive_and_repository_totals_are_separate() {
    let temp = TempWorkspace::new();
    write_source(temp.path(), "tests/evidence.rs", "#[test]\nfn good() {}\n");

    let (manifest, mut fragments) = documents_with_claim(
        covered_claim(
            "model.authority",
            "tests/evidence.rs#good",
            "tests/evidence.rs#good",
        ),
        true,
    );
    claim_mut(&mut fragments, "model", "model.authority").remove("specAnchors");
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "missing spec anchor must fail",
    );
    assert!(
        error.contains("model.authority") && error.contains("specAnchors"),
        "{error}"
    );

    let (manifest, mut fragments) =
        documents_with_claim(repository_claim("surfaces.contract", "deviation"), false);
    claim_mut(&mut fragments, "surfaces", "surfaces.contract").remove("contractId");
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "missing contract ID must fail",
    );
    assert!(
        error.contains("surfaces.contract") && error.contains("contractId"),
        "{error}"
    );

    let (manifest, mut fragments) =
        documents_with_claim(repository_claim("surfaces.contract", "deviation"), false);
    claim_mut(&mut fragments, "surfaces", "surfaces.contract").remove("surface");
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "missing contract surface must fail",
    );
    assert!(
        error.contains("surfaces.contract") && error.contains("surface"),
        "{error}"
    );

    let (manifest, mut fragments) = documents_with_claim(
        covered_claim(
            "model.authority",
            "tests/evidence.rs#good",
            "tests/evidence.rs#good",
        ),
        true,
    );
    claim_mut(&mut fragments, "model", "model.authority").insert(
        "contractId".to_string(),
        json!("repository.tests.output.v1"),
    );
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "mixed authority fields must fail",
    );
    assert!(
        error.contains("model.authority") && error.contains("repository-contract"),
        "{error}"
    );

    let (mut manifest, mut fragments) = documents_with_claim(
        covered_claim(
            "model.covered",
            "tests/evidence.rs#good",
            "tests/evidence.rs#good",
        ),
        true,
    );
    manifest["inventory"]
        .as_array_mut()
        .unwrap_or_else(|| panic!("inventory array"))
        .push(json!({ "id": "surfaces.contract", "area": "surfaces" }));
    fragments
        .get_mut("surfaces")
        .and_then(|fragment| fragment.get_mut("claims"))
        .and_then(Value::as_array_mut)
        .unwrap_or_else(|| panic!("surface claims array"))
        .push(repository_claim("surfaces.contract", "deviation"));
    let report = validate_manifest(temp.path(), &manifest, &fragments).unwrap_or_else(|err| {
        panic!("repository contract must not weaken full Arazzo compliance: {err}")
    });
    assert_eq!(report.arazzo.covered, 1);
    assert_eq!(report.repository.deviation, 1);
    assert!(report.full_compliance);
}

#[test]
fn anchor_outside_tracked_spec_authority_fails() {
    let temp = TempWorkspace::new();
    write_source(temp.path(), "tests/evidence.rs", "#[test]\nfn good() {}\n");
    let (manifest, mut fragments) = documents_with_claim(
        covered_claim(
            "model.nonexistent-spec-anchor",
            "tests/evidence.rs#good",
            "tests/evidence.rs#good",
        ),
        true,
    );
    claim_mut(&mut fragments, "model", "model.nonexistent-spec-anchor").insert(
        "specAnchors".to_string(),
        json!(["spec/arazzo/v1.1.0.html#correctly-prefixed-but-nonexistent"]),
    );

    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "anchor outside tracked specification authority must fail",
    );
    assert!(
        error.contains("model.nonexistent-spec-anchor")
            && error.contains("correctly-prefixed-but-nonexistent")
            && error.contains("tracked specAuthority"),
        "{error}"
    );
}

#[test]
fn full_compliance_marker_rejects_non_covered_arazzo_claims() {
    let temp = TempWorkspace::new();
    let (manifest, fragments) =
        documents_with_claim(non_covered_claim("model.not-covered", "unsupported"), true);
    let error = expected_error(
        validate_manifest(temp.path(), &manifest, &fragments),
        "unqualified full compliance must fail",
    );
    assert!(error.contains("fullCompliance"), "{error}");
}
