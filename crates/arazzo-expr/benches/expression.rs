#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use serde_json::{json, Value};

use arazzo_expr::{EvalContext, ExpressionEvaluator};

/// Build a realistic EvalContext with nested response bodies and step outputs.
fn rich_context() -> EvalContext {
    let mut steps = BTreeMap::new();
    let mut get_user_outputs = BTreeMap::new();
    get_user_outputs.insert(
        "body".to_string(),
        json!({
            "data": {
                "user": {
                    "id": 42,
                    "name": "Alice",
                    "email": "alice@example.com",
                    "roles": ["admin", "editor"],
                    "profile": {
                        "bio": "Software engineer",
                        "avatar": "https://example.com/avatar.png"
                    }
                },
                "items": [
                    {"id": 1, "name": "Item A", "price": 9.99},
                    {"id": 2, "name": "Item B", "price": 19.99},
                    {"id": 3, "name": "Item C", "price": 29.99}
                ]
            }
        }),
    );
    get_user_outputs.insert("statusCode".to_string(), json!(200));
    steps.insert("getUser".to_string(), get_user_outputs);

    let mut list_orders_outputs = BTreeMap::new();
    list_orders_outputs.insert(
        "body".to_string(),
        json!({
            "orders": [
                {"id": 101, "total": 99.99, "status": "shipped"},
                {"id": 102, "total": 149.50, "status": "pending"}
            ],
            "pagination": {"page": 1, "total_pages": 5}
        }),
    );
    steps.insert("listOrders".to_string(), list_orders_outputs);

    let mut inputs = BTreeMap::new();
    inputs.insert("userId".to_string(), json!(42));
    inputs.insert("apiKey".to_string(), json!("sk-test-1234567890"));
    inputs.insert("verbose".to_string(), json!(true));

    let mut response_headers = BTreeMap::new();
    response_headers.insert("content-type".to_string(), "application/json".to_string());
    response_headers.insert("x-request-id".to_string(), "abc-123-def".to_string());
    response_headers.insert("x-rate-limit-remaining".to_string(), "42".to_string());

    EvalContext {
        inputs,
        steps,
        outputs: BTreeMap::new(),
        status_code: Some(200),
        method: Some("GET".to_string()),
        url: Some("https://api.example.com/users/42".to_string()),
        request_headers: BTreeMap::new(),
        request_query: BTreeMap::new(),
        request_path: BTreeMap::new(),
        request_body: None,
        source_descriptions: BTreeMap::new(),
        response_headers,
        response_body: Some(json!({
            "data": {
                "user": {
                    "id": 42,
                    "name": "Alice",
                    "email": "alice@example.com"
                }
            }
        })),
    }
}

fn bench_evaluate(c: &mut Criterion) {
    let eval = ExpressionEvaluator::new(rich_context());

    let mut group = c.benchmark_group("evaluate");

    // Simple namespace lookups (fast path)
    group.bench_function("inputs_simple", |b| {
        b.iter(|| eval.evaluate(black_box("$inputs.userId")))
    });

    group.bench_function("statusCode", |b| {
        b.iter(|| eval.evaluate(black_box("$statusCode")))
    });

    group.bench_function("method", |b| b.iter(|| eval.evaluate(black_box("$method"))));

    // Step output lookup (common hot path)
    group.bench_function("steps_output", |b| {
        b.iter(|| eval.evaluate(black_box("$steps.getUser.outputs.body")))
    });

    // Deep nested path traversal (exercises tokenize_path)
    group.bench_function("deep_nested_path", |b| {
        b.iter(|| {
            eval.evaluate(black_box(
                "$steps.getUser.outputs.body.data.user.profile.bio",
            ))
        })
    });

    // Array index access
    group.bench_function("array_index", |b| {
        b.iter(|| eval.evaluate(black_box("$steps.getUser.outputs.body.data.items[0].name")))
    });

    // Response body access
    group.bench_function("response_body", |b| {
        b.iter(|| eval.evaluate(black_box("$response.body.data.user.name")))
    });

    // Response header access
    group.bench_function("response_header", |b| {
        b.iter(|| eval.evaluate(black_box("$response.header.x-request-id")))
    });

    // Missing key (worst case — full traversal then null)
    group.bench_function("missing_key", |b| {
        b.iter(|| {
            eval.evaluate(black_box(
                "$steps.getUser.outputs.body.data.user.nonexistent",
            ))
        })
    });

    group.finish();
}

