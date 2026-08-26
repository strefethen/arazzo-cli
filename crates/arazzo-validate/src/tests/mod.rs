//! Unit-test fragments for the `arazzo-validate` library target.
//!
//! Ordered include index only: every item stays in this `tests` module so
//! fully qualified `tests::<name>` paths, helper visibility, and item order
//! are byte-identical to the former inline module. No test bodies, fixtures,
//! or helpers belong in this file.

include!("cases/public_api.rs");
include!("cases/required_values.rs");
include!("cases/components.rs");
include!("cases/document.rs");
include!("cases/required_collections_01.rs");
include!("cases/sources.rs");
include!("cases/async_steps.rs");
include!("cases/dependencies.rs");
include!("cases/step_targets.rs");
include!("cases/expressions.rs");
include!("cases/workflow_fields.rs");
include!("cases/action_references_01.rs");
include!("cases/action_targets.rs");
include!("cases/component_inputs.rs");
include!("cases/retry_fields.rs");
include!("cases/querystring.rs");
include!("cases/parameter_context.rs");
include!("cases/direct_validation.rs");
include!("cases/action_parameters.rs");
include!("cases/identifiers_01.rs");
include!("cases/required_collections_02.rs");
include!("cases/action_references_02.rs");
include!("cases/identifiers_02.rs");
include!("cases/unknown_fields.rs");
include!("cases/action_fixed_fields.rs");
