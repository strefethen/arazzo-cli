//! Public parsed/direct field-mode contract. All checks stop at validation;
//! neither entry point constructs a runtime or can issue an HTTP request.

#![allow(clippy::unwrap_used, clippy::expect_used)] // Fixture and assertion failures are test failures.

use arazzo_spec::ArazzoSpec;
use arazzo_validate::{
    parse_bytes_with_diagnostics, validate_diagnostics, Diagnostic, Error, Severity,
    ValidationErrorKind,
};

const ENV_MESSAGE: &str =
    "unsupported runtime-expression namespace '$env'; pass the value explicitly through '$inputs'";
const STEP: &str = "workflow \"wf\" > step \"s\"";

fn document(step: &str, extra: &str) -> String {
    format!("arazzo: 1.1.0\ninfo: {{title: Test, version: '1'}}\nsourceDescriptions:\n  - {{name: api, url: https://example.invalid/openapi.yaml, type: openapi}}\n{extra}\nworkflows:\n  - workflowId: wf\n    steps:\n      - stepId: s\n        operationId: doThing\n{step}\n")
}

fn diagnostics(result: Result<impl Sized, Error>) -> Vec<Diagnostic> {
    match result {
        Ok(_) => Vec::new(),
        Err(Error::Validation(report)) => report.errors,
        Err(error) => panic!("expected structured validation: {error}"),
    }
}

fn both(yaml: &str) -> Vec<Diagnostic> {
    let parsed = diagnostics(parse_bytes_with_diagnostics(yaml.as_bytes()));
    let direct: ArazzoSpec = serde_yaml_ng::from_str(yaml).expect("typed document");
    let direct = diagnostics(validate_diagnostics(&direct));
    assert_eq!(parsed, direct, "parsed/direct parity\n{yaml}");
    for diagnostic in &parsed {
        assert!(!diagnostic.message.contains("HOST_SENTINEL_SECRET"));
    }
    parsed
}

fn expression_errors(yaml: &str) -> Vec<Diagnostic> {
    both(yaml)
        .into_iter()
        .filter(|d| d.kind == ValidationErrorKind::InvalidExpression)
        .collect()
}

fn one(yaml: &str, path: &str) -> Diagnostic {
    let errors = expression_errors(yaml);
    assert_eq!(errors.len(), 1, "{yaml}\n{errors:?}");
    assert_eq!(errors[0].path, path);
    assert_eq!(errors[0].severity, Severity::Error);
    assert_eq!(errors[0].kind.name(), "invalidExpression");
    errors.into_iter().next().unwrap()
}

#[test]
fn literal_values_classify_recursively_and_preserve_dollar_text() {
    for value in [
        "$env.HOST_SENTINEL_SECRET",
        "$environment",
        "$USD",
        "$inputs",
        "pay $inputs.x",
        "$response.body.status",
        "$response.body.items[0].id",
        "{name}",
        "a}b",
        "$inputs.x",
        "hello {$inputs.x}",
    ] {
        let quoted = format!("'{}'", value.replace('\'', "''"));
        for field in [
            format!("        parameters:\n          - {{name: p, in: query, value: {quoted}}}"),
            format!("        requestBody:\n          payload: {quoted}"),
            format!("        requestBody:\n          replacements:\n            - {{target: /id, value: {quoted}}}"),
        ] {
            assert!(expression_errors(&document(&field, "")).is_empty(), "{value}");
        }
    }
    let yaml = document("        requestBody:\n          payload:\n            outer: [safe, {inner: '{$env.HOST_SENTINEL_SECRET}'}]", "");
    assert_eq!(
        one(&yaml, &format!("{STEP}.requestBody.payload.outer[1].inner")).message,
        ENV_MESSAGE
    );
}

