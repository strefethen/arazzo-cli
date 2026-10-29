//! Public inventory contract. Fixtures include invalid direct models to prove
//! that enumeration does not become validation or expression parsing.

use arazzo_spec::{
    step_expression_sites, EffectiveParameterOrigin, ExpressionType, OutputValue, ParamLocation,
    Parameter, Replacement, RequestBody, SelectorObject, SelectorType, Step,
    StepExpressionDeclaration, StepExpressionSite, StepExpressionSiteMode, StepTarget,
    StepValuePathSegment, SuccessCriterion, ValueSource, Workflow,
};
use serde_yaml_ng::{Mapping, Value};
use StepExpressionDeclaration as Declaration;
use StepExpressionSiteMode::{
    EmbeddedCondition, LiteralOrExpression, RequiredExpression, SimpleCondition,
};
use StepValuePathSegment::{MappingEntry, SelectorContext, SequenceIndex};

fn yaml(input: &str) -> Value {
    serde_yaml_ng::from_str(input).unwrap_or_else(|error| panic!("test YAML value parses: {error}"))
}

fn workflow(step: Step) -> Workflow {
    Workflow {
        steps: vec![step],
        ..Workflow::default()
    }
}

fn sites(workflow: &Workflow) -> Vec<StepExpressionSite<'_>> {
    step_expression_sites(workflow, 0).unwrap_or_else(|| panic!("test Step exists"))
}

fn parameter(name: &str, in_: Option<ParamLocation>, value: &str) -> Parameter {
    Parameter {
        name: name.into(),
        in_,
        value: value.into(),
        ..Parameter::default()
    }
}

fn selector(context: &str) -> SelectorObject {
    SelectorObject {
        context: context.into(),
        selector: "$excluded.selector".into(),
        type_: SelectorType::Name("$excluded.type".into()),
        extensions: [("x-note".into(), yaml("'$excluded.extension'"))].into(),
    }
}

type Expected<'a> = (
    &'a str,
    StepExpressionSiteMode,
    Declaration<'a>,
    Vec<StepValuePathSegment<'a>>,
);

fn assert_sites(actual: &[StepExpressionSite<'_>], expected: &[Expected<'_>]) {
    let actual: Vec<_> = actual
        .iter()
        .map(|site| {
            (
                site.text,
                site.mode,
                site.declaration,
                site.nested_path.clone(),
            )
        })
        .collect();
    assert_eq!(actual, expected);
}

fn inherited(index: usize) -> Declaration<'static> {
    Declaration::EffectiveParameterValue {
        origin: EffectiveParameterOrigin::Workflow {
            parameter_index: index,
        },
    }
}

fn declared(index: usize) -> Declaration<'static> {
    Declaration::EffectiveParameterValue {
        origin: EffectiveParameterOrigin::Step {
            step_index: 0,
            parameter_index: index,
        },
    }
}

fn entry(index: usize, key: Option<&str>) -> StepValuePathSegment<'_> {
    MappingEntry { index, key }
}

