#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
failures=0
past_guards="CHANGELOG.md has no entry for v1.2.3"

write_fixture() {
  rm -rf "$work/repo" "$work/remote.git"
  mkdir -p "$work/repo/scripts/release" "$work/repo/plans" "$work/repo/src"
  cp "$repo_root/scripts/release/cut-tag.sh" "$work/repo/scripts/release/"
  printf '## [Unreleased]\n' >"$work/repo/CHANGELOG.md"
  printf 'plan\n' >"$work/repo/plans/plan.md"
  printf 'code\n' >"$work/repo/src/code.txt"
  git -C "$work/repo" init --quiet --initial-branch=main
  git -C "$work/repo" -c user.name=test -c user.email=test@example.invalid \
    add --all
  git -C "$work/repo" -c user.name=test -c user.email=test@example.invalid \
    commit --quiet -m fixture
  git init --quiet --bare --initial-branch=main "$work/remote.git"
  git -C "$work/repo" remote add origin "$work/remote.git"
}

expect() {
  local label="$1" pattern="$2"
  shift 2
  local output status=0
  output="$(bash "$work/repo/scripts/release/cut-tag.sh" v1.2.3 "$@" 2>&1)" || status=$?
  if [[ "$status" -eq 0 ]]; then
    echo "FAIL $label: expected a refusal, exit 0: $output" >&2
    failures=$((failures + 1))
  elif ! grep -qF -- "$pattern" <<<"$output"; then
    echo "FAIL $label: output lacks '$pattern': $output" >&2
    failures=$((failures + 1))
  else
    echo "ok $label"
  fi
}

write_fixture
expect "a clean tree passes the guards" "$past_guards"

write_fixture
printf 'edited\n' >"$work/repo/plans/plan.md"
printf 'new\n' >"$work/repo/untracked.txt"
expect "dirty plans/ and untracked files pass the guards" "$past_guards"

write_fixture
printf 'edited\n' >"$work/repo/src/code.txt"
expect "a dirty file outside plans/ is refused" "src/code.txt"

write_fixture
printf 'edited\n' >"$work/repo/plans/plan.md"
git -C "$work/repo" add plans/plan.md
expect "a staged change is refused, even under plans/" "the index must be empty"

write_fixture
expect "--push refuses a HEAD that is not on the remote main" "couldn't find remote ref main" --push origin

write_fixture
git -C "$work/repo" push --quiet origin main
printf 'more\n' >>"$work/repo/src/code.txt"
git -C "$work/repo" -c user.name=test -c user.email=test@example.invalid \
  commit --quiet -am ahead
expect "--push refuses unpushed commits" "HEAD is not on origin/main" --push origin

write_fixture
git -C "$work/repo" push --quiet origin main
expect "--push passes the guards when HEAD is on the remote main" "$past_guards" --push origin

if [[ "$failures" -ne 0 ]]; then
  echo "$failures failure(s)" >&2
  exit 1
fi
echo "cut-tag guard tests passed"