#[test]
fn required_outputs_and_contexts_reject_nonexpressions_once() {
    for value in [
        "literal",
        "$env.HOST_SENTINEL_SECRET",
        "$environment",
        "$inputs",
        "{$inputs.x}",
        "$inputs.x tail",
        "$sourceDescriptions.api.ref}tail",
    ] {
        for (field, path) in [
            (format!("        outputs: {{value: '{value}'}}"), format!("{STEP}.outputs.value")),
            (format!("        outputs:\n          value: {{context: '{value}', selector: /id, type: jsonpointer}}"), format!("{STEP}.outputs.value.context")),
            (format!("        successCriteria:\n          - {{context: '{value}', condition: '^ok$', type: regex}}"), format!("{STEP}.successCriteria[0].context")),
        ] {
            let error = one(&document(&field, ""), &path);
            if value.starts_with("$env.") { assert_eq!(error.message, ENV_MESSAGE); }
            else { assert!(error.message.starts_with("invalid runtime expression: ")); }
        }
    }
    let yaml = document("    outputs: {value: '$env.HOST_SENTINEL_SECRET'}", "");
    assert_eq!(
        one(&yaml, "workflow \"wf\".outputs.value").message,
        ENV_MESSAGE
    );
}

#[test]
fn canonical_body_pointer_matrix_and_simple_postfix_controls() {
    for base in [
        "$request.body",
        "$response.body",
        "$message.payload",
        "$REQUEST.BODY",
        "$Response.Body",
        "$MESSAGE.PAYLOAD",
    ] {
        for suffix in [
            ".status",
            ".a.b",
            "[0]",
            ".items[0].id",
            ".*",
            ".items.#(id==7)",
        ] {
            let value = format!("{base}{suffix}");
            let expected_error = arazzo_expr::parse_runtime_expression(&value).unwrap_err();
            let hint = arazzo_expr::body_pointer_migration_hint(&value, &expected_error).unwrap();
            let expected = format!("invalid runtime expression: {expected_error}; {hint}");
            for (field, path) in [
                (format!("        outputs: {{value: '{value}'}}"), format!("{STEP}.outputs.value")),
                (format!("        outputs:\n          value: {{context: '{value}', selector: /id, type: jsonpointer}}"), format!("{STEP}.outputs.value.context")),
                (format!("        successCriteria:\n          - {{context: '{value}', condition: '^ok$', type: regex}}"), format!("{STEP}.successCriteria[0].context")),
                (format!("    outputs: {{value: '{value}'}}"), "workflow \"wf\".outputs.value".to_owned()),
                (format!("    outputs:\n      value: {{context: '{value}', selector: /id, type: jsonpointer}}"), "workflow \"wf\".outputs.value.context".to_owned()),
            ] { assert_eq!(one(&document(&field, ""), &path).message, expected); }
        }
        for suffix in ["", "#/status", "#/a.b", "#/a/b", "#/a~0b", "#/a~1b"] {
            assert!(expression_errors(&document(
                &format!("        outputs: {{value: '{base}{suffix}'}}"),
                ""
            ))
            .is_empty());
            assert!(expression_errors(&document(&format!("        parameters:\n          - {{name: p, in: query, value: '{{{base}{suffix}}}'}}"), "")).is_empty());
        }
        one(&document(&format!("        parameters:\n          - {{name: p, in: query, value: '{{{base}.status}}'}}"), ""), &format!("{STEP}.parameters[0].value"));
        assert!(expression_errors(&document(
            &format!("        successCriteria:\n          - condition: '{base}.items[0].id == 7'"),
            ""
        ))
        .is_empty());
    }
}

