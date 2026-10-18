# Arazzo Debugger

Step-through debugging for [Arazzo](https://spec.openapis.org/arazzo/v1.1.0) API workflows, including supported Arazzo 1.1 features — set breakpoints, inspect variables, and follow execution in VS Code. See the [runtime's specification coverage](https://github.com/strefethen/arazzo-cli#specification-conformance-extensions-and-gaps) for extensions and remaining gaps.

![Debug session](https://raw.githubusercontent.com/strefethen/arazzo-cli/main/vscode-arazzo-debug/images/screenshots/debug-session.png)

## Features

- **Breakpoints** on workflow steps, success criteria, and actions
- **Variable inspection** — view Locals, Request, Response, Inputs, and Steps when paused
- **Watch expressions** using supported Arazzo runtime expressions (`$steps.login.outputs.token`, `$response.body#/id`)
- **Hover inspection** — inspect runtime expressions and nested JSON Pointer values while paused
- **Debug output channel** — see session events, workflow failures, and adapter diagnostics
- **Step controls** — Step Over, Step In, Step Out, Continue, Pause
- **Stop on entry** — pause at workflow entry before the first step
- **Conditional breakpoints** — break only when an expression evaluates to true
- **Sub-workflow tracking** — call stack follows `workflowId` references across workflows

## Getting Started

1. Install the **Arazzo Debugger** extension from the VS Code Marketplace
2. Open a folder containing `.arazzo.yaml` files
3. Open the Run and Debug panel (`Ctrl+Shift+D` / `Cmd+Shift+D`)
4. Click **"create a launch.json file"** → select **Arazzo: Debug Workflow** → fill in your workflow ID

## Launch Configuration

Add to `.vscode/launch.json`:

```json
{
  "type": "arazzo",
  "request": "launch",
  "name": "Debug My Workflow",
  "spec": "${file}",
  "workflowId": "my-workflow-id",
  "inputs": {
    "baseUrl": "https://api.example.com"
  },
  "stopOnEntry": false
}
```

| Option | Type | Description |
|--------|------|-------------|
| `spec` | string | Path to the `.arazzo.yaml` file. Use `${file}` for the active editor. |
| `workflowId` | string | ID of the workflow to execute. |
| `inputs` | object | Key/value map passed as workflow inputs. |
| `stopOnEntry` | boolean | Pause at workflow entry before the first step. Default: `false`. |

## Breakpoints

Set breakpoints by clicking the gutter in any `.arazzo.yaml` or `.arazzo.yml` file. Breakpoints can be placed on:

- **Workflow steps** — pause before a step executes
- **Success criteria** — pause before criteria evaluation
- **Action definitions** — pause before an action runs

Conditional breakpoints are supported — right-click a breakpoint and enter an Arazzo expression.

> **Note:** Breakpoints only appear in files with `.arazzo.yaml` or `.arazzo.yml` extensions.

## Variable Inspection

When paused at a breakpoint, the Variables panel shows the available scopes. Request and Response appear when their data is available at the current execution checkpoint:

| Scope | Contents |
|-------|----------|
| **Locals** | Current step ID, workflow ID, status |
| **Request** | Available HTTP request method, URL, headers, and body |
| **Response** | HTTP response status code, headers, content type, and body preview |
| **Inputs** | Workflow input values |
| **Steps** | Outputs from previously completed steps |

## Watch Expressions

Add supported Arazzo runtime expressions to the Watch panel:

- `$steps.getUser.outputs.userId`
- `$response.body#/data/0/name`
- `$response.header.Content-Type`
- `$statusCode`
- `$inputs.baseUrl`

Use JSON Pointer syntax after `#` to access nested body values. Evaluate `$response.body` to inspect the full parsed response body when available.

## Hover Inspection

While paused, hover over an expression in the workflow file to inspect its value. Hovering `$request` or `$response` shows an expandable object, including its body when available. Within an expression such as `$response.body#/data/0/name`, hovering individual JSON Pointer segments shows the corresponding nested value.

For a simple condition such as `$statusCode == 200`, hovering the operator evaluates the whole condition as a boolean; hovering `$statusCode` shows the status value.

## Troubleshooting

Open **View → Output** and select **Arazzo Debug** to see session start/end events, the adapter binary being launched, workflow exit codes, and error messages. The channel opens automatically when the adapter reports an error or its process exits with a non-zero code.

Workflow execution failures also appear in the **Debug Console**.

Unresolved outputs identify the output name, expression, workflow, and step.
Completion reports their count; an actual JSON `null` value is not treated as
an unresolved output. Watch failures and invalid breakpoints report their cause
instead of silently returning `null` or skipping the breakpoint.

Selecting a parent Call Stack frame shows that frame's own inputs and values.
For reproducible examples, see the
[debugger diagnostic workspace](https://github.com/strefethen/arazzo-cli/tree/vscode-v0.0.8/testdata/debugger-diagnostics).

**Upgrading from 0.0.7:** the bundled runtime now requires standard Arazzo
operation targets and reusable action references. See the
[0.0.8 migration notes](CHANGELOG.md#008---2026-10-17) before debugging workflows
that use legacy target syntax.

## Requirements

- VS Code 1.90 or later
- Target APIs must be accessible from your machine

## Development

The extension normally launches the debug adapter binary bundled in the
platform-specific VSIX. When running the extension from source (no bundled
binary), point the launch configuration at a locally built adapter with
`runtimeExecutable`:

```json
{
  "type": "arazzo",
  "request": "launch",
  "name": "Debug My Workflow (source adapter)",
  "spec": "${file}",
  "workflowId": "my-workflow-id",
  "runtimeExecutable": "${workspaceFolder}/target/release/arazzo-debug-adapter",
  "runtimeArgs": [],
  "runtimeCwd": "${workspaceFolder}"
}
```

| Option | Type | Description |
|--------|------|-------------|
| `runtimeExecutable` | string | Command used to launch the debug adapter instead of the bundled binary. |
| `runtimeArgs` | string[] | Arguments passed to `runtimeExecutable`. |
| `runtimeCwd` | string | Working directory for `runtimeExecutable`. |

Build the adapter first with `cargo build --release -p arazzo-debug-adapter`.

## Known Limitations

This is a **preview release**:

- Only supported runtime expressions and specification features are available; see the specification coverage linked above
- No hot-reload when the spec file changes during a debug session

## Links

- [Arazzo 1.1 Specification](https://spec.openapis.org/arazzo/v1.1.0)
- [Source Code](https://github.com/strefethen/arazzo-cli/tree/main/vscode-arazzo-debug)
- [Report an Issue](https://github.com/strefethen/arazzo-cli/issues)
