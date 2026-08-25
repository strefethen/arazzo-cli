//! The simple-condition ` matches ` operator.
//!
//! `matches` is **not** in the Arazzo 1.1 simple-condition grammar, which
//! `AGENTS.md` states as `< <= > >= == != ! && || () [] .`. It is a tracked
//! non-conformance slated for removal pending the maintainer decision in
//! ac-ec3a1 / ac-67bf5. Nothing here is evidence that the operator is intended;
//! this module exists so the operator that ships today fails loudly instead of
//! silently, and so deleting it later is one file plus one `match` arm.
//!
//! The module owns the operator end to end: stringify the operands, decide the
//! match, and report a pattern that does not compile. The compiled-pattern
//! table is this module's private implementation of that concern, not a shared
//! service.
//!
//! # Why this is not `runtime_core::criteria::RegexCache`
//!
//! The runtime has its own compiled-pattern cache for `type: regex` criteria.
//! It is deliberately not reused here, and the two differ for reasons specific
//! to each: `RegexCache` is owned by one `Engine`, so it is scoped and reset by
//! the engine's lifetime and needs no bound; this one is a process static
//! because an `ExpressionEvaluator` is rebuilt for every step, which is why it
//! carries the bounds below instead. `RegexCache` also matches while holding
//! its lock, which is correct for its access pattern but is what the
//! `Arc<Regex>` note below avoids here. Consolidating them is a separate
//! change, not a thing to do inline.
//!
//! # Why a cache
//!
//! The operator resolves its right operand at evaluation time, so without a
//! cache the same pattern is recompiled for every criterion, on every step, on
//! every retry attempt — and compilation dominates. `benches/expression.rs`
//! measures it: `matches_operator/uncached_baseline` (compile plus match,
//! nothing else) runs ~17.8 µs, while `matches_operator/cached_pattern`
//! evaluates a whole condition — expression resolution included — in ~1.5 µs.
//! Re-run that bench rather than trusting these figures.
//!
//! # Why `Arc<Regex>` and not `Regex`
//!
//! `Regex: Clone` is *not* a refcount bump, so caching a bare `Regex` and
//! cloning it out of the guard would throw most of the win away.
//! `regex-automata`'s `impl Clone for Regex` deliberately builds a fresh
//! `CachePool` outside the shared `Arc` — its own field comment says the pool
//! sits there "so that cloning a `Regex` results in creating a fresh
//! `CachePool`" — so every clone allocates a pool and the first match against
//! it pays a full `create_cache()`. Cloning an `Arc<Regex>` is the refcount
//! bump the naive version only claimed to be, and it still keeps the match off
//! the lock.
//!
//! # Bounds
//!
//! A pattern can be produced by a runtime expression, so its text may come from
//! response data rather than the document. Three limits keep that from growing
//! the process without bound:
//!
//! - at most [`CAPACITY`] entries, and the table is cleared wholesale when it
//!   fills, so a burst of one-shot patterns cannot permanently displace the
//!   document's literal patterns;
//! - keys are at most [`MAX_PATTERN_BYTES`], so a large response value is
//!   matched against but never retained;
//! - values are bounded by the regex crate's own default NFA size limit.
//!
//! Note the count bound alone would not be a memory bound: `regex::Error`
//! renders the offending pattern verbatim, so an uncached large pattern would
//! otherwise be retained twice over in the error text.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use regex::Regex;
use serde_json::Value;

use crate::{to_string_value, ExpressionWarning};

/// Distinct patterns retained per process. Documents carry a handful of literal
/// patterns, so this is far above the working set of ordinary use and exists
/// only to bound the response-derived case.
const CAPACITY: usize = 256;

/// Longest pattern retained. Longer patterns still evaluate correctly, they are
/// just not worth a cache entry keyed on response-sized text.
const MAX_PATTERN_BYTES: usize = 4096;

/// Longest reported pattern text. `regex::Error` echoes the whole pattern, and
/// the warning is re-emitted on every evaluation and every retry attempt, so an
/// untruncated message can flood stderr, `--json`, and the trace file.
const MAX_REPORTED_ERROR_BYTES: usize = 200;

