//! Public boundary and paired-result regressions for the RFC 9535 query owner.
//!
//! Fixture rows are reused from the frozen qualification consumers. Every
//! selected value must be the node addressed by its paired pointer, by
//! identity, in query order and with repeated occurrences preserved.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use arazzo_expr::{JsonPathError, JsonPathMatch, JsonPathQuery};
use serde_json::{json, Value};

const QUERY_BYTE_LIMIT: usize = 16_384;
const QUERY_STRUCTURAL_LIMIT: usize = 128;
const CONTEXT_DEPTH_LIMIT: usize = 128;

fn parse(expression: &str) -> JsonPathQuery {
    match JsonPathQuery::parse(expression, None) {
        Ok(query) => query,
        Err(error) => panic!("{expression:?} failed to parse: {error}"),
    }
}

fn query<'a>(expression: &str, context: &'a Value) -> Vec<JsonPathMatch<'a>> {
    match parse(expression).query(context) {
        Ok(matches) => matches,
        Err(error) => panic!("{expression:?} failed to evaluate: {error}"),
    }
}

fn parse_error(expression: &str, version: Option<&str>) -> JsonPathError {
    match JsonPathQuery::parse(expression, version) {
        Ok(_) => panic!("{expression:?} unexpectedly parsed"),
        Err(error) => error,
    }
}

fn query_error(expression: &str, context: &Value) -> JsonPathError {
    match parse(expression).query(context) {
        Ok(matches) => panic!(
            "{expression:?} unexpectedly selected {} node(s)",
            matches.len()
        ),
        Err(error) => error,
    }
}

