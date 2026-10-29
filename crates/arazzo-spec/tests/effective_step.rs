use arazzo_spec::{
    effective_step_parameters, EffectiveParameterOrigin, EffectiveParameterRef, ParamLocation,
    Parameter, Step, StepTarget, ValueSource, Workflow,
};
use EffectiveParameterOrigin::{Step as FromStep, Workflow as FromWorkflow};
use ParamLocation::{Cookie, Header, Path, Query, Querystring};

type Declaration = (&'static str, Option<ParamLocation>);

struct Case {
    name: &'static str,
    inherited: Vec<Declaration>,
    declared: Vec<Declaration>,
    expected: Vec<EffectiveParameterOrigin>,
}

fn parameter((name, in_): Declaration) -> Parameter {
    Parameter {
        name: name.into(),
        in_,
        ..Parameter::default()
    }
}

fn inherited(parameter_index: usize) -> EffectiveParameterOrigin {
    FromWorkflow { parameter_index }
}

fn declared(parameter_index: usize) -> EffectiveParameterOrigin {
    FromStep {
        step_index: 1,
        parameter_index,
    }
}

fn view(workflow: &Workflow, step_index: usize) -> Vec<EffectiveParameterRef<'_>> {
    match effective_step_parameters(workflow, step_index) {
        Some(effective) => effective,
        None => panic!("requested test Step {step_index} exists"),
    }
}

fn assert_view(workflow: &Workflow, step_index: usize, expected: &[EffectiveParameterOrigin]) {
    // The explicit type also checks the frozen crate-root public surface.
    let effective: Vec<EffectiveParameterRef<'_>> = view(workflow, step_index);
    let origins: Vec<_> = effective.iter().map(|entry| entry.origin).collect();
    assert_eq!(origins, expected);
    for entry in effective {
        let original = match entry.origin {
            FromWorkflow { parameter_index } => &workflow.parameters[parameter_index],
            FromStep {
                step_index,
                parameter_index,
            } => &workflow.steps[step_index].parameters[parameter_index],
        };
        assert!(std::ptr::eq(entry.parameter, original));
    }
}

#[test]
fn public_overlay_matrix_is_identical_for_every_target_kind() {
    let cases = [
        Case {
            name: "no declarations",
            inherited: vec![],
            declared: vec![],
            expected: vec![],
        },
        Case {
            name: "inherited only",
            inherited: vec![("first", Some(Query)), ("second", Some(Header))],
            declared: vec![],
            expected: vec![inherited(0), inherited(1)],
        },
        Case {
            name: "Step only",
            inherited: vec![],
            declared: vec![("first", Some(Path)), ("second", Some(Cookie))],
            expected: vec![declared(0), declared(1)],
        },
        Case {
            name: "exact override keeps nonmatching inheritance",
            inherited: vec![("overridden", Some(Query)), ("kept", Some(Header))],
            declared: vec![("overridden", Some(Query))],
            expected: vec![inherited(1), declared(0)],
        },
        Case {
            name: "multiple overrides retain Workflow then complete Step order",
            inherited: vec![
                ("a", Some(Query)),
                ("b", Some(Header)),
                ("c", Some(Path)),
                ("d", Some(Cookie)),
            ],
            declared: vec![
                ("c", Some(Path)),
                ("extra", Some(Querystring)),
                ("a", Some(Query)),
            ],
            expected: vec![
                inherited(1),
                inherited(3),
                declared(0),
                declared(1),
                declared(2),
            ],
        },
        Case {
            name: "unoverridden Workflow duplicates remain",
            inherited: vec![("same", Some(Query)), ("same", Some(Query))],
            declared: vec![],
            expected: vec![inherited(0), inherited(1)],
        },
        Case {
            name: "Step duplicates remain and override every inherited duplicate",
            inherited: vec![
                ("same", Some(Query)),
                ("kept", Some(Header)),
                ("same", Some(Query)),
                ("kept", Some(Header)),
            ],
            declared: vec![("same", Some(Query)), ("same", Some(Query))],
            expected: vec![inherited(1), inherited(3), declared(0), declared(1)],
        },
        Case {
            name: "name case and all concrete locations are exact identities",
            inherited: vec![
                ("Token", Some(Header)),
                ("token", Some(Header)),
                ("token", Some(Query)),
                ("token", Some(Path)),
                ("token", Some(Cookie)),
                ("token", Some(Querystring)),
            ],
            declared: vec![("Token", Some(Header))],
            expected: vec![
                inherited(1),
                inherited(2),
                inherited(3),
                inherited(4),
                inherited(5),
                declared(0),
            ],
        },
        Case {
            name: "absent location overrides only absent location",
            inherited: vec![("input", None), ("input", Some(Query))],
            declared: vec![("input", None)],
            expected: vec![inherited(1), declared(0)],
        },
        Case {
            name: "concrete location cannot remove absent location",
            inherited: vec![("input", None), ("input", Some(Query))],
            declared: vec![("input", Some(Query))],
            expected: vec![inherited(0), declared(0)],
        },
    ];
    let targets = [
        StepTarget::OperationId("fetch".into()),
        StepTarget::OperationPath("{$sourceDescriptions.api.url}#/paths/~1items/get".into()),
        StepTarget::ChannelPath("{$sourceDescriptions.events.url}#/channels/updates".into()),
        StepTarget::WorkflowId("nested".into()),
    ];

    for target in targets {
        for case in &cases {
            let workflow = Workflow {
                parameters: case.inherited.iter().copied().map(parameter).collect(),
                steps: vec![
                    Step {
                        parameters: vec![parameter(("other-step", None))],
                        ..Step::default()
                    },
                    Step {
                        target: Some(target.clone()),
                        parameters: case.declared.iter().copied().map(parameter).collect(),
                        ..Step::default()
                    },
                ],
                ..Workflow::default()
            };
            let before = workflow.clone();
            let actual = view(&workflow, 1);
            assert_eq!(
                actual.iter().map(|entry| entry.origin).collect::<Vec<_>>(),
                case.expected,
                "{}: {target:?}",
                case.name
            );
            assert_view(&workflow, 1, &case.expected);
            assert_eq!(workflow, before, "{}: {target:?}", case.name);
        }
    }
}