#[test]
fn conditions_keep_native_dollars_and_reject_reserved_candidates() {
    for type_ in ["regex", "jsonpath", "xpath"] {
        for condition in [
            "$env.HOST_SENTINEL_SECRET",
            "$response.body.status",
            "$[?(@.id == 7)]",
            "^text$",
            "{$inputs.x}",
            "a{b}c",
        ] {
            let yaml = document(&format!("        successCriteria:\n          - {{context: '$response.body', type: {type_}, condition: '{condition}'}}"), "");
            assert!(expression_errors(&yaml).is_empty(), "{yaml}");
        }
        for condition in [
            "{$env.HOST_SENTINEL_SECRET}",
            "{$inputs.x} then {$env.X}",
            "{$inputs.x",
            "{$}",
            "{$inputs.x} {$response.body.status}",
        ] {
            let yaml = document(&format!("        successCriteria:\n          - {{context: '$response.body', type: {type_}, condition: '{condition}'}}"), "");
            let error = one(&yaml, &format!("{STEP}.successCriteria[0].condition"));
            if condition.contains("{$env.") {
                assert_eq!(error.message, ENV_MESSAGE);
            }
        }
    }
    for condition in [
        "$env.HOST_SENTINEL_SECRET == 1",
        "$statusCode == $env.X",
        "true && $env.X == 1",
        "($env.X == 1)",
        "$environment == 1",
        "$inputs == 1",
        "true true",
    ] {
        let error = one(
            &document(
                &format!("        successCriteria:\n          - condition: '{condition}'"),
                "",
            ),
            &format!("{STEP}.successCriteria[0].condition"),
        );
        if condition.contains("$env.") {
            assert_eq!(error.message, ENV_MESSAGE, "{condition}");
        }
    }
    for condition in [
        "'$env.HOST_SENTINEL_SECRET' == '$env.HOST_SENTINEL_SECRET'",
        "$response.body.items[0].id == 7",
        "$inputs.x == null",
    ] {
        let quoted = serde_yaml_ng::to_string(condition).unwrap();
        assert!(expression_errors(&document(
            &format!("        successCriteria:\n          - condition: {quoted}"),
            ""
        ))
        .is_empty());
    }
}

#[test]
fn malformed_templates_fail_once_at_each_owning_field_in_append_order() {
    let yaml = document("        parameters:\n          - {name: p, in: query, value: '{$env.X} {$env.Y}'}\n        outputs: {value: '$env.Z'}\n        requestBody: {payload: '{$env.X}'}\n        successCriteria:\n          - condition: '$env.X == 1'", "");
    let errors = expression_errors(&yaml);
    assert_eq!(
        errors
            .iter()
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>(),
        [
            format!("{STEP}.parameters[0].value"),
            format!("{STEP}.outputs.value"),
            format!("{STEP}.requestBody.payload"),
            format!("{STEP}.successCriteria[0].condition")
        ]
    );
    assert!(errors.iter().all(|error| error.message == ENV_MESSAGE));
}

#[test]
fn reusable_references_validate_before_resolution_and_report_field_paths() {
    for reference in [
        "literal",
        "$env.HOST_SENTINEL_SECRET",
        "{$inputs.x}",
        "$components.parameters.p trailing",
    ] {
        for (field, path) in [
            (
                format!("        parameters:\n          - reference: '{reference}'"),
                format!("{STEP}.parameters[0].reference"),
            ),
            (
                format!("        onSuccess:\n          - reference: '{reference}'"),
                format!("{STEP}.onSuccess[0].reference"),
            ),
        ] {
            one(&document(&field, ""), &path);
        }
    }
}

#[test]
fn component_definitions_are_owned_once_and_local_overrides_report_at_use() {
    let components = "components:\n  parameters:\n    p: {name: p, in: query, value: '{$env.X}'}\n  successActions:\n    a:\n      name: done\n      type: end\n      criteria:\n        - condition: '$env.X == 1'";
    let yaml = document("        parameters:\n          - reference: '$components.parameters.p'\n          - {reference: '$components.parameters.p', value: '{$env.Y}'}\n        onSuccess:\n          - reference: '$components.successActions.a'\n          - name: '$components.successActions.a'\n            criteria:\n              - condition: '$env.Y == 1'", components);
    let errors = expression_errors(&yaml);
    assert_eq!(
        errors
            .iter()
            .map(|error| error.path.as_str())
            .collect::<Vec<_>>(),
        [
            "components.parameters.p.value".to_owned(),
            "components.successActions.a.criteria[0].condition".to_owned(),
            format!("{STEP}.parameters[1].value"),
            format!("{STEP}.onSuccess[1].criteria[0].condition")
        ]
    );
    let unused = document("", components);
    assert_eq!(expression_errors(&unused).len(), 2);
}

