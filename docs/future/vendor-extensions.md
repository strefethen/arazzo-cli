# Vendor Extensions Preservation Plan (`x-*`)

## Goal

Implement vendor extension support as a standalone foundation feature so extension fields are not dropped during parse/serialize round-trips.

Primary namespace for this project:
- `x-arazzo-cli`

Interpretation rule:
- Preserve all `x-*` extensions.
- Only interpret `x-arazzo-cli` in this codebase (others are pass-through).

## Why This Matters

Today, unknown fields are dropped during deserialization/serialization in `arazzo-spec`, which blocks spec-native feature configuration (for example auth metadata on `sourceDescriptions`).

This currently has an explicit test expectation (`parse_ignores_unknown_fields_and_drops_them_on_serialize`) and should be intentionally reversed for `x-*` keys.

## Current Behavior (Confirmed)

- `arazzo-spec` uses strongly typed structs with no extension maps.
- Unknown fields are silently ignored by serde and not serialized back.
- Existing test in `crates/arazzo-spec/tests/serde_roundtrip.rs` asserts this drop behavior.

## Scope

### In Scope
- Preserve `x-*` fields across parse -> serialize -> parse.
- Add extension containers to key spec model objects.
- Keep extension values as raw YAML values (`serde_yml::Value`) for forward compatibility.
- No runtime interpretation yet beyond storage and roundtrip.

### Out of Scope (This Phase)
- Enforcing schema for `x-arazzo-cli` contents.
- Implementing OAuth behavior itself.
- Linting unknown non-`x-*` keys.

## Data Model Design

Add a reusable type in `crates/arazzo-spec/src/lib.rs`:

- `pub type VendorExtensions = BTreeMap<String, serde_yml::Value>;`
- Add helper:
  - `fn is_vendor_extension_key(k: &str) -> bool { k.starts_with("x-") }`

Add an `extensions` field to model structs via flatten:

```rust
#[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
pub extensions: VendorExtensions
```

Recommended first-pass coverage:
- `ArazzoSpec`
- `Info`
- `SourceDescription`
- `Workflow`
- `Step` (special handling due custom serde path)
- `Parameter`
- `RequestBody`
- `SuccessCriterion`
- `OnAction`
- `Components`
- `SchemaObject`
- `PropertyDef`

This gives broad extension support where users are most likely to annotate behavior.

## Serialization/Deserialization Strategy

### Generic Structs

Use `#[serde(flatten)]` extension map so unknown keys are captured and re-emitted.

### `Step` (Custom Serde)

`Step` uses `StepSerde`; extension support must be added there explicitly:

- Add `extensions: VendorExtensions` to `Step` and `StepSerde`.
- In `Serialize for Step`, copy `self.extensions` into `StepSerde`.
- In `Deserialize for Step`, assign `extensions: raw.extensions`.

### Non-`x-*` Unknown Keys

Two implementation options:

1. Preserve everything unknown (simple, broad pass-through).
2. Preserve only `x-*` (preferred for spec hygiene).

Recommended: option 2.

Implementation detail:
- Keep flattened map.
- Filter to `x-*` after deserialize before storing.
- On serialize, emit only stored extension keys.

## Validation Impact (`arazzo-validate`)

`arazzo-validate` should remain extension-agnostic for this phase.

- No new validation errors for extension fields.
- Existing component resolution behavior should remain unchanged.
- Ensure clone/merge paths preserve `extensions` fields when replacing structs.

Key check:
- Any code path replacing whole structs (for example component `$ref` resolution) must retain the replacement struct’s extensions and must not accidentally drop parent-level extensions.

## CLI/Runtime Impact

No behavior changes in `arazzo-cli` or `arazzo-runtime` required in this phase.

This is a model/parsing contract improvement that unlocks later features.

## Test Plan

## 1) `arazzo-spec` Roundtrip Tests

Replace/update current drop test:
- Old: `parse_ignores_unknown_fields_and_drops_them_on_serialize`
- New:
  - `parse_preserves_vendor_extensions_on_root`
  - `parse_preserves_vendor_extensions_nested`
  - `parse_drops_non_vendor_unknown_fields` (if preserving only `x-*`)

Coverage examples:
- Root: `x-arazzo-cli`
- `sourceDescriptions[*].x-arazzo-cli`
- `workflows[*].x-arazzo-cli`
- `steps[*].x-arazzo-cli`
- `onSuccess[*].x-arazzo-cli`

## 2) `arazzo-validate` Parse Tests

Add tests proving extensions do not trigger structural validation failures.

## 3) Example Fixture

Add one example spec fixture containing realistic `x-arazzo-cli` blocks and assert parse/serialize stability.

## 4) Regression Guard for `Step`

Dedicated test ensuring `Step` custom serde preserves `x-*` (this is the riskiest spot).

## Implementation Steps

1. Add `VendorExtensions` alias + helpers in `arazzo-spec`.
2. Add `extensions` field to core structs listed above.
3. Update `Step` + `StepSerde` custom serialization logic.
4. Add filtering logic for non-`x-*` unknown keys (if choosing option 2).
5. Update tests in `crates/arazzo-spec/tests/serde_roundtrip.rs`.
6. Add new targeted tests in `arazzo-validate` ensuring pass-through compatibility.
7. Run full workspace verification.

## Effort Estimate

- Model + serde updates: **1-2 days**
- Test rewrites/additions: **0.5-1 day**
- Validation/regression hardening + docs: **0.5 day**

Total: **2-3 engineering days** for a clean standalone delivery.

## Acceptance Criteria

- `x-*` fields survive parse -> serialize -> parse across covered objects.
- `x-arazzo-cli` extension survives everywhere it appears.
- Non-extension unknown fields behavior is explicitly defined and tested.
- No regressions in existing validate/run behavior.
- All gates pass:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `cargo test --workspace`

## Follow-Up (After This Lands)

Add a focused plan for interpreting `x-arazzo-cli.auth`:
- Auth schema shape definition.
- CLI/runtime wiring for loopback OAuth.
- Security review for token handling and trace redaction.
