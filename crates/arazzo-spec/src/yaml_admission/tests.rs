use super::{parse_yaml_value, parse_yaml_value_within, yaml_node_budget};
use serde_yaml_ng::Value;

const ALIASED: &str = "a: &x [1, 2]\nb: *x\n";
const ALIASED_NODES: usize = 9;

fn plain(yaml: &str) -> Result<Value, String> {
    serde_yaml_ng::from_str::<Value>(yaml).map_err(|error| error.to_string())
}

fn budgeted(yaml: &str) -> Result<Value, String> {
    parse_yaml_value(yaml.as_bytes()).map_err(|error| error.to_string())
}

fn alias_bomb() -> String {
    let leaves = vec!["x"; 1500].join(", ");
    let level = |alias: &str| vec![alias; 40].join(", ");
    format!(
        "arazzo: 1.1.0\nx-bomb:\n  a: &a [{leaves}]\n  b: &b [{}]\n  c: &c [{}]\n  d: [{}]\n",
        level("*a"),
        level("*b"),
        level("*c"),
    )
}

#[test]
fn budget_is_sixteen_nodes_per_byte_plus_allowance() {
    assert_eq!(yaml_node_budget(0), 1024);
    assert_eq!(yaml_node_budget(100), 2624);
    assert_eq!(yaml_node_budget(usize::MAX), usize::MAX);
}

#[test]
fn admitted_documents_match_the_plain_value_parse() {
    let yaml = "\
root: &r
  int: -3
  big: 18446744073709551615
  float: 1.5e3
  null: ~
  empty:
  bool: true
  quoted: '12'
  core: !!str 12
  custom: !thing {k: [1, 2]}
  list: [a, {b: c}]
copy: *r
? [complex, key]
: value
";
    assert_eq!(budgeted(yaml), plain(yaml));
    assert_eq!(budgeted(""), plain(""));
}

#[test]
fn rejections_other_than_the_budget_keep_the_plain_parse_text() {
    for yaml in [
        "a: 1\na: 2\n",
        "? [k]\n: 1\n? [k]\n: 2\n",
        "a: [",
        "--- 1\n--- 2\n",
        "n: 18446744073709551616\n",
        "a: *missing\n",
    ] {
        let expected = plain(yaml);
        assert!(expected.is_err(), "fixture must fail: {yaml}");
        assert_eq!(budgeted(yaml), expected, "{yaml}");
        let Err(error) = parse_yaml_value(yaml.as_bytes()) else {
            panic!("fixture must fail: {yaml}");
        };
        assert!(!error.is_node_budget_exceeded(), "{yaml}");
    }
}

#[test]
fn counter_admits_up_to_the_budget_and_rejects_one_node_over() {
    let parse = |budget| parse_yaml_value_within(ALIASED.as_bytes(), budget);
    let Ok(under) = parse(ALIASED_NODES + 1) else {
        panic!("one under the budget must parse");
    };
    assert_eq!(Ok(under), plain(ALIASED));
    assert!(
        parse(ALIASED_NODES).is_ok(),
        "exactly the budget must parse"
    );
    let Err(over) = parse(ALIASED_NODES - 1) else {
        panic!("one node over the budget must be rejected");
    };
    assert!(over.is_node_budget_exceeded());
    let message = over.to_string();
    assert!(
        message.contains("YAML node budget exceeded")
            && message.contains(&format!("more than {} nodes", ALIASED_NODES - 1)),
        "{message}"
    );
}

#[test]
fn alias_bomb_is_rejected_naming_the_budget_and_amplification() {
    let bomb = alias_bomb();
    assert!(bomb.len() < 8 * 1024, "bomb is {} bytes", bomb.len());
    let Err(error) = parse_yaml_value(bomb.as_bytes()) else {
        panic!("alias bomb must be rejected");
    };
    assert!(error.is_node_budget_exceeded());
    let message = error.to_string();
    let budget = yaml_node_budget(bomb.len());
    assert!(
        message.contains(&format!(
            "more than {budget} nodes from {} input bytes",
            bomb.len()
        )) && message.contains("16 nodes per input byte"),
        "{message}"
    );
}

#[test]
fn unvalidated_spec_parse_rejects_the_alias_bomb() {
    let Err(error) = crate::parse_unvalidated_bytes(alias_bomb().as_bytes()) else {
        panic!("alias bomb must be rejected");
    };
    assert!(
        error.to_string().contains("YAML node budget exceeded"),
        "{error}"
    );
}

#[test]
fn dense_documents_without_aliases_fit_the_budget() {
    let flow = format!("[{}]", vec!["a"; 50_000].join(","));
    let Ok(Value::Sequence(items)) = budgeted(&flow) else {
        panic!("dense flow sequence must parse");
    };
    assert_eq!(items.len(), 50_000);
}
