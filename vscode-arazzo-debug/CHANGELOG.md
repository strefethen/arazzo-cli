# Changelog

## [0.0.8] - 2026-10-17

Debugger failures now explain what went wrong and where, with reliable inspection of parent and child workflows.

**Upgrade if** you have seen unexplained `read error` messages, missing values displayed as `null`, skipped breakpoints, or misleading workflow completion messages.

- Unresolved outputs identify the output name, expression, workflow, and step.
- Watch and inspection failures report their cause instead of showing empty data.
- Parent and child stack frames display their own inputs and values.
- Invalid breakpoints explain why they were not installed, including in Arazzo Debug output.

### Fixed
- Report unresolved step and workflow outputs before completion; the completion summary includes their count. Actual JSON `null` values remain valid values.
- Return explicit errors for missing watch values, malformed expressions, invalid frame IDs, expired variable references, and unavailable source locations.
- Preserve variable references across scope requests within a pause and prevent expired references from pointing to new values.
- Use the selected stack frame's saved context when inspecting a parent workflow; allow breakpoints in called child workflows.
- Reject invalid breakpoint syntax with an unverified breakpoint and its source location. If a condition cannot be evaluated at runtime, end the debug run with the condition, workflow, step, and underlying error rather than silently skipping the breakpoint.
- Explain missing executables, inaccessible working directories, missing specs, startup failures, failed debugger requests, and unexpected process exits with session context.
- Report termination once, distinguish successful completion from requested stops, and suppress the spurious `read error` caused by normal shutdown.
- Return refused stepping requests as debugger errors without dropping the session.
- Pass constant parameter values correctly to child workflows.

### Changed
- **Migration required for legacy operation targets:** the bundled runtime now resolves standard Arazzo `operationId` and `operationPath` references from the referenced OpenAPI documents. Replace legacy forms such as `GET service./path` with a declared operation ID (for example, `$sourceDescriptions.service.getForecast`) or a standard operation-path reference. Relative `sourceDescriptions[].url` values identify documents relative to the Arazzo file. Reusable actions use `reference: $components.failureActions.name` or `reference: $components.successActions.name`; legacy action-name lookup is removed.
- JSONPath expressions nested more than eight bracket levels are rejected explicitly.

### Added
- Runnable, server-free debugger diagnostic specs and a VS Code demonstration workspace in `testdata/debugger-diagnostics`, backed by regression tests.
- Dry-run output checkpoints use the same debugger diagnostics as live requests.

## [0.0.7] - 2026-10-09

### Fixed
- Debug hover now drills into runtime expressions again. Hovering `$response`
  shows the response object, `$response.body` shows the body, each JSON Pointer
  segment shows the corresponding nested value, and hovering the condition
  operator still evaluates the full boolean expression.
- `$request` and `$response` hover values are expandable and consistently
  include the body when it is available.
- Simple condition hovers such as
  `$response.body#/features/0/properties/display_name != null` now evaluate as
  booleans only when the whole condition is selected, avoiding misleading
  `null` results for valid sub-expressions.

## [0.0.6] - 2026-08-03

### Fixed
- A workflow that fails during a debug session now reports the error instead of
  ending the session silently. Previously the session just closed when you hit
  Continue — for example when a step's `operationId` was not found — with no
  message anywhere in the UI. Failures now appear in the Debug Console and the
  session reports a non-zero exit code. (#2)

### Added
- **Arazzo Debug output channel.** The extension previously created no output
  channel at all, so problems starting the debug adapter left no visible trace.
  The channel records session start/end, workflow exit codes, adapter errors,
  and which adapter binary each session launched; it reveals itself
  automatically when the adapter fails or exits abnormally. (#2)
- **OpenAPI documents referenced by `sourceDescriptions` load automatically.**
  When a `sourceDescriptions[].url` is a relative path, the document is resolved
  against the Arazzo file's directory and loaded, so steps that target an
  `operationId` resolve without extra configuration. Absolute `http(s)` URLs
  keep their existing meaning as the request base URL.

### Changed
- Redirects that downgrade from `https` to `http` are now refused during debug
  sessions rather than followed silently.

## [0.0.5] - 2026-08-03

### Fixed
- Debug adapter now works on Windows, Linux, and Intel macOS. The extension is
  published as platform-specific builds, each bundling a native
  `arazzo-debug-adapter` binary; previous versions were a single universal
  package that bundled only an Apple Silicon macOS binary. (#2)

### Added
- Windows ARM64 (`win32-arm64`) build target.

## [0.0.4] - 2026-03-13

### Fixed
- Marketplace screenshot URL.

## [0.0.3] - 2026-03-13

### Added
- Debug session screenshot for the Marketplace listing.

## [0.0.2] - 2026-03-13

### Changed
- Extension icon.

## [0.0.1] - 2026-02-28

### Added
- Initial preview release
- Debug adapter for Arazzo 1.0 workflow specs
- Breakpoints on workflow steps, success criteria, and actions
- Variable inspection: Locals, Request, Response, Inputs, Steps
- Watch expressions with full Arazzo expression syntax
- Step Over / Step In / Step Out / Continue / Pause controls
- Stop-on-entry mode
- Conditional breakpoints
- Sub-workflow call stack tracking
