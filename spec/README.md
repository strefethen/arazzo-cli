# `spec/` — vendored specification documents

Verbatim, unmodified copies of the published specifications this project
implements against, fetched from `spec.openapis.org`.

| File | Document | Why it is here |
|---|---|---|
| `arazzo/v1.1.0.html` | Arazzo Specification v1.1.0 | The version this project implements. The sole authority on document shape, field meaning, and the expression and `successCriteria` grammars. |
| `arazzo/v1.0.1.html` | Arazzo Specification v1.0.1 | Needed to settle "was this construct in 1.0.x?" questions, which drive the version-gating diagnostics in `arazzo-validate`. |
| `oas/v3.2.0.html` | OpenAPI Specification v3.2.0 | Arazzo 1.1.0 defers to it by reference — e.g. the Parameter Object's `querystring` rules are stated as "per OpenAPI constraints", and the constraints themselves live here. |

## These are citable; the rest of the repo is not

`AGENTS.md` forbids citing this repo's code, docs, `examples/`, or `testdata/`
as evidence of what the specification allows, because they contain known
non-conformances. That rule does not apply here: these files are the published
documents themselves, byte-for-byte, not this project's description of them.

So quote `spec/arazzo/v1.1.0.html` in a plan or ticket the same way you would
quote `spec.openapis.org` — because it is the same text. What is **not**
licensed is paraphrasing from memory and attributing it to these files; open
the file and copy the sentence.

## Reading them

The files are HTML. `grep` works for locating a phrase, but quoting wants the
prose without tags:

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

The HTML files are gitignored (`/spec/**/*.html`); only this README,
`fetch.sh`, and `SHA256SUMS` are committed. Run `./spec/fetch.sh` once per
clone or worktree before `cargo test`: the `arazzo-expr` unit tests
`include_str!` `arazzo/v1.1.0.html`, so without it that crate's tests do not
compile. CI runs the same script before building and fails if `SHA256SUMS`
changes, so a republished document is caught there instead of being adopted
silently.

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
moves a checksum means the OpenAPI Initiative republished that document — read
the diff before committing it, because a republished spec can silently change
what this project is measured against.

Adding a document (an older OpenAPI major for `arazzo-generate`, an AsyncAPI
version for `sourceDescriptions`) means adding its path to `DOCS` in
`spec/fetch.sh`, re-running the script, and adding a row to the table above.
The path is used verbatim both under `spec.openapis.org/` and inside `spec/`.

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
