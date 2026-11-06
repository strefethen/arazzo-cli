#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0

write_fixture() {
  rm -rf "$work/repo"
  mkdir -p "$work/repo/scripts/ci" "$work/repo/docs"
  cp "$repo_root/scripts/ci/verify-site-version.sh" "$work/repo/scripts/ci/"
  printf '[workspace.package]\nversion = "1.2.3"\n' >"$work/repo/Cargo.toml"
  printf '## [Unreleased]\n\n## [1.2.3] - 2030-01-15\n\n## [1.2.2] - 2029-12-01\n' >"$work/repo/CHANGELOG.md"
  cat >"$work/repo/docs/index.html" <<'EOF'
<meta name="arazzo-cli:version" content="1.2.3">
<meta name="arazzo-cli:release-tag" content="v1.2.3">
<meta name="arazzo-cli:page-updated" content="2030-01-15">
"softwareVersion": "1.2.3",
"dateModified": "2030-01-15",
<a href="releases/tag/v1.2.3">v1.2.3</a> lucide@9.9.9
EOF
}

edit_page() {
  perl -pi -e "$1" "$work/repo/docs/index.html"
}

expect() {
  local label="$1" want="$2" pattern="$3"
  shift 3
  local output status=0
  output="$(bash "$work/repo/scripts/ci/verify-site-version.sh" "$@" 2>&1)" || status=$?
  if [[ "$want" == pass && "$status" -ne 0 ]] || [[ "$want" == fail && "$status" -eq 0 ]]; then
    echo "FAIL $label: expected $want, exit $status: $output" >&2
    failures=$((failures + 1))
  elif ! grep -qF -- "$pattern" <<<"$output"; then
    echo "FAIL $label: output lacks '$pattern': $output" >&2
    failures=$((failures + 1))
  else
    echo "ok   $label"
  fi
}

write_fixture
expect "current page passes" pass "matches release v1.2.3"

write_fixture
expect "matching tag passes" pass "matches release v1.2.3" v1.2.3

write_fixture
expect "mismatched tag fails" fail "tag v1.2.4 does not match" v1.2.4

write_fixture
edit_page 's/name="arazzo-cli:version" content="1.2.3"/name="arazzo-cli:version" content="1.2.2"/'
expect "stale version meta fails" fail "arazzo-cli:version [1.2.2]"

write_fixture
edit_page 's/content="v1.2.3"/content="v1.2.2"/'
expect "stale release-tag meta fails" fail "arazzo-cli:release-tag [v1.2.2]"

write_fixture
edit_page 's/"softwareVersion": "1.2.3"/"softwareVersion": "1.2.2"/'
expect "stale structured data fails" fail "softwareVersion [1.2.2]"

write_fixture
printf 'gh release download v1.1.0\n' >>"$work/repo/docs/index.html"
expect "stale command version fails" fail "other release versions: v1.1.0"

write_fixture
edit_page 's/content="2030-01-15"/content="2029-12-01"/; s/"dateModified": "2030-01-15"/"dateModified": "2029-12-01"/'
expect "page older than release fails" fail "predates the 1.2.3 changelog date"

write_fixture
edit_page 's/"dateModified": "2030-01-15"/"dateModified": "2030-01-16"/'
expect "mismatched dateModified fails" fail "dateModified [2030-01-16]"

write_fixture
perl -pi -e 's/^## \[1\.2\.3\].*\n//' "$work/repo/CHANGELOG.md"
expect "missing changelog section fails" fail "no dated section for [1.2.3]"

write_fixture
rm "$work/repo/docs/index.html"
expect "missing page fails" fail "docs/index.html not found"

if [[ "$failures" -ne 0 ]]; then
  echo "$failures case(s) failed" >&2
  exit 1
fi
echo "verify-site-version: all cases passed"
