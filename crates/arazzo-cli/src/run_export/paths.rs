//! Resolve export destinations without letting a later trace write create an alias.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn validate_destination(
    export: &Path,
    trace: Option<&Path>,
    sources: &[&Path],
) -> Result<(), String> {
    if export.as_os_str().is_empty() {
        return Err("--export-run path must not be empty".to_string());
    }
    if let Some(trace) = trace {
        if paths_alias(export, trace)? {
            return Err("--export-run and --trace must name distinct files".to_string());
        }
    }
    for source in sources {
        if paths_alias(export, source)? {
            return Err(format!(
                "--export-run must not overwrite input file {}",
                source.display()
            ));
        }
    }
    Ok(())
}

fn paths_alias(a: &Path, b: &Path) -> Result<bool, String> {
    let a_resolved = resolved_path(a)?;
    let b_resolved = resolved_path(b)?;
    if a_resolved == b_resolved {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (fs::metadata(a), fs::metadata(b)) {
            if a.dev() == b.dev() && a.ino() == b.ino() {
                return Ok(true);
            }
        }
    }

    let a_anchor = nearest_existing_directory(&a_resolved)?;
    let b_anchor = nearest_existing_directory(&b_resolved)?;
    if a_anchor != b_anchor {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let a_metadata = fs::metadata(&a_anchor)
                .map_err(|err| format!("checking directory {}: {err}", a_anchor.display()))?;
            let b_metadata = fs::metadata(&b_anchor)
                .map_err(|err| format!("checking directory {}: {err}", b_anchor.display()))?;
            if a_metadata.dev() != b_metadata.dev() || a_metadata.ino() != b_metadata.ino() {
                return Ok(false);
            }
        }
        #[cfg(not(unix))]
        return Ok(false);
    }
    let a_suffix = a_resolved
        .strip_prefix(&a_anchor)
        .map_err(|err| format!("resolving export path {}: {err}", a.display()))?;
    let b_suffix = b_resolved
        .strip_prefix(&b_anchor)
        .map_err(|err| format!("resolving comparison path {}: {err}", b.display()))?;
    probe_alias(&a_anchor, a_suffix, b_suffix)
}

fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    resolved_path_inner(path, 0)
}

fn resolved_path_inner(path: &Path, depth: usize) -> Result<PathBuf, String> {
    if depth > 8 {
        return Err(format!(
            "too many destination symlinks at {}",
            path.display()
        ));
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| format!("determining current directory: {err}"))?
            .join(path)
    };
    let mut resolved = PathBuf::new();
    let mut components = absolute.components();
    while let Some(component) = components.next() {
        match component {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                resolved.push(component.as_os_str());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            std::path::Component::Normal(name) => {
                resolved.push(name);
                if let Ok(canonical) = fs::canonicalize(&resolved) {
                    resolved = canonical;
                } else if let Ok(target) = fs::read_link(&resolved) {
                    let target = if target.is_absolute() {
                        target
                    } else {
                        resolved.parent().unwrap_or(Path::new(".")).join(target)
                    };
                    return resolved_path_inner(&target.join(components.as_path()), depth + 1);
                }
            }
        }
    }
    Ok(resolved)
}

fn nearest_existing_directory(path: &Path) -> Result<PathBuf, String> {
    let mut parent = path
        .parent()
        .ok_or_else(|| format!("destination has no parent: {}", path.display()))?;
    loop {
        match fs::metadata(parent) {
            Ok(metadata) if metadata.is_dir() => {
                return fs::canonicalize(parent)
                    .map_err(|err| format!("resolving directory {}: {err}", parent.display()));
            }
            Ok(_) => {
                return Err(format!(
                    "destination parent is not a directory: {}",
                    parent.display()
                ))
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                parent = parent.parent().ok_or_else(|| {
                    format!("no existing parent directory for {}", path.display())
                })?;
            }
            Err(err) => {
                return Err(format!("checking directory {}: {err}", parent.display()));
            }
        }
    }
}

// Probe inside an exclusively owned directory at the existing common anchor.
// It contains only one file, so a second spelling that resolves to a file
// must name that same file under this filesystem's case/normalization rules.
fn probe_alias(anchor: &Path, a_suffix: &Path, b_suffix: &Path) -> Result<bool, String> {
    let probe = create_probe_directory(anchor)?;
    let result = (|| {
        let first = probe.join(a_suffix);
        let parent = first
            .parent()
            .ok_or_else(|| format!("probe file has no parent: {}", first.display()))?;
        fs::create_dir_all(parent)
            .map_err(|err| format!("creating probe directory {}: {err}", parent.display()))?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&first)
            .map_err(|err| format!("creating probe file {}: {err}", first.display()))?;
        drop(file);
        let second = probe.join(b_suffix);
        match fs::metadata(&second) {
            Ok(metadata) => Ok(metadata.is_file()),
            Err(err) if matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                Ok(false)
            }
            Err(err) => Err(format!("checking probe file {}: {err}", second.display())),
        }
    })();
    let cleanup = fs::remove_dir_all(&probe)
        .map_err(|err| format!("removing probe directory {}: {err}", probe.display()));
    match (result, cleanup) {
        (Ok(aliases), Ok(())) => Ok(aliases),
        (Err(err), Ok(())) | (Ok(_), Err(err)) => Err(err),
        (Err(check_err), Err(cleanup_err)) => Err(format!("{check_err}; {cleanup_err}")),
    }
}

fn create_probe_directory(anchor: &Path) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("determining probe time: {err}"))?
        .as_nanos();
    for _ in 0..16 {
        let sequence = PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let probe = anchor.join(format!(
            ".arazzo-run-export-probe-{}-{stamp}-{sequence}",
            std::process::id()
        ));
        let mut builder = DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&probe) {
            Ok(()) => return Ok(probe),
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(format!(
                    "creating probe directory {}: {err}",
                    probe.display()
                ));
            }
        }
    }
    Err(format!(
        "could not reserve unique probe directory in {}",
        anchor.display()
    ))
}
