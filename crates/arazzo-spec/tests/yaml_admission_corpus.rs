#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

use arazzo_spec::parse_yaml_value;

fn collect_documents(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|err| panic!("reading {}: {err}", dir.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|err| panic!("reading {} entry: {err}", dir.display()))
            .path();
        if path.is_dir() {
            collect_documents(&path, out);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "yaml" | "yml" | "json"))
        {
            out.push(path);
        }
    }
}

#[test]
fn every_example_and_testdata_document_is_admitted_exactly_as_parsed_before() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut documents = Vec::new();
    for dir in ["examples", "testdata"] {
        collect_documents(&root.join(dir), &mut documents);
    }
    assert!(documents.len() > 40, "corpus shrank: {}", documents.len());
    for path in documents {
        let bytes =
            fs::read(&path).unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
        let admitted = parse_yaml_value(&bytes).map_err(|err| err.to_string());
        let plain = serde_yaml_ng::from_slice::<serde_yaml_ng::Value>(&bytes)
            .map_err(|err| err.to_string());
        assert_eq!(admitted, plain, "{}", path.display());
    }
}
