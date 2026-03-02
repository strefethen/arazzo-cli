# arazzo-cli Feature Requirements for PromptDeck Provider

> **Status: All features implemented** — see commit `61c9d9b` on `main`.

## Background

PromptDeck's provider system discovers items from external tools and renders them as tappable buttons on iOS. The Arazzo provider will discover API workflow specs via `arazzo-cli` and allow executing workflows/steps from the button grid.

**User's vision:** Workflows as groups, steps as buttons. Tap a step to execute it. Tap a workflow to execute the whole thing.

**Data mapping:**

| PromptDeck concept | Arazzo mapping |
|--------------------|----------------|
| **Page** | Target page from config (e.g., "APIs") |
| **Group** | One workflow (e.g., "Get Pets") |
| **Item** | One step within that workflow |

## Current CLI Capabilities (working well)

- `catalog --json <dir>` — lists all specs with workflows, inputs, outputs ✅
- `show --json --dir <dir> <workflow-id>` — workflow detail ✅ (but only step *count*, not step details)
- `run --json <spec> <workflow-id>` — execute workflow ✅
- `run --dry-run --json` — preview requests ✅
- `schema --json <command>` — JSON schema for output ✅

## Feature 1: `steps` subcommand — list steps within a workflow ✅

**Priority: P0 — blocks step-level discovery** | **Status: Implemented**

The provider maps steps as buttons within workflow groups. Currently `show` returns step *count* but not step *details*.

### Proposed command

```bash
arazzo-cli steps --json <spec-file> <workflow-id>
# or
arazzo-cli steps --json --dir <dir> <workflow-id>
```

### Required JSON output per step

```json
[
  {
    "stepId": "get-all-pets",
    "description": "Retrieve all pets from the store",
    "method": "GET",
    "url": "/pets",
    "operationId": "listPets",
    "position": 0
  },
  {
    "stepId": "filter-by-status",
    "description": "Filter pets by availability status",
    "method": "GET",
    "url": "/pets?status=available",
    "operationId": "listPets",
    "position": 1
  }
]
```

### Why each field matters to the provider

| Field | Used for |
|-------|----------|
| `stepId` | Button identifier + execution routing (`_arazzo_step` metadata key) |
| `description` | Button label (`displayLabel`) — falls back to stepId if absent |
| `method` | Display hint (icon selection: GET→arrow.down, POST→plus, DELETE→trash) |
| `url` | Button description text (shows "GET /pets" under the label) |
| `operationId` | Metadata for debugging/display (optional) |
| `position` | Sort order within the group (maintain workflow step sequence) |

### Edge cases

- `method` and `url` may not always be available (some steps are `workflowId` references, not direct HTTP calls). These should be `null` when not applicable.
- If a step references another workflow (via `workflowId`), include a `referencedWorkflow` field instead of `method`/`url` so the provider can show an appropriate icon (e.g., "arrow.triangle.swap" instead of "play.circle").

## Feature 2: `run --step` — execute a single step ✅

**Priority: P0 — blocks step-level execution** | **Status: Implemented**

When the user taps a step button, the host must execute just that one step.

### Proposed command

```bash
arazzo-cli run --step <step-id> --json <spec-file> <workflow-id>
```

### Expected JSON output

Same structure as existing `run --json`:

```json
{"kind": "success", "outputs": {"petId": "123", "status": "available"}}
```
or
```json
{"kind": "error", "error": "Step requires input 'petId' from previous step", "code": "missing_input"}
```

### Key design question: dependency resolution

If step 3 depends on output from steps 1-2 (e.g., uses `$steps.get-pets.outputs.body`), does single-step execution:

- **(a) Run all prerequisite steps** automatically (transparent dependency resolution), or
- **(b) Fail with a clear error** explaining which inputs are missing, or
- **(c) Accept inputs via CLI flags** (e.g., `--input petId=123`) to satisfy dependencies manually?

**Recommendation:** Option (a) with a flag. By default, run prerequisites. Add `--no-deps` for users who want isolated execution. The provider would use the default (run prerequisites) since that matches user expectation — "tap step 3" should make step 3 work, not fail because steps 1-2 weren't run first.

**Implementation note:** This is what was implemented. `--no-deps` fails early with error code `STEP_MISSING_DEPENDENCY` listing the missing step references. Without `--no-deps`, transitive dependencies are computed via BFS over `$steps.*` references and executed in workflow order before the target step.

### Dry-run variant

Should also work:
```bash
arazzo-cli run --step <step-id> --dry-run --json <spec-file> <workflow-id>
```

## Feature 3: Enhanced `show` output (optional optimization) ✅

**Priority: P1 — reduces CLI calls during discovery, not blocking** | **Status: Implemented**

The current `show --json` output has `"steps": 3` (just a count). Including step summaries inline would let the provider get steps from `show` without a separate `steps` call per workflow, reducing CLI invocations during discovery.

### Previous output (before implementation)

```json
{
  "id": "get-pets",
  "file": "pet-store.arazzo.yaml",
  "title": "Pet Store API",
  "summary": "Get all pets",
  "steps": 3,
  "inputs": {...},
  "outputs": [...],
  "sources": [...]
}
```

### Current output (implemented)

**Note:** The count field is `step_count` (snake_case), not `stepCount` — `WorkflowDetail` does not use `rename_all = "camelCase"`. The `steps` array items use camelCase (`stepId`) since `StepSummary` does.

```json
{
  "id": "get-pets",
  "file": "pet-store.arazzo.yaml",
  "title": "Pet Store API",
  "summary": "Get all pets",
  "step_count": 3,
  "steps": [
    {"stepId": "list-pets", "description": "Get all pets", "method": "GET", "url": "/pets"},
    {"stepId": "filter", "description": "Filter by status", "method": "GET", "url": "/pets?status=available"},
    {"stepId": "get-first", "description": "Get first pet details", "method": "GET", "url": "/pets/{petId}"}
  ],
  "inputs": {...},
  "outputs": [...],
  "sources": [...]
}
```

### Why this helps

Discovery performance depends on CLI call count. Currently `catalog` returns all specs+workflows in one call (excellent). But to get steps, the provider needs `steps <spec> <workflow>` per workflow. For a directory with 5 specs averaging 4 workflows each, that's 1 + 20 = 21 CLI invocations during discovery. If `show` included step details inline, the provider could use `catalog` (1 call) to get workflows, then only call `show` for workflows the user navigates into.

## Summary

| Priority | Feature | CLI command | Status |
|----------|---------|-------------|--------|
| **P0** | List steps | `steps --json <spec> <workflow>` | ✅ Implemented |
| **P0** | Execute single step | `run --step <id> --json <spec> <workflow>` | ✅ Implemented |
| **P0** | Isolated execution | `run --step <id> --no-deps --json <spec> <workflow>` | ✅ Implemented |
| **P1** | Inline steps in `show` | Enhanced `show --json` | ✅ Implemented |
