# Debugger User Guide (v1 Preview)

This project now includes an early debugger stack:

1. Runtime debug controls in `crates/arazzo-runtime`
2. Adapter transport in `crates/arazzo-debug-adapter`
3. VSCode extension scaffold in `vscode-arazzo-debug/`

## CLI Debug Transport

For the newline protocol backend:

```bash
cargo run -p arazzo-cli -- debug-stdio
```

For DAP transport backend:

```bash
cargo run -p arazzo-debug-adapter --
```

## VSCode Extension (Scaffold)

```bash
cd vscode-arazzo-debug
npm install
npm run build
```

Then launch an extension host in VSCode and use a launch config of type `arazzo`.

## Current v1 Capabilities

1. Step breakpoints in runtime debug controller
2. Continue / step over / step in / step out semantics in runtime
3. Paused stack and scope snapshots
4. Watch/evaluate APIs in runtime controller
5. DAP transcript-level command handling for initialize/breakpoints/stepping/evaluate

## Known Gaps

1. End-to-end DAP to runtime execution wiring is still in progress.
2. YAML line-to-step mapping in extension is placeholder-level.
3. VSCode UI integration is scaffolded but not production-ready.
