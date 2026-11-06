use std::path::Path;

pub(crate) fn discover_specs(dir: &str) -> Result<Vec<String>, String> {
    let root = Path::new(dir);
    if !root.is_dir() {
        return Err(format!("not a directory: {dir}"));
    }
    let mut paths = Vec::new();
    collect_arazzo_files(root, &mut paths).map_err(|err| format!("scanning {dir}: {err}"))?;
    paths.sort();
    Ok(paths)
}

fn collect_arazzo_files(dir: &Path, out: &mut Vec<String>) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_arazzo_files(&entry.path(), out)?;
        } else if ft.is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".arazzo.json")
                || name.ends_with(".arazzo.yaml")
                || name.ends_with(".arazzo.yml")
            {
                if let Some(path) = entry.path().to_str() {
                    out.push(path.to_string());
                }
            }
        }
    }
    Ok(())
}
