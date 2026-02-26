# Debugger User Guide

This project includes a debugger stack for stepping through Arazzo workflows:

1. Runtime debug controls in `crates/arazzo-runtime`
2. DAP adapter in `crates/arazzo-debug-adapter`
3. VSCode extension in `vscode-arazzo-debug/`

## CLI Debug Transport

Start the DAP transport backend:

```bash
cargo run -p arazzo-debug-adapter --
```

## VSCode Extension

```bash
cd vscode-arazzo-debug
npm install
npm run build
```

Then launch an extension host in VSCode and use a launch config of type `arazzo`.

## Capabilities

1. Step breakpoints with conditional expressions
2. Continue / Step Over / Step In / Step Out controls
3. Paused stack frames with sub-workflow depth tracking
4. Variable scopes: Locals, Request, Response, Inputs, Steps
5. Watch and evaluate expressions at pause points
6. Full DAP protocol support (initialize, breakpoints, stepping, evaluate)