#[test]
fn exact_overlay_and_provenance_cover_every_target_kind() {
    use ParamLocation::{Header, Query};
    let targets = [
        None,
        Some(StepTarget::OperationId("$excluded.operation".into())),
        Some(StepTarget::OperationPath("$excluded.path".into())),
        Some(StepTarget::ChannelPath("$excluded.channel".into())),
        Some(StepTarget::WorkflowId("$excluded.workflow".into())),
    ];
    let cases = [
        (vec![], vec![], vec![]),
        (
            vec![parameter("one", None, "inherited")],
            vec![],
            vec![("inherited", inherited(0))],
        ),
        (
            vec![],
            vec![parameter("one", None, "declared")],
            vec![("declared", declared(0))],
        ),
        (
            vec![
                parameter("same", Some(Query), "stale-1"),
                parameter("kept", None, "inherited"),
                parameter("same", Some(Query), "stale-2"),
            ],
            vec![
                parameter("same", Some(Query), "override-1"),
                parameter("same", Some(Query), "override-2"),
            ],
            vec![
                ("inherited", inherited(1)),
                ("override-1", declared(0)),
                ("override-2", declared(1)),
            ],
        ),
        (
            vec![
                parameter("same", Some(Query), "query"),
                parameter("same", None, "absent"),
                parameter("Same", Some(Header), "case-sensitive"),
            ],
            vec![parameter("same", Some(Header), "header")],
            vec![
                ("query", inherited(0)),
                ("absent", inherited(1)),
                ("case-sensitive", inherited(2)),
                ("header", declared(0)),
            ],
        ),
        (
            vec![
                parameter("duplicate", None, "first"),
                parameter("duplicate", None, "second"),
            ],
            vec![],
            vec![("first", inherited(0)), ("second", inherited(1))],
        ),
    ];
    for target in targets {
        for (inherited_parameters, step_parameters, expected) in &cases {
            let mut input = workflow(Step {
                target: target.clone(),
                parameters: step_parameters.clone(),
                ..Step::default()
            });
            input.parameters = inherited_parameters.clone();
            let expected: Vec<_> = expected
                .iter()
                .map(|(text, declaration)| (*text, LiteralOrExpression, *declaration, vec![]))
                .collect();
            assert_sites(&sites(&input), &expected);
            for site in sites(&input) {
                let original = match site.declaration {
                    Declaration::EffectiveParameterValue {
                        origin: EffectiveParameterOrigin::Workflow { parameter_index },
                    } => &input.parameters[parameter_index],
                    Declaration::EffectiveParameterValue {
                        origin:
                            EffectiveParameterOrigin::Step {
                                step_index,
                                parameter_index,
                            },
                    } => &input.steps[step_index].parameters[parameter_index],
                    other => panic!("unexpected parameter declaration {other:?}"),
                };
                assert!(std::ptr::eq(
                    site.text,
                    original
                        .value
                        .as_literal()
                        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
                        .as_str()
                        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
                ));
            }
        }
    }
}

#[test]
fn inherited_declaration_remains_effective_at_each_step_index() {
    let mut input = Workflow {
        parameters: vec![parameter("shared", None, "$inputs.shared")],
        steps: vec![Step::default(), Step::default()],
        ..Workflow::default()
    };
    for index in 0..2 {
        assert_sites(
            &step_expression_sites(&input, index)
                .unwrap_or_else(|| panic!("fixture has the asserted field and shape")),
            &[("$inputs.shared", LiteralOrExpression, inherited(0), vec![])],
        );
    }
    input.steps[1].parameters = vec![parameter("local", None, "second-step")];
    assert_sites(
        &step_expression_sites(&input, 1)
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape")),
        &[
            ("$inputs.shared", LiteralOrExpression, inherited(0), vec![]),
            (
                "second-step",
                LiteralOrExpression,
                Declaration::EffectiveParameterValue {
                    origin: EffectiveParameterOrigin::Step {
                        step_index: 1,
                        parameter_index: 0,
                    },
                },
                vec![],
            ),
        ],
    );
    assert!(step_expression_sites(&input, 2).is_none());
    assert!(step_expression_sites(&input, usize::MAX).is_none());
    assert!(step_expression_sites(&Workflow::default(), 0).is_none());
    assert!(sites(&workflow(Step::default())).is_empty());
}

