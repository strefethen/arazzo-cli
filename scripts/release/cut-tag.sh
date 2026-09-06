#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = "" ]; then
  echo "Usage: bash scripts/release/cut-tag.sh <tag> [--push] [remote]" >&2
  echo "Example: bash scripts/release/cut-tag.sh v0.1.0 --push origin" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TAG="$1"
PUSH_FLAG="${2:-}"
REMOTE="${3:-origin}"

if [[ ! "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "ERROR: tag must match v<major>.<minor>.<patch> (got: $TAG)" >&2
  exit 1
fi

if ! git -C "$ROOT_DIR" diff --quiet || ! git -C "$ROOT_DIR" diff --cached --quiet; then
  echo "ERROR: working tree must be clean before cutting a release tag." >&2
  exit 1
fi

if git -C "$ROOT_DIR" rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "ERROR: tag already exists: $TAG" >&2
  exit 1
fi

# The release workflow publishes this section as the release body. Catch a
# missing entry here, while the tag is still uncreated and unpushed, rather
# than after CI has built three binaries for a release page with no notes.
VERSION="${TAG#v}"
NOTES="$(awk -v ver="$VERSION" '
  index($0, "## [" ver "]") == 1 { capture = 1; next }
  capture && index($0, "## [") == 1 { exit }
  capture { print }
' "$ROOT_DIR/CHANGELOG.md")"
if [ -z "$(printf '%s' "$NOTES" | tr -d '[:space:]')" ]; then
  echo "ERROR: CHANGELOG.md has no entry for $TAG." >&2
  echo "Expected a section headed '## [$VERSION] - <date>'." >&2
  echo "Add it before cutting the tag; a release must not publish empty notes." >&2
  exit 1
fi

echo "Running release preflight checks..."
bash "$ROOT_DIR/scripts/release/verify-readiness.sh"

echo "Creating annotated tag $TAG..."
git -C "$ROOT_DIR" tag -a "$TAG" -m "Release $TAG"

if [ "$PUSH_FLAG" = "--push" ]; then
  echo "Pushing tag $TAG to $REMOTE..."
  git -C "$ROOT_DIR" push "$REMOTE" "$TAG"
  echo "Tag pushed. Internal release workflow should now run for $TAG."

  echo "Installing $TAG locally..."
  (cd "$ROOT_DIR" && cargo install --path "$ROOT_DIR/crates/arazzo-cli" --locked --force)
  echo "Local arazzo-cli install updated."
else
  echo "Tag created locally."
  echo "To push and trigger release workflow:"
  echo "  git push $REMOTE $TAG"
fi
