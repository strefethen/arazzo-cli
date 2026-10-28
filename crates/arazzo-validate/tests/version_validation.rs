#![forbid(unsafe_code)]

//! Public parsed YAML/JSON and direct-model version-policy regression tests.

use arazzo_spec::{parse_unvalidated_bytes, ArazzoSpec};
use arazzo_validate::{
    parse_bytes_with_diagnostics, validate_diagnostics, Diagnostic, Error, Severity,
    ValidationErrorKind,
};

const BODY: (&str, &str) = (
    r#"
info: {title: Version policy, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: wf
    steps: [{stepId: request, operationId: fixturerequest}]
"#,
    r#"
"info": {"title": "Version policy", "version": "1.0.0"},
"sourceDescriptions": [{"name": "api", "url": "https://example.com", "type": "openapi"}],
"workflows": [{"workflowId": "wf", "steps": [{"stepId": "request", "operationId": "fixturerequest"}]}]
"#,
);

fn document(version: Option<&str>, body: (&str, &str), json: bool) -> String {
    // JSON string quoting also works for YAML double-quoted scalar values.
    let declaration = version.map_or_else(String::new, |version| {
        if json {
            format!("\"arazzo\":{},", quoted(version))
        } else {
            format!("arazzo: {}\n", quoted(version))
        }
    });
    if json {
        format!("{{{declaration}{}}}", body.1)
    } else {
        format!("{declaration}{}", body.0)
    }
}

fn quoted(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
            .replace('\r', "\\r")
    )
}

fn error(kind: ValidationErrorKind, path: &str, message: String) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        kind,
        path: path.to_string(),
        message,
    }
}

fn version_error(version: &str, malformed: bool) -> Diagnostic {
    let message = if malformed {
        format!(
            "invalid arazzo version: {version} (expected major.minor.patch with decimal components)"
        )
    } else {
        format!("unsupported arazzo version: {version} (supported feature sets: 1.0, 1.1)")
    };
    error(ValidationErrorKind::UnsupportedVersion, "arazzo", message)
}