#[test]
fn full_inventory_order_and_borrowed_text_and_output_labels() {
    let input = workflow(Step {
        parameters: vec![parameter("p", None, "plain text, no expression")],
        request_body: Some(RequestBody {
            payload: Some("$inputs.payload".into()),
            replacements: vec![
                Replacement {
                    value: "replacement-0".into(),
                    ..Replacement::default()
                },
                Replacement {
                    value: ValueSource::Selector(selector("$inputs.replacement")),
                    ..Replacement::default()
                },
            ],
            ..RequestBody::default()
        }),
        success_criteria: vec![SuccessCriterion {
            context: "$response.body".into(),
            condition: "'$inputs.quoted' == '$inputs.quoted'".into(),
            ..SuccessCriterion::default()
        }],
        outputs: [
            (
                "z".into(),
                OutputValue::RuntimeExpression("$steps.z".into()),
            ),
            ("a".into(), OutputValue::Selector(selector("$steps.a"))),
        ]
        .into(),
        correlation_id: Some("$inputs.orderId".into()),
        ..Step::default()
    });
    let actual = sites(&input);
    assert_sites(
        &actual,
        &[
            (
                "plain text, no expression",
                LiteralOrExpression,
                declared(0),
                vec![],
            ),
            (
                "$inputs.payload",
                LiteralOrExpression,
                Declaration::RequestBodyPayload,
                vec![],
            ),
            (
                "replacement-0",
                LiteralOrExpression,
                Declaration::RequestBodyReplacementValue {
                    replacement_index: 0,
                },
                vec![],
            ),
            (
                "$inputs.replacement",
                RequiredExpression,
                Declaration::RequestBodyReplacementValue {
                    replacement_index: 1,
                },
                vec![SelectorContext],
            ),
            (
                "$response.body",
                RequiredExpression,
                Declaration::SuccessCriterionContext { criterion_index: 0 },
                vec![],
            ),
            (
                "'$inputs.quoted' == '$inputs.quoted'",
                SimpleCondition,
                Declaration::SuccessCriterionCondition { criterion_index: 0 },
                vec![],
            ),
            (
                "$steps.a",
                RequiredExpression,
                Declaration::Output { name: "a" },
                vec![SelectorContext],
            ),
            (
                "$steps.z",
                RequiredExpression,
                Declaration::Output { name: "z" },
                vec![],
            ),
            (
                "$inputs.orderId",
                LiteralOrExpression,
                Declaration::CorrelationId,
                vec![],
            ),
        ],
    );
    let step = &input.steps[0];
    let body = step
        .request_body
        .as_ref()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"));
    let original_text = [
        step.parameters[0]
            .value
            .as_literal()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
            .as_str()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape")),
        body.payload
            .as_ref()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
            .as_literal()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
            .as_str()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape")),
        body.replacements[0]
            .value
            .as_literal()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
            .as_str()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape")),
        body.replacements[1]
            .value
            .as_selector()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
            .context
            .as_str(),
        step.success_criteria[0].context.as_str(),
        step.success_criteria[0].condition.as_str(),
        step.outputs["a"]
            .as_selector()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
            .context
            .as_str(),
        step.outputs["z"]
            .as_runtime_expression()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape")),
        step.correlation_id
            .as_deref()
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape")),
    ];
    for (site, original) in actual.iter().zip(original_text) {
        assert!(std::ptr::eq(site.text, original));
    }
    for site in &actual[6..8] {
        let Declaration::Output { name } = site.declaration else {
            panic!("expected output");
        };
        let original = step
            .outputs
            .get_key_value(name)
            .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
            .0;
        assert!(std::ptr::eq(name, original.as_str()));
    }
}

#[test]
fn every_value_source_family_honors_typed_root_and_raw_child_admission() {
    // Direct Literal selector-shaped roots stay literal. The identical mapping
    // at a container edge is admitted, as is an explicitly typed Selector root.
    let raw = yaml("{context: '$inputs.context', selector: '$excluded.selector', type: '$excluded.type', x-note: '$excluded.extension'}");
    let values = [
        ValueSource::Literal(raw.clone()),
        ValueSource::Selector(selector("$inputs.context")),
        ValueSource::Literal(Value::Sequence(vec![raw])),
    ];
    for family in 0..4 {
        for (variant, value) in values.iter().enumerate() {
            let mut input = workflow(Step::default());
            let declaration = match family {
                0 => {
                    input.parameters.push(Parameter {
                        value: value.clone(),
                        ..Parameter::default()
                    });
                    inherited(0)
                }
                1 => {
                    input.steps[0].parameters.push(Parameter {
                        value: value.clone(),
                        ..Parameter::default()
                    });
                    declared(0)
                }
                2 => {
                    input.steps[0].request_body = Some(RequestBody {
                        payload: Some(value.clone()),
                        ..RequestBody::default()
                    });
                    Declaration::RequestBodyPayload
                }
                _ => {
                    input.steps[0].request_body = Some(RequestBody {
                        replacements: vec![Replacement {
                            value: value.clone(),
                            ..Replacement::default()
                        }],
                        ..RequestBody::default()
                    });
                    Declaration::RequestBodyReplacementValue {
                        replacement_index: 0,
                    }
                }
            };
            let expected = match variant {
                0 => [
                    "$inputs.context",
                    "$excluded.selector",
                    "$excluded.type",
                    "$excluded.extension",
                ]
                .into_iter()
                .zip(["context", "selector", "type", "x-note"])
                .enumerate()
                .map(|(index, (text, key))| {
                    (
                        text,
                        LiteralOrExpression,
                        declaration,
                        vec![MappingEntry {
                            index,
                            key: Some(key),
                        }],
                    )
                })
                .collect::<Vec<_>>(),
                1 => vec![(
                    "$inputs.context",
                    RequiredExpression,
                    declaration,
                    vec![SelectorContext],
                )],
                _ => vec![(
                    "$inputs.context",
                    RequiredExpression,
                    declaration,
                    vec![SequenceIndex(0), SelectorContext],
                )],
            };
            assert_sites(&sites(&input), &expected);
        }
    }
}

