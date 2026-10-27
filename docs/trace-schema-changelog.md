# Trace Schema Changelog

## `trace.v1` (current)

- Introduced stable trace artifact schema for CLI `run --trace`.
- Added deterministic per-step sequence ordering.
- Included request/response summaries, criteria outcomes, flow decision, outputs, and error fields.
- Added built-in redaction for sensitive headers/query params/JSON keys.
- Added optional `run.sequentialFallbacks`: each workflow that ran its steps one at a time although `parallel` was requested, with the reason.
- Added optional `steps[*].message` for AsyncAPI receive steps (Kafka): transport, host, topic, partition, offset, timestamp, key, headers, payload preview and payload with the response-body redaction and truncation rules, and `simulated` for dry runs. `steps[*].kind` may now be `asyncapi`.

## Change policy

1. Additive fields are allowed within the same schema version.
2. Renames/removals/type changes require a new schema version (for example `trace.v2`).
3. Any version bump must include:
   - updated schema docs
   - migration notes
   - contract tests covering both old and new behavior where applicable
