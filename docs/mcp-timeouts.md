# MCP workflow timeouts and limits

`http_timeout_seconds` defaults to its ceiling, 30 seconds, and accepts
integers from 1 through that ceiling. `execution_timeout_seconds` defaults to
its ceiling, 300 seconds, and accepts integers from 1 through that ceiling.
An operator can change either ceiling per launch with `--max-http-timeout` and
`--max-execution-timeout`, which take durations such as `45s` or `10m` and are
rounded down to whole seconds. Tool schemas advertise the configured ceilings,
and the handler enforces them before constructing the runtime or starting
execution.

Only omission selects the default. Null, booleans, strings, arrays, objects,
zero, negatives, fractional JSON numbers (including 1.0), values above the
ceiling and integer overflow are rejected. The tool returns `isError: true`
with a text JSON payload carrying `MCP_INVALID_ARGUMENT` and a fixed field/range
message. Invalid input never falls back to a default.

The server also bounds what it reads, queues and returns. Each limit has a
flag on the standalone `arazzo-mcp` binary and a field on `McpLimits`:

| Flag | Default | Bounds |
|---|---:|---|
| `--max-request-bytes` | 262144 | one inbound frame: a newline-delimited line, a header line, or a declared `Content-Length` |
| `--max-pending-requests` | 16 | decoded requests waiting for the serial dispatcher |
| `--max-result-bytes` | 1048576 | the text of one tool result or tool error |
| `--max-response-body-bytes` | 4194304 | one HTTP response body during `run_workflow` |
| `--max-document-bytes` | 4194304 | one file read by a tool or source loader |
| `--max-http-timeout` | 30s | ceiling and default for `http_timeout_seconds` |
| `--max-execution-timeout` | 300s | ceiling and default for `execution_timeout_seconds` |

Every value must be at least 1 (1 second for the timeouts); the server refuses
to start otherwise. The active limits are printed on stderr at startup, on the
file policy line.

A frame over `--max-request-bytes`, a frame with more than 16 header lines, or
an unparsable frame header is answered with one JSON-RPC error,
`{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Request exceeds the server frame limit"}}`
(or `"Invalid request frame"`), and the session ends. A complete frame within
the limit that is not a valid request is skipped. When the backlog is full,
the reader stops reading until the dispatcher catches up. A tool result over
`--max-result-bytes` is replaced by an `MCP_LIMIT_EXCEEDED` tool error, and a
response body over `--max-response-body-bytes` fails the workflow with
`RUNTIME_RESPONSE_TOO_LARGE`.

The dispatcher runs one tool call at a time but stays responsive while a
`run_workflow` is in flight: `ping`, `initialize`, `tools/list` and other
non-tool requests are answered immediately, and up to `--max-pending-requests`
further `tools/call` requests wait in order until the active run finishes.
Once that queue is full, the server waits for the active run to finish before
reading more input, so a `ping`, a cancellation notice or a closed input is not
seen until then.
`notifications/cancelled` naming the active run's request id cancels it, and
no response is written for that id. Closing the input, or a rejected frame that
ends the session, cancels the active run before the server returns, and the
run gets no response.

Completed workflows stop their owned watchdog;
deadline expiry and cancellation follow the synchronized runtime cancellation
contract described in [Runtime cancellation](runtime-cancellation.md).
These limits do not claim to stop detached broker tasks.
