# Agent-First Command Discovery Plan

Implementation-ready plan for making `arazzo-cli` excellent for coding agents
without disrupting existing human CLI users.

## 1. Product Goal

Agents should be able to discover the complete command surface, machine
contracts, safe execution path, and recovery hints without scraping prose help.

The default human help output must remain familiar and stable. Agent-first
features should be additive, explicit, and JSON-native.

## 2. Compatibility Decision

### Keep Default Help Human-First

Default help remains the normal Clap output:

```bash
arazzo-cli --help
arazzo-cli run --help
```

No large agent workflow blocks should be injected into default help. Open-source
users may rely on concise help output, shell completions, docs snippets, and
examples that assume ordinary CLI help.

### Canonical Agent Contract Is JSON

JSON is the source of truth for agent surfaces:

```bash
arazzo-cli --json agent commands
arazzo-cli --json agent manifest
arazzo-cli schema --all
arazzo-cli --json inspect <spec-or-dir>
```

TOON can be considered later as a derived prompt-compression rendering, but it
must never be the canonical contract.

### Revert/Reshape The First Help Rewrite

The current uncommitted help rewrite touched:

- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`

Before implementing this plan, remove the first-pass long help blocks and
agent-note assertions so default help returns to its prior shape. Keep only
small human-neutral command/argument descriptions if the maintainer explicitly
wants them; otherwise revert the help surface fully.

Concrete cleanup:

1. In `crates/arazzo-cli/src/cli.rs`, remove the root `long_about` and
   `after_help` agent workflow block.
2. Remove every command-level `#[command(after_help = "Agent notes:...")]`.
3. If preserving old exact help output is required, also remove new doc comments
   added solely for the first-pass help rewrite.
4. In `crates/arazzo-cli/tests/cli_integration.rs`, remove:
   - `stdout_text`
   - `root_help_surfaces_agent_workflow`
   - `run_help_surfaces_agent_safe_execution_notes`
5. Run:

```bash
cargo fmt --all -- --check
cargo test -p arazzo-cli --test cli_integration schema_lists_available_commands
```

## 3. Non-Goals

- Do not make agents parse prose help.
- Do not change existing `run`, `validate`, `list`, `catalog`, `show`, `steps`,
  `generate`, `replay`, or `test` JSON output in Phase 1.
- Do not add TOON support in Phase 1.
- Do not replace the MCP server. The CLI agent manifest should complement MCP,
  not become a second MCP protocol.
- Do not hide unsafe live-network behavior. Live execution must be labeled.

## 4. Existing Code Ownership

Follow the existing extension guide ownership:

- `crates/arazzo-cli/src/cli.rs`: command and flag shape.
- `crates/arazzo-cli/src/main.rs`: dispatch from `Commands` to handlers.
- `crates/arazzo-cli/src/handlers.rs`: command behavior.
- `crates/arazzo-cli/src/output.rs`: JSON output types and schema generation.
- `crates/arazzo-cli/src/test_runner.rs`: test command implementation, already
  separate from generic handlers.
- `crates/arazzo-cli/tests/cli_integration.rs`: command behavior tests.
- `crates/arazzo-cli/tests/schema_drift.rs`: checked-in schema drift tests.
- `docs/schemas/*.schema.json`: generated JSON Schema documents.

Existing useful patterns:

- `handlers::schema` maps command names to `schemars::schema_for!(...)`.
- `output_json` is the canonical pretty JSON writer.
- Existing JSON contracts are modeled with `Serialize + JsonSchema` types in
  `output.rs`.
- `arazzo_mcp::state::discover_specs` already discovers `.arazzo.yaml` and
  `.arazzo.yml` files recursively.

## 5. User-Facing Feature Set

### 5.1 `agent commands`

Purpose: compact machine-readable command catalog.

Command:

```bash
arazzo-cli --json agent commands
```

Human fallback:

```bash
arazzo-cli agent commands
```

Human fallback should print a short table only. The complete surface is JSON.

JSON shape:

```json
{
  "schemaVersion": "agent.commands.v1",
  "tool": {
    "name": "arazzo-cli",
    "version": "0.2.2"
  },
  "commands": [
    {
      "name": "run",
      "intent": "executeWorkflow",
      "summary": "Execute a workflow from an Arazzo spec",
      "safety": "networkLive",
      "defaultOutput": "human",
      "jsonOutput": true,
      "schema": {
        "command": "arazzo-cli schema run",
        "schemaName": "run"
      },
      "args": [
        {
          "name": "spec",
          "required": true,
          "valueName": "SPEC",
          "description": "Path to an Arazzo YAML spec file"
        }
      ],
      "flags": [
        {
          "name": "--dry-run",
          "type": "boolean",
          "repeatable": false,
          "description": "Build the request plan without sending HTTP requests"
        }
      ],
      "examples": [
        {
          "name": "Preview workflow requests",
          "command": "arazzo-cli --json run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429",
          "safety": "readOnlyPreview"
        }
      ],
      "next": [
        {
          "command": "arazzo-cli schema run",
          "reason": "Inspect the JSON output contract"
        }
      ]
    }
  ]
}
```

Safety enum:

- `readOnly`: parses, validates, lists, inspects, or prints schemas.
- `readOnlyPreview`: constructs request plans without network I/O.
- `networkLive`: may make outbound HTTP requests.
- `filesystemWrite`: writes generated specs, reports, or trace files.
- `stdioServer`: reserves stdout for protocol messages.

### 5.2 `agent manifest`

Purpose: complete tool manifest for agents, wrappers, docs generators, and MCP
bridges.

Command:

```bash
arazzo-cli --json agent manifest
```

JSON shape:

```json
{
  "schemaVersion": "agent.manifest.v1",
  "tool": {
    "name": "arazzo-cli",
    "version": "0.2.2",
    "description": "Standalone Arazzo workflow executor"
  },
  "protocol": {
    "stdout": "JSON when --json is set, human text otherwise",
    "stderr": "diagnostics and errors not already represented in JSON",
    "exitCodes": {
      "0": "success",
      "1": "command error, validation failure, runtime failure, or test failure"
    }
  },
  "commands": [],
  "schemas": {
    "indexCommand": "arazzo-cli schema --index",
    "allCommand": "arazzo-cli schema --all"
  },
  "formats": ["json"],
  "recommendedWorkflow": [
    "arazzo-cli --json agent commands",
    "arazzo-cli --json inspect <spec-or-dir>",
    "arazzo-cli schema <command>",
    "arazzo-cli --json run <spec> <workflow-id> --dry-run",
    "arazzo-cli --json run <spec> <workflow-id> --trace <trace.json>"
  ]
}
```

Difference from `agent commands`:

- `agent commands` is optimized for command selection.
- `agent manifest` includes protocol rules, global defaults, version metadata,
  and references to schemas.

### 5.3 `schema --index` and `schema --all`

Purpose: avoid hard-coded schema command discovery.

Commands:

```bash
arazzo-cli schema --index
arazzo-cli schema --all
```

Compatibility:

- Preserve current `arazzo-cli schema` behavior: still prints the command-name
  JSON array.
- Preserve current `arazzo-cli schema <command>` behavior exactly.

`schema --index` shape:

```json
{
  "schemaVersion": "schema.index.v1",
  "schemas": [
    {
      "name": "run",
      "command": "arazzo-cli schema run",
      "file": "docs/schemas/run.schema.json"
    },
    {
      "name": "agent.commands",
      "command": "arazzo-cli schema agent.commands",
      "file": "docs/schemas/agent-commands.schema.json"
    }
  ]
}
```

`schema --all` shape:

```json
{
  "schemaVersion": "schema.bundle.v1",
  "schemas": {
    "run": { "...": "JSON Schema document" },
    "agent.commands": { "...": "JSON Schema document" }
  }
}
```

### 5.4 `inspect <spec-or-dir>`

Purpose: one-shot read-only context bundle for agents.

Command:

```bash
arazzo-cli --json inspect <spec-or-dir>
```

Behavior:

