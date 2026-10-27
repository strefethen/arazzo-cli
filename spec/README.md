# `spec/` — vendored specification documents

Verbatim, unmodified copies of the published specifications this project
implements against, fetched from their publishers.

| File | Document | Why it is here |
|---|---|---|
| `arazzo/v1.1.0.html` | Arazzo Specification v1.1.0 | The version this project implements. The sole authority on document shape, field meaning, and the expression and `successCriteria` grammars. |
| `arazzo/v1.0.1.html` | Arazzo Specification v1.0.1 | Needed to settle "was this construct in 1.0.x?" questions, which drive the version-gating diagnostics in `arazzo-validate`. |
| `oas/v3.2.0.html` | OpenAPI Specification v3.2.0 | Arazzo 1.1.0 defers to it by reference — e.g. the Parameter Object's `querystring` rules are stated as "per OpenAPI constraints", and the constraints themselves live here. |
| `asyncapi/v3.0.0.md` | AsyncAPI Specification v3.0.0 | Official Markdown source at the release tag's immutable commit. Defines the channel shape and address meaning for local AsyncAPI source resolution. |

## These are citable; the rest of the repo is not

`AGENTS.md` forbids citing this repo's code, docs, `examples/`, or `testdata/`
as evidence of what the specification allows, because they contain known
non-conformances. That rule does not apply here: these files are the published
documents themselves, byte-for-byte, not this project's description of them.

So quote `spec/arazzo/v1.1.0.html` in a plan or ticket the same way you would
quote its published source — because it is the same text. What is **not**
licensed is paraphrasing from memory and attributing it to these files; open
the file and copy the sentence.

## Reading them

The OpenAPI Initiative files are HTML; AsyncAPI is Markdown. `grep` works for
locating a phrase, but quoting HTML wants the prose without tags:

```bash
grep -n 'cannot coexist with' spec/arazzo/v1.1.0.html
```

To read a passage as plain text — adjust the anchor and the window:

```bash
python3 -c 'import re,html,sys
h=open("spec/arazzo/v1.1.0.html").read(); i=h.find("cannot coexist with")
print(html.unescape(re.sub(r"\s+"," ",re.sub(r"<[^>]+>","",h[i-1400:i+200]))))'
```

## Not tracked: fetch after cloning

The downloaded HTML and AsyncAPI Markdown files are gitignored; only this README,
`fetch.sh`, and `SHA256SUMS` are committed. Run this once per clone or
worktree before `cargo test`:

```bash
./spec/fetch.sh --if-missing
```

The `arazzo-expr` unit tests `include_str!` `arazzo/v1.1.0.html`, so without
it that crate's tests do not compile. `--if-missing` downloads only absent
files, leaves present ones alone, and then verifies against `SHA256SUMS`
without ever writing it, so a republished document fails the check instead of
being adopted. CI runs the plain script before building and fails if
`SHA256SUMS` changes, which catches the same thing there.

## Refreshing

Never hand-edit a vendored file. Re-run the fetcher, which also rewrites
`SHA256SUMS`:

```bash
./spec/fetch.sh
```

To confirm the working copies are still the documents that were fetched:

```bash
./spec/fetch.sh --verify
```

A `--verify` failure means a file was edited or truncated. A plain run that
moves a checksum means a publisher changed that document — read
the diff before committing it, because a republished spec can silently change
what this project is measured against.

Adding a document means adding its path and publisher URL to `spec/fetch.sh`,
re-running the script, and adding a row to the table above. OpenAPI Initiative
documents use `spec.openapis.org`; AsyncAPI 3.0.0 uses its official source URL
pinned to commit `d78dcea70c9b094a3df72f9a4e811828cec778cc` (the `v3.0.0` tag).

## Provenance and license

First fetched 2026-08-05 from `https://spec.openapis.org/{arazzo/v1.1.0,arazzo/v1.0.1,oas/v3.2.0}.html`;
all three carried `last-modified: Mon, 03 Aug 2026 10:21:46 GMT`. Re-fetched
2026-10-07 (`last-modified: Tue, 06 Oct 2026 15:11:48 GMT` on all three): the
two Arazzo documents were byte-identical; `oas/v3.2.0.html` changed and its
checksum in `SHA256SUMS` moved with it. The previous copy was not retained, so
there is no diff of that change.

The OpenAPI Initiative publishes these specifications under the Apache License,
Version 2.0. They are redistributed here unmodified, under that license, and
remain the copyright of their authors. This vendoring does not extend the
project's own license to them.

AsyncAPI 3.0.0 was fetched 2026-10-19 from
`https://www.asyncapi.com/docs/reference/specification/v3.0.0`, with SHA256
`0ce8b14bc28fa68caaf439ac31352061392f63a2dcc71cc84e9af94311dc4a65`.
Its published page is also reproduced verbatim and remains copyright of its
authors under the license stated in that document.

On 2026-10-27, AsyncAPI fetching switched to the unmodified official Markdown
source at
`https://raw.githubusercontent.com/asyncapi/spec/d78dcea70c9b094a3df72f9a4e811828cec778cc/spec/asyncapi.md`.
The rendered website's HTML checksum changed between fetches; pinning the
release source avoids website deployment churn while retaining exact-byte
verification. Existing local `asyncapi/v3.0.0.html` copies are historical;
use `asyncapi/v3.0.0.md` for current citations. The source identifies version
3.0.0 and carries the Apache License, Version 2.0.
