use super::*;
use arazzo_spec::{CriterionExpressionType, CriterionType};
use proptest::prelude::*;
use serde_json::json;

fn expression_criterion_type(type_: &str, version: &str) -> CriterionType {
    CriterionType::ExpressionType(CriterionExpressionType {
        type_: type_.to_string(),
        version: version.to_string(),
        ..CriterionExpressionType::default()
    })
}

fn named_criterion(name: &str, context: &str, condition: &str) -> SuccessCriterion {
    SuccessCriterion {
        type_: Some(CriterionType::Name(name.to_string())),
        context: context.to_string(),
        condition: condition.to_string(),
        ..SuccessCriterion::default()
    }
}

fn typed_criterion(type_: CriterionType, context: &str, condition: &str) -> SuccessCriterion {
    SuccessCriterion {
        type_: Some(type_),
        context: context.to_string(),
        condition: condition.to_string(),
        ..SuccessCriterion::default()
    }
}

fn jsonpath_criterion(context: &str, condition: &str) -> SuccessCriterion {
    typed_criterion(
        expression_criterion_type("jsonpath", "draft-goessner-dispatch-jsonpath-00"),
        context,
        condition,
    )
}

fn xpath_criterion(context: &str, condition: &str) -> SuccessCriterion {
    typed_criterion(
        expression_criterion_type("xpath", "xpath-10"),
        context,
        condition,
    )
}

#[test]
fn parse_method_supports_known_verbs() {
    assert_eq!(parse_method("GET /items"), ("GET", "/items"));
    assert_eq!(parse_method("POST /items"), ("POST", "/items"));
    assert_eq!(parse_method("DELETE /items/1"), ("DELETE", "/items/1"));
    assert_eq!(parse_method("PATCH /items/1"), ("PATCH", "/items/1"));
    assert_eq!(parse_method("HEAD /health"), ("HEAD", "/health"));
    assert_eq!(parse_method("OPTIONS /api"), ("OPTIONS", "/api"));
    assert_eq!(parse_method("/items"), ("", "/items"));
    assert_eq!(parse_method(""), ("", ""));
    assert_eq!(parse_method("UNKNOWN /items"), ("", "UNKNOWN /items"));
}

#[test]
fn evaluate_criterion_modes() {
    let cache = RegexCache::new();
    let eval = ExpressionEvaluator::new(EvalContext {
        status_code: Some(200),
        response_body: Some(json!({
            "name":"alice",
            "ok":true,
            "pets":[{"id":1}],
            "items":[
                {"id":1,"ok":false,"pets":[]},
                {"id":2,"ok":true,"pets":[{"id":"a"}]}
            ]
        })),
        ..EvalContext::default()
    });

    let plain = SuccessCriterion {
        condition: "$statusCode == 200".to_string(),
        ..SuccessCriterion::default()
    };
    assert!(evaluate_criterion(&plain, &eval, None, &cache));

    let regex = named_criterion("regex", "$response.body.name", "^[a-z]+$");
    assert!(evaluate_criterion(&regex, &eval, None, &cache));

    let jsonpath = jsonpath_criterion("$response.body", "$.name");
    assert!(evaluate_criterion(&jsonpath, &eval, None, &cache));

    let jp_existence = jsonpath_criterion("$response.body", "$.ok");
    assert!(evaluate_criterion(&jp_existence, &eval, None, &cache));

    let jp_nested = jsonpath_criterion("$response.body", "$.pets[0].id");
    assert!(evaluate_criterion(&jp_nested, &eval, None, &cache));

    let jp_missing = jsonpath_criterion("$response.body", "$.nonexistent");
    assert!(!evaluate_criterion(&jp_missing, &eval, None, &cache));

    let jp_filter_at_ok = jsonpath_criterion("$response.body.items", "$[?(@.ok == true)]");
    assert!(evaluate_criterion(&jp_filter_at_ok, &eval, None, &cache));

    let jp_filter_none = jsonpath_criterion("$response.body.items", "$[?(@.id == 999)]");
    assert!(!evaluate_criterion(&jp_filter_none, &eval, None, &cache));

    let jp_count = jsonpath_criterion("$response.body.items", "$[?(count(@.pets) > 0)]");
    assert!(evaluate_criterion(&jp_count, &eval, None, &cache));

    let jp_and = jsonpath_criterion("$response.body.items", "$[?(@.ok == true && @.id == 2)]");
    assert!(evaluate_criterion(&jp_and, &eval, None, &cache));

    let jp_or = jsonpath_criterion("$response.body.items", "$[?(@.id == 99 || @.id == 1)]");
    assert!(evaluate_criterion(&jp_or, &eval, None, &cache));

    let jp_comparison = jsonpath_criterion("$response.body.items", "$[?(@.id > 1)]");
    assert!(evaluate_criterion(&jp_comparison, &eval, None, &cache));

    let jp_root_count = jsonpath_criterion("$response.body.items", "$[?(count($) > 0)]");
    assert!(evaluate_criterion(&jp_root_count, &eval, None, &cache));
}

