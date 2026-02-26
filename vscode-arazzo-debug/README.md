# vscode-arazzo-debug

VSCode debugger extension for Arazzo workflows. Provides step-through debugging with breakpoints, variable inspection, and watch expressions.

## Features

- Registers debugger type: `arazzo`
- Launch configuration with spec path, workflow ID, and input parameters
- Launches the Rust DAP adapter (`arazzo-debug-adapter`) automatically
- YAML step indexing for breakpoint mapping

## Local Development

```bash
cd vscode-arazzo-debug
npm install
npm run build
```

In VS Code:

1. Open the `vscode-arazzo-debug` directory
2. Select launch config `Run Arazzo Debug Extension`
3. Press `F5` to start an Extension Development Host

Do not run `dist/extension.js` directly with Node. The `vscode` module is provided by the Extension Host runtime.
