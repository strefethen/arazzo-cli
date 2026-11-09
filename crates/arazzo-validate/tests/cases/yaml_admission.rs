use arazzo_validate::{parse_bytes_with_diagnostics, Error};

const HEADER: &str = "\
arazzo: 1.1.0
info:
  title: YAML admission
  version: 1.0.0
sourceDescriptions:
  - name: api
    url: https://api.example.com/openapi.yaml
    type: openapi
";

fn alias_bomb() -> String {
    let leaves = vec!["x"; 1500].join(", ");
    let level = |alias: &str| vec![alias; 40].join(", ");
    format!(
        "{HEADER}workflows:\n  - workflowId: wf\n    steps:\n      - stepId: s\n        operationId: op\n        x-bomb:\n          a: &a [{leaves}]\n          b: &b [{}]\n          c: &c [{}]\n          d: [{}]\n",
        level("*a"),
        level("*b"),
        level("*c"),
    )
}

#[test]
fn alias_bomb_is_a_yaml_parse_error_naming_the_node_budget() {
    let bomb = alias_bomb();
    assert!(bomb.len() < 8 * 1024, "bomb is {} bytes", bomb.len());
    match parse_bytes_with_diagnostics(bomb.as_bytes()) {
        Err(Error::ParseYaml(error)) => {
            let message = error.to_string();
            let budget = arazzo_spec::yaml_node_budget(bomb.len());
            assert!(
                message.contains("YAML node budget exceeded")
                    && message.contains(&format!("more than {budget} nodes")),
                "{message}"
            );
        }
        Err(other) => panic!("expected a YAML parse error, got: {other}"),
        Ok(_) => panic!("alias bomb must be rejected"),
    }
}

#[test]
fn anchors_within_the_budget_validate_like_their_expansion() {
    let anchored = format!(
        "{HEADER}workflows:\n  - workflowId: wf\n    steps:\n      - stepId: a\n        operationId: op\n        parameters: &shared\n          - name: id\n            in: query\n            value: 1\n      - stepId: b\n        operationId: op\n        parameters: *shared\n"
    );
    let expanded = format!(
        "{HEADER}workflows:\n  - workflowId: wf\n    steps:\n      - stepId: a\n        operationId: op\n        parameters:\n          - name: id\n            in: query\n            value: 1\n      - stepId: b\n        operationId: op\n        parameters:\n          - name: id\n            in: query\n            value: 1\n"
    );
    let parse = |yaml: &str| match parse_bytes_with_diagnostics(yaml.as_bytes()) {
        Ok(parsed) => parsed,
        Err(error) => panic!("anchored document must validate: {error}"),
    };
    assert_eq!(parse(&anchored), parse(&expanded));
}