/// A compile outcome. Failures are memoized alongside successes so a malformed
/// pattern on a retried step is not re-parsed on every attempt. `regex::Error`
/// is `Clone`, so it is stored typed and rendered only at the point of use.
type Compiled = Result<Arc<Regex>, regex::Error>;

static SHARED: LazyLock<PatternCache> = LazyLock::new(PatternCache::new);

/// Evaluate `left matches right` for the enclosing `condition`.
///
/// A pattern that does not compile is not a non-match: it appends a warning
/// phrased the way the `type: regex` criterion path phrases it, so a malformed
/// pattern is distinguishable from a legitimate non-match, and fails closed.
pub(crate) fn evaluate(
    condition: &str,
    left: &Value,
    right: &Value,
    warnings: &mut Vec<ExpressionWarning>,
) -> bool {
    match SHARED.is_match(&to_string_value(right), &to_string_value(left)) {
        Ok(matched) => matched,
        Err(err) => {
            warnings.push(ExpressionWarning {
                expression: condition.to_string(),
                message: format!("invalid regex: {}", truncate(&err.to_string())),
            });
            false
        }
    }
}

/// Clip a rendered compile error to [`MAX_REPORTED_ERROR_BYTES`] on a character
/// boundary, marking the elision so a reader knows the text is partial.
fn truncate(message: &str) -> String {
    if message.len() <= MAX_REPORTED_ERROR_BYTES {
        return message.to_string();
    }
    let mut end = MAX_REPORTED_ERROR_BYTES;
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}… ({} bytes elided)",
        &message[..end],
        message.len() - end
    )
}

struct PatternCache {
    table: Mutex<HashMap<String, Compiled>>,
    /// Compiles performed, i.e. lookups that missed. The point of the cache is
    /// that this stops growing for a repeated pattern, which is not observable
    /// from the entry count alone.
    #[cfg(test)]
    compiles: std::sync::atomic::AtomicUsize,
}

