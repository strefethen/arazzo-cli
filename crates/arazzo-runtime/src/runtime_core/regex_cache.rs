//! Bounded compiled-regex results owned by one engine, shared by its steps.

use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use regex::Regex;

const CAPACITY: usize = 256;
const MAX_PATTERN_BYTES: usize = 4096;
type Compiled = Result<Regex, regex::Error>;

pub(crate) struct RegexCache {
    cache: Mutex<HashMap<String, Compiled>>,
    #[cfg(test)]
    compilations: AtomicUsize,
}

impl RegexCache {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            #[cfg(test)]
            compilations: AtomicUsize::new(0),
        }
    }

    pub(crate) fn is_match(&self, pattern: &str, text: &str) -> Result<bool, regex::Error> {
        // The owned result outlives every map guard, including on cache hits.
        self.compiled(pattern, Regex::new)
            .map(|regex| regex.is_match(text))
    }

    fn compiled(&self, pattern: &str, compile: impl FnOnce(&str) -> Compiled) -> Compiled {
        let retain = pattern.len() <= MAX_PATTERN_BYTES;
        if retain {
            let cached = {
                let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
                cache.get(pattern).cloned()
            };
            if let Some(compiled) = cached {
                return compiled;
            }
        }

        // No map guard is live during compilation. Cold concurrent misses may
        // compile redundantly; both successful and failed results are reusable.
        #[cfg(test)]
        self.compilations.fetch_add(1, Ordering::Relaxed);
        let compiled = compile(pattern);
        if !retain {
            return compiled;
        }

        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        // A racing miss may already have published this key. Check before
        // clearing so an existing entry at capacity does not evict the map.
        if let Some(existing) = cache.get(pattern) {
            return existing.clone();
        }
        if cache.len() == CAPACITY {
            cache.clear();
        }
        cache.insert(pattern.to_owned(), compiled.clone());
        compiled
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::runtime_core::criteria::evaluate_criterion_detailed;
    use arazzo_expr::{EvalContext, ExpressionEvaluator};
    use arazzo_spec::{CriterionType, SuccessCriterion};
    use serde_json::json;
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn a_repeated_pattern_is_compiled_once() {
        let cache = RegexCache::new();
        assert!(cache.is_match("^ready$", "ready").unwrap());
        assert!(!cache.is_match("^ready$", "waiting").unwrap());
        assert!(cache.is_match("^ready$", "ready").unwrap());
        assert_eq!(cache.compilations.load(Ordering::Relaxed), 1);
        // Each engine starts with its own cold cache.
        let other = RegexCache::new();
        assert!(other.is_match("^ready$", "ready").unwrap());
        assert_eq!(other.compilations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn an_invalid_pattern_is_compiled_once_with_identical_criterion_errors() {
        let cache = RegexCache::new();
        let criterion = SuccessCriterion {
            context: "$response.body#/text".to_owned(),
            condition: "[".to_owned(),
            type_: Some(CriterionType::Name("regex".to_owned())),
            ..SuccessCriterion::default()
        };
        let evaluator = ExpressionEvaluator::new(EvalContext {
            response_body: Some(json!({"text": "ready"})),
            ..EvalContext::default()
        });
        let expected = format!(
            "invalid regex: {}",
            Regex::new(&criterion.condition).unwrap_err()
        );
        let first = evaluate_criterion_detailed(&criterion, &evaluator, None, &cache);
        let second = evaluate_criterion_detailed(&criterion, &evaluator, None, &cache);
        for evaluation in [&first, &second] {
            assert_eq!(evaluation.error.as_deref(), Some(expected.as_str()));
            assert!(!evaluation.condition_result);
            assert!(!evaluation.matched);
            assert_eq!(evaluation.type_name, "regex");
            assert_eq!(evaluation.type_version, None);
            assert_eq!(evaluation.condition, "[");
            assert_eq!(evaluation.context_expr, criterion.context);
            assert_eq!(evaluation.context_value, json!("ready"));
            assert!(evaluation.warnings.is_empty());
        }
        assert_eq!(format!("{first:?}"), format!("{second:?}"));
        assert_eq!(cache.compilations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn capacity_plus_one_clears_and_retains_the_newest_pattern() {
        let cache = RegexCache::new();
        for index in 0..=CAPACITY {
            let text = format!("value-{index}");
            assert!(cache.is_match(&format!("^{text}$"), &text).unwrap());
            assert!(cache.cache.lock().unwrap().len() <= CAPACITY);
        }
        let retained = cache.cache.lock().unwrap();
        assert_eq!(retained.len(), 1, "capacity eviction clears wholesale");
        assert!(retained.contains_key(&format!("^value-{CAPACITY}$")));
        drop(retained);
        assert!(cache
            .is_match(&format!("^value-{CAPACITY}$"), &format!("value-{CAPACITY}"))
            .unwrap());
        assert_eq!(cache.compilations.load(Ordering::Relaxed), CAPACITY + 1);
        assert!(cache.is_match("^value-0$", "value-0").unwrap());
        assert_eq!(cache.compilations.load(Ordering::Relaxed), CAPACITY + 2);
    }

    #[test]
    fn exact_utf8_byte_boundary_is_retained_for_valid_and_invalid_patterns() {
        let cache = RegexCache::new();
        let valid = "é".repeat(MAX_PATTERN_BYTES / 2);
        let invalid = format!("{}[", "é".repeat((MAX_PATTERN_BYTES - 2) / 2)) + "a";
        for pattern in [&valid, &invalid] {
            assert_eq!(pattern.len(), MAX_PATTERN_BYTES);
            let expected = Regex::new(pattern).map(|regex| regex.is_match(&valid));
            for _ in 0..2 {
                assert_eq!(cache.is_match(pattern, &valid), expected);
            }
            assert!(cache.cache.lock().unwrap().contains_key(pattern));
        }
        assert_eq!(cache.compilations.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn oversized_valid_and_invalid_patterns_evaluate_without_retention() {
        let cache = RegexCache::new();
        let valid = "é".repeat(MAX_PATTERN_BYTES / 2) + "a";
        let invalid = "é".repeat(MAX_PATTERN_BYTES / 2) + "[";
        for pattern in [&valid, &invalid] {
            assert_eq!(pattern.len(), MAX_PATTERN_BYTES + 1);
            let expected = Regex::new(pattern).map(|regex| regex.is_match(&valid));
            for _ in 0..2 {
                assert_eq!(cache.is_match(pattern, &valid), expected);
                assert!(cache.cache.lock().unwrap().is_empty());
            }
        }
        assert_eq!(cache.compilations.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn compilation_runs_without_the_map_lock_and_rechecks_raced_entries() {
        let cache = RegexCache::new();
        for index in 0..CAPACITY - 1 {
            assert!(cache
                .is_match(&index.to_string(), &index.to_string())
                .unwrap());
        }
        let compiled = cache
            .compiled("raced", |pattern| {
                // This probe is deterministic: compilation can acquire the
                // same mutex and publish a raced result without deadlocking.
                assert!(cache.cache.try_lock().is_ok());
                assert!(cache.is_match(pattern, "raced").unwrap());
                assert_eq!(cache.cache.lock().unwrap().len(), CAPACITY);
                Regex::new(pattern)
            })
            .unwrap();
        assert!(compiled.is_match("raced"));
        assert_eq!(cache.cache.lock().unwrap().len(), CAPACITY);
        assert_eq!(cache.compilations.load(Ordering::Relaxed), CAPACITY + 1);
    }

    #[test]
    fn poisoned_map_lock_is_recovered() {
        let cache = RegexCache::new();
        let _ = std::panic::catch_unwind(|| {
            let _guard = cache.cache.lock().unwrap();
            panic!("poison the cache map");
        });
        assert!(cache.is_match("^ready$", "ready").unwrap());
        assert!(cache.is_match("^ready$", "ready").unwrap());
        assert_eq!(cache.compilations.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn warm_concurrent_hits_do_not_compile_again() {
        let cache = Arc::new(RegexCache::new());
        assert!(cache.is_match("^ready$", "ready").unwrap());
        assert!(cache.is_match("[", "ready").is_err());
        let start = Arc::new(Barrier::new(8));
        let workers = (0..8)
            .map(|_| {
                let cache = Arc::clone(&cache);
                let start = Arc::clone(&start);
                thread::spawn(move || {
                    start.wait();
                    for _ in 0..32 {
                        assert!(cache.is_match("^ready$", "ready").unwrap());
                        assert!(!cache.is_match("^ready$", "waiting").unwrap());
                        assert!(cache.is_match("[", "ready").is_err());
                    }
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(cache.compilations.load(Ordering::Relaxed), 2);
        assert_eq!(cache.cache.lock().unwrap().len(), 2);
    }

    #[test]
    fn simultaneous_cold_misses_return_correct_results_and_stay_bounded() {
        for pattern in ["^ready$", "["] {
            let cache = Arc::new(RegexCache::new());
            let compiling = Arc::new(Barrier::new(8));
            let workers = (0..8)
                .map(|_| {
                    let cache = Arc::clone(&cache);
                    let compiling = Arc::clone(&compiling);
                    thread::spawn(move || {
                        cache.compiled(pattern, |pattern| {
                            compiling.wait();
                            Regex::new(pattern)
                        })
                    })
                })
                .collect::<Vec<_>>();
            let expected = Regex::new(pattern).map(|regex| regex.is_match("ready"));
            for worker in workers {
                assert_eq!(
                    worker.join().unwrap().map(|regex| regex.is_match("ready")),
                    expected
                );
            }
            assert_eq!(cache.compilations.load(Ordering::Relaxed), 8);
            assert_eq!(cache.cache.lock().unwrap().len(), 1);
            assert_eq!(cache.is_match(pattern, "ready"), expected);
            assert_eq!(cache.compilations.load(Ordering::Relaxed), 8);
        }
    }
}
