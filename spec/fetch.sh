#!/usr/bin/env sh
# Re-fetch the vendored specification documents and refresh SHA256SUMS.
#
# These files are verbatim copies of the published documents at
# spec.openapis.org. Never hand-edit them — re-run this script instead.
#
#   ./spec/fetch.sh           re-download and rewrite SHA256SUMS
#   ./spec/fetch.sh --verify  check the working copies against SHA256SUMS
#
# A --verify failure means a vendored file was edited or truncated; a plain
# run that changes SHA256SUMS means the OpenAPI Initiative republished a
# document. Both are worth reading the diff for before committing.
#
# To vendor another document, add its path to DOCS below and re-run. The path
# is used verbatim both under spec.openapis.org/ and inside spec/.
set -eu

DOCS='arazzo/v1.1.0.html
arazzo/v1.0.1.html
oas/v3.2.0.html'

BASE=https://spec.openapis.org

sha256() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$@"
    else
        sha256sum "$@"
    fi
}

cd -- "$(dirname -- "$0")"

if [ "${1:-}" = "--verify" ]; then
    sha256 -c SHA256SUMS
    exit
fi

for doc in $DOCS; do
    mkdir -p -- "$(dirname -- "$doc")"
    echo "fetching $BASE/$doc"
    curl -sSfL -o "$doc" "$BASE/$doc"
done

: >SHA256SUMS
for doc in $DOCS; do
    # sha256sum defaults to binary mode on Windows (Git Bash) and marks the
    # path with "*". The digest is identical, so write the text-mode two-space
    # form everywhere; otherwise SHA256SUMS differs by platform and CI's
    # "git diff --exit-code" after fetching fails on the Windows runner.
    sha256 "$doc" | sed 's/ \*/  /' >>SHA256SUMS
done
echo "wrote $(pwd)/SHA256SUMS"
