# VS Code Marketplace Release Plan: Arazzo Debugger

Phased plan for publishing `arazzo-debug` to the VS Code Marketplace.
Each phase is independently shippable and leaves the extension in a working state.

**Extension ID:** `strefethen.arazzo-debug`
**Display name:** Arazzo Debugger
**Marketplace URL (after publish):** `https://marketplace.visualstudio.com/items?itemName=strefethen.arazzo-debug`

---

## Prerequisites

Complete these before starting Phase 1.

| Item | How | Notes |
|------|-----|-------|
| VS Code Marketplace publisher account | https://marketplace.visualstudio.com/manage → Create publisher | Publisher ID: `strefethen` (already set in package.json) |
| Azure DevOps Personal Access Token (PAT) | https://dev.azure.com → User Settings → Personal Access Tokens | Scope: **Marketplace (Manage)**. Save securely; needed for `vsce login` and CI publish. |
| Install `@vscode/vsce` | `npm install -g @vscode/vsce` | CLI for packaging and publishing `.vsix` files |
| Install `vsce` locally in extension | `cd vscode-arazzo-debug && npm install --save-dev @vscode/vsce` | For the `vscode:prepublish` script |
| GitHub Actions secret | Settings → Secrets → `VSCE_PAT` | Store the Azure DevOps PAT for CI publish (Phase 3) |

---

## Phase 1: Foundation

**Goal:** Fix the critical binary-bundling architecture, add all required Marketplace metadata, and produce a valid (preview) `.vsix` that installs and launches on the local platform.

**Effort:** 3-5 hours

### Tasks

#### 1.1 Fix DAP binary resolution (`adapterClient.ts`)

**File:** `vscode-arazzo-debug/src/adapterClient.ts`

The current code resolves the DAP binary via `cargo run` from the monorepo root:

```typescript
const repoRoot = path.resolve(this.extensionPath, "..");
const command = "cargo";
const args = ["run", "--manifest-path", manifestPath, "-p", "arazzo-debug-adapter", ...];
```

Replace with platform-aware bundled binary resolution:

```typescript
import * as os from "node:os";

function getBundledBinaryPath(extensionPath: string): string {
  const platform = os.platform();   // "linux" | "darwin" | "win32"
  const arch = os.arch();           // "x64" | "arm64"
  const ext = platform === "win32" ? ".exe" : "";
  const binaryName = `arazzo-debug-adapter${ext}`;
  return path.join(extensionPath, "bin", binaryName);
}
```

In `createDebugAdapterDescriptor`:
1. If `runtimeExecutable` is set in the launch config, use it (dev override).
2. Otherwise, resolve to the bundled binary at `<extensionPath>/bin/arazzo-debug-adapter`.
3. If the bundled binary does not exist, show an error message and return `undefined`.

The binary goes in `vscode-arazzo-debug/bin/` at package time (via `vscode:prepublish`).

**Dependencies:** None
**Verification:** After building the adapter binary manually (`cargo build --release -p arazzo-debug-adapter`) and copying it to `vscode-arazzo-debug/bin/`, the extension launches the debug session without `cargo` installed.

#### 1.2 Add `vscode:prepublish` script

**File:** `vscode-arazzo-debug/package.json`

Add to `scripts`:

```json
"vscode:prepublish": "npm run build && node scripts/copy-binary.js"
```

Create `vscode-arazzo-debug/scripts/copy-binary.js`:
- Detects the current platform/arch
- Copies `../target/release/arazzo-debug-adapter` to `bin/arazzo-debug-adapter`
- Makes the binary executable (`chmod +x`) on Unix
- Fails with a clear error if the binary is not found (forces `cargo build --release` first)

**Dependencies:** 1.1
**Verification:** `npm run vscode:prepublish` succeeds and `bin/arazzo-debug-adapter` exists.

#### 1.3 Fix `.vscodeignore`

**File:** `vscode-arazzo-debug/.vscodeignore`

Replace contents with:

```
.vscode/**
src/**
node_modules/**
scripts/**
tsconfig.json
*.tsbuildinfo
*.map
dist/test/**
.gitignore
```

