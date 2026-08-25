# Arazzo Runtime `engine_execution` Test Inventory

**Captured:** 2026-08-22 at `04ee287`
**Source:** `crates/arazzo-runtime/tests/engine_execution.rs`
**Purpose:** Exact grounding for
`plans/current/arazzo-runtime-engine-execution-decomposition.md` and its future
Tier 2 movement tickets. This assessment records current reachability and
logical cohorts; the plan owns the target architecture.

## Baseline

- Physical lines: 5,018.
- Git blob: `5552a97d1d6f5b27ddc01e5236c48fbc675813a1`.
- SHA-256: `49fae06f13fbbb933fed386bc433f6ad99e64efc5fafd42bdabba9afc9a30258`.
- Test entrypoints: 96 (92 Tokio, four synchronous), none ignored.
- Other function items: 11 (eight free helpers, three observer methods).
- Other owned items: two observer structs, both with exact `#[derive(Default)]`,
  and three impl blocks.
- No constants, statics, macros, enums, unions, type aliases, traits, or nested
  modules are owned by the file.
- Test attributes: 92 exact `#[tokio::test]` attributes, four exact `#[test]`
  attributes, four attached `#[allow(deprecated)]` attributes, and no
  `ignore`, `cfg`, `should_panic`, or custom runtime-flavor attributes.
- Documentation attributes: 35 `///` lines attached to 11 tests, inventoried
  below.
- `cargo test -p arazzo-runtime --test engine_execution -- --list`: 96 tests,
  zero benchmarks.
- `cargo test -p arazzo-runtime --test engine_execution`: 96 passed, zero
  failed.

## Exact logical cohorts

Every baseline test appears exactly once.

### Workflow dependencies (6)

- `workflow_dependencies_fail_closed_and_accept_explicit_completion`
- `nested_workflow_completion_satisfies_later_dependency`
- `failed_nested_workflow_counts_as_completion_for_goto_dependency`
- `nested_dependency_rejection_preserves_stable_error_and_makes_no_request`
- `unrelated_completion_evidence_does_not_satisfy_unknown_dependency`
- `channel_execution_and_dry_run_fail_before_http_with_the_same_error`

### Workflow execution (4)

- `execute_sequential_steps`
- `execute_honors_external_cancel_flag`
- `execute_workflow_not_found`
- `execute_default_sequential_without_on_success`

### Request parameters (2)

- `workflow_parameter_merge_matches_validation_effective_list`
- `execute_path_param_with_special_chars_is_percent_encoded`

### Action routing (10)

- `execute_on_success_goto_interpolation_bug`
- `execute_failure_no_handler`
- `execute_on_failure_end`
- `execute_on_success_end`
- `execute_on_failure_goto`
- `execute_on_success_goto`
- `execute_on_failure_criteria_matching`
- `execute_on_failure_criteria_none_match`
- `execute_goto_errors`
- `execute_circular_goto_returns_iteration_limit_exceeded`

### Retry limits (7)

- `execute_on_failure_retry`
- `execute_retry_exceeds_max`
- `execute_retry_custom_limit`
- `execute_retry_custom_limit_exceeded`
- `execute_retry_limit_zero_means_no_retries`
- `arazzo_11_retry_limit_counts_omitted_zero_one_and_two`
- `arazzo_11_workflow_failure_retry_fallback_uses_the_same_omitted_limit`

### Retry fallthrough (5)

- `arazzo_11_exhausted_retry_falls_through_to_matching_end_with_final_trace_route`
- `arazzo_11_exhausted_retry_skips_nonmatching_actions_then_goto`
- `arazzo_11_exhausted_retry_nonmatching_later_actions_returns_stable_limit`
- `arazzo_11_later_action_errors_do_not_become_retry_exhaustion`
- `arazzo_11_later_retry_reference_failure_does_not_resume_action_scanning`

### Retry decisions (5)

- `arazzo_11_retry_sites_are_independent_in_serial_and_execute_step_observers`
- `arazzo_11_omitted_retry_limit_stays_absent_in_trace_and_uses_one_observer_attempt`
- `retry_after_trace_and_observer_keep_configured_and_effective_delays_distinct`
- `exhausted_retry_does_not_convert_or_schedule_an_unused_finite_overflow_delay`
- `invalid_retry_after_is_rejected_before_exhausted_retry_fallthrough`

### Retry timing (4)

- `arazzo_11_later_retry_delay_timeout_does_not_resume_action_scanning`
- `arazzo_11_later_retry_delay_cancellation_does_not_resume_action_scanning`
- `execute_retry_with_delay`
- `execute_retry_delay_honors_execution_timeout`

### Retry references (5)

- `execute_retry_workflow_reference_recovers_then_retries`
- `execute_retry_workflow_reference_parameters_become_callee_inputs`
- `execute_retry_step_reference_executes_with_outputs_visible`
- `execute_retry_reference_runs_once_per_retried_attempt`
- `execute_retry_reference_failure_fails_workflow`

