# Changelog

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