This ensures:
- Source TypeScript is excluded
- Source maps (`.map`) are excluded
- Test output is excluded
- The `bin/` directory with the compiled binary is **included**
- `dist/` (compiled JS) is included
- `package.json`, `README.md`, `LICENSE`, `CHANGELOG.md`, icon are included

**Dependencies:** None
**Verification:** Run `vsce ls` from the extension directory and confirm only the intended files appear.

#### 1.4 Add required Marketplace metadata to `package.json`

**File:** `vscode-arazzo-debug/package.json`

Add/update the following fields:

```json
{
  "preview": true,
  "icon": "images/icon.png",
  "repository": {
    "type": "git",
    "url": "https://github.com/strefethen/arazzo-cli"
  },
  "homepage": "https://github.com/strefethen/arazzo-cli/tree/main/vscode-arazzo-debug",
  "bugs": {
    "url": "https://github.com/strefethen/arazzo-cli/issues"
  },
  "keywords": [
    "arazzo",
    "debugger",
    "openapi",
    "workflow",
    "api",
    "dap",
    "yaml"
  ]
}
```

**Dependencies:** None
**Verification:** `vsce package` does not emit metadata warnings.

#### 1.5 Create extension icon

**File:** `vscode-arazzo-debug/images/icon.png`

Requirements:
- 256x256 PNG
- Visually represents Arazzo workflow debugging (e.g., workflow nodes with a debug/play overlay)
- Works at small sizes (16px sidebar icon)
- No transparency required, but looks good on both light and dark backgrounds

**Dependencies:** None
**Verification:** Icon renders correctly in `vsce package` output and in the Extensions sidebar.

#### 1.6 Copy LICENSE

**File:** `vscode-arazzo-debug/LICENSE`

Copy from the repo root:

```bash
cp LICENSE vscode-arazzo-debug/LICENSE
```

**Dependencies:** None
**Verification:** File exists and `vsce package` includes it.

#### 1.7 Fix hardcoded `workflowId` placeholder

**File:** `vscode-arazzo-debug/package.json`

In `contributes.debuggers[0].initialConfigurations`, change:

```json
"workflowId": "status-check"
```

to:

```json
"workflowId": "${input:workflowId}"
```

And add an `inputs` contribution:

```json
"inputs": [
  {
    "id": "workflowId",
    "type": "promptString",
    "description": "The workflow ID to debug (from your .arazzo.yaml file)"
  }
]
```

Alternatively, use a simpler placeholder that makes it obvious the user must edit it:

```json
"workflowId": ""
```

with a comment-like description in the `configurationAttributes` making it clear this is required.

The preferred approach is the `${input:workflowId}` prompt, since it gives a guided experience.

**Dependencies:** None
**Verification:** Creating a new launch.json from the Arazzo debug type prompts for workflowId instead of inserting `status-check`.

#### 1.8 Mark as preview

**File:** `vscode-arazzo-debug/package.json`

Add:

```json
"preview": true
```

This shows a "Preview" badge on the Marketplace listing, setting user expectations.

**Dependencies:** None (covered in 1.4, listed separately for clarity)

### Phase 1 Verification Criteria

- [ ] `vsce package` produces a `.vsix` without errors or warnings
- [ ] Installing the `.vsix` locally (`code --install-extension arazzo-debug-0.0.1.vsix`) works
- [ ] Opening an `.arazzo.yaml` file and starting a debug session launches the bundled DAP binary
- [ ] The extension works without Rust/Cargo installed on the machine
- [ ] `vsce ls` shows only intended files (no source maps, no test files, no `node_modules`)

---

## Phase 2: User-Facing Quality

**Goal:** Make the Marketplace listing look professional and the extension usable by someone who has never seen the codebase.

**Effort:** 4-6 hours

### Tasks

#### 2.1 Rewrite README for Marketplace

**File:** `vscode-arazzo-debug/README.md`

The current README is developer-facing. Replace with a user-facing README structured as:

