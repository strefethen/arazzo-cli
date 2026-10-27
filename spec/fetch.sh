#!/usr/bin/env sh
# Re-fetch the vendored specification documents and refresh SHA256SUMS.
#
# These files are verbatim copies of the published documents. Never hand-edit
# them — re-run this script instead.
#
#   ./spec/fetch.sh               re-download and rewrite SHA256SUMS
#   ./spec/fetch.sh --verify      check the working copies against SHA256SUMS
#   ./spec/fetch.sh --if-missing  download only absent documents, then verify;
#                                 never writes SHA256SUMS
#
# A --verify failure means a vendored file was edited or truncated; a plain
# run that changes SHA256SUMS means a publisher changed a document. Both are
# worth investigating before committing.
#
# To vendor another document, add its path and explicit source URL below.
set -eu

OPENAPI_DOCS='arazzo/v1.1.0.html
arazzo/v1.0.1.html
oas/v3.2.0.html'
# Use the official release source, pinned to the v3.0.0 tag's commit. The
# rendered website includes deployment-specific assets and is not byte-stable.
ASYNCAPI_DOC=asyncapi/v3.0.0.md
ASYNCAPI_URL=https://raw.githubusercontent.com/asyncapi/spec/d78dcea70c9b094a3df72f9a4e811828cec778cc/spec/asyncapi.md

sha256() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$@"
    else
        sha256sum "$@"
    fi
}

cd -- "$(dirname -- "$0")"

fetch() {
    mkdir -p -- "$(dirname -- "$1")"
    echo "fetching $2"
    curl -sSfL -o "$1" "$2"
}

case "${1:-}" in
--verify)
    sha256 -c SHA256SUMS
    exit
    ;;
--if-missing)
    # Safe for agents and fresh worktrees: present files are left alone and
    # SHA256SUMS is only read, so a republished document fails the check
    # below instead of being adopted. A plain run is the deliberate refresh.
    for doc in $OPENAPI_DOCS; do
        [ -f "$doc" ] || fetch "$doc" "https://spec.openapis.org/$doc"
    done
    [ -f "$ASYNCAPI_DOC" ] || fetch "$ASYNCAPI_DOC" "$ASYNCAPI_URL"
    sha256 -c SHA256SUMS
    exit
    ;;
"") ;;
*)
    echo "usage: $0 [--verify | --if-missing]" >&2
    exit 2
    ;;
esac

for doc in $OPENAPI_DOCS; do
    fetch "$doc" "https://spec.openapis.org/$doc"
done
fetch "$ASYNCAPI_DOC" "$ASYNCAPI_URL"

: >SHA256SUMS
for doc in $OPENAPI_DOCS $ASYNCAPI_DOC; do
    # sha256sum defaults to binary mode on Windows (Git Bash) and marks the
    # path with "*". The digest is identical, so write the text-mode two-space
    # form everywhere; otherwise SHA256SUMS differs by platform and CI's
    # "git diff --exit-code" after fetching fails on the Windows runner.
    sha256 "$doc" | sed 's/ \*/  /' >>SHA256SUMS
done
echo "wrote $(pwd)/SHA256SUMS"
