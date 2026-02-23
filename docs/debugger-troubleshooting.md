# Debugger Troubleshooting

## Adapter Exits Immediately

Cause:
1. No input stream from client.
2. Malformed framing for DAP (`Content-Length` mismatch).

Checks:
1. Verify DAP messages use `Content-Length: N` and `\r\n\r\n` separator.
2. Confirm payload byte count matches header length exactly.

## Breakpoint Not Hit

Cause:
1. Breakpoint identity mismatch (`workflowId`, `stepId`).
2. Conditional expression evaluates false.

Checks:
1. Confirm workflow and step ids match spec exactly.
2. Validate condition with runtime evaluate APIs against paused context.

## Step Output Missing in Evaluate

Cause:
1. Step has not executed yet.
2. Dry-run path may not populate output data for every scenario.

Checks:
1. Pause on a later step where prior outputs are available.
2. Inspect `current_scopes` to confirm stored step outputs.

## VSCode Extension Launch Fails

Cause:
1. `runtimeExecutable` or `runtimeArgs` not valid for local environment.

Action:
1. Start with defaults from extension scaffold.
2. Confirm `cargo run -p arazzo-debug-adapter --` works from repo root.