```markdown
# Arazzo Debugger

Step-through debugger for [Arazzo 1.0](https://spec.openapis.org/arazzo/v1.0.0) workflow specifications.

## Features

- Set breakpoints on workflow steps in `.arazzo.yaml` files
- Step through workflows one step at a time
- Inspect variables: `$inputs`, `$steps`, `$statusCode`, `$response.body`
- Watch expressions with full Arazzo expression syntax
- Stop-on-entry mode to pause before the first step

![Debugging an Arazzo workflow](images/screenshots/debug-session.png)

## Getting Started

1. Install the extension from the Marketplace
2. Open a folder containing `.arazzo.yaml` files
3. Open an `.arazzo.yaml` file
4. Press F5 or go to Run → Start Debugging
5. Select "Arazzo" when prompted for a debug type
6. Enter the workflow ID you want to debug

## Launch Configuration

Add to `.vscode/launch.json`:

    {
      "type": "arazzo",
      "request": "launch",
      "name": "Debug Workflow",
      "spec": "${file}",
      "workflowId": "my-workflow-id",
      "inputs": {
        "baseUrl": "https://api.example.com"
      }
    }

### Configuration Options

| Option | Required | Description |
|--------|----------|-------------|
| `spec` | Yes | Path to the `.arazzo.yaml` file |
| `workflowId` | Yes | ID of the workflow to execute |
| `inputs` | No | Key-value map of workflow inputs |
| `stopOnEntry` | No | Pause at workflow entry (default: `false`) |

## Requirements

- VS Code 1.90 or later
- The target APIs referenced in your Arazzo spec must be accessible

## Known Limitations

- Breakpoints apply to all YAML files (scoped breakpoints coming soon)
- This is a preview release — please report issues

## Links

- [Arazzo Specification](https://spec.openapis.org/arazzo/v1.0.0)
- [Report an Issue](https://github.com/strefethen/arazzo-cli/issues)
- [Source Code](https://github.com/strefethen/arazzo-cli/tree/main/vscode-arazzo-debug)
```

**Dependencies:** 2.3 (screenshots — can use placeholder paths initially)
**Verification:** README renders correctly on the Marketplace preview (`vsce show` or local preview).

#### 2.2 Create CHANGELOG.md

**File:** `vscode-arazzo-debug/CHANGELOG.md`

```markdown
# Changelog

## [0.0.1] - 2026-XX-XX

### Added
- Initial preview release
- Debug adapter launching for Arazzo 1.0 workflow specs
- Breakpoint support on workflow steps
- Variable inspection for `$inputs`, `$steps`, `$statusCode`, `$response`
- Watch expressions with Arazzo expression syntax
- Stop-on-entry mode
- Launch configuration with spec path, workflow ID, and inputs
```

**Dependencies:** None
**Verification:** File exists and is included in `.vsix`.

#### 2.3 Add screenshots

**Directory:** `vscode-arazzo-debug/images/screenshots/`

Create placeholder paths for the following screenshots. Each must be captured manually.

| Filename | What to capture | Description for README |
|----------|----------------|----------------------|
| `debug-session.png` | A full debug session in progress: editor open on an `.arazzo.yaml` file, breakpoint hit, yellow highlight on current step, Debug toolbar visible | Hero screenshot showing the core debugging experience |
| `variables-panel.png` | The Variables panel expanded during a paused session, showing `$inputs`, `$steps`, `$statusCode`, `$response.body` | Shows what runtime state is inspectable |
| `launch-config.png` | The `launch.json` editor with an Arazzo configuration filled in | Shows how to configure a debug session |
| `breakpoints.png` | Editor gutter with red breakpoint dots on step lines in an `.arazzo.yaml` file | Shows breakpoint placement |

Screenshot guidelines:
- Use a clean VS Code theme (Dark+ or Light+ default)
- Use a realistic Arazzo spec (e.g., the httpbin example from `examples/`)
- Crop to the relevant area, but include enough context
- Target ~800px wide for good Marketplace rendering
- PNG format

**Dependencies:** Phase 1 complete (extension must work to capture screenshots)
**Verification:** Screenshots render in README when viewed on GitHub and in Marketplace preview.

#### 2.4 Add `configurationSnippets`