### Subworkflows (10)

- `execute_sub_workflow_step`
- `execute_sub_workflow_with_inputs`
- `execute_sub_workflow_failure`
- `execute_goto_workflow`
- `goto_workflow_action_parameters_become_callee_inputs`
- `goto_workflow_without_parameters_forwards_caller_inputs`
- `execute_recursion_guard`
- `execute_sub_workflow_not_found`
- `sub_workflow_interpolated_param_preserves_number_type`
- `sub_workflow_selector_param_preserves_number_type`

### Operation resolution (11)

- `load_openapi_spec_and_resolve_operation_ids`
- `load_openapi_spec_not_found_and_skips_non_http_fields`
- `execute_operation_id_and_path_params`
- `execute_operation_id_not_loaded`
- `relative_source_dry_run_derives_base_from_servers`
- `relative_source_missing_file_fails_build`
- `relative_source_unparseable_file_fails_build`
- `relative_source_without_base_dir_fails_build`
- `explicit_openapi_spec_overrides_source_operation`
- `legacy_absolute_source_url_stays_literal_in_expressions`
- `relative_source_openapi_32_json_document_indexes_and_derives_base`

### Selectors (2)

- `structured_selectors_share_one_runtime_across_callers`
- `selector_failures_return_null_and_visible_trace_diagnostics`

### Replacements (6)

- `replacements_overlay_json_payload_before_send`
- `replacements_with_expression_value_resolves_against_inputs`
- `replacements_with_dependent_step_outputs_resolves_in_order`
- `replacements_dry_run_emits_merged_body`
- `dry_run_resolves_explicit_null_and_empty_parameter_and_replacement_values`
- `replacements_warnings_propagate_to_step_trace`

### Dry run (3)

- `dry_run_captures_requests_and_headers`
- `dry_run_resolves_expressions_and_skips_http_calls`
- `dry_run_multi_step_and_custom_headers`

### Execute step (10)

- `execute_step_standalone_no_deps`
- `execute_step_with_transitive_deps`
- `execute_step_goto_into_filtered_gap_resumes_at_or_after_target`
- `execute_step_no_deps_flag_standalone_succeeds`
- `execute_step_no_deps_flag_with_refs_fails`
- `execute_step_unknown_step_errors`
- `execute_step_unknown_workflow_errors`
- `execute_step_retries_on_failure`
- `execute_step_retry_limit_exceeded`
- `execute_step_on_success_end_stops_early`

### Runtime contract (4)

- `runtime_error_is_displayable`
- `runtime_error_kind_has_stable_code`
- `runtime_error_chain_preserved`
- `internal_runtime_api_version_is_v1`

### Response limits (2)

- `response_exceeding_size_limit_produces_error`
- `response_within_size_limit_succeeds`

## Attached documentation ledger

These 11 doc blocks move with the same-named owner case, not its root adapter.
Each SHA-256 covers the exact contiguous UTF-8 `///` lines including trailing
LF bytes:

| Owner case | Baseline lines | Doc SHA-256 |
|---|---:|---|
| `workflow_parameter_merge_matches_validation_effective_list` | 456-458 | `e3e011dde282c1fd868102dd8eeb6b5d3cb87646ab513c99f4e51c065876de61` |
| `retry_after_trace_and_observer_keep_configured_and_effective_delays_distinct` | 1473-1474 | `6316f7a17c3f825afef4639f737664914a4e049577651ef9e60b353645c25915` |
| `exhausted_retry_does_not_convert_or_schedule_an_unused_finite_overflow_delay` | 1538-1539 | `387797b10f8e6a7b08c87e23be4e0bb60396550633210fdbab1599d1e2cbe206` |
| `invalid_retry_after_is_rejected_before_exhausted_retry_fallthrough` | 1594-1595 | `d944986f163940a95c1815a40fee7301587a4a61970a290a175176b82eb3e9f8` |
| `execute_retry_workflow_reference_recovers_then_retries` | 1989-1994 | `090d8d9a7d862a5c110c810b8dade35764ac91e334467fc2dc60713a043ef25c` |
| `execute_retry_workflow_reference_parameters_become_callee_inputs` | 2089-2091 | `a3243fb4cfcbc7ff2afc6ab2b208fb86859223f01629a13656e4a0d24e09fede` |
| `execute_retry_step_reference_executes_with_outputs_visible` | 2189-2192 | `01b8882ccf7dfb5879b24dc062b89e72bfb88f98d2f286dc4cb98e0efc8c5003` |
| `execute_retry_reference_runs_once_per_retried_attempt` | 2265-2266 | `91542f676e4443a26fe4ef89494937d486d2eb445d129725e41abf640ff2dfbf` |
| `execute_retry_reference_failure_fails_workflow` | 2328-2330 | `13b3e77875f11ce6fa2c711f390da8957b8996aa664f098a099f7ac7c75a8ee4` |
| `goto_workflow_action_parameters_become_callee_inputs` | 2900-2905 | `01dea0fdde166ec3e193a53712943002081632f8d1f97ce789ed1f823f124022` |
| `goto_workflow_without_parameters_forwards_caller_inputs` | 2991-2992 | `3e0abfca556e8b01d4fea036b888dba33b3843e485731c0e283c97a4918547d2` |

