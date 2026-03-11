#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use serde_json::{json, Value};

use arazzo_expr::{EvalContext, ExpressionEvaluator};
fn bench_evaluator() -> ExpressionEvaluator {
    let ctx = EvalContext {
        inputs: BTreeMap::from([("userId".to_string(), json!(42))]),
        status_code: Some(200),
        method: Some("GET".to_string()),
        response_body: Some(json!({
            "data": { "id": 42, "name": "Alice", "active": true }
        })),
        response_headers: BTreeMap::from([
            ("content-type".to_string(), "application/json".to_string()),
        ]),
        ..Default::default()
    };
    ExpressionEvaluator::new(ctx)
}

/// Benchmark regex compilation — the #1 optimization target.
///
/// This measures the cost of `Regex::new()` called per criterion evaluation.
/// After optimization (caching), only the first call should pay compilation cost.
fn bench_regex_compilation(c: &mut Criterion) {
    let mut group = c.benchmark_group("regex_compilation");

    let patterns = [
        ("simple_literal", r"Alice"),
        ("char_class", r"^[A-Za-z]+$"),
        ("email_pattern", r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$"),
        ("url_pattern", r"https?://[^\s/$.?#].[^\s]*"),
        ("complex_alternation", r"^(GET|POST|PUT|DELETE|PATCH)\s+/api/v[0-9]+/"),
    ];

    for (name, pattern) in &patterns {
        group.bench_with_input(BenchmarkId::new("compile", name), pattern, |b, pat| {
            b.iter(|| regex::Regex::new(black_box(pat)))
        });
    }

    // Measure compile + match (what currently happens per criterion eval)
    let text = "alice@example.com";
    for (name, pattern) in &patterns {
        group.bench_with_input(
            BenchmarkId::new("compile_and_match", name),
            pattern,
            |b, pat| {
                b.iter(|| {
                    let re = regex::Regex::new(black_box(pat)).unwrap();
                    re.is_match(black_box(text))
                })
            },
        );
    }

    // Measure match-only (what it would cost with caching)
    for (name, pattern) in &patterns {
        let re = regex::Regex::new(pattern).unwrap();
        group.bench_with_input(
            BenchmarkId::new("match_only", name),
            pattern,
            |b, _pat| b.iter(|| re.is_match(black_box(text))),
        );
    }

    group.finish();
}

/// Benchmark repeated regex criterion evaluations — simulates N steps
/// each checking the same regex pattern (the real-world hot path).
fn bench_regex_repeated(c: &mut Criterion) {
    let mut group = c.benchmark_group("regex_repeated");

    let pattern = r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$";
    let text = "alice@example.com";

    for count in [1, 5, 10, 25, 50] {
        group.bench_with_input(
            BenchmarkId::new("uncached", count),
            &count,
            |b, &n| {
                b.iter(|| {
                    for _ in 0..n {
                        let re = regex::Regex::new(black_box(pattern)).unwrap();
                        let _ = re.is_match(black_box(text));
                    }
                })
            },
        );

        let cached_re = regex::Regex::new(pattern).unwrap();
        group.bench_with_input(
            BenchmarkId::new("cached", count),
            &count,
            |b, &n| {
                b.iter(|| {
                    for _ in 0..n {
                        let _ = cached_re.is_match(black_box(text));
                    }
                })
            },
        );
    }

    group.finish();
}

/// Benchmark expression evaluation in criterion context
/// (the condition evaluation path for simple/default criteria).
fn bench_condition_evaluation(c: &mut Criterion) {
    let eval = bench_evaluator();

    let mut group = c.benchmark_group("condition_evaluation");

    group.bench_function("simple_statuscode_eq", |b| {
        b.iter(|| eval.evaluate_condition(black_box("$statusCode == 200")))
    });

    group.bench_function("compound_and", |b| {
        b.iter(|| {
            eval.evaluate_condition(black_box(
                "$statusCode == 200 && $response.body.data.active == true",
            ))
        })
    });

    group.bench_function("string_comparison", |b| {
        b.iter(|| {
            eval.evaluate_condition(black_box(
                "$response.body.data.name == \"Alice\"",
            ))
        })
    });

    group.finish();
}

/// Benchmark response body cloning — measures the cost of cloning
/// JSON values of various sizes (optimization target #3: Arc sharing).
fn bench_response_body_clone(c: &mut Criterion) {
    let mut group = c.benchmark_group("response_body_clone");

    let sizes: Vec<(&str, Value)> = vec![
        ("tiny_1field", json!({"id": 1})),
        (
            "small_10fields",
            json!({
                "id": 1, "name": "Alice", "email": "a@b.com",
                "active": true, "role": "admin", "created": "2024-01-01",
                "updated": "2024-06-01", "score": 95.5, "level": 3, "verified": true
            }),
        ),
        (
            "medium_nested",
            {
                let items: Vec<Value> = (0..50)
                    .map(|i| {
                        json!({
                            "id": i,
                            "name": format!("Item {i}"),
                            "price": i as f64 * 1.99,
                            "tags": ["electronics", "sale"],
                            "metadata": {"weight": 0.5, "dimensions": "10x10x10"}
                        })
                    })
                    .collect();
                json!({"data": {"items": items, "total": 50, "page": 1}})
            },
        ),
        (
            "large_100_items",
            {
                let items: Vec<Value> = (0..100)
                    .map(|i| {
                        json!({
                            "id": i,
                            "name": format!("Product {i}"),
                            "description": "A longer description that simulates real API responses with more text content",
                            "price": i as f64 * 2.49,
                            "category": "electronics",
                            "tags": ["new", "featured", "sale"],
                            "inventory": {"warehouse_a": 10, "warehouse_b": 25},
                            "reviews": [
                                {"rating": 5, "text": "Great product!"},
                                {"rating": 4, "text": "Pretty good"}
                            ]
                        })
                    })
                    .collect();
                json!({"data": items, "pagination": {"page": 1, "total": 1000}})
            },
        ),
    ];

    for (name, value) in &sizes {
        group.bench_with_input(BenchmarkId::new("clone", name), value, |b, v| {
            b.iter(|| black_box(v).clone())
        });
    }

    group.finish();
}

/// Benchmark case-insensitive header lookup — current O(n) scan
/// vs potential O(1) HashMap with normalized keys.
fn bench_header_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("header_lookup");

    // Simulate a response with varying header counts
    let header_counts = [5, 10, 20, 50];

    for &count in &header_counts {
        let mut headers = BTreeMap::new();
        for i in 0..count {
            headers.insert(format!("X-Custom-Header-{i}"), format!("value-{i}"));
        }
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        // Current approach: linear scan with eq_ignore_ascii_case
        group.bench_with_input(
            BenchmarkId::new("btreemap_case_insensitive", count),
            &headers,
            |b, hdrs| {
                b.iter(|| {
                    hdrs.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(black_box("content-type")))
                        .map(|(_, v)| v)
                })
            },
        );

        // Optimized approach: pre-lowercased HashMap
        let normalized: std::collections::HashMap<String, String> = headers
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect();

        group.bench_with_input(
            BenchmarkId::new("hashmap_normalized", count),
            &normalized,
            |b, hdrs| b.iter(|| hdrs.get(black_box("content-type"))),
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_regex_compilation,
    bench_regex_repeated,
    bench_condition_evaluation,
    bench_response_body_clone,
    bench_header_lookup,
);
criterion_main!(benches);
