//! Workflow-to-Step selection is privately pinned alongside this public
//! parsed/direct contract: clean validation alone cannot expose a selected input.

use arazzo_spec::ArazzoSpec;
use arazzo_validate::{
    parse_bytes_with_diagnostics, validate_diagnostics, Diagnostic, Error, Severity,
    ValidationErrorKind as Kind,
};

const WF: &str = "workflow \"parent\"";
const STEP: &str = "workflow \"parent\" > step \"invoke\"";
const OP_PATH: &str = "operationPath: '{$sourceDescriptions.api.url}#/paths/~1item/get'";
const LOCATIONS: [Option<&str>; 6] = [
    None,
    Some("path"),
    Some("query"),
    Some("querystring"),
    Some("header"),
    Some("cookie"),
];

fn document(components: &str, parameters: &str, steps: &str) -> String {
    format!("arazzo: 1.1.0\ninfo: {{title: Inheritance, version: '1'}}\nsourceDescriptions:\n  - {{name: api, url: https://example.invalid/openapi.yaml, type: openapi}}\n{components}workflows:\n  - workflowId: parent\n    parameters: {parameters}\n    steps:\n{steps}\n  - workflowId: child\n    steps: []\n")
}

fn parameter(name: &str, location: Option<&str>) -> String {
    let location = location.map_or(String::new(), |location| format!(", in: {location}"));
    format!("{{name: {name}{location}, value: 'a=1'}}")
}

fn errors(result: Result<impl Sized, Error>) -> Vec<Diagnostic> {
    match result {
        Ok(_) => Vec::new(),
        Err(Error::Validation(report)) => {
            assert!(report.warnings.is_empty(), "{:?}", report.warnings);
            report.errors
        }
        Err(error) => panic!("expected structured validation, got {error}"),
    }
}

fn both(yaml: &str) -> Vec<Diagnostic> {
    let parsed = match parse_bytes_with_diagnostics(yaml.as_bytes()) {
        Ok((_, warnings)) => {
            assert!(warnings.is_empty(), "{warnings:?}");
            Vec::new()
        }
        result => errors(result),
    };
    let direct: ArazzoSpec =
        serde_yaml_ng::from_str(yaml).unwrap_or_else(|error| panic!("typed document: {error}"));
    let before = direct.clone();
    let direct_errors = match validate_diagnostics(&direct) {
        Ok(warnings) => {
            assert!(warnings.is_empty(), "{warnings:?}");
            Vec::new()
        }
        result => errors(result),
    };
    assert_eq!(parsed, direct_errors, "parsed/direct parity\n{yaml}");
    assert_eq!(
        direct, before,
        "direct validation must preserve the caller's model"
    );
    parsed
}

fn diagnostic(kind: Kind, path: impl Into<String>, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        kind,
        path: path.into(),
        message: message.into(),
    }
}

fn required_in(path: &str) -> Diagnostic {
    diagnostic(
        Kind::InvalidParameterLocation,
        format!("{path}.in"),
        format!("{path}.in is required when the Step targets an operation"),
    )
}

fn duplicate(path: &str, first: &str, name: &str) -> Diagnostic {
    diagnostic(Kind::DuplicateIdentifier, format!("{path}.name"), format!(
        "{path}.name \"{name}\" is a duplicate: the parameter list \"MUST NOT include duplicate parameters\"; first declaration is at {first}.name"))
}

fn coexistence(path: &str, whole: &str, query: &str) -> Diagnostic {
    diagnostic(Kind::InvalidParameterLocation, format!("{path}.parameters"), format!(
        "{path} carries in: querystring parameter(s) [\"{whole}\"] alongside in: query parameter(s) [\"{query}\"]; the querystring location supplies the entire query component and cannot coexist with query parameters in the same operation. Workflow-level parameters are inherited by this step and count here."))
}

#[test]
fn absent_and_every_valid_location_map_to_workflow_inputs() {
    // All 36 inherited/local combinations include query + querystring in both
    // directions, and two differently named querystrings. None is an identity.
    for inherited in LOCATIONS {
        for local in LOCATIONS {
            let yaml = document(
                "",
                &format!("[{}]", parameter("inherited", inherited)),
                &format!(
                    "      - stepId: invoke\n        workflowId: child\n        parameters: [{}]",
                    parameter("local", local)
                ),
            );
            assert_eq!(both(&yaml), [], "inherited={inherited:?}, local={local:?}");
        }
    }
}