#[test]
fn nested_selector_context_borrows_original_literal_text() {
    let input = workflow(Step {
        request_body: Some(RequestBody {
            payload: Some(ValueSource::Literal(yaml(
                "[{context: original-context, selector: ignored, type: jsonpath}]",
            ))),
            ..RequestBody::default()
        }),
        ..Step::default()
    });
    let Some(body) = &input.steps[0].request_body else {
        panic!("fixture declares requestBody");
    };
    let Some(ValueSource::Literal(Value::Sequence(children))) = &body.payload else {
        panic!("fixture payload is a literal sequence");
    };
    let Some(original) = children[0]["context"].as_str() else {
        panic!("fixture child context is a string");
    };
    let actual = sites(&input);
    assert_sites(
        &actual,
        &[(
            "original-context",
            RequiredExpression,
            Declaration::RequestBodyPayload,
            vec![SequenceIndex(0), SelectorContext],
        )],
    );
    assert!(std::ptr::eq(actual[0].text, original));
}

#[test]
fn recursive_values_keep_depth_first_order_and_mapping_entry_identity() {
    let mut mapping = Mapping::new();
    mapping.insert(Value::String("$key-never-scanned".into()), Value::Null);
    mapping.insert(
        yaml("1"),
        yaml("[false, 'numeric-key', {deep: ['leaf', 3]}]"),
    );
    mapping.insert(Value::String("1".into()), yaml("'string-key'"));
    mapping.insert(yaml("[key, '$not-a-site']"), yaml("'sequence-key'"));
    mapping.insert(yaml("{key: '$not-a-site'}"), yaml("'mapping-key'"));
    mapping.insert(Value::Bool(true), yaml("'bool-key'"));
    mapping.insert(Value::Null, yaml("'null-key'"));
    mapping.insert(yaml("!label tagged"), yaml("'tagged-key'"));
    let input = workflow(Step {
        request_body: Some(RequestBody {
            payload: Some(ValueSource::Literal(Value::Mapping(mapping))),
            ..RequestBody::default()
        }),
        ..Step::default()
    });
    let declaration = Declaration::RequestBodyPayload;
    let actual = sites(&input);
    assert_sites(
        &actual,
        &[
            (
                "numeric-key",
                LiteralOrExpression,
                declaration,
                vec![entry(1, None), SequenceIndex(1)],
            ),
            (
                "leaf",
                LiteralOrExpression,
                declaration,
                vec![
                    entry(1, None),
                    SequenceIndex(2),
                    entry(0, Some("deep")),
                    SequenceIndex(0),
                ],
            ),
            (
                "string-key",
                LiteralOrExpression,
                declaration,
                vec![entry(2, Some("1"))],
            ),
            (
                "sequence-key",
                LiteralOrExpression,
                declaration,
                vec![entry(3, None)],
            ),
            (
                "mapping-key",
                LiteralOrExpression,
                declaration,
                vec![entry(4, None)],
            ),
            (
                "bool-key",
                LiteralOrExpression,
                declaration,
                vec![entry(5, None)],
            ),
            (
                "null-key",
                LiteralOrExpression,
                declaration,
                vec![entry(6, None)],
            ),
            (
                "tagged-key",
                LiteralOrExpression,
                declaration,
                vec![entry(7, Some("tagged"))],
            ),
        ],
    );
    let root = input.steps[0]
        .request_body
        .as_ref()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
        .payload
        .as_ref()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
        .as_literal()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"));
    let map = root
        .as_mapping()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"));
    let key = map
        .iter()
        .nth(2)
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
        .0
        .as_str()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"));
    let MappingEntry {
        key: Some(label), ..
    } = actual[2].nested_path[0]
    else {
        panic!("string-key label")
    };
    assert!(std::ptr::eq(label, key));
    let original_leaf = map
        .iter()
        .nth(1)
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))
        .1
        .as_sequence()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"))[2]["deep"][0]
        .as_str()
        .unwrap_or_else(|| panic!("fixture has the asserted field and shape"));
    assert!(std::ptr::eq(actual[1].text, original_leaf));
}

