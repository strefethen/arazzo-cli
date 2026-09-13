# Vendored serde_json_path_core 0.2.2 with the numeric-equality repair

This directory is a vendored copy of the published `serde_json_path_core`
0.2.2 crate, applied to the workspace through `[patch.crates-io]` in the root
`Cargo.toml`. It exists to carry exactly one repair; nothing else differs from
the upstream release.

## Upstream provenance

- Crate: `serde_json_path_core` 0.2.2, MIT,
  <https://github.com/hiltontj/serde_json_path>.
- Registry package `serde_json_path_core-0.2.2.crate`, SHA-256
  `dde67d8dfe7d4967b5a95e247d4148368ddd1e753e500adb34b3ffe40c6bc1bc`
  (the crates.io checksum Cargo records for this version).
- Upstream VCS revision recorded by the package:
  `66283b5e2fa9d099439882c6afba395d5de7b9f3`, path `serde_json_path_core`.
- Vendored files: `Cargo.toml`, `Cargo.toml.orig`, `CHANGELOG.md`,
  `LICENSE-MIT`, `README.md` and `src/**` — what Cargo needs to build the crate,
  unchanged apart from the patch below. Registry metadata (`.cargo-ok`,
  `.cargo_vcs_info.json`) and the package `Cargo.lock` are deliberately not
  vendored.

## The repair

`numeric-equality.patch` (SHA-256
`b29b9415f5c4b3ada5e424739c22f30af1140f9006e9a5ea83fb57f9fecf24ee`) adds
eight lines to `value_equal_to` in `src/spec/selector/filter.rs` so that filter
equality compares arrays element-wise and objects member-wise with the same
numeric equality it already applies to scalars. Upstream 0.2.2 falls through to
`serde_json::Value`'s `PartialEq` for containers, so `[1] == [1.0]` and
`{"x": 1} == {"x": 1.0}` are false inside a filter while `1 == 1.0` is true.

The patch was qualified in the frozen I-Regexp/serde integration proof of
2026-09-12 before this workspace adopted it. It is applied verbatim.

## Verify

From this directory:

```sh
patch -p1 -R --dry-run < numeric-equality.patch
shasum -a 256 numeric-equality.patch
```

The reverse dry run must apply cleanly and the checksum must match the value
above. Any other difference from the upstream package is a defect.

## Removal criterion

Remove the `[patch.crates-io]` entry and this directory when a published
`serde_json_path_core` release passes the nested numeric-equality cases in
`crates/arazzo-expr/tests/jsonpath_backend.rs`
(`nested_numeric_equality_compares_containers_recursively`) unpatched. Move
`serde_json_path` and its core together; do not carry this patch onto a newer
release without re-qualifying it.