- If path is a file, parse and inspect that spec.
- If path is a directory, discover `.arazzo.yaml` and `.arazzo.yml` files
  recursively using `arazzo_mcp::state::discover_specs`.
- Never execute workflows.
- Never make network requests.
- Return parse/validation errors per file without failing the entire directory
  scan unless the input path itself is invalid.

JSON shape:

```json
{
  "schemaVersion": "inspect.v1",
  "root": "examples",
  "summary": {
    "files": 3,
    "valid": 3,
    "invalid": 0,
    "workflows": 9,
    "steps": 18
  },
  "files": [
    {
      "file": "examples/httpbin-get.arazzo.yaml",
      "valid": true,
      "title": "HTTPBin Demo",
      "version": "1.0.0",
      "sources": [],
      "workflows": [
        {
          "id": "status-check",
          "summary": "",
          "inputs": [],
          "outputs": [],
          "steps": []
        }
      ]
    }
  ],
  "recommendedNext": [
    {
      "command": "arazzo-cli --json run examples/httpbin-get.arazzo.yaml status-check --dry-run",
      "reason": "Preview requests before live execution"
    }
  ]
}
```

### 5.5 Agent Error Recovery Hints

Do not add default `next` fields to existing command error envelopes in Phase 1.
That changes existing JSON schemas and may surprise strict clients.

Instead, include static recovery guidance in `agent manifest`:

```json
{
  "errorHints": {
    "RUNTIME_WORKFLOW_NOT_FOUND": [
      {
        "commandTemplate": "arazzo-cli --json list <spec>",
        "reason": "Discover valid workflow IDs"
      }
    ],
    "RUN_SPEC_READ_FILE": [
      {
        "commandTemplate": "arazzo-cli --json validate <spec>",
        "reason": "Confirm the spec path and parseability"
      }
    ]
  }
}
```

Future optional enhancement:

```bash
arazzo-cli --json --agent-hints run <spec> <workflow-id>
```

Only add opt-in inline error hints after the manifest surfaces are stable.

### 5.6 Optional TOON Renderer Later

Do not implement TOON in Phase 1.

If added later:

```bash
arazzo-cli agent manifest --format toon
arazzo-cli agent commands --format toon
```

Rules:

- JSON remains canonical.
- TOON is generated from the same Rust model.
- No TOON-only fields.
- Any dependency must go through a package review before adoption.

## 6. File-by-File Implementation Plan

### 6.1 New Module: `crates/arazzo-cli/src/agent_manifest.rs`

Owns static command metadata and shared types.

Types:

```rust
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandCatalog {
    pub schema_version: String,
    pub tool: AgentToolInfo,
    pub commands: Vec<AgentCommand>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentManifest {
    pub schema_version: String,
    pub tool: AgentToolInfo,
    pub protocol: AgentProtocol,
    pub commands: Vec<AgentCommand>,
    pub schemas: AgentSchemaPointers,
    pub formats: Vec<String>,
    pub recommended_workflow: Vec<String>,
    pub error_hints: BTreeMap<String, Vec<AgentNextCommand>>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommand {
    pub name: String,
    pub intent: AgentIntent,
    pub summary: String,
    pub safety: AgentSafety,
    pub default_output: String,
    pub json_output: bool,
    pub schema: Option<AgentSchemaRef>,
    pub args: Vec<AgentArg>,
    pub flags: Vec<AgentFlag>,
    pub examples: Vec<AgentExample>,
    pub next: Vec<AgentNextCommand>,
}
```

Enums:

```rust
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AgentSafety {
    ReadOnly,
    ReadOnlyPreview,
    NetworkLive,
    FilesystemWrite,
    StdioServer,
}
```

Functions:

```rust
pub fn build_agent_commands() -> AgentCommandCatalog;
pub fn build_agent_manifest() -> AgentManifest;
pub fn build_error_hints() -> BTreeMap<String, Vec<AgentNextCommand>>;
```

Do not derive this metadata from Clap in v1. Static metadata is more explicit,
stable, and can include safety semantics that Clap does not know.

### 6.2 `crates/arazzo-cli/src/output.rs`

Add schema/export types for:

