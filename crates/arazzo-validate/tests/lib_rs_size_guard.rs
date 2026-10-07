#![forbid(unsafe_code)]

//! Guard: `arazzo-validate`'s `src/lib.rs` never grows past its line cap.
//!
//! `lib.rs` is a recorded God module (see `god-files.md`). Its disposition is
//! incremental containment: new validator responsibilities go in a private
//! sibling module under `src/`, and `lib.rs` may gain only module
//! declarations, delegating calls, and variants of the existing public
//! diagnostic types. This test turns that rule into a build failure by
//! capping the file's line count. `include_str!` makes `lib.rs` a build input
//! of this target, so Cargo re-runs the guard whenever the file changes.

/// Line count of `src/lib.rs` when the guard landed (2026-10-07). Lower it
/// when `lib.rs` shrinks; raising it needs Steve's approval.
const LIB_RS_MAX_LINES: usize = 3_729;

const LIB_RS_SOURCE: &str = include_str!("../src/lib.rs");

fn assert_within_cap(source: &str, cap: usize) {
    let lines = source.lines().count();
    assert!(
        lines <= cap,
        "crates/arazzo-validate/src/lib.rs has {lines} lines, over its cap of {cap}. \
         lib.rs is a recorded God module (god-files.md). New validator \
         responsibilities belong in a private sibling module under \
         crates/arazzo-validate/src/; lib.rs may gain only module declarations, \
         delegating calls, and variants of the existing public diagnostic types. \
         LIB_RS_MAX_LINES may be lowered when lib.rs shrinks, but raising it \
         needs Steve's approval."
    );
}

#[test]
fn lib_rs_stays_within_line_cap() {
    assert_within_cap(LIB_RS_SOURCE, LIB_RS_MAX_LINES);
}

#[test]
#[should_panic(expected = "god-files.md")]
fn source_over_cap_fails_the_guard() {
    let cap = 3;
    let over_cap = "line\n".repeat(cap + 1);
    assert_within_cap(&over_cap, cap);
}