/// Assert every match pairs the node its pointer addresses, by identity.
fn assert_paired(context: &Value, matches: &[JsonPathMatch<'_>]) {
    for found in matches {
        let addressed = match context.pointer(&found.pointer) {
            Some(node) => node,
            None => panic!("pointer {:?} does not resolve in {context}", found.pointer),
        };
        assert!(
            std::ptr::eq(addressed, found.value),
            "pointer {:?} addresses a different node than the paired value",
            found.pointer
        );
    }
}

fn pointers<'m>(matches: &'m [JsonPathMatch<'_>]) -> Vec<&'m str> {
    matches.iter().map(|found| found.pointer.as_str()).collect()
}

fn values(matches: &[JsonPathMatch<'_>]) -> Vec<Value> {
    matches.iter().map(|found| found.value.clone()).collect()
}

#[test]
fn paired_values_and_pointers_follow_query_order_and_repeat_occurrences() {
    let document = json!({
        "items": [false, 0, "", null],
        "ids": [{"id": 1}, {"id": 2}, {"id": 3}],
        "groups": [[{"id": 1}], [{"id": 2}, {"id": 3}]],
        "wanted": 2,
        "value": null,
        "left": [1, 2],
        "right": [3, 4],
        "#": {"q": "hash"},
        "a/b": "slash",
        "a~b": "tilde",
        "O'Reilly": "quote",
        "雪": "unicode",
        "": "empty"
    });
    let table: Vec<(&str, Vec<&str>, Vec<Value>)> = vec![
        ("$", vec![""], vec![document.clone()]),
        ("$.missing", vec![], vec![]),
        ("$.value", vec!["/value"], vec![Value::Null]),
        (
            "$.items[*]",
            vec!["/items/0", "/items/1", "/items/2", "/items/3"],
            vec![json!(false), json!(0), json!(""), Value::Null],
        ),
        ("$.items[-1]", vec!["/items/3"], vec![Value::Null]),
        (
            "$.items[1:3]",
            vec!["/items/1", "/items/2"],
            vec![json!(0), json!("")],
        ),
        (
            "$.items[::-1]",
            vec!["/items/3", "/items/2", "/items/1", "/items/0"],
            vec![Value::Null, json!(""), json!(0), json!(false)],
        ),
        ("$.items[::0]", vec![], vec![]),
        (
            "$.ids[2,0,2].id",
            vec!["/ids/2/id", "/ids/0/id", "/ids/2/id"],
            vec![json!(3), json!(1), json!(3)],
        ),
        (
            "$['right','left']",
            vec!["/right", "/left"],
            vec![json!([3, 4]), json!([1, 2])],
        ),
        (
            "$.ids..id",
            vec!["/ids/0/id", "/ids/1/id", "/ids/2/id"],
            vec![json!(1), json!(2), json!(3)],
        ),
        (
            "$.groups..id",
            vec!["/groups/0/0/id", "/groups/1/0/id", "/groups/1/1/id"],
            vec![json!(1), json!(2), json!(3)],
        ),
        (
            "$.ids[?@.id == $.wanted].id",
            vec!["/ids/1/id"],
            vec![json!(2)],
        ),
        (
            "$.ids[?@.id > 1].id",
            vec!["/ids/1/id", "/ids/2/id"],
            vec![json!(2), json!(3)],
        ),
        (
            "$.groups[?@[?@.id == 2]][?@.id == 2].id",
            vec!["/groups/1/0/id"],
            vec![json!(2)],
        ),
        ("$['#'].q", vec!["/#/q"], vec![json!("hash")]),
        (r#"$["a/b"]"#, vec!["/a~1b"], vec![json!("slash")]),
        (r#"$["a~b"]"#, vec!["/a~0b"], vec![json!("tilde")]),
        (r"$['O\'Reilly']", vec!["/O'Reilly"], vec![json!("quote")]),
        (r#"$["雪"]"#, vec!["/雪"], vec![json!("unicode")]),
        (r#"$[""]"#, vec!["/"], vec![json!("empty")]),
    ];
    for (expression, expected_pointers, expected_values) in table {
        let matches = query(expression, &document);
        assert_eq!(pointers(&matches), expected_pointers, "{expression}");
        assert_eq!(values(&matches), expected_values, "{expression}");
        assert_paired(&document, &matches);
    }
}

#[test]
fn nested_numeric_equality_compares_containers_recursively() {
    let document = json!([
        {"a": [1], "b": [1.0]},
        {"a": {"x": 1}, "b": {"x": 1.0}},
        {"a": [1, 2], "b": [1.0]},
        {"a": [{"x": [1, 2.0, {"z": -0.0}]}], "b": [{"x": [1.0, 2, {"z": 0}]}]},
        {"a": {"x": 1, "y": 2}, "b": {"x": 1.0}},
        {"a": [1], "b": ["1"]}
    ]);
    let equal = query("$[?@.a == @.b]", &document);
    assert_eq!(pointers(&equal), ["/0", "/1", "/3"]);
    assert_paired(&document, &equal);
    let unequal = query("$[?@.a != @.b]", &document);
    assert_eq!(pointers(&unequal), ["/2", "/4", "/5"]);
    assert_paired(&document, &unequal);
    let symmetric = query("$[?@.b == @.a]", &document);
    assert_eq!(pointers(&symmetric), ["/0", "/1", "/3"]);
}

#[test]
fn parse_validates_syntax_and_version_before_any_context_exists() {
    assert!(JsonPathQuery::parse("$.items[*]", Some("rfc9535")).is_ok());
    for invalid in [
        "",
        "@.items",
        "items",
        "$.items[",
        "$ trailing",
        "$.items.#",
        "$[?@ ||| @]",
        "$[?unknown(@)]",
        "$[?match(@)]",
    ] {
        assert!(
            matches!(
                parse_error(invalid, None),
                JsonPathError::InvalidSyntax { .. }
            ),
            "{invalid:?}"
        );
    }
    for version in [
        "draft-goessner-dispatch-jsonpath-00",
        "goessner",
        "RFC9535",
        "rfc9535 ",
        "",
    ] {
        // The expression is invalid too: the version is rejected first.
        assert_eq!(
            parse_error("$.items[", Some(version)),
            JsonPathError::UnsupportedVersion {
                version: version.to_string()
            }
        );
    }
    let oversized = format!("${}", ".a".repeat(QUERY_STRUCTURAL_LIMIT + 1));
    assert!(matches!(
        parse_error(&oversized, Some("draft-goessner-dispatch-jsonpath-00")),
        JsonPathError::UnsupportedVersion { .. }
    ));

    let syntax = parse_error("$.items[", None);
    assert!(syntax.to_string().starts_with("invalid JSONPath syntax: "));
    let version = parse_error("$", Some("goessner"));
    assert!(version.to_string().contains("goessner"));
    let _: &dyn std::error::Error = &syntax;
}

#[test]
fn match_and_search_use_full_and_search_semantics_with_literal_anchors() {
    let cases = [
        ("$[?match(@, '^a$')]", json!(["^a$"]), 1),
        ("$[?match(@, '^a$')]", json!(["a"]), 0),
        ("$[?search(@, '^a$')]", json!(["prefix ^a$ suffix"]), 1),
        ("$[?search(@, '^a$')]", json!(["a"]), 0),
        ("$[?match(@, 'a|b')]", json!(["b"]), 1),
        ("$[?match(@, 'a|b')]", json!(["ab"]), 0),
        ("$[?search(@, 'a|b')]", json!(["prefix b suffix"]), 1),
        ("$[?search(@, 'a|b')]", json!(["xyz"]), 0),
    ];
    for (expression, document, expected) in cases {
        let matches = query(expression, &document);
        assert_eq!(matches.len(), expected, "{expression} on {document}");
        assert_paired(&document, &matches);
    }
}

#[test]
fn dynamic_patterns_are_compiled_per_node() {
    let document = json!([
        {"value": "abc", "pattern": "a.c"},
        {"value": "abc", "pattern": "b"},
        {"value": "abc", "pattern": "["}
    ]);
    let full = query("$[?match(@.value, @.pattern)].value", &document);
    assert_eq!(pointers(&full), ["/0/value"]);
    let search = query("$[?search(@.value, @.pattern)].value", &document);
    assert_eq!(pointers(&search), ["/0/value", "/1/value"]);
    assert_paired(&document, &search);
}

#[test]
fn invalid_patterns_and_non_string_arguments_are_logical_false_not_errors() {
    for (expression, document) in [
        ("$[?match(@, '(?i)a')]", json!(["A"])),
        (r"$[?match(@, '\\d')]", json!(["1"])),
        ("$[?search(@, '[')]", json!(["a"])),
        ("$[?match(@, 'a{3,2}')]", json!(["a"])),
        ("$[?match(@, '7')]", json!([7])),
        ("$[?match(@, 7)]", json!(["7"])),
        ("$[?match(@.missing, 'a')]", json!([{"x": 1}])),
    ] {
        assert!(query(expression, &document).is_empty(), "{expression}");
    }
    // Negating an invalid pattern is a valid, true predicate.
    for function in ["match", "search"] {
        for invalid in ["[", "[z-a]", "a{3,2}"] {
            let expression = format!("$[?!{function}(@, '{invalid}')]");
            assert_eq!(
                pointers(&query(&expression, &json!(["a"]))),
                ["/0"],
                "{expression}"
            );
        }
    }
}

#[test]
fn operational_regex_failures_invalidate_the_whole_query_even_under_negation() {
    let document = json!([{"value": "a", "pattern": "a{1000000000}"}]);
    for function in ["match", "search"] {
        for pattern in ["@.pattern", "'a{1000000000}'"] {
            for negation in ["", "!"] {
                let expression = format!("$[?{negation}{function}(@.value, {pattern})]");
                match query_error(&expression, &document) {
                    JsonPathError::Evaluation { detail } => {
                        assert!(detail.contains("limit"), "{expression}: {detail}")
                    }
                    other => panic!("{expression}: expected Evaluation, got {other:?}"),
                }
            }
        }
    }
    // A failed evaluation neither contaminates the next query on this thread
    // nor poisons the compiled query.
    let compiled = parse("$[?!match(@.value, @.pattern)].value");
    assert!(compiled.query(&document).is_err());
    let recovered = json!([{"value": "a", "pattern": "b"}]);
    let matches = match compiled.query(&recovered) {
        Ok(matches) => matches,
        Err(error) => panic!("reused query failed: {error}"),
    };
    assert_eq!(pointers(&matches), ["/0/value"]);
    assert_eq!(
        pointers(&query("$[?match(@.value, 'a')].value", &document)),
        ["/0/value"]
    );
}

#[test]
fn iregexp_keeps_its_own_pattern_nesting_limit() {
    let pattern = format!("{}a{}", "(".repeat(4096), ")".repeat(4096));
    let document = json!([{"value": "a", "pattern": pattern}]);
    assert!(matches!(
        query_error("$[?match(@.value, @.pattern)]", &document),
        JsonPathError::Evaluation { .. }
    ));
}

fn name_selector_query(name_bytes: usize) -> String {
    format!("$['{}']", "a".repeat(name_bytes))
}

#[test]
fn query_byte_limit_is_enforced_before_parsing() {
    for total in [QUERY_BYTE_LIMIT - 1, QUERY_BYTE_LIMIT] {
        let expression = name_selector_query(total - 5);
        assert_eq!(expression.len(), total);
        assert!(
            JsonPathQuery::parse(&expression, None).is_ok(),
            "{total} bytes"
        );
    }
    let over = name_selector_query(QUERY_BYTE_LIMIT + 1 - 5);
    assert_eq!(over.len(), QUERY_BYTE_LIMIT + 1);
    assert_eq!(
        parse_error(&over, None),
        JsonPathError::ResourceLimit {
            resource: "query bytes",
            limit: QUERY_BYTE_LIMIT
        }
    );
    // Over the limit and syntactically invalid: the parser never sees it.
    let over_invalid = format!("$['{}", "a".repeat(QUERY_BYTE_LIMIT));
    assert_eq!(
        parse_error(&over_invalid, None),
        JsonPathError::ResourceLimit {
            resource: "query bytes",
            limit: QUERY_BYTE_LIMIT
        }
    );
    // Multi-byte characters count as bytes, not characters.
    let multibyte = format!("$['{}']", "雪".repeat((QUERY_BYTE_LIMIT - 4) / 3));
    assert_eq!(multibyte.len(), QUERY_BYTE_LIMIT + 1);
    assert!(matches!(
        parse_error(&multibyte, None),
        JsonPathError::ResourceLimit {
            resource: "query bytes",
            ..
        }
    ));
}

#[test]
fn structural_character_limit_counts_quoted_punctuation_conservatively() {
    for dots in [QUERY_STRUCTURAL_LIMIT - 1, QUERY_STRUCTURAL_LIMIT] {
        let expression = format!("${}", ".a".repeat(dots));
        assert!(
            JsonPathQuery::parse(&expression, None).is_ok(),
            "{dots} dots"
        );
    }
    let over = format!("${}", ".a".repeat(QUERY_STRUCTURAL_LIMIT + 1));
    assert_eq!(
        parse_error(&over, None),
        JsonPathError::ResourceLimit {
            resource: "query structural characters",
            limit: QUERY_STRUCTURAL_LIMIT
        }
    );
    // Over the limit and syntactically invalid: the parser never sees it.
    let over_invalid = format!("${}[", ".a".repeat(QUERY_STRUCTURAL_LIMIT + 1));
    assert!(matches!(
        parse_error(&over_invalid, None),
        JsonPathError::ResourceLimit { .. }
    ));
    // Every budgeted byte counts, not just dots.
    let mixed = format!("$[?{}@.a]", "!".repeat(QUERY_STRUCTURAL_LIMIT - 1));
    assert!(matches!(
        parse_error(&mixed, None),
        JsonPathError::ResourceLimit { .. }
    ));

    // One bracket plus 127 dots inside a quoted name is exactly at the limit,
    // is a valid name selector, and evaluates.
    let dotted_name = ".".repeat(QUERY_STRUCTURAL_LIMIT - 1);
    let key = dotted_name.as_str();
    let document = json!({ key: 1 });
    let at_limit = format!("$['{dotted_name}']");
    let matches = query(&at_limit, &document);
    assert_eq!(pointers(&matches), [format!("/{dotted_name}").as_str()]);
    assert_paired(&document, &matches);
    // One bracket plus 128 quoted dots is 129: rejected even though the
    // expression would otherwise be a valid name selector.
    let over_quoted = format!("$['{}']", ".".repeat(QUERY_STRUCTURAL_LIMIT));
    assert_eq!(
        parse_error(&over_quoted, None),
        JsonPathError::ResourceLimit {
            resource: "query structural characters",
            limit: QUERY_STRUCTURAL_LIMIT
        }
    );
    // Escaped punctuation inside a literal counts too.
    let escaped = format!("$[?@.a == '{}']", "\\|".repeat(QUERY_STRUCTURAL_LIMIT));
    assert!(matches!(
        parse_error(&escaped, None),
        JsonPathError::ResourceLimit { .. }
    ));
}

fn nested_arrays(containers: usize) -> Value {
    let mut value = json!(1);
    for _ in 0..containers {
        value = json!([value]);
    }
    value
}

#[test]
fn context_depth_limit_counts_nesting_not_siblings() {
    let root = parse("$");
    for containers in [CONTEXT_DEPTH_LIMIT - 1, CONTEXT_DEPTH_LIMIT] {
        let document = nested_arrays(containers);
        let matches = match root.query(&document) {
            Ok(matches) => matches,
            Err(error) => panic!("{containers} nested containers rejected: {error}"),
        };
        assert_eq!(matches.len(), 1);
    }
    // Descent through an admitted context at the limit exercises the upstream
    // evaluator on the deepest accepted input.
    let at_limit = nested_arrays(CONTEXT_DEPTH_LIMIT);
    let descendants = query("$..*", &at_limit);
    assert_eq!(descendants.len(), CONTEXT_DEPTH_LIMIT);
    assert_paired(&at_limit, &descendants);

    let over = nested_arrays(CONTEXT_DEPTH_LIMIT + 1);
    assert_eq!(
        query_error("$", &over),
        JsonPathError::ResourceLimit {
            resource: "context nesting depth",
            limit: CONTEXT_DEPTH_LIMIT
        }
    );
    let over_object = json!({"wrapper": nested_arrays(CONTEXT_DEPTH_LIMIT)});
    assert!(matches!(
        query_error("$.wrapper", &over_object),
        JsonPathError::ResourceLimit {
            resource: "context nesting depth",
            ..
        }
    ));

    // Siblings are counted independently: a wide document stays shallow.
    let wide = Value::Array((0..1000).map(|index| json!({"i": [index]})).collect());
    assert_eq!(query("$[*].i[0]", &wide).len(), 1000);
    // A scalar context has no containers at all.
    assert_eq!(query("$", &json!("scalar")).len(), 1);
}

/// The retained qualification reproducer: 4096 nested parentheses overflowed
/// the upstream parser's stack in a release binary before admission limits.
fn excessive_nesting_query() -> String {
    format!("$[?{}@{}]", "(".repeat(4096), ")".repeat(4096))
}

#[test]
fn excessive_nesting_query_is_rejected_by_admission_in_process() {
    assert_eq!(
        parse_error(&excessive_nesting_query(), None),
        JsonPathError::ResourceLimit {
            resource: "query structural characters",
            limit: QUERY_STRUCTURAL_LIMIT
        }
    );
}

const REPRODUCER_CHILD_ENV: &str = "ARAZZO_JSONPATH_REPRODUCER_CHILD";
const REPRODUCER_CHILD_TEST: &str = "excessive_nesting_reproducer_child";
const REPRODUCER_TIMEOUT: Duration = Duration::from_secs(60);

/// Child half of the bounded reproducer. It does nothing unless the parent
/// test spawned it, so it is inert under an ordinary test run.
#[test]
fn excessive_nesting_reproducer_child() {
    if std::env::var_os(REPRODUCER_CHILD_ENV).is_none() {
        return;
    }
    match JsonPathQuery::parse(&excessive_nesting_query(), None) {
        Err(JsonPathError::ResourceLimit { resource, limit }) => {
            println!("REPRODUCER_REJECTED resource={resource:?} limit={limit}");
        }
        Err(other) => println!("REPRODUCER_UNEXPECTED_ERROR {other:?}"),
        Ok(_) => println!("REPRODUCER_UNEXPECTED_PARSE"),
    }
}

fn drain_in_background<R: Read + Send + 'static>(reader: Option<R>) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buffer = String::new();
        if let Some(mut reader) = reader {
            let _ = reader.read_to_string(&mut buffer);
        }
        buffer
    })
}

fn join_output(handle: JoinHandle<String>) -> String {
    match handle.join() {
        Ok(output) => output,
        Err(_) => panic!("reader thread panicked"),
    }
}

#[test]
fn excessive_nesting_reproducer_is_rejected_in_a_bounded_child_process() {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => panic!("current_exe: {error}"),
    };
    let mut child = match Command::new(&executable)
        .args([
            "--exact",
            REPRODUCER_CHILD_TEST,
            "--nocapture",
            "--test-threads=1",
        ])
        .env(REPRODUCER_CHILD_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => panic!("spawning {} failed: {error}", executable.display()),
    };
    let stdout = drain_in_background(child.stdout.take());
    let stderr = drain_in_background(child.stderr.take());
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < REPRODUCER_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("reproducer child exceeded {REPRODUCER_TIMEOUT:?} and was killed");
            }
            Err(error) => panic!("waiting for the reproducer child failed: {error}"),
        }
    };
    let stdout = join_output(stdout);
    let stderr = join_output(stderr);
    println!(
        "reproducer child exit status: {status}; elapsed {:?}",
        started.elapsed()
    );
    // With --nocapture the harness prints the test name and the test's own
    // output on one line, so the marker can appear anywhere in a line.
    for line in stdout.lines().filter(|line| line.contains("REPRODUCER_")) {
        println!("reproducer child reported: {line}");
    }
    assert!(
        status.success(),
        "reproducer child exited with {status}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&format!(
            "REPRODUCER_REJECTED resource=\"query structural characters\" limit={QUERY_STRUCTURAL_LIMIT}"
        )),
        "child stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("overflowed its stack"),
        "child stderr:\n{stderr}"
    );
}
