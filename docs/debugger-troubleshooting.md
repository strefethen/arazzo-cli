# Debugger Troubleshooting

## Reading Arazzo Debug Output

Open **View → Output → Arazzo Debug**. Failures include the configured spec,
workflow, last debugger request, and last observed stack frame when available.
The last observed frame is historical context, not necessarily the failure site.

| Message | Meaning and next step |
|---|---|
| `Cannot start ... cannot access executable` | Check the reported `runtimeExecutable` path and permissions; build the adapter if it is missing. |
| `Cannot start ... runtimeCwd` | Set the working directory to an existing, accessible directory. |
| `bundled debug adapter is missing` | Reinstall the extension or configure an explicitly built adapter. |
| `no Arazzo spec is selected` | Open an Arazzo file or set `spec` in `.vscode/launch.json`. |
| `<command> failed: ...` | The adapter rejected that request. The original diagnostic follows the command. |
| `workflow failed [CODE]: ...` | The runtime reported a workflow failure. Use its code and diagnostic with the session context. |
| `adapter output stream closed unexpectedly` | The adapter closed its stream before normal termination. Look for its process exit code or signal in the same log. |
| `workflow completed successfully (exit code 0)` | The runtime finished successfully. |
| `unresolved output value: "..." expression: "..." in workflow: "...", step: "...".` | The declared output did not resolve in that step's evaluation context. Inspect the response and expression. Its stored value remains null; this diagnostic does not change the workflow's success criteria. Explicit JSON null values are not flagged. |
| `workflow completed with N unresolved output(s)` | The runtime completed, but output-resolution diagnostics were reported. Inspect those messages before relying on the outputs. |
| `workflow failed (exit code N)` | The runtime failed; its diagnostic appears above this summary. |
| `debug session stopped by request` | The client requested disconnect before completion. |
| `workflow ended; adapter did not report an exit status` | An adapter ended the workflow without reporting success or failure. This does not establish success. |
| `session ended` | Debugger lifecycle message. Normal stream closure and requested shutdown signals do not produce an adapter-error message. |

## Inspection and Breakpoint Diagnostics

Watches and hovers return a failed debugger request when evaluation fails;
missing values are distinct from a value that actually contains JSON `null`.
The diagnostic includes the selected workflow and step. Selecting a parent
Call Stack frame inspects its saved context, including its own inputs.

Invalid breakpoint syntax is reported on an unverified breakpoint. A condition
that parses but cannot be evaluated ends the debug run with the condition,
workflow, step, and underlying evaluation error. Source files that cannot be
read or parsed also produce unverified breakpoints with the file and cause.

Expired variable references, unavailable frames, and missing source locations
produce errors instead of empty data or an invented line number. Workflow-level
output diagnostics are included in the unresolved-output completion count.

For runnable examples and regression fixtures, open
[`debugger-diagnostics.code-workspace`](../testdata/debugger-diagnostics/debugger-diagnostics.code-workspace).
The [fixture guide](../testdata/debugger-diagnostics/README.md) lists the checks.

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