- `AgentCommandCatalog`
- `AgentManifest`
- `SchemaIndex`
- `SchemaBundle`
- `InspectOutput`

Alternative: keep most types in `agent_manifest.rs` and `inspect.rs`, but
re-export or import them in `handlers::schema`. Existing pattern currently keeps
schema types in `output.rs`; choose one consistent approach before coding. The
lowest-risk path is:

- Put agent manifest types in `agent_manifest.rs`.
- Put inspect output types in new `inspect.rs`.
- Import those concrete types from `handlers::schema`.
- Keep generic output helper functions in `output.rs`.

### 6.3 `crates/arazzo-cli/src/cli.rs`

Add commands:

```rust
Agent {
    #[command(subcommand)]
    command: AgentCommands,
},
Inspect {
    path: String,
},
```

Add:

```rust
#[derive(Subcommand, Debug)]
pub enum AgentCommands {
    Commands,
    Manifest,
}
```

Add schema flags:

```rust
Schema {
    #[arg(long)]
    all: bool,
    #[arg(long)]
    index: bool,
    command: Option<String>,
}
```

Validation rule:

- Reject combinations like `schema --all run` or `schema --index run`.
- Preserve `schema` with no args as the existing array output.

### 6.4 `crates/arazzo-cli/src/main.rs`

Register modules:

```rust
mod agent_manifest;
mod inspect;
```

Dispatch:

```rust
Commands::Agent { command } => handlers::agent(command, global),
Commands::Inspect { path } => handlers::inspect(&path, global),
Commands::Schema { all, index, command } => {
    handlers::schema(SchemaRequest { all, index, command: command.as_deref() })
}
```

### 6.5 `crates/arazzo-cli/src/handlers.rs`

Add:

```rust
pub struct SchemaRequest<'a> {
    pub all: bool,
    pub index: bool,
    pub command: Option<&'a str>,
}

pub fn agent(command: AgentCommands, global: GlobalOptions) -> Result<(), String>;
pub fn inspect(path: &str, global: GlobalOptions) -> Result<(), String>;
pub fn schema(request: SchemaRequest<'_>) -> Result<(), String>;
```

Agent handler behavior:

- If `global.json` is true, emit complete JSON.
- If `global.json` is false, emit a short table and a hint:
  `Run with --json for the full machine-readable catalog.`

Schema handler behavior:

- Existing `schema` and `schema <command>` outputs unchanged.
- `schema --index` emits `SchemaIndex`.
- `schema --all` emits `SchemaBundle`.
- Add schema names:
  - `agent.commands`
  - `agent.manifest`
  - `schema.index`
  - `schema.bundle`
  - `inspect`

### 6.6 New Module: `crates/arazzo-cli/src/inspect.rs`

Owns one-shot read-only inspection.

Functions:

```rust
pub fn inspect_path(path: &str) -> InspectOutput;
```

Types:

```rust
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InspectOutput {
    pub schema_version: String,
    pub root: String,
    pub summary: InspectSummary,
    pub files: Vec<InspectFile>,
    pub recommended_next: Vec<AgentNextCommand>,
}
```

Implementation details:

- Reuse `arazzo_mcp::state::discover_specs` for directory scans.
- Reuse `output::build_sources`, `output::build_workflow_info`, and
  `output::build_step_info` where possible.
- Add missing shared helpers if MCP/CLI duplicate code becomes painful, but do
  not refactor MCP in this feature unless needed.

### 6.7 `docs/schemas`

Add generated schemas:

- `docs/schemas/agent-commands.schema.json`
- `docs/schemas/agent-manifest.schema.json`
- `docs/schemas/schema-index.schema.json`
- `docs/schemas/schema-bundle.schema.json`
- `docs/schemas/inspect.schema.json`

Generation commands:

```bash
cargo run -p arazzo-cli -- schema agent.commands > docs/schemas/agent-commands.schema.json
cargo run -p arazzo-cli -- schema agent.manifest > docs/schemas/agent-manifest.schema.json
cargo run -p arazzo-cli -- schema schema.index > docs/schemas/schema-index.schema.json
cargo run -p arazzo-cli -- schema schema.bundle > docs/schemas/schema-bundle.schema.json
cargo run -p arazzo-cli -- schema inspect > docs/schemas/inspect.schema.json
```