#[test]
fn selector_admission_is_shared_shape_compatibility_without_validation() {
    let cases = [
        (
            "{context: context, selector: ignored, type: unknown, x-note: ignored}",
            vec![("context", RequiredExpression, vec![SelectorContext])],
        ),
        (
            "{context: context, selector: ignored, type: {}}",
            vec![("context", RequiredExpression, vec![SelectorContext])],
        ),
        (
            "{context: context, selector: ignored, type: {version: missing-name}}",
            vec![("context", RequiredExpression, vec![SelectorContext])],
        ),
        (
            "{context: context, selector: ignored}",
            vec![
                (
                    "context",
                    LiteralOrExpression,
                    vec![entry(0, Some("context"))],
                ),
                (
                    "ignored",
                    LiteralOrExpression,
                    vec![entry(1, Some("selector"))],
                ),
            ],
        ),
        (
            "{context: 3, selector: literal, type: jsonpath}",
            vec![
                (
                    "literal",
                    LiteralOrExpression,
                    vec![entry(1, Some("selector"))],
                ),
                (
                    "jsonpath",
                    LiteralOrExpression,
                    vec![entry(2, Some("type"))],
                ),
            ],
        ),
        (
            "{context: context, selector: [nested], type: jsonpath}",
            vec![
                (
                    "context",
                    LiteralOrExpression,
                    vec![entry(0, Some("context"))],
                ),
                (
                    "nested",
                    LiteralOrExpression,
                    vec![entry(1, Some("selector")), SequenceIndex(0)],
                ),
                (
                    "jsonpath",
                    LiteralOrExpression,
                    vec![entry(2, Some("type"))],
                ),
            ],
        ),
        (
            "{context: context, selector: literal, type: {version: 4}}",
            vec![
                (
                    "context",
                    LiteralOrExpression,
                    vec![entry(0, Some("context"))],
                ),
                (
                    "literal",
                    LiteralOrExpression,
                    vec![entry(1, Some("selector"))],
                ),
            ],
        ),
        (
            "{context: context, selector: literal, type: jsonpath, 4: extra}",
            vec![
                (
                    "context",
                    LiteralOrExpression,
                    vec![entry(0, Some("context"))],
                ),
                (
                    "literal",
                    LiteralOrExpression,
                    vec![entry(1, Some("selector"))],
                ),
                (
                    "jsonpath",
                    LiteralOrExpression,
                    vec![entry(2, Some("type"))],
                ),
                ("extra", LiteralOrExpression, vec![entry(3, None)]),
            ],
        ),
    ];
    for (raw, expected) in cases {
        for family in 0..4 {
            let value = ValueSource::Literal(Value::Sequence(vec![yaml(raw)]));
            let mut input = workflow(Step::default());
            let declaration = match family {
                0 => {
                    input.parameters.push(Parameter {
                        value,
                        ..Parameter::default()
                    });
                    inherited(0)
                }
                1 => {
                    input.steps[0].parameters.push(Parameter {
                        value,
                        ..Parameter::default()
                    });
                    declared(0)
                }
                2 => {
                    input.steps[0].request_body = Some(RequestBody {
                        payload: Some(value),
                        ..RequestBody::default()
                    });
                    Declaration::RequestBodyPayload
                }
                _ => {
                    input.steps[0].request_body = Some(RequestBody {
                        replacements: vec![Replacement {
                            value,
                            ..Replacement::default()
                        }],
                        ..RequestBody::default()
                    });
                    Declaration::RequestBodyReplacementValue {
                        replacement_index: 0,
                    }
                }
            };
            let expected: Vec<_> = expected
                .iter()
                .map(|(text, mode, suffix)| {
                    let mut path = vec![SequenceIndex(0)];
                    path.extend(suffix);
                    (*text, *mode, declaration, path)
                })
                .collect();
            assert_sites(&sites(&input), &expected);
        }
    }
}

