# Changelog

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