fn bench_resolve_value(c: &mut Criterion) {
    let eval = ExpressionEvaluator::new(rich_context());

    let mut group = c.benchmark_group("resolve_value");

    // Literal string (no-op fast path)
    group.bench_function("literal", |b| {
        b.iter(|| eval.resolve_value(black_box("hello world")))
    });

    // Full expression dispatch
    group.bench_function("expression", |b| {
        b.iter(|| eval.resolve_value(black_box("$inputs.userId")))
    });

    // String interpolation
    group.bench_function("interpolation_single", |b| {
        b.iter(|| eval.resolve_value(black_box("User {$inputs.userId}")))
    });

    group.bench_function("interpolation_multiple", |b| {
        b.iter(|| {
            eval.resolve_value(black_box(
                "User {$inputs.userId} with key {$inputs.apiKey} (verbose={$inputs.verbose})",
            ))
        })
    });

    group.finish();
}

fn bench_evaluate_condition(c: &mut Criterion) {
    let eval = ExpressionEvaluator::new(rich_context());

    let mut group = c.benchmark_group("evaluate_condition");

    // Simple comparison
    group.bench_function("simple_eq", |b| {
        b.iter(|| eval.evaluate_condition(black_box("$statusCode == 200")))
    });

    // Boolean AND
    group.bench_function("and_condition", |b| {
        b.iter(|| {
            eval.evaluate_condition(black_box("$statusCode == 200 && $inputs.verbose == true"))
        })
    });

    // Boolean OR
    group.bench_function("or_condition", |b| {
        b.iter(|| eval.evaluate_condition(black_box("$statusCode == 200 || $statusCode == 201")))
    });

    group.finish();
}

fn bench_interpolate_string(c: &mut Criterion) {
    let eval = ExpressionEvaluator::new(rich_context());

    let mut group = c.benchmark_group("interpolate_string");

    // No expressions (fast scan)
    group.bench_function("no_expressions", |b| {
        b.iter(|| eval.interpolate_string(black_box("plain text with no expressions")))
    });

    // Single expression
    group.bench_function("single_expr", |b| {
        b.iter(|| eval.interpolate_string(black_box("Hello {$inputs.userId}")))
    });

    // Multiple expressions
    group.bench_function("three_exprs", |b| {
        b.iter(|| eval.interpolate_string(black_box("{$method} {$url} => {$statusCode}")))
    });

    group.finish();
}

/// Benchmark scaling: how does evaluation time grow with path depth?
fn bench_path_depth_scaling(c: &mut Criterion) {
    // Build a deeply nested JSON value
    fn build_nested(depth: usize) -> Value {
        let mut v = json!("leaf");
        for i in (0..depth).rev() {
            let key = format!("level{i}");
            v = json!({ key: v });
        }
        v
    }

    let depths = [1, 3, 5, 8, 12];

    let mut group = c.benchmark_group("path_depth_scaling");
    for &depth in &depths {
        let nested = build_nested(depth);
        let path: String = (0..depth)
            .map(|i| format!("level{i}"))
            .collect::<Vec<_>>()
            .join(".");
        let expr = format!("$response.body.{path}");

        let ctx = EvalContext {
            response_body: Some(nested),
            ..Default::default()
        };
        let eval = ExpressionEvaluator::new(ctx);

        group.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, _| {
            b.iter(|| eval.evaluate(black_box(&expr)))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_evaluate,
    bench_resolve_value,
    bench_evaluate_condition,
    bench_interpolate_string,
    bench_path_depth_scaling,
);
criterion_main!(benches);