### 6.8 `crates/arazzo-cli/tests/schema_drift.rs`

Add drift tests:

```rust
#[test]
fn schema_agent_commands_matches_checked_in_file() {
    assert_schema_matches_file("agent.commands", "agent-commands.schema.json");
}
```

Repeat for every new schema.

### 6.9 `crates/arazzo-cli/tests/cli_integration.rs`

Add tests:

1. `agent_commands_json_lists_commands_and_safety`
   - Run `--json agent commands`.
   - Assert `schemaVersion == "agent.commands.v1"`.
   - Assert `run`, `validate`, `schema`, `inspect`, `agent` exist.
   - Assert `run.safety == "networkLive"`.
   - Assert at least one `run` example includes `--dry-run`.

2. `agent_manifest_json_includes_protocol_and_error_hints`
   - Run `--json agent manifest`.
   - Assert protocol stdout/stderr fields exist.
   - Assert error hint for `RUNTIME_WORKFLOW_NOT_FOUND`.

3. `agent_commands_human_is_short`
   - Run `agent commands`.
   - Assert success.
   - Assert output contains the hint to rerun with `--json`.
   - Assert it does not dump JSON.

4. `schema_index_lists_new_agent_schemas`
   - Run `schema --index`.
   - Assert `agent.commands`, `agent.manifest`, and `inspect`.

5. `schema_all_contains_existing_and_new_schemas`
   - Run `schema --all`.
   - Assert keys include `run`, `test`, `agent.commands`, `inspect`.

6. `schema_flags_reject_conflicting_command`
   - Run `schema --all run`.
   - Assert non-zero and useful stderr.

7. `inspect_file_json_summarizes_workflows`
   - Run `--json inspect examples/httpbin-get.arazzo.yaml`.
   - Assert `valid == 1`, workflows > 0, steps > 0.

8. `inspect_directory_json_reports_invalid_file_without_aborting`
   - Create temp dir with one valid `.arazzo.yaml` and one invalid
     `.arazzo.yaml`.
   - Run `--json inspect <dir>`.
   - Assert output success, `invalid == 1`, invalid file has errors.

9. `default_help_stays_human_without_agent_workflow_block`
   - Run `--help`.
   - Assert it does not contain the first-pass phrase `Agent workflow:`.
   - Assert `agent` appears as a normal command summary.

## 7. Acceptance Criteria

### Compatibility

- `arazzo-cli --help` remains concise human help.
- `arazzo-cli run --help` remains ordinary command help.
- Existing `schema` and `schema <command>` outputs are unchanged.
- Existing command JSON schemas still match checked-in files.
- Existing command JSON outputs are unchanged in Phase 1.

### Agent Discovery

- `arazzo-cli --json agent commands` returns a complete command catalog.
- `arazzo-cli --json agent manifest` returns protocol, commands, schema
  pointers, recommended workflow, and error hints.
- Every command entry has:
  - intent
  - safety
  - args
  - flags
  - examples
  - schema pointer where applicable
  - next-command hints

### Schema Discovery

- `schema --index` lists every schemaable output.
- `schema --all` bundles every schemaable output.
- Checked-in schemas cover all new agent outputs.
- Schema drift tests fail when generated schemas differ.

### One-Shot Inspect

- `inspect <file> --json` returns spec/workflow/step context without network.
- `inspect <dir> --json` scans recursively and is deterministic.
- Invalid specs are reported per file instead of hiding valid files.

## 8. Verification Commands

Run all before handoff:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Additional manual smoke:

```bash
cargo run -p arazzo-cli -- --help
cargo run -p arazzo-cli -- agent commands
cargo run -p arazzo-cli -- --json agent commands
cargo run -p arazzo-cli -- --json agent manifest
cargo run -p arazzo-cli -- schema --index
cargo run -p arazzo-cli -- schema --all
cargo run -p arazzo-cli -- --json inspect examples/httpbin-get.arazzo.yaml
```

