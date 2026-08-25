//! Spec-surface coverage for the `unsupportedXpathVersion` advisory.
//!
//! The Arazzo v1.1.0 §5.8.12.1 table allows XPath versions `xpath-31`,
//! `xpath-30`, `xpath-20`, and `xpath-10`, with `xpath-31` the
//! omitted-version default ("If this object is not defined, the default
//! version for the selector type MUST be used"). The runtime executes only
//! an explicit `xpath-10` (ac-46638) and rejects every other declared or
//! omitted version before evaluation, so validation flags each such
//! declaration with a warning naming that rejection and the `xpath-10`
//! remedy. The declarations stay schema-valid: warnings never fail
//! validation on their own.

use arazzo_validate::{parse_bytes_with_diagnostics, Diagnostic, Severity, ValidationErrorKind};

/// Parses a document that must be schema-valid and splits its warnings into
/// the XPath advisories and everything else.
fn advisories(yaml: &str) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
    match parse_bytes_with_diagnostics(yaml.as_bytes()) {
        Ok((_, warnings)) => warnings
            .into_iter()
            .partition(|w| w.kind == ValidationErrorKind::UnsupportedXpathVersion),
        Err(error) => panic!("expected the document to stay schema-valid, got: {error}"),
    }
}

fn criterion_doc(arazzo: &str, type_yaml: &str) -> String {
    format!(
        r#"arazzo: {arazzo}
info:
  title: XPath advisory fixture
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://api.example.com/openapi.yaml
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationId: doThing
        successCriteria:
          - context: $response.body
            condition: //ok
            type: {type_yaml}
"#
    )
}

#[test]
fn criterion_name_form_warns_with_the_1_1_omitted_version_default() {
    let (advisories, others) = advisories(&criterion_doc("1.1.0", "xpath"));
    assert_eq!(others, vec![], "unrelated warnings must not appear");
    assert_eq!(advisories.len(), 1, "{advisories:?}");

    let warning = &advisories[0];
    assert_eq!(warning.severity, Severity::Warning);
    assert_eq!(warning.kind.name(), "unsupportedXpathVersion");
    assert_eq!(
        warning.path,
        "workflow \"wf\" > step \"s1\".successCriteria[0].type"
    );
    for expected in [
        "without a version",
        "\"xpath-31\"",
        "XML Path Language 3.1",
        "rejects the omitted form before evaluation",
        "Expression Type Object",
        "\"xpath-10\"",
        "XPath 1.0 engine",
    ] {
        assert!(
            warning.message.contains(expected),
            "missing {expected:?} in: {}",
            warning.message
        );
    }
}

#[test]
fn criterion_name_form_words_the_pre_1_1_default_without_the_xpath_31_token() {
    // Arazzo 1.0.x also defaults the omitted version to XML Path Language 3.1
    // (§4.6.11.3, §4.6.12) but has no `xpath-31` token — its §4.6.12.1 allows
    // only xpath-30 | xpath-20 | xpath-10 — and names the object the
    // "Criterion Expression Type Object".
    let (advisories, others) = advisories(&criterion_doc("1.0.1", "xpath"));
    assert_eq!(others, vec![], "unrelated warnings must not appear");
    assert_eq!(advisories.len(), 1, "{advisories:?}");

    let message = &advisories[0].message;
    assert!(
        !message.contains("xpath-31"),
        "a 1.0.x advisory must not cite the 1.1-only token: {message}"
    );
    for expected in [
        "arazzo 1.0.1",
        "XML Path Language 3.1",
        "rejects it before evaluation",
        "Criterion Expression Type Object",
        "\"xpath-10\"",
    ] {
        assert!(
            message.contains(expected),
            "missing {expected:?} in: {message}"
        );
    }
}

#[test]
fn criterion_declared_non_10_versions_warn_with_the_declared_token() {
    for version in ["xpath-20", "xpath-30", "xpath-31"] {
        let type_yaml = format!("{{type: xpath, version: {version}}}");
        let (advisories, others) = advisories(&criterion_doc("1.1.0", &type_yaml));
        assert_eq!(others, vec![], "unrelated warnings must not appear");
        assert_eq!(advisories.len(), 1, "{version}: {advisories:?}");

        let warning = &advisories[0];
        assert_eq!(
            warning.path,
            "workflow \"wf\" > step \"s1\".successCriteria[0].type.version"
        );
        for expected in [
            &format!("\"{version}\""),
            "valid Arazzo metadata",
            "rejects it before evaluation",
            "declare version \"xpath-10\"",
            "XPath 1.0 engine",
        ] {
            assert!(
                warning.message.contains(expected),
                "{version}: missing {expected:?} in: {}",
                warning.message
            );
        }
    }
}

/// Selector Objects and `targetSelectorType` have no Arazzo 1.0.x
/// vocabulary — the constructs arrived in 1.1 — so a pre-1.1 advisory at
/// those sites names no versioning object (the "Criterion Expression Type
/// Object" applies only to criteria) and claims no spec default, while
/// still naming the rejection and the `xpath-10` remedy.
#[test]
fn pre_1_1_selector_sites_name_no_versioning_object() {
    let yaml = r#"arazzo: 1.0.1
info:
  title: Pre-1.1 selector advisory
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://api.example.com/openapi.yaml
    type: openapi
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        operationId: doThing
        requestBody:
          contentType: application/xml
          replacements:
            - target: //id
              targetSelectorType: xpath
              value: '42'
        successCriteria:
          - condition: $statusCode == 200
        outputs:
          node:
            context: $response.body
            selector: //node
            type: xpath
"#;

    let (advisories, others) = advisories(yaml);
    assert_eq!(others, vec![], "unrelated warnings must not appear");
    assert_eq!(advisories.len(), 2, "{advisories:?}");

    for warning in &advisories {
        let message = &warning.message;
        // "Criterion Expression Type Object" contains "Expression Type
        // Object", so this single assertion excludes both object names.
        assert!(
            !message.contains("Expression Type Object"),
            "a pre-1.1 selector-site advisory must name no versioning object: {message}"
        );
        assert!(
            !message.contains("xpath-31") && !message.contains("XML Path Language 3.1"),
            "a pre-1.1 selector-site advisory must claim no spec default: {message}"
        );
        for expected in [
            "without a version",
            "rejects the omitted form before evaluation",
            "declare version \"xpath-10\"",
            "XPath 1.0 engine",
        ] {
            assert!(
                message.contains(expected),
                "missing {expected:?} in: {message}"
            );
        }
    }
}

