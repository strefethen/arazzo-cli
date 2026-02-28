# Internal Release Playbook

This repository uses private GitHub releases for distribution.

No crates are published externally.

## 1. Preflight (Required)

Run from repository root:

```bash
bash scripts/ci/verify-private-workspace.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
bash scripts/release/verify-local-release.sh
```

## 2. Tag A Release

Create and push a semantic tag:

```bash
git tag v0.1.0
git push origin v0.1.0
```

This triggers `.github/workflows/release-internal.yml`.

Automated alternative (runs preflight checks, creates tag, and optionally pushes):

```bash
bash scripts/release/cut-tag.sh v0.1.0 --push origin
```

## 3. What The Workflow Produces

For each tag, the workflow builds release binaries on:

- Linux (`linux-x86_64`)
- macOS (`macos`)
- Windows (`windows-x86_64`)

It publishes release assets named like:

- `arazzo-cli-v0.1.0-linux-x86_64`
- `arazzo-cli-v0.1.0-macos`
- `arazzo-cli-v0.1.0-windows-x86_64.exe`
- `SHA256SUMS.txt`

## 4. Optional Manual Trigger

You can run the workflow manually (`workflow_dispatch`) by supplying an **existing** tag (for example `v0.1.0`).

## 5. Post-Release Validation

From the release page, download a binary and verify:

```bash
./arazzo-cli --json validate examples/httpbin-get.arazzo.yaml
```

For checksum validation:

```bash
shasum -a 256 -c SHA256SUMS.txt
```

Automated alternative (downloads assets via GitHub CLI, verifies checksums, and smoke-tests the local-host binary):

```bash
bash scripts/release/verify-downloaded-release.sh v0.1.0
```