#[test]
fn component_action_parameter_references_keep_definition_and_override_ownership() {
    for (component_field, action_field) in [
        ("successActions", "onSuccess"),
        ("failureActions", "onFailure"),
    ] {
        for reference in ["$env.X", "literal", "$components.parameters.p trailing"] {
            let components = format!("components:\n  {component_field}:\n    a:\n      name: next\n      type: goto\n      workflowId: wf\n      parameters:\n        - reference: '{reference}'");
            let definition_path = format!("components.{component_field}.a.parameters[0].reference");
            let unused = both(&document("", &components));
            assert_eq!(unused.len(), 1, "{unused:?}");
            assert_eq!(unused[0].path, definition_path);
            assert_eq!(unused[0].kind, ValidationErrorKind::InvalidExpression);

            let inherited = format!("        {action_field}:\n          - reference: '$components.{component_field}.a'\n          - name: '$components.{component_field}.a'");
            let yaml = document(&inherited, &components).replace(
                "    steps:",
                &format!("    {component_field}:\n      - reference: '$components.{component_field}.a'\n      - name: '$components.{component_field}.a'\n    steps:"),
            );
            assert_eq!(both(&yaml), unused, "inherited copies: {yaml}");

            let local = format!("{inherited}\n          - name: '$components.{component_field}.a'\n            parameters:\n              - reference: '$env.LOCAL'");
            let errors = both(&document(&local, &components));
            assert_eq!(errors.len(), 2, "{errors:?}");
            assert_eq!(errors[0], unused[0]);
            assert_eq!(
                errors[1].path,
                format!("{STEP}.{action_field}[2].parameters[0].reference")
            );
            assert_eq!(errors[1].kind, ValidationErrorKind::InvalidExpression);
            assert_eq!(errors[1].message, ENV_MESSAGE);

            let workflow_local = document(&inherited, &components).replace(
                "    steps:",
                &format!("    {component_field}:\n      - name: '$components.{component_field}.a'\n        parameters:\n          - reference: '$env.LOCAL'\n    steps:"),
            );
            let errors = both(&workflow_local);
            assert_eq!(errors.len(), 2, "{errors:?}");
            assert_eq!(errors[0], unused[0]);
            assert_eq!(
                errors[1].path,
                format!("workflow \"wf\".{component_field}[0].parameters[0].reference")
            );
            assert_eq!(errors[1].kind, ValidationErrorKind::InvalidExpression);
            assert_eq!(errors[1].message, ENV_MESSAGE);

            let valid_local = format!("{inherited}\n          - name: '$components.{component_field}.a'\n            parameters:\n              - {{name: p, value: literal}}");
            assert_eq!(both(&document(&valid_local, &components)), unused);
        }
    }
}

#[test]
fn excluded_text_and_specialized_targets_are_not_expression_scanned() {
    let yaml = document("        description: '{$env.HOST_SENTINEL_SECRET}'\n        x-extension: '{$env.X}'\n        parameters:\n          - {name: '{$env.X}', in: query, value: literal}\n        onSuccess:\n          - {name: '{$env.X}', type: goto, stepId: '{$env.X}'}", "");
    assert!(expression_errors(&yaml).is_empty());
    // Existing specialized target/reference diagnostics remain their owners.
    let yaml = document("        dependsOn: ['$env.X']", "")
        .replace("operationId: doThing", "operationId: $env.X");
    assert!(expression_errors(&yaml).is_empty());
}

#[test]
fn every_value_and_criterion_site_uses_its_frozen_mode() {
    let cases = [
        ("    parameters:\n      - {name: p, value: '{$env.X}'}", "workflow \"wf\".parameters[0].value".to_owned()),
        ("        parameters:\n          - name: p\n            in: query\n            value: {context: '$env.X', selector: /id, type: jsonpointer}", format!("{STEP}.parameters[0].value.context")),
        ("        requestBody:\n          payload: {context: '$env.X', selector: /id, type: jsonpointer}", format!("{STEP}.requestBody.payload.context")),
        ("        requestBody:\n          replacements:\n            - {target: /id, value: '{$env.X}'}", format!("{STEP}.requestBody.replacements[0].value")),
        ("        onFailure:\n          - name: retry\n            type: retry\n            criteria:\n              - condition: '$env.X == 1'", format!("{STEP}.onFailure[0].criteria[0].condition")),
        ("        onSuccess:\n          - name: next\n            type: goto\n            workflowId: wf\n            parameters:\n              - {name: p, value: '{$env.X}'}", format!("{STEP}.onSuccess[0].parameters[0].value")),
        ("    successActions:\n      - name: next\n        type: goto\n        workflowId: wf\n        parameters:\n          - {name: p, value: '{$env.X}'}", "workflow \"wf\".successActions[0].parameters[0].value".to_owned()),
        ("    outputs:\n      value: {context: '$env.X', selector: /id, type: jsonpointer}", "workflow \"wf\".outputs.value.context".to_owned()),
    ];
    for (field, path) in cases {
        assert_eq!(one(&document(field, ""), &path).message, ENV_MESSAGE);
    }
    for candidate in [
        "{$}",
        "{$inputs.x",
        "{$inputs.x trailing}",
        "{$request.payload}",
        "{$inputs.x} then {$inputs.x",
    ] {
        one(
            &document(
                &format!("        requestBody: {{payload: '{candidate}'}}"),
                "",
            ),
            &format!("{STEP}.requestBody.payload"),
        );
    }
}