#[test]
fn exact_overrides_name_case_and_nonmatching_locations_remain_valid_inputs() {
    // Private adapter assertions prove retained selection; these exercise the
    // public paths without introducing child input-name/schema checking.
    for (inherited, local) in [
        (
            "{name: same, value: inherited}",
            "{name: same, value: local}",
        ),
        (
            "{name: same, in: header, value: inherited}",
            "{name: same, in: header, value: local}",
        ),
        (
            "{name: Case, in: header, value: inherited}",
            "{name: case, in: header, value: local}",
        ),
        (
            "{name: same, value: inherited}",
            "{name: same, in: query, value: local}",
        ),
        (
            "{name: same, in: query, value: inherited}",
            "{name: same, value: local}",
        ),
        (
            "{name: same, in: query, value: inherited}",
            "{name: same, in: querystring, value: local}",
        ),
    ] {
        let yaml = document(
            "",
            &format!("[{inherited}]"),
            &format!(
                "      - stepId: invoke\n        workflowId: child\n        parameters: [{local}]"
            ),
        );
        assert_eq!(both(&yaml), []);
    }
}

#[test]
fn malformed_location_spellings_are_rejected_at_both_deserialization_boundaries() {
    for location in ["Header", "QUERY", "QueryString", "body", ""] {
        let yaml = document("", "[]", &format!(
            "      - stepId: invoke\n        workflowId: child\n        parameters: [{{name: input, in: '{location}', value: v}}]"));
        assert!(
            matches!(
                parse_bytes_with_diagnostics(yaml.as_bytes()),
                Err(Error::ParseYaml(_))
            ),
            "{location:?}"
        );
        assert!(
            serde_yaml_ng::from_str::<ArazzoSpec>(&yaml).is_err(),
            "{location:?}"
        );
    }
}

#[test]
fn mixed_targets_keep_operation_constraints_override_paths_and_suppression_order() {
    let yaml = document(
        "",
        "[{name: inherited, value: global}, {name: q, in: query, value: q}]",
        &format!(
            r#"      - stepId: child-first
        workflowId: child
        parameters: [{{name: whole, in: querystring, value: q=1}}]
      - stepId: by-id
        operationId: invoke
        parameters:
          - {{name: inherited, in: header, value: different-location}}
          - {{name: whole, in: querystring, value: q=1}}
      - stepId: by-path
        {OP_PATH}
        parameters:
          - {{name: inherited, value: exact-override}}
          - {{name: q, in: query, value: exact-query-override}}
          - {{name: whole, in: querystring, value: q=2}}
      - stepId: by-id-again
        operationId: again
      - stepId: child-last
        workflowId: child"#
        ),
    );
    assert_eq!(
        both(&yaml),
        vec![
            required_in(&format!("{WF}.parameters[0]")),
            coexistence("workflow \"parent\" > step \"by-id\"", "whole", "q"),
            required_in("workflow \"parent\" > step \"by-path\".parameters[0]"),
            coexistence("workflow \"parent\" > step \"by-path\"", "whole", "q"),
        ]
    );
}