fn assert_outcome<T: std::fmt::Debug>(result: Result<T, Error>, expected: &[Diagnostic]) {
    match result {
        Ok(_) => assert!(expected.is_empty(), "expected {expected:?}"),
        Err(Error::Validation(report)) => {
            assert!(!expected.is_empty(), "unexpected {report:?}");
            assert_eq!(report.errors, expected);
            assert!(report.warnings.is_empty(), "{report:?}");
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}

fn assert_contract(version: Option<&str>, body: (&str, &str), expected: &[Diagnostic]) {
    // Build a direct model independently of the version's wire representation.
    let mut direct: ArazzoSpec = match parse_unvalidated_bytes(body.0.as_bytes()) {
        Ok(spec) => spec,
        Err(err) => panic!("parsing fixture model: {err}"),
    };
    direct.arazzo = version.unwrap_or_default().to_string();
    let original = direct.clone();
    let result = validate_diagnostics(&direct);
    if let Ok(warnings) = &result {
        assert!(warnings.is_empty(), "{warnings:?}");
    }
    assert_outcome(result, expected);
    assert_eq!(
        direct, original,
        "direct validation must not mutate the model"
    );

    for json in [false, true] {
        let wire = document(version, body, json);
        let unvalidated = match parse_unvalidated_bytes(wire.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing fixture wire document: {err}"),
        };
        assert_eq!(unvalidated.arazzo, version.unwrap_or_default());
        let result = parse_bytes_with_diagnostics(wire.as_bytes());
        if let Ok((parsed, warnings)) = &result {
            assert_eq!(parsed.arazzo, version.unwrap_or_default());
            assert!(warnings.is_empty(), "{warnings:?}");
        }
        assert_outcome(result, expected);
    }
}

/// Stable positive adapter for model.version-grammar-feature-set.
#[test]
fn supported_versions_preserve_decimal_patch_and_declared_text() {
    for version in [
        "1.0.0",
        "1.0.27",
        "1.1.0",
        "1.1.27",
        "1.0.1844674407370955161600000000000000000000000000000000",
        "1.1.1844674407370955161600000000000000000000000000000000",
        "01.00.00027",
        "00000000000000000000000000000000001.00001.00000000000000000000000000027",
    ] {
        assert_contract(Some(version), BODY, &[]);
    }
}

/// Stable negative adapter for model.version-grammar-feature-set.
#[test]
fn malformed_and_unsupported_versions_report_exact_diagnostics() {
    for version in [
        "1",
        "1.0",
        "1.1",
        ".1.0",
        "1..0",
        "1.0.",
        ".",
        "1.foo.0",
        "v1.1.0",
        "+1.1.0",
        "1.+1.0",
        "1.1.+0",
        "-1.1.0",
        "1.-1.0",
        "1.1.-0",
        "1.1.0-rc.1",
        "1.1.0+build",
        "1.1.0.0",
        " 1.1.0",
        "1.1.0 ",
        "1. 1.0",
        "1.1.0\n",
        "\t1.1.0",
        "１.1.0",
        "1.١.0",
        "1.1.０",
    ] {
        assert_contract(Some(version), BODY, &[version_error(version, true)]);
    }
    for version in [
        "0.0.0",
        "1.2.0",
        "1.10.0",
        "2.0.0",
        "01.002.000",
        "18446744073709551617.0.0",
        "1.18446744073709551616.0",
        "999999999999999999999999999999999999999.1.0",
    ] {
        assert_contract(Some(version), BODY, &[version_error(version, false)]);
    }
}

#[test]
fn missing_and_empty_versions_keep_the_required_field_error() {
    for version in [None, Some("")] {
        assert_contract(
            version,
            BODY,
            &[error(
                ValidationErrorKind::MissingRequiredField,
                "arazzo",
                "arazzo version is required".to_string(),
            )],
        );
    }
}

const VOCABULARY_BODY: (&str, &str) = (
    r#"
info: {title: Version policy, version: "1.0.0"}
sourceDescriptions: [{name: api, url: https://example.com, type: openapi}]
workflows:
  - workflowId: wf
    steps:
      - stepId: request
        operationId: fixturerequest
        parameters: [{name: filter, in: querystring, value: "q=red"}]
        onSuccess:
          - name: success
            type: goto
            workflowId: target
            parameters: [{name: reason, value: success}]
        onFailure:
          - name: failure
            type: goto
            workflowId: target
            parameters: [{name: reason, value: failure}]
  - workflowId: target
    steps: [{stepId: fallback, operationId: fixturefallback}]
"#,
    r#"
"info": {"title": "Version policy", "version": "1.0.0"},
"sourceDescriptions": [{"name": "api", "url": "https://example.com", "type": "openapi"}],
"workflows": [
  {"workflowId": "wf", "steps": [{
    "stepId": "request", "operationId": "fixturerequest",
    "parameters": [{"name": "filter", "in": "querystring", "value": "q=red"}],
    "onSuccess": [{"name": "success", "type": "goto", "workflowId": "target", "parameters": [{"name": "reason", "value": "success"}]}],
    "onFailure": [{"name": "failure", "type": "goto", "workflowId": "target", "parameters": [{"name": "reason", "value": "failure"}]}]
  }]},
  {"workflowId": "target", "steps": [{"stepId": "fallback", "operationId": "fixturefallback"}]}
]
"#,
);

#[test]
fn querystring_and_action_parameters_follow_the_same_feature_set() {
    for version in ["1.0.0", "1.0.27", "01.00.00027", "1.0.18446744073709551616"] {
        let path = "workflow \"wf\" > step \"request\".parameters[0].in";
        let mut expected = vec![error(
            ValidationErrorKind::UnsupportedVersion,
            path,
            format!(
                "{path} \"querystring\" was introduced in Arazzo 1.1.0, but this \
                 document declares arazzo: {version}, whose vocabulary has no such \
                 parameter location; declare arazzo: 1.1.0 to use it"
            ),
        )];
        for action in ["onFailure", "onSuccess"] {
            let path = format!("workflow \"wf\" > step \"request\".{action}[0].parameters");
            expected.push(error(
                ValidationErrorKind::UnsupportedVersion,
                &path,
                format!(
                    "{path} — success/failure action parameters were introduced in \
                     Arazzo 1.1.0, but this document declares arazzo: {version}, whose \
                     vocabulary has no such field; declare arazzo: 1.1.0 to use them"
                ),
            ));
        }
        assert_contract(Some(version), VOCABULARY_BODY, &expected);
    }
    for version in ["1.1.0", "1.1.27", "01.01.00027", "1.1.18446744073709551616"] {
        assert_contract(Some(version), VOCABULARY_BODY, &[]);
    }
}

#[test]
fn invalid_versions_do_not_select_a_vocabulary_from_their_prefix() {
    for version in ["1.0", "1.0.bad", " 1.0.0", "1.0.0 "] {
        assert_contract(
            Some(version),
            VOCABULARY_BODY,
            &[version_error(version, true)],
        );
    }
    for version in ["1.2.0", "2.0.0"] {
        assert_contract(
            Some(version),
            VOCABULARY_BODY,
            &[version_error(version, false)],
        );
    }
}

#[test]
fn version_error_keeps_its_order_among_unrelated_errors() {
    let yaml = BODY.0.replace("title: Version policy", "title: \"\"");
    let json = BODY
        .1
        .replace("\"title\": \"Version policy\"", "\"title\": \"\"");
    assert_contract(
        Some("1.bad.0"),
        (&yaml, &json),
        &[
            version_error("1.bad.0", true),
            error(
                ValidationErrorKind::MissingRequiredField,
                "info.title",
                "info.title is required".to_string(),
            ),
        ],
    );
}
