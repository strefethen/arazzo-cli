# MCP workflow timeouts

Both MCP server entrypoints use the same fixed policy. `http_timeout_seconds`
defaults to 30 seconds and accepts integers from 1 through 30.
`execution_timeout_seconds` defaults to 300 seconds and accepts integers from
1 through 300. Tool schemas advertise the same bounds that the handler enforces
before constructing the runtime or starting execution.

Only omission selects the default. Null, booleans, strings, arrays, objects,
zero, negatives, fractional JSON numbers (including 1.0), values above the
maximum and integer overflow are rejected. The tool returns `isError: true`
with a text JSON payload carrying `MCP_INVALID_ARGUMENT` and a fixed field/range
message. Invalid input never falls back to a default. Configuration does not
allow raising these ceilings.

`ServerState` keeps its public `specs` and `allowed_dirs` fields, allowing
external struct literals as before. Its internal fieldless accessor supplies
the fixed timeout policy. `McpServerConfig` and both runners use that same
policy without storing a new private field in `ServerState`.

The dispatcher remains serial. Completed workflows stop their owned watchdog;
deadline expiry and cancellation follow the synchronized runtime cancellation
contract described in [Runtime cancellation](runtime-cancellation.md).
These limits do not claim to stop detached broker tasks.