#[test]
fn failure_component_criteria_are_checked_unused_and_inherited_once() {
    let components = "components:\n  failureActions:\n    retry:\n      name: retry\n      type: retry\n      criteria:\n        - {context: '$env.X', type: regex, condition: '^ok$'}";
    for field in [
        "",
        "        onFailure:\n          - reference: '$components.failureActions.retry'",
    ] {
        assert_eq!(
            one(
                &document(field, components),
                "components.failureActions.retry.criteria[0].context"
            )
            .message,
            ENV_MESSAGE
        );
    }
}

#[test]
fn literal_mapping_keys_and_subworkflow_values_are_not_required_expressions() {
    let yaml = document(
        "        requestBody:\n          payload: {'{$env.X}': '$env.HOST_SENTINEL_SECRET'}",
        "",
    );
    assert!(expression_errors(&yaml).is_empty());
    let yaml = document(
        "        parameters:\n          - {name: p, value: '$response.body.status'}",
        "",
    )
    .replace("operationId: doThing", "workflowId: wf");
    assert!(expression_errors(&yaml).is_empty());
}

#[test]
fn invalid_workflow_step_reference_reports_only_its_syntax_failure() {
    for value in ["$steps.absent.outputs.", "$steps.absent.outputs.x trailing"] {
        let yaml = document(&format!("    outputs: {{value: '{value}'}}"), "");
        let errors = both(&yaml);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].kind, ValidationErrorKind::InvalidExpression);
        assert_eq!(errors[0].path, "workflow \"wf\".outputs.value");
    }
}

#[test]
fn invalid_expressions_stop_the_public_validation_continuation() {
    let yaml = document("        outputs: {value: '$response.body.status'}", "");
    let mut execution_reached = false;
    let parsed = parse_bytes_with_diagnostics(yaml.as_bytes()).map(|_| execution_reached = true);
    assert!(matches!(parsed, Err(Error::Validation(_))));
    assert!(!execution_reached);
    let spec: ArazzoSpec = serde_yaml_ng::from_str(&yaml).unwrap();
    let direct = validate_diagnostics(&spec).map(|_| execution_reached = true);
    assert!(matches!(direct, Err(Error::Validation(_))));
    assert!(!execution_reached);
}

#[test]
fn concrete_parameter_empty_reference_roundtrips_without_becoming_reusable() {
    let yaml = document(
        "        parameters:\n          - {name: p, in: query, value: '$env.X', reference: ''}",
        "",
    );
    assert!(both(&yaml).is_empty());
    let spec: ArazzoSpec = serde_yaml_ng::from_str(&yaml).unwrap();
    let roundtrip = serde_yaml_ng::to_string(&spec).unwrap();
    assert!(both(&roundtrip).is_empty());

    let empty_reusable = document("        parameters:\n          - {reference: ''}", "");
    let errors = diagnostics(parse_bytes_with_diagnostics(empty_reusable.as_bytes()));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].kind, ValidationErrorKind::InvalidExpression);
    assert_eq!(errors[0].path, format!("{STEP}.parameters[0].reference"));

    let nonempty_malformed = document("        parameters:\n          - {name: p, in: query, value: literal, reference: '$env.X'}", "");
    assert_eq!(
        one(
            &nonempty_malformed,
            &format!("{STEP}.parameters[0].reference")
        )
        .message,
        ENV_MESSAGE
    );
}
