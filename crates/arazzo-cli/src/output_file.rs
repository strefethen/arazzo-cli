use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use arazzo_runtime::Blob;
use serde_json::Value;

static TEMP_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone)]
struct Target {
    name: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct OutputFiles {
    targets: Vec<Target>,
}

impl OutputFiles {
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

pub fn prepare(flags: &[String], declared: Option<&[String]>) -> Result<OutputFiles, String> {
    let mut targets: Vec<Target> = Vec::new();
    let mut resolved: Vec<PathBuf> = Vec::new();
    for flag in flags {
        let (name, raw_path) = parse_flag(flag)?;
        if let Some(declared) = declared {
            if !declared.iter().any(|output| output == name) {
                return Err(format!(
                    "--output-file names \"{name}\", which is not a declared output of the workflow"
                ));
            }
        }
        if targets.iter().any(|target| target.name == name) {
            return Err(format!(
                "--output-file names output \"{name}\" more than once"
            ));
        }
        let path = PathBuf::from(raw_path);
        let location = resolve(&path)?;
        probe_writable(&path)?;
        if resolved.contains(&location) {
            return Err(format!(
                "--output-file gives more than one output the target {}",
                path.display()
            ));
        }
        resolved.push(location);
        targets.push(Target {
            name: name.to_owned(),
            path,
        });
    }
    Ok(OutputFiles { targets })
}

pub fn write(
    files: &OutputFiles,
    values: &BTreeMap<String, Value>,
    blobs: &BTreeMap<String, Blob>,
) -> Result<(), String> {
    let mut chosen: Vec<(&Target, &Blob)> = Vec::new();
    for target in &files.targets {
        match blobs.get(&target.name) {
            Some(blob) => chosen.push((target, blob)),
            None if values.contains_key(&target.name) => {
                return Err(format!(
                    "--output-file output \"{}\" is not bytes; nothing was written",
                    target.name
                ));
            }
            None => {
                return Err(format!(
                    "--output-file output \"{}\" is absent from the run's outputs; nothing was written",
                    target.name
                ));
            }
        }
    }
    let mut staged: Vec<(PathBuf, &Target)> = Vec::new();
    for (target, blob) in chosen {
        match stage(&target.path, blob.bytes()) {
            Ok(temp) => staged.push((temp, target)),
            Err(err) => {
                discard(&staged);
                return Err(err);
            }
        }
    }
    for (index, (temp, target)) in staged.iter().enumerate() {
        if let Err(err) = fs::rename(temp, &target.path) {
            discard(&staged[index..]);
            return Err(format!(
                "replacing {} for output \"{}\": {err}",
                target.path.display(),
                target.name
            ));
        }
    }
    Ok(())
}

fn parse_flag(flag: &str) -> Result<(&str, &str), String> {
    let Some((name, path)) = flag.split_once('=') else {
        return Err(format!(
            "invalid --output-file \"{flag}\" (expected name=path)"
        ));
    };
    if name.is_empty() || path.is_empty() {
        return Err(format!(
            "invalid --output-file \"{flag}\" (expected name=path)"
        ));
    }
    Ok((name, path))
}

fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn resolve(path: &Path) -> Result<PathBuf, String> {
    let parent = parent_of(path);
    let canonical = fs::canonicalize(parent).map_err(|err| {
        format!(
            "--output-file target directory {} is unusable: {err}",
            parent.display()
        )
    })?;
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("--output-file target {} must name a file", path.display()))?;
    Ok(canonical.join(file_name))
}

fn probe_writable(path: &Path) -> Result<(), String> {
    let parent = parent_of(path);
    if !parent.is_dir() {
        return Err(format!(
            "--output-file target directory {} is not a directory",
            parent.display()
        ));
    }
    if path.is_dir() {
        return Err(format!(
            "--output-file target {} is a directory",
            path.display()
        ));
    }
    let temp = stage(path, &[]).map_err(|err| {
        format!(
            "--output-file target directory {} is not writable: {err}",
            parent.display()
        )
    })?;
    fs::remove_file(&temp).map_err(|err| format!("removing probe file {}: {err}", temp.display()))
}

fn stage(path: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} must name a file", path.display()))?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent_of(path).join(format!(
        ".{}.tmp-{}-{stamp}-{sequence}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|err| format!("creating {}: {err}", temp.display()))
        .and_then(|mut file| {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|err| format!("writing {}: {err}", temp.display()))
        });
    match written {
        Ok(()) => Ok(temp),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

fn discard(staged: &[(PathBuf, &Target)]) {
    for (temp, _) in staged {
        let _ = fs::remove_file(temp);
    }
}
