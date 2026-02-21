# vscode-arazzo-debug

Early scaffolding for a VSCode debugger extension targeting Arazzo workflows.

## Current Status

1. Registers debugger type: `arazzo`
2. Provides launch configuration defaults
3. Launches Rust adapter executable via `runtimeExecutable`/`runtimeArgs`
4. Includes placeholder modules for YAML step indexing and breakpoint mapping

## Local Development

```bash
cd vscode-arazzo-debug
npm install
npm run build
```

Use VSCode "Run Extension" to start an extension development host.