#[test]
fn evaluate_criterion_xpath_uses_context_and_condition() {
    let cache = RegexCache::new();
    let criterion = xpath_criterion("$response.body", "//item[1]/title");
    let response = Response {
            status_code: 200,
            headers: BTreeMap::new(),
            body: br#"<?xml version="1.0"?><rss><channel><item><title>Hello</title></item></channel></rss>"#
                .to_vec(),
            body_json: None,
            content_type: ContentType::Xml,
        };
    let eval = ExpressionEvaluator::new(EvalContext::default());

    assert!(evaluate_criterion(
        &criterion,
        &eval,
        Some(&response),
        &cache
    ));
}

#[test]
fn extract_step_refs_and_control_flow() {
    let step = Step {
        step_id: "s2".to_string(),
        target: Some(StepTarget::OperationPath(
            "/items/$steps.s1.outputs.id".to_string(),
        )),
        parameters: vec![arazzo_spec::Parameter {
            name: "q".to_string(),
            in_: Some(ParamLocation::Query),
            value: serde_yaml_ng::Value::String("$steps.s1.outputs.query".to_string()),
            ..arazzo_spec::Parameter::default()
        }],
        outputs: BTreeMap::from([("val".to_string(), "$steps.s1.outputs.value".to_string())]),
        on_failure: vec![OnAction {
            type_: Some(ActionType::Retry),
            criteria: vec![SuccessCriterion {
                condition: "$steps.s1.outputs.code == 429".to_string(),
                ..SuccessCriterion::default()
            }],
            ..OnAction::default()
        }],
        ..Step::default()
    };

    let refs = extract_step_refs(&step);
    assert_eq!(refs, vec!["s1".to_string()]);

    let wf_no_flow = Workflow {
        workflow_id: "no-flow".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/ok".to_string())),
            ..Step::default()
        }],
        ..Workflow::default()
    };
    assert!(!has_control_flow(&wf_no_flow));

    let wf_with_flow = Workflow {
        workflow_id: "with-flow".to_string(),
        steps: vec![Step {
            step_id: "s1".to_string(),
            target: Some(StepTarget::OperationPath("/ok".to_string())),
            on_failure: vec![OnAction {
                type_: Some(ActionType::Goto),
                step_id: "fallback".to_string(),
                ..OnAction::default()
            }],
            ..Step::default()
        }],
        ..Workflow::default()
    };
    assert!(has_control_flow(&wf_with_flow));
}

#[test]
fn evaluate_jsonpath_count_predicate_handles_quotes_bug() {
    let cache = RegexCache::new();
    let eval = ExpressionEvaluator::new(EvalContext {
        response_body: Some(json!({"items": [{"type": "foo)bar"}]})),
        ..EvalContext::default()
    });

    let jp_count = jsonpath_criterion(
        "$response.body",
        "$[?(count(@.items[?(@.type == 'foo)bar')]) > 0)]",
    );
    assert!(
        evaluate_criterion(&jp_count, &eval, None, &cache),
        "Count should handle parentheses in strings"
    );
}