**File:** `vscode-arazzo-debug/package.json`

Add to `contributes.debuggers[0]`:

```json
"configurationSnippets": [
  {
    "label": "Arazzo: Debug Workflow",
    "description": "Debug an Arazzo workflow spec with breakpoints and step controls.",
    "body": {
      "type": "arazzo",
      "request": "launch",
      "name": "Debug ${1:workflow-name}",
      "spec": "^\"\\${file}\"",
      "workflowId": "${2:workflow-id}",
      "inputs": {}
    }
  },
  {
    "label": "Arazzo: Debug with Inputs",
    "description": "Debug an Arazzo workflow with input parameters.",
    "body": {
      "type": "arazzo",
      "request": "launch",
      "name": "Debug ${1:workflow-name}",
      "spec": "^\"\\${file}\"",
      "workflowId": "${2:workflow-id}",
      "inputs": {
        "${3:key}": "${4:value}"
      }
    }
  },
  {
    "label": "Arazzo: Debug (Stop on Entry)",
    "description": "Debug an Arazzo workflow, pausing immediately at entry.",
    "body": {
      "type": "arazzo",
      "request": "launch",
      "name": "Debug ${1:workflow-name} (stop on entry)",
      "spec": "^\"\\${file}\"",
      "workflowId": "${2:workflow-id}",
      "inputs": {},
      "stopOnEntry": true
    }
  }
]
```

**Dependencies:** None
**Verification:** In a project with no `launch.json`, clicking "Add Configuration" in the debug panel shows the three Arazzo snippets.

#### 2.5 Scope breakpoints to `.arazzo.yaml` files

**File:** `vscode-arazzo-debug/package.json`

The current breakpoint contribution targets all YAML files:

```json
"breakpoints": [{ "language": "yaml" }]
```

Option A — Register a custom language ID (recommended):

```json
"languages": [
  {
    "id": "arazzo",
    "aliases": ["Arazzo"],
    "extensions": [".arazzo.yaml", ".arazzo.yml"],
    "configuration": "./language-configuration.json"
  }
],
"grammars": [
  {
    "language": "arazzo",
    "scopeName": "source.arazzo",
    "path": "./syntaxes/arazzo.tmLanguage.json"
  }
]
```

Then change breakpoints to:

```json
"breakpoints": [{ "language": "arazzo" }]
```

This requires creating:
- `vscode-arazzo-debug/language-configuration.json` (minimal: bracket pairs, comment tokens)
- `vscode-arazzo-debug/syntaxes/arazzo.tmLanguage.json` (can embed/extend the YAML grammar)

Option B — Keep `yaml` language but filter in the debug adapter:

Leave breakpoints scoped to `yaml` but only accept breakpoints from files matching `*.arazzo.yaml` in the DAP adapter. Simpler but less clean.

**Recommended:** Option A. It gives Arazzo files their own file icon and language mode, which is a better UX.

**Dependencies:** None
**Verification:** Setting a breakpoint in a `.arazzo.yaml` file works; setting one in a plain `.yaml` file does not show the Arazzo debug type.

#### 2.6 Add `galleryBanner`

**File:** `vscode-arazzo-debug/package.json`

```json
"galleryBanner": {
  "color": "#1e1e2e",
  "theme": "dark"
}
```

Pick a color that complements the icon. Dark themes tend to look better on the Marketplace.

**Dependencies:** 1.5 (icon)
**Verification:** Marketplace listing header uses the banner color.

### Phase 2 Verification Criteria

- [ ] README is user-facing with feature descriptions, getting-started guide, and configuration reference
- [ ] At least a hero screenshot is included and renders in the README
- [ ] CHANGELOG.md exists and is included in the `.vsix`
- [ ] `configurationSnippets` appear when adding a new debug configuration
- [ ] Breakpoints only activate in `.arazzo.yaml` files (if Option A implemented)
- [ ] Gallery banner renders on Marketplace listing

---

## Phase 3: Binary Distribution Pipeline

**Goal:** Automate building platform-specific `.vsix` packages and publishing to the Marketplace from CI.

**Effort:** 6-10 hours

### Tasks