#[test]
fn declared_duplicates_and_intrinsic_errors_keep_exact_paths_order_and_cardinality() {
    let yaml = document("", "[{name: duplicate, in: header, value: first}, {name: duplicate, in: header, value: second}, {in: cookie, value: unnamed}]", r#"      - stepId: invoke
        workflowId: child
        parameters:
          - {name: input, in: header, value: first}
          - {name: input, in: query, value: second}
          - {name: Case, value: upper}
          - {name: case, value: lower}
      - stepId: repeat
        workflowId: child"#);
    assert_eq!(
        both(&yaml),
        vec![
            diagnostic(
                Kind::MissingRequiredField,
                format!("{WF}.parameters[2].name"),
                format!("{WF}.parameters[2].name is required (unless using reference)")
            ),
            duplicate(
                &format!("{WF}.parameters[1]"),
                &format!("{WF}.parameters[0]"),
                "duplicate"
            ),
            duplicate(
                &format!("{STEP}.parameters[1]"),
                &format!("{STEP}.parameters[0]"),
                "input"
            ),
        ]
    );
}

#[test]
fn reusable_declarations_resolve_before_inheritance_and_keep_component_ownership() {
    let components = "components:\n  parameters:\n    shared: {name: input, in: header, value: component}\n    whole: {name: whole, in: querystring, value: q=1}\n    invalid: {in: cookie, value: unnamed}\n";
    let yaml = document(components, "[{reference: '$components.parameters.shared'}, {reference: '$components.parameters.whole'}, {reference: '$components.parameters.invalid'}]", r#"      - stepId: invoke
        workflowId: child
        parameters:
          - {reference: '$components.parameters.shared', value: override}
          - {name: q, in: query, value: q}
      - stepId: repeat
        workflowId: child"#);
    assert_eq!(
        both(&yaml),
        vec![diagnostic(
            Kind::MissingRequiredField,
            "components.parameters.invalid.name",
            "components.parameters.invalid.name is required (unless using reference)"
        )]
    );
}

#[test]
fn success_and_failure_actions_still_prohibit_every_explicit_location() {
    for location in LOCATIONS {
        let value = parameter("input", location);
        let yaml = document(
            "",
            "[]",
            &format!(
                r#"      - stepId: invoke
        workflowId: child
        onSuccess:
          - name: next
            type: goto
            workflowId: child
            parameters: [{value}]
        onFailure:
          - name: recover
            type: retry
            workflowId: child
            parameters: [{value}]"#
            ),
        );
        let expected = if location.is_none() {
            vec![]
        } else {
            ["onFailure", "onSuccess"].map(|action| {
                let path = format!("{STEP}.{action}[0].parameters[0].in");
                diagnostic(Kind::InvalidParameterLocation, &path, format!(
                    "{path} must not be set: action parameters map to workflow inputs and \"The in field MUST NOT be used\""))
            }).to_vec()
        };
        assert_eq!(both(&yaml), expected, "location={location:?}");
    }
}

#[test]
fn exclusivity_is_openapi_only_and_transport_target_diagnostics_remain() {
    for (target, action, target_error, transport) in [
        ("operationId: invoke", "", None, None),
        (OP_PATH, "", None, None),
        (
            "operationId: publish",
            "        action: send\n",
            None,
            Some("asynchronous operation"),
        ),
        (
            OP_PATH,
            "        action: receive\n",
            Some((
                "action",
                "action is only supported with operationId or channelPath async targets",
            )),
            Some("asynchronous operation"),
        ),
        (
            "channelPath: /events",
            "        action: send\n",
            None,
            Some("channelPath"),
        ),
        (
            "channelPath: /events",
            "",
            Some(("action", "channelPath requires action send or receive")),
            Some("channelPath"),
        ),
        (
            "workflowId: child",
            "        action: send\n",
            Some(("action", "action is not applicable to workflowId targets")),
            None,
        ),
        (
            "",
            "",
            Some((
                "",
                "must have operationId, operationPath, channelPath, or workflowId",
            )),
            None,
        ),
    ] {
        let yaml = document(
            "",
            "[{name: q, in: query, value: q}, {name: whole, in: querystring, value: q=1}]",
            &format!("      - stepId: invoke\n        {target}\n{action}"),
        );
        let mut expected = Vec::new();
        if let Some((field, message)) = target_error {
            expected.push(diagnostic(
                if field.is_empty() {
                    Kind::InvalidStepTarget
                } else {
                    Kind::InvalidAsyncStep
                },
                if field.is_empty() {
                    STEP.to_string()
                } else {
                    format!("{STEP}.{field}")
                },
                if field.is_empty() {
                    format!("{STEP} {message}")
                } else {
                    format!("{STEP}.{message}")
                },
            ));
        }
        if let Some(transport) = transport {
            for index in 0..2 {
                let path = format!("{WF}.parameters[{index}]");
                let message = if transport == "channelPath" {
                    format!("{path} cannot be used with a channelPath target: AsyncAPI parameter transport is not supported by this executor")
                } else {
                    format!("{path} cannot be used with an asynchronous operation target: AsyncAPI parameter transport is not supported by this executor")
                };
                expected.push(diagnostic(Kind::InvalidParameterLocation, path, message));
            }
        } else if target.starts_with("operation") && action.is_empty() {
            expected.push(coexistence(STEP, "whole", "q"));
        }
        assert_eq!(both(&yaml), expected, "target={target}, action={action}");
    }
}
