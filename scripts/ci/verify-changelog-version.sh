#!/usr/bin/env bash
# Fails when the newest CHANGELOG.md version heading does not match the
# workspace version in Cargo.toml. Keeps releases from shipping with a
# stale changelog (see docs/internal-release.md step 0).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cargo_version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo_root/Cargo.toml" | head -n 1)"
changelog_version="$(sed -n 's/^## \[\([0-9][^]]*\)\].*/\1/p' "$repo_root/CHANGELOG.md" | head -n 1)"

if [[ -z "$cargo_version" ]]; then
  echo "error: could not read workspace version from Cargo.toml" >&2
  exit 1
fi
if [[ -z "$changelog_version" ]]; then
  echo "error: could not find a '## [x.y.z]' heading in CHANGELOG.md" >&2
  exit 1
fi

if [[ "$cargo_version" != "$changelog_version" ]]; then
  echo "error: CHANGELOG.md top version [$changelog_version] does not match Cargo.toml version [$cargo_version]" >&2
  echo "hint: add a '## [$cargo_version]' section to CHANGELOG.md (docs/internal-release.md step 0)" >&2
  exit 1
fi

echo "CHANGELOG.md top version [$changelog_version] matches Cargo.toml [$cargo_version]"