#### 3.1 Create the CI workflow for `.vsix` builds

**File:** `.github/workflows/vscode-release.yml`

```yaml
name: VS Code Extension Release

on:
  push:
    tags:
      - "vscode-v*"
  workflow_dispatch:
    inputs:
      tag:
        description: "Tag to release (e.g., vscode-v0.0.1)"
        required: true

permissions:
  contents: write

jobs:
  build:
    name: Build VSIX (${{ matrix.label }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            label: linux-x64
            target: x86_64-unknown-linux-gnu
            vsce_target: linux-x64
            exe_suffix: ""
          - os: macos-latest
            label: darwin-arm64
            target: aarch64-apple-darwin
            vsce_target: darwin-arm64
            exe_suffix: ""
          - os: windows-latest
            label: win32-x64
            target: x86_64-pc-windows-msvc
            vsce_target: win32-x64
            exe_suffix: ".exe"
    steps:
      - uses: actions/checkout@v4

      - name: Set up Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Cache Rust
        uses: Swatinem/rust-cache@v2

      - name: Build DAP binary
        run: cargo build --release -p arazzo-debug-adapter --target ${{ matrix.target }}

      - name: Set up Node
        uses: actions/setup-node@v4
        with:
          node-version: "20"

      - name: Install extension deps
        working-directory: vscode-arazzo-debug
        run: npm ci

      - name: Copy binary into extension
        shell: bash
        run: |
          mkdir -p vscode-arazzo-debug/bin
          cp target/${{ matrix.target }}/release/arazzo-debug-adapter${{ matrix.exe_suffix }} \
             vscode-arazzo-debug/bin/arazzo-debug-adapter${{ matrix.exe_suffix }}

      - name: Build extension TypeScript
        working-directory: vscode-arazzo-debug
        run: npm run build

      - name: Package platform-specific VSIX
        working-directory: vscode-arazzo-debug
        run: npx @vscode/vsce package --target ${{ matrix.vsce_target }} -o arazzo-debug-${{ matrix.vsce_target }}.vsix

      - name: Upload VSIX artifact
        uses: actions/upload-artifact@v4
        with:
          name: vsix-${{ matrix.label }}
          path: vscode-arazzo-debug/arazzo-debug-${{ matrix.vsce_target }}.vsix

  test:
    name: Smoke test VSIX (${{ matrix.label }})
    runs-on: ${{ matrix.os }}
    needs: build
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            label: linux-x64
            vsce_target: linux-x64
          - os: macos-latest
            label: darwin-arm64
            vsce_target: darwin-arm64
          - os: windows-latest
            label: win32-x64
            vsce_target: win32-x64
    steps:
      - uses: actions/download-artifact@v4
        with:
          name: vsix-${{ matrix.label }}

      - name: Install VSIX
        run: code --install-extension arazzo-debug-${{ matrix.vsce_target }}.vsix

      - name: Verify extension installed
        shell: bash
        run: code --list-extensions | grep -i arazzo

  publish:
    name: Publish to Marketplace
    runs-on: ubuntu-latest
    needs: test
    steps:
      - uses: actions/download-artifact@v4
        with:
          pattern: vsix-*
          merge-multiple: true

      - name: Set up Node
        uses: actions/setup-node@v4
        with:
          node-version: "20"

      - name: Install vsce
        run: npm install -g @vscode/vsce

      - name: Publish all platform targets
        env:
          VSCE_PAT: ${{ secrets.VSCE_PAT }}
        run: |
          for vsix in arazzo-debug-*.vsix; do
            echo "Publishing $vsix"
            vsce publish --packagePath "$vsix"
          done

  release:
    name: GitHub Release
    runs-on: ubuntu-latest
    needs: test
    steps:
      - uses: actions/download-artifact@v4
        with:
          pattern: vsix-*
          merge-multiple: true

      - name: Generate checksums
        run: sha256sum arazzo-debug-*.vsix > SHA256SUMS.txt

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: ${{ github.ref_name }}
          files: |
            arazzo-debug-*.vsix
            SHA256SUMS.txt
          generate_release_notes: true
```