#[test]
fn tags_are_opaque_and_rejected_selector_siblings_still_traverse() {
    let input = workflow(Step {
        request_body: Some(RequestBody {
            payload: Some(ValueSource::Literal(yaml(
                r#"
- !opaque '$excluded.scalar'
- !opaque {nested: '$excluded.map'}
- !opaque ['$excluded.sequence']
- context: literal-context
  selector: literal-selector
  type: jsonpath
  x-note: !opaque {hidden: '$excluded.extension'}
  child:
    context: admitted-child
    selector: '$excluded.child-selector'
    type: jsonpath
- context: tagged-key-context
  selector: tagged-key-selector
  type: jsonpath
  !opaque extra: visible-value
"#,
            ))),
            ..RequestBody::default()
        }),
        ..Step::default()
    });
    let declaration = Declaration::RequestBodyPayload;
    assert_sites(
        &sites(&input),
        &[
            (
                "literal-context",
                LiteralOrExpression,
                declaration,
                vec![SequenceIndex(3), entry(0, Some("context"))],
            ),
            (
                "literal-selector",
                LiteralOrExpression,
                declaration,
                vec![SequenceIndex(3), entry(1, Some("selector"))],
            ),
            (
                "jsonpath",
                LiteralOrExpression,
                declaration,
                vec![SequenceIndex(3), entry(2, Some("type"))],
            ),
            (
                "admitted-child",
                RequiredExpression,
                declaration,
                vec![SequenceIndex(3), entry(4, Some("child")), SelectorContext],
            ),
            (
                "tagged-key-context",
                LiteralOrExpression,
                declaration,
                vec![SequenceIndex(4), entry(0, Some("context"))],
            ),
            (
                "tagged-key-selector",
                LiteralOrExpression,
                declaration,
                vec![SequenceIndex(4), entry(1, Some("selector"))],
            ),
            (
                "jsonpath",
                LiteralOrExpression,
                declaration,
                vec![SequenceIndex(4), entry(2, Some("type"))],
            ),
            (
                "visible-value",
                LiteralOrExpression,
                declaration,
                vec![SequenceIndex(4), entry(3, Some("extra"))],
            ),
        ],
    );
    let mut typed = selector("typed-context");
    typed
        .extensions
        .insert("x-tag".into(), yaml("!opaque ignored"));
    let input = workflow(Step {
        parameters: vec![Parameter {
            value: ValueSource::Selector(typed),
            ..Parameter::default()
        }],
        ..Step::default()
    });
    assert_sites(
        &sites(&input),
        &[(
            "typed-context",
            RequiredExpression,
            declared(0),
            vec![SelectorContext],
        )],
    );
    for raw in [
        "!opaque root",
        "!opaque [root]",
        "!opaque {root: hidden}",
        "null",
        "true",
        "42",
        "1.5",
    ] {
        let input = workflow(Step {
            parameters: vec![Parameter {
                value: ValueSource::Literal(yaml(raw)),
                ..Parameter::default()
            }],
            ..Step::default()
        });
        assert!(sites(&input).is_empty(), "opaque/non-string root: {raw}");
    }
}