#[test]
fn evaluate_jsonpath_count_predicate_counts_nodelist_cardinality() {
    let cache = RegexCache::new();
    let eval = ExpressionEvaluator::new(EvalContext {
        response_body: Some(json!({
            "items": [
                {"type": "match", "name": "kept", "extra": true},
                {"type": "other", "name": "skipped"}
            ]
        })),
        ..EvalContext::default()
    });

    let matching_object = jsonpath_criterion(
        "$response.body",
        "$[?(count(@.items[?(@.type == 'match')]) == 1)]",
    );
    assert!(evaluate_criterion(&matching_object, &eval, None, &cache));

    let missing = jsonpath_criterion(
        "$response.body",
        "$[?(count(@.items[?(@.type == 'missing')]) == 0)]",
    );
    assert!(evaluate_criterion(&missing, &eval, None, &cache));

    let indexed = jsonpath_criterion("$response.body", "$[?(count(@.items[0]) == 1)]");
    assert!(evaluate_criterion(&indexed, &eval, None, &cache));

    let unsupported = jsonpath_criterion("$response.body", "$[?(count(@.items[0:1]) == 1)]");
    assert!(!evaluate_criterion(&unsupported, &eval, None, &cache));
}

#[test]
fn build_levels_supports_independent_chain_and_cycle() {
    let independent = Workflow {
        workflow_id: "independent".to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                target: Some(StepTarget::OperationPath("/b".to_string())),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };
    let independent_levels = match build_levels(&independent) {
        Ok(levels) => levels,
        Err(err) => panic!("building levels: {err}"),
    };
    assert_eq!(independent_levels, vec![vec![0, 1]]);

    let chain = Workflow {
        workflow_id: "chain".to_string(),
        steps: vec![
            Step {
                step_id: "s1".to_string(),
                target: Some(StepTarget::OperationPath("/one".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                target: Some(StepTarget::OperationPath("/two".to_string())),
                parameters: vec![arazzo_spec::Parameter {
                    name: "from".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("$steps.s1.outputs.id".to_string()),
                    ..arazzo_spec::Parameter::default()
                }],
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };
    let chain_levels = match build_levels(&chain) {
        Ok(levels) => levels,
        Err(err) => panic!("building levels: {err}"),
    };
    assert_eq!(chain_levels, vec![vec![0], vec![1]]);

    let cycle = Workflow {
        workflow_id: "cycle".to_string(),
        steps: vec![
            Step {
                step_id: "s1".to_string(),
                parameters: vec![arazzo_spec::Parameter {
                    name: "from".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("$steps.s2.outputs.id".to_string()),
                    ..arazzo_spec::Parameter::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "s2".to_string(),
                parameters: vec![arazzo_spec::Parameter {
                    name: "from".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("$steps.s1.outputs.id".to_string()),
                    ..arazzo_spec::Parameter::default()
                }],
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };
    let cycle_result = build_levels(&cycle);
    let cycle_err = match cycle_result {
        Ok(_) => panic!("expected cycle detection error"),
        Err(err) => err,
    };
    assert!(cycle_err.message.contains("dependency cycle detected"));
    assert_eq!(cycle_err.kind, RuntimeErrorKind::DependencyCycle);
}

#[test]
fn build_levels_supports_diamond_dependency() {
    let workflow = Workflow {
        workflow_id: "diamond".to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                target: Some(StepTarget::OperationPath("/b".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "c".to_string(),
                parameters: vec![
                    arazzo_spec::Parameter {
                        name: "x".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yaml_ng::Value::String("$steps.a.outputs.id".to_string()),
                        ..arazzo_spec::Parameter::default()
                    },
                    arazzo_spec::Parameter {
                        name: "y".to_string(),
                        in_: Some(ParamLocation::Query),
                        value: serde_yaml_ng::Value::String("$steps.b.outputs.id".to_string()),
                        ..arazzo_spec::Parameter::default()
                    },
                ],
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };

    let levels = match build_levels(&workflow) {
        Ok(levels) => levels,
        Err(err) => panic!("building levels: {err}"),
    };
    assert_eq!(levels, vec![vec![0, 1], vec![2]]);
}

#[test]
fn compute_transitive_deps_no_deps_returns_empty() {
    let workflow = Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                target: Some(StepTarget::OperationPath("/b".to_string())),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };

    let deps = match compute_transitive_deps(&workflow, "a") {
        Ok(d) => d,
        Err(e) => panic!("no-deps step a should resolve: {e}"),
    };
    assert!(deps.is_empty(), "step with no refs should have no deps");

    let deps = match compute_transitive_deps(&workflow, "b") {
        Ok(d) => d,
        Err(e) => panic!("no-deps step b should resolve: {e}"),
    };
    assert!(deps.is_empty(), "step with no refs should have no deps");
}

#[test]
fn compute_transitive_deps_direct_dependency() {
    let workflow = Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                target: Some(StepTarget::OperationPath("/b".to_string())),
                outputs: BTreeMap::from([(
                    "val".to_string(),
                    "$steps.a.outputs.result".to_string(),
                )]),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };

    let deps = match compute_transitive_deps(&workflow, "b") {
        Ok(d) => d,
        Err(e) => panic!("direct dep should resolve: {e}"),
    };
    assert_eq!(deps.len(), 1);
    assert!(deps.contains(&0), "b depends on a (index 0)");
}

#[test]
fn compute_transitive_deps_transitive_chain() {
    // c -> b -> a (transitive)
    let workflow = Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                target: Some(StepTarget::OperationPath("/b".to_string())),
                outputs: BTreeMap::from([(
                    "val".to_string(),
                    "$steps.a.outputs.result".to_string(),
                )]),
                ..Step::default()
            },
            Step {
                step_id: "c".to_string(),
                target: Some(StepTarget::OperationPath("/c".to_string())),
                outputs: BTreeMap::from([("val".to_string(), "$steps.b.outputs.val".to_string())]),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };

    let deps = match compute_transitive_deps(&workflow, "c") {
        Ok(d) => d,
        Err(e) => panic!("transitive chain should resolve: {e}"),
    };
    assert_eq!(deps.len(), 2);
    assert!(deps.contains(&0), "c transitively depends on a");
    assert!(deps.contains(&1), "c directly depends on b");
}

#[test]
fn compute_transitive_deps_diamond() {
    // d -> b, d -> c, b -> a, c -> a
    let workflow = Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![
            Step {
                step_id: "a".to_string(),
                target: Some(StepTarget::OperationPath("/a".to_string())),
                ..Step::default()
            },
            Step {
                step_id: "b".to_string(),
                parameters: vec![arazzo_spec::Parameter {
                    name: "x".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("$steps.a.outputs.id".to_string()),
                    ..arazzo_spec::Parameter::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "c".to_string(),
                parameters: vec![arazzo_spec::Parameter {
                    name: "y".to_string(),
                    in_: Some(ParamLocation::Query),
                    value: serde_yaml_ng::Value::String("$steps.a.outputs.id".to_string()),
                    ..arazzo_spec::Parameter::default()
                }],
                ..Step::default()
            },
            Step {
                step_id: "d".to_string(),
                outputs: BTreeMap::from([
                    ("v1".to_string(), "$steps.b.outputs.r".to_string()),
                    ("v2".to_string(), "$steps.c.outputs.r".to_string()),
                ]),
                ..Step::default()
            },
        ],
        ..Workflow::default()
    };

    let deps = match compute_transitive_deps(&workflow, "d") {
        Ok(d) => d,
        Err(e) => panic!("diamond deps should resolve: {e}"),
    };
    assert_eq!(deps.len(), 3, "d depends on a, b, c");
    assert!(deps.contains(&0));
    assert!(deps.contains(&1));
    assert!(deps.contains(&2));
}

#[test]
fn compute_transitive_deps_unknown_step_errors() {
    let workflow = Workflow {
        workflow_id: "wf".to_string(),
        steps: vec![Step {
            step_id: "a".to_string(),
            ..Step::default()
        }],
        ..Workflow::default()
    };

    let err = match compute_transitive_deps(&workflow, "missing") {
        Ok(_) => panic!("unknown step should error"),
        Err(e) => e,
    };
    assert_eq!(err.kind, RuntimeErrorKind::StepNotFound);
}

#[test]
fn test_xpath_extraction() {
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title>First Story</title>
      <link>https://example.com/1</link>
    </item>
    <item>
      <title>Second Story</title>
      <link>https://example.com/2</link>
    </item>
  </channel>
</rss>"#;
    assert_eq!(
        extract_xpath(xml, "//item[1]/title"),
        Value::String("First Story".to_string())
    );
    assert_eq!(
        extract_xpath(xml, "//item[2]/title"),
        Value::String("Second Story".to_string())
    );
}

#[test]
fn test_xpath_extraction_cdata() {
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <item>
      <title><![CDATA[Story with <special> chars]]></title>
    </item>
  </channel>
</rss>"#;
    assert_eq!(
        extract_xpath(xml, "//item[1]/title"),
        Value::String("Story with <special> chars".to_string())
    );
}

#[test]
fn test_xpath_extraction_atom() {
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:media="http://search.yahoo.com/mrss/">
  <title>top scoring links : technology</title>
  <entry>
    <title>First Reddit Post</title>
    <link href="https://reddit.com/1"/>
  </entry>
  <entry>
    <title>Second Reddit Post</title>
    <link href="https://reddit.com/2"/>
  </entry>
</feed>"#;
    assert_eq!(
        extract_xpath(xml, "//entry[1]/title"),
        Value::String("First Reddit Post".to_string())
    );
    assert_eq!(
        extract_xpath(xml, "//entry[2]/title"),
        Value::String("Second Reddit Post".to_string())
    );
}

#[test]
fn test_xpath_extraction_no_match() {
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0"><channel></channel></rss>"#;
    assert_eq!(extract_xpath(xml, "//item[1]/title"), Value::Null);
}

#[test]
fn test_xpath_extraction_invalid_xml() {
    let body = b"this is not xml at all <broken>";
    assert_eq!(extract_xpath(body, "//item[1]/title"), Value::Null);
}

proptest! {
    #[test]
    fn parse_method_round_trips_known_verbs(
        method in prop_oneof![
            Just("GET"),
            Just("POST"),
            Just("PUT"),
            Just("PATCH"),
            Just("DELETE"),
            Just("HEAD"),
            Just("OPTIONS"),
        ],
        path in "[a-zA-Z0-9/_\\-\\?=&]{0,32}",
    ) {
        let operation_path = format!("{method} /{path}");
        let (parsed_method, parsed_path) = parse_method(&operation_path);
        prop_assert_eq!(parsed_method, method);
        prop_assert_eq!(parsed_path, format!("/{path}"));
    }

    #[test]
    fn build_levels_respects_dependency_order_for_generated_dags(
        size in 1usize..8usize,
        mask in any::<u64>(),
    ) {
        let mut bit_index = 0u32;
        let mut steps = Vec::<Step>::new();
        for idx in 0..size {
            let mut parameters = Vec::new();
            for dep in 0..idx {
                let has_edge = ((mask >> bit_index) & 1) == 1;
                bit_index = bit_index.saturating_add(1);
                if has_edge {
                    parameters.push(arazzo_spec::Parameter {
                        name: format!("p{dep}"),
                        in_: Some(ParamLocation::Query),
                        value: serde_yaml_ng::Value::String(format!(
                            "$steps.s{dep}.outputs.value"
                        )),
                        ..arazzo_spec::Parameter::default()
                    });
                }
            }

            steps.push(Step {
                step_id: format!("s{idx}"),
                target: Some(StepTarget::OperationPath(format!("/s{idx}"))),
                parameters,
                ..Step::default()
            });
        }

        let workflow = Workflow {
            workflow_id: "wf".to_string(),
            steps,
            ..Workflow::default()
        };

        let levels = build_levels(&workflow).unwrap_or_else(|err| {
            panic!("expected DAG levels, got error: {err}");
        });

        let mut flattened = Vec::<usize>::new();
        for level in &levels {
            for step_idx in level {
                flattened.push(*step_idx);
            }
        }
        let mut sorted = flattened.clone();
        sorted.sort_unstable();
        prop_assert_eq!(sorted, (0..size).collect::<Vec<_>>());

        let mut rank = vec![usize::MAX; size];
        for (level_idx, level) in levels.iter().enumerate() {
            for step_idx in level {
                rank[*step_idx] = level_idx;
            }
        }

        for step_idx in 0..size {
            let refs = extract_step_refs(&workflow.steps[step_idx]);
            for dep in refs {
                let dep_idx = dep
                    .trim_start_matches('s')
                    .parse::<usize>()
                    .unwrap_or(usize::MAX);
                prop_assert!(dep_idx < size);
                prop_assert!(rank[dep_idx] < rank[step_idx]);
            }
        }
    }
}