**Dependencies:** Phase 1 complete
**Verification:** Pushing a `vscode-v0.0.1` tag triggers the workflow and produces three `.vsix` artifacts.

#### 3.2 Add platform targets to the existing CI build matrix

**File:** `.github/workflows/ci.yml`

Extend the existing `vscode-extension` job to also compile the DAP binary and run `vsce package --target` as a dry run (without publishing). This catches packaging regressions on every PR.

Add after the existing "Build extension" step:

```yaml
- name: Build DAP binary
  run: cargo build --release -p arazzo-debug-adapter

- name: Copy binary for packaging test
  run: |
    mkdir -p vscode-arazzo-debug/bin
    cp target/release/arazzo-debug-adapter vscode-arazzo-debug/bin/

- name: Dry-run package
  working-directory: vscode-arazzo-debug
  run: npx @vscode/vsce package --target linux-x64 -o /tmp/test.vsix
```

**Dependencies:** 1.1, 1.2, 1.3
**Verification:** CI passes on PRs that modify extension files.

#### 3.3 Version bumping strategy

Decide on versioning:
- Use the same version for the Rust workspace and the VS Code extension, or decouple them.
- **Recommendation:** Decouple. The extension has its own `package.json` version. Tag with `vscode-v<version>` (e.g., `vscode-v0.1.0`) to trigger extension releases, separate from CLI releases tagged `v<version>`.

Document this in the extension README or a `RELEASING.md` file in the extension directory.

**File:** `vscode-arazzo-debug/RELEASING.md`

```markdown
# Releasing the Arazzo Debugger Extension

1. Update `version` in `package.json`
2. Update `CHANGELOG.md` with the new version
3. Commit: `git commit -m "release(vscode): v0.X.Y"`
4. Tag: `git tag vscode-v0.X.Y`
5. Push: `git push origin main --tags`
6. CI builds platform-specific .vsix packages and publishes to Marketplace
```

**Dependencies:** 3.1
**Verification:** Document exists and matches the CI workflow trigger.

#### 3.4 Additional platform targets (optional)

If demand exists, add:
- `linux-arm64` (Raspberry Pi, AWS Graviton)
- `darwin-x64` (Intel Macs)
- `alpine-x64` (musl-based, for Remote-Containers)

These require cross-compilation targets in Rust (`cross` tool or `cargo-zigbuild`).

**Dependencies:** 3.1
**Verification:** Additional `.vsix` packages install and launch on target platforms.

### Phase 3 Verification Criteria

- [ ] Pushing a `vscode-v*` tag produces `.vsix` artifacts for linux-x64, darwin-arm64, win32-x64
- [ ] Each `.vsix` installs cleanly on its target platform (`code --install-extension`)
- [ ] The bundled binary launches and responds to DAP initialize request
- [ ] `vsce publish` succeeds and the extension appears on the Marketplace
- [ ] GitHub Release contains all `.vsix` files and checksums
- [ ] CI dry-run packaging catches breakage on PRs

---

## Phase 4: Polish

**Goal:** Replace placeholder implementations with real functionality, add meaningful tests, and optimize startup.

**Effort:** 8-12 hours

### Tasks

#### 4.1 Implement `yamlStepIndex.ts`

**File:** `vscode-arazzo-debug/src/yamlStepIndex.ts`

The current implementation is a stub that returns an empty array:

```typescript
export function buildWorkflowStepIndex(_text: string): WorkflowStepIndex {
  return { steps: [] };
}
```

Implement YAML-aware step extraction:
- Parse the YAML text (use a YAML library like `yaml` from npm — add as a dependency)
- Walk the parsed document to find `workflows[*].steps[*]`
- For each step, record:
  - `workflowId`: the parent workflow's `workflowId`
  - `stepId`: the step's `stepId`
  - `line`: the 1-based line number of the `stepId` key in the source
- Return the index sorted by line number

Use the `yaml` package's CST (Concrete Syntax Tree) mode to get line numbers, since the standard parse loses positional information.

**Dependencies:** None
**Verification:** Unit tests pass with sample `.arazzo.yaml` content, returning correct step IDs and line numbers.

