# Trace Schema Changelog

## `trace.v2` (current)

- Responses record `mediaType`, the parsed `Content-Type`, replacing the `contentType` classification.
- JSON and text response bodies are recorded as exactly decoded text in any charset, redacted and truncated as before.
- Bytes and undecodable response bodies are recorded only as a `bodyBlob` descriptor (`kind`, `size`, `sha256`, `mediaType`) plus `bodyError`; content is never written.
- Removed `bodyPreview` and `bodyLossy`.
- Defined out-of-band bytes slots for later producers: `inputBlobs`, `steps[*].outputBlobs`, `steps[*].request.bodyBlobs`, and the redacted descriptor marker.
- `replay` refuses `trace.v1` files. Migration notes: [trace-schema-v2.md](trace-schema-v2.md#migrating-from-tracev1).

## `trace.v1`

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