Manual compatibility check:

```bash
cargo run -p arazzo-cli -- schema
cargo run -p arazzo-cli -- schema run
cargo run -p arazzo-cli -- --json run examples/httpbin-get.arazzo.yaml status-check --dry-run --input code=429
```

## 9. Suggested Implementation Slices

### Slice 0: Restore Human Help Baseline

Goal: remove first-pass agent prose from default help.

Files:

- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`

Tests:

```bash
cargo fmt --all -- --check
cargo test -p arazzo-cli --test cli_integration schema_lists_available_commands
```

### Slice 1: Static Agent Command Catalog

Goal: add `--json agent commands`.

Files:

- `crates/arazzo-cli/src/agent_manifest.rs`
- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/main.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`

Tests:

```bash
cargo test -p arazzo-cli --test cli_integration agent_commands
```

### Slice 2: Agent Manifest

Goal: add `--json agent manifest`, protocol metadata, recommended workflow,
and error hints.

Files:

- `crates/arazzo-cli/src/agent_manifest.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`

Tests:

```bash
cargo test -p arazzo-cli --test cli_integration agent_manifest
```

### Slice 3: Schema Index And Bundle

Goal: add `schema --index`, `schema --all`, and schemas for new surfaces.

Files:

- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `docs/schemas/*.schema.json`
- `crates/arazzo-cli/tests/schema_drift.rs`
- `crates/arazzo-cli/tests/cli_integration.rs`

Tests:

```bash
cargo test -p arazzo-cli --test cli_integration schema_
cargo test -p arazzo-cli --test schema_drift
```

### Slice 4: One-Shot Inspect

Goal: add `--json inspect <spec-or-dir>`.

Files:

- `crates/arazzo-cli/src/inspect.rs`
- `crates/arazzo-cli/src/cli.rs`
- `crates/arazzo-cli/src/main.rs`
- `crates/arazzo-cli/src/handlers.rs`
- `docs/schemas/inspect.schema.json`
- `crates/arazzo-cli/tests/cli_integration.rs`
- `crates/arazzo-cli/tests/schema_drift.rs`

Tests:

```bash
cargo test -p arazzo-cli --test cli_integration inspect_
cargo test -p arazzo-cli --test schema_drift
```

### Slice 5: Documentation

Goal: document agent workflows without bloating default help.

Files:

- `README.md`
- `docs/extension-guide.md`
- `docs/schemas/*.schema.json`

Content:

- Add a short "Agent Discovery" section.
- Point to `--json agent commands`, `--json agent manifest`, `schema --all`,
  and `--json inspect`.
- State that default help is intentionally human-oriented.

## 10. Risks And Mitigations

### Risk: Static Metadata Drifts From Clap Definitions

Mitigation:

- Add integration tests that compare `agent commands` names against the known
  `schema` list plus command names expected in `--help`.
- Keep metadata in one Rust module with explicit tests.

### Risk: Agent Catalog Becomes Too Verbose

Mitigation:

- Keep `agent commands` compact.
- Put full protocol and recovery details in `agent manifest`.
- Keep examples short and canonical.

### Risk: Existing JSON Consumers Break

Mitigation:

- Do not change existing command JSON in Phase 1.
- Add new JSON contracts under new commands.
- Schema drift tests protect existing schemas.

### Risk: `schema --all` Output Is Large

Mitigation:

- Keep `schema --index` as the cheap discovery path.
- `schema --all` is explicit and opt-in.

### Risk: TOON Dependency Adds Supply-Chain Or Format Risk

Mitigation:

- Do not implement TOON in Phase 1.
- If later added, keep it derived from JSON and review the dependency first.

## 11. Definition Of Done

- Default help is not the agent documentation surface.
- Agents have stable JSON discovery via `agent commands` and `agent manifest`.
- Agents can retrieve every schema via `schema --index` and `schema --all`.
- Agents can inspect specs/directories without executing network calls.
- Existing tests and schema drift tests pass.
- New schemas are checked in.
- README documents the agent workflow without changing human help semantics.