#### 4.2 Implement `breakpointMapper.ts`

**File:** `vscode-arazzo-debug/src/breakpointMapper.ts`

The current implementation maps breakpoints to line numbers only:

```typescript
return breakpoints.map((bp) => ({
  line: bp.location.range.start.line + 1
}));
```

Implement proper mapping:
- Use the `WorkflowStepIndex` from 4.1 to resolve each breakpoint line to the nearest step
- If a breakpoint is set on a line inside a step block (e.g., on a `parameters` line), snap it to the step's `stepId` line
- If a breakpoint is set outside any step, mark it as unmapped (the DAP adapter can reject it)
- Populate the `location` field with the resolved `StepLocation`

**Dependencies:** 4.1
**Verification:** Setting a breakpoint anywhere within a step block resolves to that step's checkpoint.

#### 4.3 Replace placeholder smoke test

**File:** `vscode-arazzo-debug/src/test/smoke.test.ts`

The current test is:

```typescript
test("Arazzo Debug Extension smoke", () => {
  assert.equal(true, true);
});
```

Replace with meaningful tests:

```typescript
// Unit tests (no VS Code API needed):
test("buildWorkflowStepIndex parses steps from YAML", () => { ... });
test("buildWorkflowStepIndex handles empty document", () => { ... });
test("buildWorkflowStepIndex handles malformed YAML", () => { ... });
test("mapBreakpoints resolves line to nearest step", () => { ... });
test("mapBreakpoints ignores lines outside step blocks", () => { ... });

// Integration tests (need @vscode/test-electron):
test("extension activates on arazzo debug type", () => { ... });
test("debug configuration resolves spec from active editor", () => { ... });
test("DAP binary is found in bin/ directory", () => { ... });
```

Add `@vscode/test-electron` as a dev dependency for integration tests.

Update `package.json` scripts:

```json
"test:unit": "node --test dist/test/smoke.test.js",
"test:integration": "node dist/test/runIntegration.js",
"test": "npm run test:unit"
```

**Dependencies:** 4.1, 4.2
**Verification:** `npm test` runs actual assertions and they pass.

#### 4.4 Activation time optimization

**File:** `vscode-arazzo-debug/package.json`, `vscode-arazzo-debug/src/extension.ts`

Current `activationEvents`:

```json
"activationEvents": ["onDebug", "onDebugResolve:arazzo"]
```

`onDebug` activates on ANY debug session start, which is too broad. Remove it:

```json
"activationEvents": ["onDebugResolve:arazzo"]
```

This ensures the extension only activates when an Arazzo debug session is requested.

If Phase 2 Option A was implemented (custom `arazzo` language ID), also add:

```json
"activationEvents": [
  "onDebugResolve:arazzo",
  "onLanguage:arazzo"
]
```

Consider bundling with esbuild for faster load times:
- Add esbuild as a dev dependency
- Create `esbuild.config.mjs` to bundle `src/extension.ts` into a single `dist/extension.js`
- Update `build` script to use esbuild instead of `tsc`
- Keep `tsc` for type checking only (`lint` script)

**Dependencies:** None
**Verification:** Extension activation time is under 100ms (check with "Developer: Show Running Extensions").

#### 4.5 Add `esbuild` bundling (optional but recommended)

**Files:**
- `vscode-arazzo-debug/esbuild.config.mjs` (new)
- `vscode-arazzo-debug/package.json` (update scripts)

```javascript
// esbuild.config.mjs
import * as esbuild from "esbuild";

const production = process.argv.includes("--production");

await esbuild.build({
  entryPoints: ["src/extension.ts"],
  bundle: true,
  outfile: "dist/extension.js",
  external: ["vscode"],
  format: "cjs",
  platform: "node",
  target: "node20",
  sourcemap: !production,
  minify: production,
});
```

Update scripts:

```json
"build": "node esbuild.config.mjs",
"build:production": "node esbuild.config.mjs --production",
"vscode:prepublish": "npm run build:production && node scripts/copy-binary.js",
"watch": "node esbuild.config.mjs --watch",
"lint": "tsc -p . --noEmit"
```