#[test]
fn workflow_id_inherits_inputs_and_overrides_without_losing_other_inputs() {
    // Excluding WorkflowId from inheritance would fail both positive cases.
    let mut workflow = Workflow {
        parameters: vec![parameter(("token", None)), parameter(("limit", None))],
        steps: vec![Step {
            target: Some(StepTarget::WorkflowId("nested".into())),
            ..Step::default()
        }],
        ..Workflow::default()
    };
    workflow.parameters[0].value =
        ValueSource::Literal(serde_yaml_ng::Value::String("inherited-token".into()));
    assert_view(&workflow, 0, &[inherited(0), inherited(1)]);

    let mut override_parameter = parameter(("token", None));
    override_parameter.value = ValueSource::Literal(serde_yaml_ng::Value::Null);
    workflow.steps[0].parameters.push(override_parameter);
    assert_view(
        &workflow,
        0,
        &[
            inherited(1),
            FromStep {
                step_index: 0,
                parameter_index: 0,
            },
        ],
    );
    let effective = view(&workflow, 0);
    assert!(matches!(
        effective[1].parameter.value,
        ValueSource::Literal(serde_yaml_ng::Value::Null)
    ));
}

#[test]
fn invalid_indexes_are_distinct_from_valid_empty_views() {
    let mut workflow = Workflow::default();
    assert!(effective_step_parameters(&workflow, 0).is_none());
    workflow.steps.push(Step::default());
    assert!(view(&workflow, 0).is_empty());
    for invalid_index in [1, 2, usize::MAX] {
        assert!(effective_step_parameters(&workflow, invalid_index).is_none());
    }
}

#[test]
fn location_wire_spellings_remain_lowercase_without_normalization() {
    for (wire, location) in [
        ("path", Path),
        ("query", Query),
        ("querystring", Querystring),
        ("header", Header),
        ("cookie", Cookie),
    ] {
        let lowercase = serde_json::json!({"name": "value", "in": wire, "value": 1});
        let parameter: Parameter = match serde_json::from_value(lowercase) {
            Ok(parameter) => parameter,
            Err(error) => panic!("lowercase location is valid: {error}"),
        };
        assert_eq!(parameter.in_, Some(location));
        let uppercase = wire.to_ascii_uppercase();
        let mixed_case = format!("{}{}", &uppercase[..1], &wire[1..]);
        for invalid in [uppercase, mixed_case] {
            let nonconformant = serde_json::json!({"name": "value", "in": invalid, "value": 1});
            assert!(
                serde_json::from_value::<Parameter>(nonconformant).is_err(),
                "accepted non-lowercase location: {invalid}"
            );
        }
    }
}