impl PatternCache {
    fn new() -> Self {
        Self {
            table: Mutex::new(HashMap::new()),
            #[cfg(test)]
            compiles: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn is_match(&self, pattern: &str, text: &str) -> Result<bool, regex::Error> {
        // Matching happens here, outside the guard, against the shared compiled
        // program — no per-clone scratch pool is built or discarded.
        self.compile(pattern).map(|re| re.is_match(text))
    }

    fn compile(&self, pattern: &str) -> Compiled {
        if let Some(compiled) = self.lock().get(pattern) {
            return compiled.clone();
        }

        // Compiling outside the guard keeps one slow pattern from blocking
        // every other evaluation. Two threads racing on the same miss duplicate
        // the compile, which is wasted work and never a wrong result.
        #[cfg(test)]
        self.compiles
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let compiled = Regex::new(pattern).map(Arc::new);

        if pattern.len() <= MAX_PATTERN_BYTES {
            let mut table = self.lock();
            // Clear rather than refuse once full: a long-lived process (the MCP
            // stdio server serves arbitrary specs for a whole session) must not
            // let the first CAPACITY patterns it ever saw win permanently.
            if table.len() >= CAPACITY {
                table.clear();
            }
            table.insert(pattern.to_string(), compiled.clone());
        }
        compiled
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Compiled>> {
        // Poisoning is recoverable here: the map is only ever mutated by a
        // single `clear`/`insert` pair, so no panic can leave it partially
        // written.
        self.table.lock().unwrap_or_else(|err| err.into_inner())
    }

    #[cfg(test)]
    fn retained(&self) -> usize {
        self.lock().len()
    }

    #[cfg(test)]
    fn compiles(&self) -> usize {
        self.compiles.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::{EvalContext, ExpressionEvaluator};

    #[test]
    fn a_repeated_pattern_is_compiled_once() {
        let cache = PatternCache::new();

        assert_eq!(cache.is_match("^a+$", "aaa"), Ok(true));
        assert_eq!(cache.is_match("^a+$", "bbb"), Ok(false));
        assert_eq!(cache.is_match("^b+$", "bbb"), Ok(true));

        // The entry count alone would also be 2 for an implementation that
        // recompiled and overwrote every time; the compile count is what proves
        // the lookup short-circuits the compile.
        assert_eq!(cache.retained(), 2);
        assert_eq!(cache.compiles(), 2);
    }

    #[test]
    fn a_cached_entry_wins_over_recompiling() {
        let cache = PatternCache::new();

        // Seed a deliberately wrong program under a pattern it does not match.
        let sentinel = "^never-matches-this$";
        cache
            .lock()
            .insert(sentinel.to_string(), Regex::new("^sentinel$").map(Arc::new));

        // A compile of `sentinel` would answer `false`; the cached entry says
        // `true`, so the answer names which path ran.
        assert_eq!(cache.is_match(sentinel, "sentinel"), Ok(true));
        assert_eq!(cache.compiles(), 0);
    }

    #[test]
    fn a_pattern_that_does_not_compile_reports_its_error() {
        let cache = PatternCache::new();

        let err = match cache.is_match("[invalid", "anything") {
            Err(err) => err,
            Ok(matched) => panic!("an uncompilable pattern must not report {matched}"),
        };

        assert!(!err.to_string().is_empty());
        // Memoized like any other outcome: the repeat does not re-parse.
        assert_eq!(cache.retained(), 1);
        assert!(cache.is_match("[invalid", "anything").is_err());
        assert_eq!(cache.compiles(), 1);
    }

    #[test]
    fn a_full_table_is_cleared_rather_than_frozen() {
        let cache = PatternCache::new();

        for index in 0..CAPACITY {
            assert_eq!(
                cache.is_match(&format!("^{index}$"), &index.to_string()),
                Ok(true)
            );
        }
        assert_eq!(cache.retained(), CAPACITY);

        // The overflow entry is retained, so a pattern arriving after a burst
        // is not permanently condemned to recompile.
        assert_eq!(cache.is_match("^overflow$", "overflow"), Ok(true));
        assert_eq!(cache.retained(), 1);
        let compiles = cache.compiles();
        assert_eq!(cache.is_match("^overflow$", "other"), Ok(false));
        assert_eq!(cache.compiles(), compiles);
    }

    #[test]
    fn an_oversized_pattern_matches_but_is_not_retained() {
        let cache = PatternCache::new();
        let pattern = "a".repeat(MAX_PATTERN_BYTES + 1);
        let text = "a".repeat(MAX_PATTERN_BYTES + 1);

        assert_eq!(cache.is_match(&pattern, &text), Ok(true));
        assert_eq!(cache.is_match(&pattern, "b"), Ok(false));

        assert_eq!(cache.retained(), 0);
        assert_eq!(cache.compiles(), 2);
    }

    #[test]
    fn a_reported_compile_error_is_truncated() {
        let pattern = format!("[{}", "x".repeat(10_000));
        let mut warnings = Vec::new();

        let result = evaluate("cond", &json!("text"), &json!(pattern), &mut warnings);

        assert!(!result);
        assert_eq!(warnings.len(), 1);
        let message = &warnings[0].message;
        assert!(message.starts_with("invalid regex: "));
        assert!(message.contains("bytes elided"), "got: {message}");
        // The whole warning stays small even though the pattern was not.
        assert!(message.len() < 400, "length {}", message.len());
    }

    /// A `matches` pattern that does not compile still fails the condition, but
    /// it is no longer indistinguishable from a legitimate non-match: the
    /// compile error is reported the way the `type: regex` criterion path
    /// reports it.
    #[test]
    fn an_uncompilable_pattern_warns_where_a_plain_non_match_stays_silent() {
        let mut ctx = EvalContext::default();
        Arc::make_mut(&mut ctx.steps).insert(
            "s1".to_string(),
            BTreeMap::from([("email".to_string(), json!("alice@example.com"))]),
        );
        let eval = ExpressionEvaluator::new(ctx);

        let condition = r#"$steps.s1.outputs.email matches "[invalid""#;
        let (result, warnings) = eval.evaluate_condition_with_diagnostics(condition);

        assert!(!result);
        let warning = match warnings
            .iter()
            .find(|w| w.message.contains("invalid regex"))
        {
            Some(warning) => warning,
            None => panic!("an uncompilable pattern must warn, got: {warnings:?}"),
        };
        assert_eq!(warning.expression, condition);

        let (result, warnings) = eval
            .evaluate_condition_with_diagnostics(r#"$steps.s1.outputs.email matches "^[0-9]+""#);
        assert!(!result);
        assert!(warnings.is_empty(), "got: {warnings:?}");
    }
}