**Dependencies:** None
**Verification:** Single bundled `dist/extension.js`, extension still works, activation time improves.

### Phase 4 Verification Criteria

- [ ] `buildWorkflowStepIndex` correctly parses step locations from real `.arazzo.yaml` files
- [ ] Breakpoints set anywhere in a step block resolve to the correct step checkpoint
- [ ] `npm test` runs real assertions (not `assert.equal(true, true)`)
- [ ] Extension activation is scoped to Arazzo debug sessions only
- [ ] Activation time is under 100ms

---

## Screenshots Guide

### When to capture

After Phase 1 is complete and the extension works end-to-end with a bundled binary.

### Setup

1. Use VS Code with the **Dark+** (default dark) theme
2. Open the `arazzo-cli` repository
3. Open `examples/httpbin-get.arazzo.yaml` (or a representative example spec)
4. Set up a working `launch.json` with the Arazzo debug type
5. Use a clean VS Code window (close other tabs, hide unnecessary panels)

### Screenshots to capture

#### 1. `debug-session.png` (hero image)

**What:** Full VS Code window during an active debug session.
**How:**
1. Start a debug session on an example workflow
2. Let it hit a breakpoint
3. Ensure visible: editor with `.arazzo.yaml`, yellow step highlight, Debug toolbar, Variables panel, Call Stack
4. Capture the full window
5. Crop to ~1200x800px

**Save to:** `vscode-arazzo-debug/images/screenshots/debug-session.png`

#### 2. `variables-panel.png`

**What:** Close-up of the Variables panel during a paused session.
**How:**
1. Pause at a step that has made at least one HTTP call
2. Expand the Variables panel to show `$inputs`, `$steps`, `$statusCode`, `$response.body`
3. Capture just the Variables panel area
4. Crop to ~600x400px

**Save to:** `vscode-arazzo-debug/images/screenshots/variables-panel.png`

#### 3. `launch-config.png`

**What:** A filled-in `launch.json` with Arazzo configuration.
**How:**
1. Open `.vscode/launch.json`
2. Show a complete Arazzo launch configuration
3. Capture the editor area
4. Crop to ~800x300px

**Save to:** `vscode-arazzo-debug/images/screenshots/launch-config.png`

#### 4. `breakpoints.png`

**What:** Editor gutter showing breakpoints on step lines.
**How:**
1. Open an `.arazzo.yaml` file
2. Set 2-3 breakpoints on different `stepId` lines
3. Capture the editor area showing the red dots in the gutter
4. Crop to ~800x400px

**Save to:** `vscode-arazzo-debug/images/screenshots/breakpoints.png`

### Image format

- PNG, no compression artifacts
- Retina/HiDPI: if capturing on a Retina display, the natural resolution will be 2x — this is fine and looks sharp on the Marketplace

---

## Summary Timeline

| Phase | Effort | Prerequisite | Delivers |
|-------|--------|-------------|----------|
| Prerequisites | 30 min | — | Publisher account, PAT, tooling |
| Phase 1: Foundation | 3-5 hours | Prerequisites | Installable `.vsix` with bundled binary, preview badge |
| Phase 2: User-Facing Quality | 4-6 hours | Phase 1 | Professional Marketplace listing, screenshots, scoped breakpoints |
| Phase 3: Distribution Pipeline | 6-10 hours | Phase 1 | Automated cross-platform builds and Marketplace publish via CI |
| Phase 4: Polish | 8-12 hours | Phase 2 | Real YAML parsing, proper breakpoint mapping, meaningful tests |

**Total estimated effort:** 22-34 hours

Phases 2 and 3 can run in parallel after Phase 1 is complete.
Phase 4 can begin at any time (the TypeScript work is independent) but should ship after Phase 3 so the improved functionality reaches users via the pipeline.

### Recommended publish sequence

1. Complete Phase 1 + Phase 2 screenshots
2. First manual publish: `vsce package && vsce publish` (preview release)
3. Complete Phase 3 (CI pipeline)
4. All subsequent releases go through CI
5. Phase 4 ships as `v0.1.0` (remove `preview: true` when confident)
