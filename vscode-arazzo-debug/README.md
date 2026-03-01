# Arazzo Debugger

Step-through debugging for [Arazzo 1.0](https://spec.openapis.org/arazzo/v1.0.0) API workflow specifications — set breakpoints, inspect variables, and trace execution in VS Code.

![Debug session](images/screenshots/debug-session.png)

## Features

- **Breakpoints** on workflow steps, success criteria, and actions
- **Variable inspection** — view Locals, Request, Response, Inputs, and Steps when paused
- **Watch expressions** using full Arazzo expression syntax (`$steps.login.outputs.token`, `$response.body.id`)
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

When paused at a breakpoint, the Variables panel shows:

| Scope | Contents |
|-------|----------|
| **Locals** | Current step ID, workflow ID, status |
| **Request** | Method, URL, headers, body of the pending/completed HTTP request |
| **Response** | Status code, headers, body of the last HTTP response |
| **Inputs** | Workflow input values |
| **Steps** | Outputs from previously completed steps |

## Watch Expressions

Add any Arazzo expression to the Watch panel:

- `$steps.getUser.outputs.userId`
- `$response.body.data[0].name`
- `$response.header.Content-Type`
- `$statusCode`
- `$inputs.baseUrl`

## Requirements

- VS Code 1.90 or later
- Target APIs must be accessible from your machine

## Known Limitations

This is a **preview release**. Some features are still in development:

- Breakpoint positions are mapped by YAML line index — complex multi-document specs may have offset issues
- No hot-reload when the spec file changes during a debug session

## Links

- [Arazzo 1.0 Specification](https://spec.openapis.org/arazzo/v1.0.0)
- [Source Code](https://github.com/strefethen/arazzo-cli/tree/main/vscode-arazzo-debug)
- [Report an Issue](https://github.com/strefethen/arazzo-cli/issues)