## Helper ownership and dependencies

- `RetryDelayObserver`, its inherent `delays` method, and its
  `ExecutionObserver::on_event` method serve retry-decision tests only. The
  struct and both impl blocks move together into `retry_decisions.rs`.
- `RetryCancellationObserver` and its `ExecutionObserver::on_event` method serve
  retry-timing cancellation only. The struct and impl block move together into
  `retry_timing.rs`.
- `request_body_with_replacements`, `replacement`, and `captured_string` serve
  replacement tests only.
- `testdata_dir`, `relative_source_spec`, and `list_pets_workflow` serve
  operation-resolution tests only.
- `selector` moves to `selector_fixture.rs` with `pub(super)` visibility; it is
  used by retry-reference, subworkflow, and selector cohorts.
- `parse_json_body` moves to `captured_response.rs` with `pub(super)`
  visibility; it is used by selector and replacement cohorts.
- Exact cross-leaf edges are `selectors -> {selector_fixture,
  captured_response}`, `subworkflows/retry_references -> selector_fixture`, and
  `replacements -> captured_response`.
- The target consumes `MockHttpResponse`, `start_server`, `make_spec`,
  `make_spec_with_base`, `new_test_engine`, `success_200`, `to_yaml`, and
  `TestObserver` from `tests/common/mod.rs`. It does not consume that module's
  redirect, concurrent-server, TLS-server, request-log, trace-hook, or
  observer-engine helpers.

## Hard references

The following six top-level test names are executable conformance evidence and
therefore intentional compatibility entrypoints:

| Root adapter / same-named case | Case owner |
|---|---|
| `arazzo_11_retry_limit_counts_omitted_zero_one_and_two` | `retry_limits.rs` |
| `arazzo_11_retry_sites_are_independent_in_serial_and_execute_step_observers` | `retry_decisions.rs` |
| `arazzo_11_exhausted_retry_nonmatching_later_actions_returns_stable_limit` | `retry_fallthrough.rs` |
| `retry_after_trace_and_observer_keep_configured_and_effective_delays_distinct` | `retry_decisions.rs` |
| `invalid_retry_after_is_rejected_before_exhausted_retry_fallthrough` | `retry_decisions.rs` |
| `exhausted_retry_does_not_convert_or_schedule_an_unused_finite_overflow_delay` | `retry_decisions.rs` |

They are referenced by
`crates/arazzo-cli/tests/conformance/{runtime,expressions}.json` and validated by
`recognized_non_ignored_test_items` / `validate_evidence_reference` in
`crates/arazzo-cli/tests/conformance_manifest.rs`.

During migration, every original body becomes a same-named `pub(super)` case
without a test attribute in its assigned owner. The six rows above are awaited by
physical root `#[tokio::test]` adapters. The other 86 Tokio cases use
`root_async_case!(owner::name)` and the four runtime-contract cases use
`root_sync_case!(owner::name)`, preserving all 96 root harness names and
attribute flavors. The four `#[allow(deprecated)]` attributes remain attached
to their owner cases: one in `replacements.rs` and three in `dry_run.rs`.

## Complete non-test item ledger

The eight free helpers move exactly once:

| Item | Owner |
|---|---|
| `request_body_with_replacements` | `replacements.rs` |
| `replacement` | `replacements.rs` |
| `selector` | `selector_fixture.rs` |
| `captured_string` | `replacements.rs` |
| `parse_json_body` | `captured_response.rs` |
| `testdata_dir` | `operation_resolution.rs` |
| `relative_source_spec` | `operation_resolution.rs` |
| `list_pets_workflow` | `operation_resolution.rs` |

The three methods are `RetryDelayObserver::delays`,
`RetryDelayObserver::on_event`, and `RetryCancellationObserver::on_event`.
Together with their two structs and three impl blocks, they move only with the
observer ownership stated above. Imports are reconstructed per owner from actual
use; no wildcard prelude or production visibility change is allowed.

## Frozen-ledger rule

This snapshot grounds the plan but is not yet the implementation ledger. Ready
or in-progress behavior writers must land or be resolved first. Before
acceptance or ticket creation, a matching pre/post `tkt rev` survey captures a
fresh commit, blob, exact root-name/owner/adapter map, attributes, exact doc-text
hash/attachment, helper/struct/impl ownership, and Cargo reachability. The ledger
is immutable across every migration slice; an unavoidable inventoried change
stops the chain for re-planning.
