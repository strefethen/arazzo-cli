    /// Builds a one-step document with the given Arazzo version, workflow-level
    /// parameters, and step-level parameters, rendered as YAML so the whole
    /// parse-then-validate path is exercised.
    fn querystring_doc(version: &str, workflow_params: &str, step_params: &str) -> String {
        format!(
            r#"arazzo: "{version}"
info:
  title: Querystring
  version: "1.0.0"
sourceDescriptions:
  - name: search
    url: https://search.example.com/v1
    type: openapi
workflows:
  - workflowId: wf1
    parameters:{workflow_params}
    steps:
      - stepId: s1
        operationPath: /index
        parameters:{step_params}
"#
        )
    }

    const NO_PARAMS: &str = " []";
    const STEP_QUERYSTRING: &str =
        "\n          - name: filter\n            in: querystring\n            value: q=red\n";
    const WORKFLOW_QUERY: &str = "\n      - name: q\n        in: query\n        value: inherited\n";
    const WORKFLOW_QUERYSTRING: &str =
        "\n      - name: inherited\n        in: querystring\n        value: a=1\n";

    /// Parameter Object: the `querystring` location cannot coexist with `query`
    /// parameters in the same operation. The two arrive from different levels
    /// here, which `merge_workflow_params` would otherwise combine into one
    /// request without complaint.
    #[test]
    fn inherited_query_conflicts_with_step_querystring() {
        let yaml = querystring_doc("1.1.0", WORKFLOW_QUERY, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("expected a querystring/query conflict to fail validation");
        };
        let conflicts = report
            .errors
            .iter()
            .filter(|item| item.kind == ValidationErrorKind::InvalidParameterLocation)
            .collect::<Vec<_>>();
        assert_eq!(conflicts.len(), 1, "errors={:?}", report.errors);
        assert!(
            conflicts[0].message.contains("\"filter\"") && conflicts[0].message.contains("\"q\""),
            "both parameters must be named: {}",
            conflicts[0].message
        );
        assert_eq!(
            conflicts[0].path,
            "workflow \"wf1\" > step \"s1\".parameters"
        );
    }

    /// The same conflict declared entirely at the step level.
    #[test]
    fn step_level_query_conflicts_with_step_querystring() {
        let step_params = format!(
            "{STEP_QUERYSTRING}          - name: q\n            in: query\n            value: red\n"
        );
        let yaml = querystring_doc("1.1.0", NO_PARAMS, &step_params);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("expected a querystring/query conflict to fail validation");
        };
        assert!(report
            .errors
            .iter()
            .any(|item| item.kind == ValidationErrorKind::InvalidParameterLocation));
    }

    /// A step targeting another workflow does not inherit workflow parameters,
    /// so there is no operation for the two locations to collide in.
    #[test]
    fn workflow_target_step_does_not_inherit_the_conflict() {
        let yaml = format!(
            r#"arazzo: "1.1.0"
info:
  title: Querystring
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:{WORKFLOW_QUERY}
    steps:
      - stepId: s1
        workflowId: wf2
        parameters:
          - name: workflow-input
            value: accepted
  - workflowId: wf2
    steps:
      - stepId: s2
        operationPath: /index
"#
        );
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected no conflict, got: {err}"),
        }
    }

    /// Every `InvalidParameterLocation` diagnostic the document produced.
    fn location_conflicts(spec: &ArazzoSpec) -> Vec<Diagnostic> {
        let Err(Error::Validation(report)) = validate_diagnostics(spec) else {
            panic!("expected the document to fail validation");
        };
        report
            .errors
            .into_iter()
            .filter(|item| item.kind == ValidationErrorKind::InvalidParameterLocation)
            .collect()
    }

    /// OpenAPI 3.2.0 Parameter Object: `querystring` "MUST NOT appear more than
    /// once". `merge_workflow_params` keys on `(name, in)`, so a workflow-level
    /// and a step-level `querystring` with *different* names do not collapse —
    /// both reach the same operation, and the last one silently wins.
    #[test]
    fn inherited_querystring_conflicts_with_step_querystring() {
        let yaml = querystring_doc("1.1.0", WORKFLOW_QUERYSTRING, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let conflicts = location_conflicts(&spec);
        assert_eq!(conflicts.len(), 1, "conflicts={conflicts:?}");
        assert_eq!(conflicts[0].severity, Severity::Error);
        assert!(
            conflicts[0].message.contains("\"inherited\"")
                && conflicts[0].message.contains("\"filter\""),
            "both parameters must be named: {}",
            conflicts[0].message
        );
        assert_eq!(
            conflicts[0].path,
            "workflow \"wf1\" > step \"s1\".parameters"
        );
    }

    /// The same duplication declared entirely at the step level.
    #[test]
    fn two_step_level_querystrings_conflict() {
        let step_params = format!(
            "{STEP_QUERYSTRING}          - name: extra\n            in: querystring\n            \
             value: b=2\n"
        );
        let yaml = querystring_doc("1.1.0", NO_PARAMS, &step_params);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let conflicts = location_conflicts(&spec);
        assert_eq!(conflicts.len(), 1, "conflicts={conflicts:?}");
        assert!(
            conflicts[0].message.contains("\"filter\"")
                && conflicts[0].message.contains("\"extra\""),
            "both parameters must be named: {}",
            conflicts[0].message
        );
    }

    /// The step-overrides-workflow path: same `(name, in)` key, so
    /// `merge_workflow_params` collapses the two into the step-level one and a
    /// single `querystring` parameter reaches the operation.
    #[test]
    fn same_name_querystring_override_validates_clean() {
        let workflow_params =
            "\n      - name: filter\n        in: querystring\n        value: q=inherited\n";
        let yaml = querystring_doc("1.1.0", workflow_params, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("the override path must stay clean, got: {err}"),
        }
    }

    /// A step targeting another workflow does not inherit workflow parameters,
    /// so the workflow-level `querystring` never joins the step-level one.
    #[test]
    fn workflow_target_step_does_not_inherit_the_duplicate() {
        let yaml = format!(
            r#"arazzo: "1.1.0"
info:
  title: Querystring
  version: "1.0.0"
sourceDescriptions:
  - name: api
    url: https://example.com
    type: openapi
workflows:
  - workflowId: wf1
    parameters:{WORKFLOW_QUERYSTRING}
    steps:
      - stepId: s1
        workflowId: wf2
        parameters:
          - name: workflow-input
            value: accepted
  - workflowId: wf2
    steps:
      - stepId: s2
        operationPath: /index
"#
        );
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected no duplicate, got: {err}"),
        }
    }

    /// A single `querystring` parameter arriving by inheritance is still one
    /// parameter: the count is over the effective set, not the step's list.
    #[test]
    fn inherited_querystring_alone_validates_clean() {
        let yaml = querystring_doc("1.1.0", WORKFLOW_QUERYSTRING, NO_PARAMS);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected a clean 1.1.0 document, got: {err}"),
        }
    }

    /// `querystring` alone is valid in a 1.1.0 document, with no diagnostics.
    #[test]
    fn querystring_alone_validates_clean_at_1_1_0() {
        let yaml = querystring_doc("1.1.0", NO_PARAMS, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected a clean 1.1.0 document, got: {err}"),
        }
    }

    /// `querystring` was introduced in Arazzo 1.1.0, so a 1.0.x document using
    /// it is refused rather than executed: the `arazzo` field is what tooling
    /// interprets the document with, and a warning never reached `run`, which
    /// loads through the diagnostic-discarding `parse`.
    #[test]
    fn querystring_in_a_1_0_document_fails_validation() {
        let yaml = querystring_doc("1.0.1", NO_PARAMS, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("a 1.0.x document using querystring must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::UnsupportedVersion
        );
        assert_eq!(report.errors[0].severity, Severity::Error);
        assert_eq!(
            report.errors[0].path,
            "workflow \"wf1\" > step \"s1\".parameters[0].in"
        );
        assert!(
            report.errors[0].message.contains("1.1.0")
                && report.errors[0].message.contains("1.0.1"),
            "the error must name both versions: {}",
            report.errors[0].message
        );
        // Rejecting is only defensible because the fix is one line, so the
        // message has to carry it rather than leave the author to infer it.
        assert!(
            report.errors[0].message.contains("declare arazzo: 1.1.0"),
            "the error must state the remedy, not only the violation: {}",
            report.errors[0].message
        );
    }

    /// `1.0.0` gates identically to `1.0.1` — the rule reads major.minor, and
    /// the message names whichever version the document actually declared.
    #[test]
    fn querystring_in_a_1_0_0_document_fails_validation() {
        let yaml = querystring_doc("1.0.0", NO_PARAMS, STEP_QUERYSTRING);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        let Err(Error::Validation(report)) = validate_diagnostics(&spec) else {
            panic!("a 1.0.0 document using querystring must fail validation");
        };
        assert_eq!(report.errors.len(), 1, "errors={:?}", report.errors);
        assert_eq!(
            report.errors[0].kind,
            ValidationErrorKind::UnsupportedVersion
        );
        assert!(
            report.errors[0].message.contains("arazzo: 1.0.0"),
            "the error must name the declared version: {}",
            report.errors[0].message
        );
    }

    /// The gate keys on `querystring`, not on the declared version: a 1.0.x
    /// document that stays inside the 1.0 vocabulary is untouched.
    #[test]
    fn a_1_0_document_without_querystring_is_unaffected() {
        let step_query = "\n          - name: q\n            in: query\n            value: red\n";
        let yaml = querystring_doc("1.0.1", NO_PARAMS, step_query);
        let spec = match arazzo_spec::parse_unvalidated_bytes(yaml.as_bytes()) {
            Ok(spec) => spec,
            Err(err) => panic!("parsing test spec: {err}"),
        };

        match validate_diagnostics(&spec) {
            Ok(warnings) => assert!(warnings.is_empty(), "warnings={warnings:?}"),
            Err(err) => panic!("expected a clean 1.0.1 document, got: {err}"),
        }
    }

    /// The version gate reads the declared major.minor, not a string prefix:
    /// a hypothetical 1.10.0 is not 1.0.x.
    #[test]
    fn version_gate_reads_major_minor_not_a_prefix() {
        use super::declares_pre_1_1;

        assert!(declares_pre_1_1("1.0.0"));
        assert!(declares_pre_1_1("1.0.1"));
        assert!(declares_pre_1_1("1.0"));
        assert!(!declares_pre_1_1("1.1.0"));
        assert!(!declares_pre_1_1("1.10.0"));
        assert!(!declares_pre_1_1("2.0.0"));
    }

