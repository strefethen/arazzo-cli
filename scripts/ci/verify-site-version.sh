#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
page="$repo_root/docs/index.html"
expected_tag_arg="${1:-}"
hint="hint: review docs/index.html against this release, then update its version metadata (docs/internal-release.md step 0)"
failed=0

fail() {
  echo "error: $1" >&2
  failed=1
}

meta_content() {
  sed -n "s/.*<meta name=\"arazzo-cli:$1\" content=\"\([^\"]*\)\".*/\1/p" "$page" | head -n 1
}

cargo_version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$repo_root/Cargo.toml" | head -n 1)"
if [[ -z "$cargo_version" ]]; then
  echo "error: could not read workspace version from Cargo.toml" >&2
  exit 1
fi
if [[ ! -f "$page" ]]; then
  echo "error: docs/index.html not found" >&2
  exit 1
fi
tag="v$cargo_version"

if [[ -n "$expected_tag_arg" && "$expected_tag_arg" != "$tag" ]]; then
  fail "tag $expected_tag_arg does not match Cargo.toml version [$cargo_version]"
fi

page_version="$(meta_content version)"
if [[ "$page_version" != "$cargo_version" ]]; then
  fail "docs/index.html arazzo-cli:version [$page_version] does not match Cargo.toml [$cargo_version]"
fi

page_tag="$(meta_content release-tag)"
if [[ "$page_tag" != "$tag" ]]; then
  fail "docs/index.html arazzo-cli:release-tag [$page_tag] does not match [$tag]"
fi

software_version="$(sed -n 's/.*"softwareVersion": "\([^"]*\)".*/\1/p' "$page" | head -n 1)"
if [[ "$software_version" != "$cargo_version" ]]; then
  fail "docs/index.html softwareVersion [$software_version] does not match Cargo.toml [$cargo_version]"
fi

stale_tags="$(grep -oE '\bv[0-9]+\.[0-9]+\.[0-9]+' "$page" | sort -u | grep -vxF "$tag" || true)"
if [[ -n "$stale_tags" ]]; then
  fail "docs/index.html mentions other release versions: $(echo "$stale_tags" | tr '\n' ' ')"
fi

release_date="$(sed -n "s/^## \[$cargo_version\] - \([0-9-]*\).*/\1/p" "$repo_root/CHANGELOG.md" | head -n 1)"
page_updated="$(meta_content page-updated)"
date_modified="$(sed -n 's/.*"dateModified": "\([^"]*\)".*/\1/p' "$page" | head -n 1)"
if [[ -z "$release_date" ]]; then
  fail "CHANGELOG.md has no dated section for [$cargo_version]"
elif [[ ! "$page_updated" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  fail "docs/index.html arazzo-cli:page-updated [$page_updated] is not YYYY-MM-DD"
elif [[ "$page_updated" < "$release_date" ]]; then
  fail "docs/index.html arazzo-cli:page-updated [$page_updated] predates the $cargo_version changelog date [$release_date]"
fi
if [[ "$date_modified" != "$page_updated" ]]; then
  fail "docs/index.html dateModified [$date_modified] does not match arazzo-cli:page-updated [$page_updated]"
fi

if [[ "$failed" -ne 0 ]]; then
  echo "$hint" >&2
  exit 1
fi

echo "docs/index.html matches release $tag (updated $page_updated)"
