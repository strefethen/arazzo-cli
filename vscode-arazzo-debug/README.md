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

In VS Code:

1. Open `/Users/stevetrefethen/github/arazzo-cli/vscode-arazzo-debug`
2. Select launch config `Run Arazzo Debug Extension`
3. Press `F5` to start an Extension Development Host

Do not run `dist/extension.js` directly with Node. The `vscode` module is provided by the Extension Host runtime.