#[test]
fn criterion_modes_use_only_normalized_names_not_syntax_form_or_version() {
    let cases = [
        (None, Some(SimpleCondition)),
        (
            Some(SelectorType::Name(" SiMpLe ".into())),
            Some(SimpleCondition),
        ),
        (
            Some(SelectorType::Name(" REGEX ".into())),
            Some(EmbeddedCondition),
        ),
        (
            Some(SelectorType::Name("JsonPath".into())),
            Some(EmbeddedCondition),
        ),
        (
            Some(SelectorType::Name(" xpath ".into())),
            Some(EmbeddedCondition),
        ),
        (Some(SelectorType::Name("unknown".into())), None),
        (Some(SelectorType::Name("  ".into())), None),
        (
            Some(SelectorType::ExpressionType(ExpressionType {
                type_: " SIMPLE ".into(),
                version: "invalid".into(),
                ..ExpressionType::default()
            })),
            Some(SimpleCondition),
        ),
        (
            Some(SelectorType::ExpressionType(ExpressionType {
                type_: " Regex ".into(),
                ..ExpressionType::default()
            })),
            Some(EmbeddedCondition),
        ),
        (
            Some(SelectorType::ExpressionType(ExpressionType {
                type_: " jsonpath ".into(),
                version: "invalid".into(),
                ..ExpressionType::default()
            })),
            Some(EmbeddedCondition),
        ),
        (
            Some(SelectorType::ExpressionType(ExpressionType {
                type_: " XPATH ".into(),
                ..ExpressionType::default()
            })),
            Some(EmbeddedCondition),
        ),
        (
            Some(SelectorType::ExpressionType(ExpressionType::default())),
            None,
        ),
    ];
    for condition in [
        "",
        "quoted '$inputs.fake'",
        "{$inputs.embedded}",
        "ordinary words",
    ] {
        for context in ["", " ", "$response.body"] {
            let input = workflow(Step {
                success_criteria: cases
                    .iter()
                    .map(|(type_, _)| SuccessCriterion {
                        condition: condition.into(),
                        context: context.into(),
                        type_: type_.clone(),
                        ..SuccessCriterion::default()
                    })
                    .collect(),
                ..Step::default()
            });
            let mut expected = Vec::new();
            for (criterion_index, (_, mode)) in cases.iter().enumerate() {
                if !context.is_empty() {
                    expected.push((
                        context,
                        RequiredExpression,
                        Declaration::SuccessCriterionContext { criterion_index },
                        vec![],
                    ));
                }
                if let Some(mode) = mode {
                    expected.push((
                        condition,
                        *mode,
                        Declaration::SuccessCriterionCondition { criterion_index },
                        vec![],
                    ));
                }
            }
            assert_sites(&sites(&input), &expected);
        }
    }
}

#[test]
fn excluded_fields_and_unresolved_references_do_not_create_sites() {
    let input: Workflow = serde_yaml_ng::from_str(
        r#"
workflowId: $excluded.workflow-id
summary: $excluded.summary
description: $excluded.description
dependsOn: [$excluded.workflow-dependency]
inputs:
  type: object
  properties:
    input:
      type: string
      default: $excluded.input-default
parameters:
- reference: $components.parameters.unresolved
  x-note: $excluded.parameter-extension
outputs:
  unused: $excluded.workflow-output
successActions:
- reference: $components.successActions.unresolved
failureActions:
- reference: $components.failureActions.unresolved
x-note: $excluded.workflow-extension
steps:
- stepId: $excluded.step-id
  description: $excluded.step-description
  operationId: $excluded.target
  timeout: 30
  dependsOn: [$excluded.step-dependency]
  parameters:
  - name: $excluded.parameter-name
    in: query
    reference: $components.parameters.unresolved
    x-note: $excluded.parameter-extension
  requestBody:
    reference: $excluded.request-reference
    contentType: $excluded.content-type
    replacements:
    - target: $excluded.replacement-target
      targetSelectorType: $excluded.replacement-type
      x-note: $excluded.replacement-extension
    x-note: $excluded.body-extension
  onSuccess:
  - name: $excluded.action-name
    type: goto
    workflowId: $excluded.action-workflow
    stepId: $excluded.action-step
    criteria:
    - context: $excluded.action-context
      condition: $excluded.action-condition
    parameters:
    - name: action-parameter
      value: $excluded.action-parameter
    value: $excluded.action-value
    x-note: $excluded.action-extension
  onFailure:
  - reference: $components.failureActions.unresolved
  x-note: $excluded.step-extension
"#,
    )
    .unwrap_or_else(|error| panic!("excluded fields fit the current model: {error}"));
    assert!(sites(&input).is_empty());
}

#[test]
fn correlation_id_is_declared_literal_or_expression_even_when_empty() {
    for text in ["$inputs.orderId", "ordinary correlation", ""] {
        let input = workflow(Step {
            correlation_id: Some(text.into()),
            ..Step::default()
        });
        assert_sites(
            &sites(&input),
            &[(
                text,
                LiteralOrExpression,
                Declaration::CorrelationId,
                vec![],
            )],
        );
    }
    assert!(sites(&workflow(Step::default())).is_empty());
}
