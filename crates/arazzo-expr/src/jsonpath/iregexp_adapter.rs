//! Private I-Regexp callbacks behind the upstream `match` and `search` functions.
//!
//! Upstream function callbacks return a boolean, so an operational failure (an
//! I-Regexp resource limit or a backend rejection) cannot travel through them.
//! A synchronous, RAII-scoped, thread-local frame records the first such
//! failure while a query runs: [`with_frame`] installs the frame, runs the
//! query, and turns a recorded failure into an error while dropping the
//! computed output. The guard restores the previous frame on normal, early and
//! unwinding exits, so nested calls and concurrent threads are isolated. No
//! frame ever spans an `await`; everything inside one is synchronous.
//!
//! Every upstream query call in this crate goes through [`with_frame`]. The
//! callbacks are private and reachable only from upstream evaluation, so they
//! never run without an active frame. Invalid pattern syntax or semantics and
//! non-string arguments are logical false; nothing here parses, rewrites or
//! translates regex text.

use std::cell::RefCell;

use iregexp::{Error, IRegexp, MatchMode};
use serde_json::Value;
use serde_json_path::functions::{LogicalType, ValueType};

thread_local! {
    static ACTIVE_FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

/// Operational-error state for one synchronous query on the current thread.
#[derive(Debug)]
struct Frame {
    /// The first operational error observed while this frame was active.
    error: Option<String>,
    /// Number of frames active on this thread, including this one.
    depth: usize,
}

/// Installs a fresh frame on entry and restores the previous one on drop.
struct FrameGuard {
    previous: Option<Frame>,
    depth: usize,
}

impl FrameGuard {
    fn enter() -> Self {
        ACTIVE_FRAME.with(|slot| {
            let mut slot = slot.borrow_mut();
            let previous = slot.take();
            let depth = previous.as_ref().map_or(1, |frame| frame.depth + 1);
            *slot = Some(Frame { error: None, depth });
            Self { previous, depth }
        })
    }

    /// Take the recorded error, verifying the active frame is still this guard's.
    ///
    /// A mismatch means the frame discipline was broken; that is reported as an
    /// error rather than allowing the query to succeed silently.
    fn take_error(&self) -> Result<Option<String>, String> {
        ACTIVE_FRAME.with(|slot| match slot.borrow_mut().as_mut() {
            Some(frame) if frame.depth == self.depth => Ok(frame.error.take()),
            _ => Err(String::from(
                "JSONPath callback frame invariant violated: the active frame is not the one installed for this query",
            )),
        })
    }
}

impl Drop for FrameGuard {
    fn drop(&mut self) {
        // try_with: a drop must never panic, least of all during an unwind.
        let previous = self.previous.take();
        let _ = ACTIVE_FRAME.try_with(|slot| *slot.borrow_mut() = previous);
    }
}

/// Run one synchronous upstream query inside a fresh operational-error frame.
///
/// Returns the query output when no callback recorded an operational error.
/// Otherwise the output is dropped and the error detail is returned, so a
/// negated predicate cannot turn a resource failure into a false match.
pub(super) fn with_frame<T>(query: impl FnOnce() -> T) -> Result<T, String> {
    let guard = FrameGuard::enter();
    let output = query();
    match guard.take_error() {
        Ok(None) => Ok(output),
        Ok(Some(detail)) | Err(detail) => Err(detail),
    }
}

/// Record an operational error in the active frame; only the first is kept.
///
/// Returns `false` when no frame is active. The registered callbacks cannot
/// observe that, because every upstream query runs inside [`with_frame`].
fn record_operational_error(detail: String) -> bool {
    ACTIVE_FRAME.with(|slot| match slot.borrow_mut().as_mut() {
        Some(frame) => {
            frame.error.get_or_insert(detail);
            true
        }
        None => false,
    })
}

fn as_str<'v>(argument: &'v ValueType<'_>) -> Option<&'v str> {
    argument.as_value().and_then(Value::as_str)
}

/// Shared body of both callbacks: compile `pattern` in `mode` and test `value`.
fn evaluate(value: &ValueType<'_>, pattern: &ValueType<'_>, mode: MatchMode) -> LogicalType {
    let (Some(value), Some(pattern)) = (as_str(value), as_str(pattern)) else {
        return LogicalType::False;
    };
    match IRegexp::compile(pattern, mode) {
        Ok(compiled) => compiled.is_match(value).into(),
        Err(Error::Syntax { .. } | Error::Semantic { .. }) => LogicalType::False,
        Err(error) => {
            // ResourceLimit, Backend and any future variant: the pattern was
            // never evaluated, so the whole query is invalid rather than false.
            let recorded = record_operational_error(error.to_string());
            debug_assert!(recorded, "I-Regexp callback ran outside a query frame");
            LogicalType::False
        }
    }
}

#[serde_json_path::function(name = "match")]
fn regexp_match(value: ValueType<'_>, pattern: ValueType<'_>) -> LogicalType {
    evaluate(&value, &pattern, MatchMode::Full)
}

#[serde_json_path::function(name = "search")]
fn regexp_search(value: ValueType<'_>, pattern: ValueType<'_>) -> LogicalType {
    evaluate(&value, &pattern, MatchMode::Search)
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use serde_json::json;

    use super::*;

    fn active_depth() -> Option<usize> {
        ACTIVE_FRAME.with(|slot| slot.borrow().as_ref().map(|frame| frame.depth))
    }

    fn active_error() -> Option<String> {
        ACTIVE_FRAME.with(|slot| slot.borrow().as_ref().and_then(|frame| frame.error.clone()))
    }

    fn text(value: &str) -> ValueType<'static> {
        ValueType::Value(Value::String(value.to_string()))
    }

    #[test]
    fn frame_keeps_only_the_first_operational_error() {
        let result = with_frame(|| {
            assert!(record_operational_error("first".to_string()));
            assert!(record_operational_error("second".to_string()));
            assert_eq!(active_error().as_deref(), Some("first"));
        });
        assert_eq!(result, Err("first".to_string()));
        assert_eq!(active_depth(), None);
    }

    #[test]
    fn sequential_queries_do_not_inherit_an_earlier_error() {
        let failed = with_frame(|| {
            record_operational_error("stale".to_string());
            1
        });
        assert_eq!(failed, Err("stale".to_string()));
        assert_eq!(with_frame(|| 2), Ok(2));
        assert_eq!(active_depth(), None);
    }

    #[test]
    fn nested_frames_are_isolated_and_restore_the_outer_frame() {
        let outer = with_frame(|| {
            assert_eq!(active_depth(), Some(1));
            assert!(record_operational_error("outer".to_string()));
            let clean = with_frame(|| {
                assert_eq!(active_depth(), Some(2));
                assert_eq!(active_error(), None);
                "inner ok"
            });
            assert_eq!(clean, Ok("inner ok"));
            let failing = with_frame(|| {
                record_operational_error("inner".to_string());
            });
            assert_eq!(failing, Err("inner".to_string()));
            assert_eq!(active_depth(), Some(1));
            assert_eq!(active_error().as_deref(), Some("outer"));
            "outer done"
        });
        assert_eq!(outer, Err("outer".to_string()));
        assert_eq!(active_depth(), None);
    }

    #[test]
    fn frames_are_thread_local() {
        let result = with_frame(|| {
            assert!(record_operational_error("main thread".to_string()));
            let worker = std::thread::spawn(|| {
                assert_eq!(active_depth(), None);
                assert!(!record_operational_error("orphan".to_string()));
                with_frame(|| {
                    assert_eq!(active_depth(), Some(1));
                    "worker"
                })
            });
            let joined = match worker.join() {
                Ok(value) => value,
                Err(_) => panic!("worker thread panicked"),
            };
            assert_eq!(joined, Ok("worker"));
            assert_eq!(active_depth(), Some(1));
            assert_eq!(active_error().as_deref(), Some("main thread"));
        });
        assert_eq!(result, Err("main thread".to_string()));
        assert_eq!(active_depth(), None);
    }

    #[test]
    fn unwinding_restores_the_previous_frame() {
        let outer = with_frame(|| {
            let unwound = catch_unwind(AssertUnwindSafe(|| {
                with_frame(|| -> () { panic!("unwind through the inner frame") })
            }));
            assert!(unwound.is_err());
            assert_eq!(active_depth(), Some(1));
            with_frame(|| 5)
        });
        assert_eq!(outer, Ok(Ok(5)));
        assert_eq!(active_depth(), None);

        let top_level = catch_unwind(AssertUnwindSafe(|| {
            with_frame(|| -> () { panic!("unwind through the only frame") })
        }));
        assert!(top_level.is_err());
        assert_eq!(active_depth(), None);
        assert_eq!(with_frame(|| 6), Ok(6));
    }

    #[test]
    fn recording_without_an_active_frame_is_reported_not_swallowed() {
        assert_eq!(active_depth(), None);
        assert!(!record_operational_error("orphan".to_string()));
        assert_eq!(active_depth(), None);
    }

    #[test]
    fn frame_mismatch_is_an_error_not_a_silent_success() {
        let guard = FrameGuard::enter();
        ACTIVE_FRAME.with(|slot| {
            *slot.borrow_mut() = Some(Frame {
                error: None,
                depth: 99,
            })
        });
        assert!(guard.take_error().is_err());
        drop(guard);
        assert_eq!(active_depth(), None);
    }

    #[test]
    fn callbacks_delegate_full_and_search_modes_to_iregexp() {
        let full =
            with_frame(|| bool::from(evaluate(&text("xaby"), &text("a|ab"), MatchMode::Full)));
        let search =
            with_frame(|| bool::from(evaluate(&text("xaby"), &text("a|ab"), MatchMode::Search)));
        assert_eq!(full, Ok(false));
        assert_eq!(search, Ok(true));
    }

    #[test]
    fn non_string_arguments_and_invalid_patterns_are_logical_false() {
        let cases = [
            (ValueType::Value(json!(7)), text("7")),
            (text("a"), ValueType::Value(json!(["a"]))),
            (text("a"), ValueType::Nothing),
            (ValueType::Nothing, text("a")),
            (text("a"), text("[")),
            (text("a"), text("a{3,2}")),
            (text("A"), text("(?i)a")),
        ];
        for (value, pattern) in cases {
            for mode in [MatchMode::Full, MatchMode::Search] {
                let result = with_frame(|| bool::from(evaluate(&value, &pattern, mode)));
                assert_eq!(result, Ok(false), "{value:?} / {pattern:?} / {mode:?}");
            }
        }
    }

    #[test]
    fn resource_failures_are_recorded_as_operational_errors() {
        for mode in [MatchMode::Full, MatchMode::Search] {
            let result =
                with_frame(|| bool::from(evaluate(&text("a"), &text("a{1000000000}"), mode)));
            let detail = match result {
                Err(detail) => detail,
                Ok(value) => panic!("expected an operational error, got {value}"),
            };
            assert!(detail.contains("limit"), "{detail}");
        }
        assert_eq!(active_depth(), None);
    }
}