/// Every declaration site funnels through the same advisory: step and
/// workflow outputs (Selector Objects), step parameter values, request-body
/// payload selectors, replacement `targetSelectorType` in both forms,
/// success criteria, and action criteria.
#[test]
fn every_xpath_declaration_site_reports_the_advisory() {
    let yaml = r#"arazzo: 1.1.0
info:
  title: XPath advisory sites
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://api.example.com/openapi.yaml
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        doc:
          type: string
    steps:
      - stepId: s1
        operationId: doThing
        parameters:
          - name: probe
            in: query
            value:
              context: $inputs.doc
              selector: //probe
              type: {type: xpath, version: xpath-31}
        requestBody:
          contentType: application/xml
          payload:
            context: $inputs.doc
            selector: //payload
            type: xpath
          replacements:
            - target: //id
              targetSelectorType: {type: xpath, version: xpath-20}
              value: '42'
            - target: //name
              targetSelectorType: xpath
              value: renamed
        successCriteria:
          - context: $response.body
            condition: //ok
            type: {type: xpath, version: xpath-30}
        onFailure:
          - name: retryIt
            type: retry
            retryAfter: 1
            retryLimit: 2
            criteria:
              - context: $response.body
                condition: //err
                type: xpath
        outputs:
          node:
            context: $response.body
            selector: //node
            type: xpath
    outputs:
      agg:
        context: $steps.s1.outputs.node
        selector: //agg
        type: {type: xpath, version: xpath-31}
"#;

    let (advisories, others) = advisories(yaml);
    assert_eq!(others, vec![], "unrelated warnings must not appear");

    let mut paths: Vec<&str> = advisories.iter().map(|w| w.path.as_str()).collect();
    paths.sort_unstable();
    let mut expected = vec![
        "workflow \"wf\" > step \"s1\".parameters[0].value.type.version",
        "workflow \"wf\" > step \"s1\".requestBody.payload.type",
        "workflow \"wf\" > step \"s1\".requestBody.replacements[0].targetSelectorType.version",
        "workflow \"wf\" > step \"s1\".requestBody.replacements[1].targetSelectorType",
        "workflow \"wf\" > step \"s1\".successCriteria[0].type.version",
        "workflow \"wf\" > step \"s1\".onFailure[0].criteria[0].type",
        "workflow \"wf\" > step \"s1\".outputs.node.type",
        "workflow \"wf\".outputs.agg.type.version",
    ];
    expected.sort_unstable();
    assert_eq!(paths, expected);
}

/// The one executable declaration — an explicit `xpath-10` — and every
/// non-XPath declaration stay advisory-free at every site.
#[test]
fn xpath_10_and_non_xpath_declarations_stay_clean() {
    let yaml = r#"arazzo: 1.1.0
info:
  title: XPath advisory negatives
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://api.example.com/openapi.yaml
    type: openapi
workflows:
  - workflowId: wf
    inputs:
      type: object
      properties:
        doc:
          type: string
    steps:
      - stepId: s1
        operationId: doThing
        requestBody:
          contentType: application/xml
          payload:
            context: $inputs.doc
            selector: //payload
            type: {type: xpath, version: xpath-10}
          replacements:
            - target: //id
              targetSelectorType: {type: xpath, version: xpath-10}
              value: '42'
        successCriteria:
          - condition: $statusCode == 200
          - context: $response.body
            condition: //ok
            type: {type: xpath, version: xpath-10}
          - context: $response.body
            condition: $.ok
            type: jsonpath
        outputs:
          node:
            context: $response.body
            selector: //node
            type: {type: xpath, version: xpath-10}
          pointer:
            context: $response.body
            selector: /items/0
            type: {type: jsonpointer, version: rfc6901}
"#;

    match parse_bytes_with_diagnostics(yaml.as_bytes()) {
        Ok((_, warnings)) => assert_eq!(warnings, vec![], "expected no warnings"),
        Err(error) => panic!("expected the document to validate, got: {error}"),
    }
}

/// A version token outside the §5.8.12.1 table keeps its existing error and
/// gains no advisory: the advisory covers only schema-valid declarations.
#[test]
fn schema_invalid_versions_keep_their_error_without_an_advisory() {
    let yaml = criterion_doc("1.1.0", "{type: xpath, version: xpath-99}");
    let report = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
        Err(arazzo_validate::Error::Validation(report)) => report,
        Ok(_) => panic!("expected xpath-99 to fail validation"),
        Err(error) => panic!("expected a validation report, got: {error}"),
    };

    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert_eq!(
        report.errors[0].kind,
        ValidationErrorKind::InvalidCriterionType
    );
    assert!(
        report.errors[0]
            .message
            .contains("\"xpath-99\" is not supported for xpath"),
        "{}",
        report.errors[0].message
    );
    assert!(
        report
            .warnings
            .iter()
            .all(|w| w.kind != ValidationErrorKind::UnsupportedXpathVersion),
        "an invalid token must not also produce the advisory: {:?}",
        report.warnings
    );
}
