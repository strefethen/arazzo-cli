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

echo "Running release preflight checks..."
bash "$ROOT_DIR/scripts/release/verify-local-release.sh"

echo "Creating annotated tag $TAG..."
git -C "$ROOT_DIR" tag -a "$TAG" -m "Release $TAG"

if [ "$PUSH_FLAG" = "--push" ]; then
  echo "Pushing tag $TAG to $REMOTE..."
  git -C "$ROOT_DIR" push "$REMOTE" "$TAG"
  echo "Tag pushed. Internal release workflow should now run for $TAG."
else
  echo "Tag created locally."
  echo "To push and trigger release workflow:"
  echo "  git push $REMOTE $TAG"
fi
