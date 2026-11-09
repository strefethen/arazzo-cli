use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const DOCUMENT_BYTES: usize = 4 * 1024 * 1024;
const SOURCE_BYTES: usize = 16 * 1024 * 1024;
const SOURCE_ATTEMPTS: usize = 16;
const STARTUP_DENIED: &str = "Invalid server file access configuration";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileAccessError {
    Denied,
    Limit,
}

impl FileAccessError {
    fn code(self) -> &'static str {
        match self {
            Self::Denied => "MCP_FILE_DENIED",
            Self::Limit => "MCP_LIMIT_EXCEEDED",
        }
    }

    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Denied => "File access denied by server policy",
            Self::Limit => "Operation exceeded a server resource limit",
        }
    }

    pub(crate) fn tool_result(self) -> Value {
        error_tool_result(&json!({"code": self.code(), "message": self.message()}))
    }

    pub(crate) fn source_tool_result(self, source_name: &str) -> Value {
        error_tool_result(&json!({
            "code": self.code(),
            "message": self.message(),
            "sourceDescription": source_name,
        }))
    }
}

fn error_tool_result(payload: &Value) -> Value {
    json!({
        "content": [{"type": "text", "text": payload.to_string()}],
        "isError": true,
    })
}

#[derive(Default)]
pub(crate) struct SourceReadBudget {
    attempts: usize,
    bytes: usize,
}

pub(crate) type SharedSourceReadBudget = Arc<Mutex<SourceReadBudget>>;

impl SourceReadBudget {
    pub(crate) fn shared() -> SharedSourceReadBudget {
        Arc::new(Mutex::new(Self::default()))
    }

    fn reserve_attempt(&mut self) -> Result<(), FileAccessError> {
        let next = self.attempts.checked_add(1).ok_or(FileAccessError::Limit)?;
        if next > SOURCE_ATTEMPTS {
            return Err(FileAccessError::Limit);
        }
        self.attempts = next;
        Ok(())
    }

    fn allowance(&self) -> Result<usize, FileAccessError> {
        Ok(DOCUMENT_BYTES.min(
            SOURCE_BYTES
                .checked_sub(self.bytes)
                .ok_or(FileAccessError::Limit)?,
        ))
    }

    fn debit(&mut self, bytes: usize) -> Result<(), FileAccessError> {
        let next = self
            .bytes
            .checked_add(bytes)
            .ok_or(FileAccessError::Limit)?;
        if next > SOURCE_BYTES {
            return Err(FileAccessError::Limit);
        }
        self.bytes = next;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

pub(crate) struct AcquiredDocument {
    pub(crate) bytes: Vec<u8>,
    pub(crate) document_name: String,
    #[cfg_attr(
        not(all(test, target_os = "linux")),
        expect(
            dead_code,
            reason = "Acquisition provenance retained for the runtime loader adapter"
        )
    )]
    pub(crate) root_id: usize,
    #[cfg_attr(
        not(all(test, target_os = "linux")),
        expect(
            dead_code,
            reason = "Acquisition provenance retained for the runtime loader adapter"
        )
    )]
    pub(crate) file_identity: FileIdentity,
}

struct RootCapability {
    selectors: Vec<PathBuf>,
    canonical: PathBuf,
    #[cfg_attr(
        not(target_os = "linux"),
        expect(
            dead_code,
            reason = "Only the Linux backend opens beneath a pinned root"
        )
    )]
    handle: File,
    identity: FileIdentity,
}

struct Resolution<'a> {
    root_id: usize,
    root: &'a RootCapability,
    relative: PathBuf,
    document_name: String,
}

pub(crate) struct FileAccess {
    cwd: Option<PathBuf>,
    roots: Vec<RootCapability>,
    mode: &'static str,
}

impl FileAccess {
    pub(crate) fn deny_all() -> Self {
        Self {
            cwd: None,
            roots: Vec::new(),
            mode: "denied",
        }
    }

    pub(crate) fn configure(
        paths: &[String],
        directories: Option<Vec<String>>,
    ) -> Result<Self, String> {
        let cwd = std::env::current_dir().map_err(|_| STARTUP_DENIED.to_string())?;
        let (selectors, mode) = match directories {
            Some(directories) => (
                directories
                    .into_iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>(),
                "explicit",
            ),
            None => {
                let parents = paths
                    .iter()
                    .map(|path| startup_root(&cwd, path).ok_or_else(|| STARTUP_DENIED.to_string()))
                    .collect::<Result<Vec<_>, _>>()?;
                (parents, "startup-defaults")
            }
        };
        let mut roots: Vec<RootCapability> = Vec::new();
        for selector in selectors {
            let authored = without_current_dir(&cwd.join(selector));
            let selector =
                std::fs::canonicalize(&authored).map_err(|_| STARTUP_DENIED.to_string())?;
            if !std::fs::metadata(&selector)
                .map_err(|_| STARTUP_DENIED.to_string())?
                .is_dir()
            {
                return Err(STARTUP_DENIED.to_string());
            }
            let handle = open_root(&selector).map_err(|_| STARTUP_DENIED.to_string())?;
            let metadata = handle.metadata().map_err(|_| STARTUP_DENIED.to_string())?;
            if !metadata.is_dir() {
                return Err(STARTUP_DENIED.to_string());
            }
            let identity =
                root_identity(&handle, &metadata).map_err(|_| STARTUP_DENIED.to_string())?;
            let forms = selector_forms(authored, &selector);
            if let Some(root) = roots
                .iter_mut()
                .find(|root| root.identity == identity && identities_available())
            {
                for form in forms {
                    if !root.selectors.contains(&form) {
                        root.selectors.push(form);
                    }
                }
            } else if !roots.iter().any(|root| root.selectors.contains(&selector)) {
                roots.push(RootCapability {
                    selectors: forms,
                    canonical: selector,
                    handle,
                    identity,
                });
            }
        }
        Ok(Self {
            cwd: Some(cwd),
            mode: if roots.is_empty() { "denied" } else { mode },
            roots,
        })
    }

    /// Reports the policy mode and root count. Outside Linux, containment is a
    /// canonical-path check followed by an open, so it does not hold against
    /// another local process replacing a path between the two. Referenced
    /// AsyncAPI documents are authorized before the engine is built and then
    /// read by the runtime itself.
    pub(crate) fn notice(&self) -> String {
        format!(
            "arazzo-mcp: file policy mode={}, roots={}",
            self.mode,
            self.roots.len()
        )
    }

    pub(crate) fn selectors(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .flat_map(|root| root.selectors.iter().cloned())
            .collect()
    }

    pub(crate) fn acquire_shared(
        &self,
        path: &str,
        budget: &SharedSourceReadBudget,
    ) -> Result<AcquiredDocument, FileAccessError> {
        let mut budget = budget.lock().map_err(|_| FileAccessError::Denied)?;
        self.acquire_native(path, &mut budget)
    }

    pub(crate) fn acquire_native(
        &self,
        path: &str,
        budget: &mut SourceReadBudget,
    ) -> Result<AcquiredDocument, FileAccessError> {
        self.acquire_using(path, budget, acquire_from_root)
    }

    fn acquire_using<F>(
        &self,
        path: &str,
        budget: &mut SourceReadBudget,
        acquire: F,
    ) -> Result<AcquiredDocument, FileAccessError>
    where
        F: FnOnce(
            &RootCapability,
            &Path,
            &mut SourceReadBudget,
        ) -> Result<(Vec<u8>, FileIdentity), FileAccessError>,
    {
        budget.reserve_attempt()?;
        let resolution = self.resolve(path)?;
        let (bytes, file_identity) = acquire(resolution.root, &resolution.relative, budget)?;
        Ok(AcquiredDocument {
            bytes,
            root_id: resolution.root_id,
            file_identity,
            document_name: resolution.document_name,
        })
    }

    pub(crate) fn authorize(&self, path: &str) -> Result<(), FileAccessError> {
        let resolution = self.resolve(path)?;
        unaliased_target(resolution.root, &resolution.relative).map(drop)
    }

    fn resolve(&self, path: &str) -> Result<Resolution<'_>, FileAccessError> {
        if path.contains('\0')
            || Path::new(path)
                .components()
                .any(|component| match component {
                    Component::ParentDir => true,
                    Component::Prefix(prefix) => {
                        !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
                    }
                    _ => false,
                })
        {
            return Err(FileAccessError::Denied);
        }
        let cwd = self.cwd.as_ref().ok_or(FileAccessError::Denied)?;
        let absolute = without_current_dir(&cwd.join(path));
        let (root_id, root, selector) = self
            .roots
            .iter()
            .enumerate()
            .flat_map(|(index, root)| {
                root.selectors
                    .iter()
                    .map(move |selector| (index, root, selector))
            })
            .filter(|(_, _, selector)| absolute.starts_with(selector))
            .max_by_key(|(_, _, selector)| selector.components().count())
            .ok_or(FileAccessError::Denied)?;
        let relative = absolute
            .strip_prefix(selector)
            .map_err(|_| FileAccessError::Denied)?;
        if relative.components().any(|component| matches!(component, Component::Normal(name) if name.as_encoded_bytes().starts_with(b"."))) {
            return Err(FileAccessError::Denied);
        }
        let document_name = relative
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(FileAccessError::Denied)?
            .to_string();
        Ok(Resolution {
            root_id,
            root,
            relative: relative.to_path_buf(),
            document_name,
        })
    }
}

fn startup_root(cwd: &Path, path: &str) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?.parent()?.to_path_buf();
    let authored = cwd.join(path).parent()?.to_path_buf();
    let same_directory = std::fs::canonicalize(&authored).is_ok_and(|real| real == canonical);
    Some(if same_directory { authored } else { canonical })
}

fn without_current_dir(path: &Path) -> PathBuf {
    path.components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect()
}

fn selector_forms(authored: PathBuf, canonical: &Path) -> Vec<PathBuf> {
    let mut forms = Vec::new();
    for form in [
        Some(authored),
        plain_disk_form(canonical),
        Some(canonical.to_path_buf()),
    ]
    .into_iter()
    .flatten()
    {
        if !forms.contains(&form) {
            forms.push(form);
        }
    }
    forms
}

fn plain_disk_form(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return None;
    };
    let Prefix::VerbatimDisk(disk) = prefix.kind() else {
        return None;
    };
    let mut plain = PathBuf::from(format!("{}:", char::from(disk)));
    plain.extend(components);
    Some(plain)
}

fn unaliased_target(root: &RootCapability, relative: &Path) -> Result<PathBuf, FileAccessError> {
    let expected = root.canonical.join(relative);
    match std::fs::canonicalize(&expected) {
        Ok(canonical) if canonical == expected => Ok(canonical),
        _ => Err(FileAccessError::Denied),
    }
}

fn open_root(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0200_0000);
    }
    options.open(path)
}

fn identities_available() -> bool {
    cfg!(target_os = "linux")
}

fn root_identity(
    handle: &File,
    metadata: &std::fs::Metadata,
) -> Result<FileIdentity, FileAccessError> {
    #[cfg(target_os = "linux")]
    {
        let _ = metadata;
        let stat = rustix::fs::fstat(handle).map_err(|_| FileAccessError::Denied)?;
        Ok(FileIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (handle, metadata);
        Ok(UNPINNED_IDENTITY)
    }
}

fn read_bounded(
    reader: &mut impl Read,
    budget: &mut SourceReadBudget,
) -> Result<Vec<u8>, FileAccessError> {
    let allowance = budget.allowance()?;
    let mut bytes = Vec::new();
    let mut scratch = [0u8; 8192];
    loop {
        let remaining = allowance
            .checked_sub(bytes.len())
            .ok_or(FileAccessError::Limit)?;
        let chunk = remaining.clamp(1, scratch.len());
        let count = reader
            .read(&mut scratch[..chunk])
            .map_err(|_| FileAccessError::Denied)?;
        if count == 0 {
            return Ok(bytes);
        }
        if count > remaining {
            return Err(FileAccessError::Limit);
        }
        budget.debit(count)?;
        bytes.extend_from_slice(&scratch[..count]);
    }
}

#[cfg(target_os = "linux")]
fn open_contained(root: &File, relative: &Path) -> Result<File, FileAccessError> {
    open_contained_using(root, relative, |root, relative, flags, mode, resolve| {
        rustix::fs::openat2(root, relative, flags, mode, resolve)
    })
}

#[cfg(target_os = "linux")]
fn open_contained_using<O>(root: &File, relative: &Path, open: O) -> Result<File, FileAccessError>
where
    O: FnOnce(
        &File,
        &Path,
        rustix::fs::OFlags,
        rustix::fs::Mode,
        rustix::fs::ResolveFlags,
    ) -> rustix::io::Result<std::os::fd::OwnedFd>,
{
    use rustix::fs::{Mode, OFlags, ResolveFlags};
    let descriptor = open(
        root,
        relative,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|_| FileAccessError::Denied)?;
    inspect_descriptor(&descriptor)?;
    Ok(File::from(descriptor))
}

#[cfg(target_os = "linux")]
fn inspect_descriptor(
    handle: &impl std::os::fd::AsFd,
) -> Result<(FileIdentity, u64), FileAccessError> {
    use rustix::fs::{fstat, FileType};
    let stat = fstat(handle).map_err(|_| FileAccessError::Denied)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(FileAccessError::Denied);
    }
    let length = u64::try_from(stat.st_size).map_err(|_| FileAccessError::Denied)?;
    Ok((
        FileIdentity {
            device: stat.st_dev,
            inode: stat.st_ino,
        },
        length,
    ))
}

#[cfg(target_os = "linux")]
fn acquire_from_root(
    root: &RootCapability,
    relative: &Path,
    budget: &mut SourceReadBudget,
) -> Result<(Vec<u8>, FileIdentity), FileAccessError> {
    acquire_linux(
        &root.handle,
        relative,
        budget,
        open_contained,
        inspect_descriptor,
        read_bounded,
    )
}

#[cfg(target_os = "linux")]
fn acquire_linux<O, M, R>(
    root: &File,
    relative: &Path,
    budget: &mut SourceReadBudget,
    open: O,
    mut metadata: M,
    read: R,
) -> Result<(Vec<u8>, FileIdentity), FileAccessError>
where
    O: FnOnce(&File, &Path) -> Result<File, FileAccessError>,
    M: FnMut(&File) -> Result<(FileIdentity, u64), FileAccessError>,
    R: FnOnce(&mut File, &mut SourceReadBudget) -> Result<Vec<u8>, FileAccessError>,
{
    let mut file = open(root, relative)?;
    let (identity, length) = metadata(&file)?;
    let allowance = budget.allowance()? as u64;
    if length > allowance {
        return Err(FileAccessError::Limit);
    }
    let bytes = read(&mut file, budget)?;
    let (final_identity, final_length) = metadata(&file)?;
    if final_length > allowance {
        return Err(FileAccessError::Limit);
    }
    if final_identity != identity || final_length > bytes.len() as u64 {
        return Err(FileAccessError::Denied);
    }
    Ok((bytes, identity))
}

#[cfg(not(target_os = "linux"))]
const UNPINNED_IDENTITY: FileIdentity = FileIdentity {
    device: 0,
    inode: 0,
};

#[cfg(not(target_os = "linux"))]
fn acquire_from_root(
    root: &RootCapability,
    relative: &Path,
    budget: &mut SourceReadBudget,
) -> Result<(Vec<u8>, FileIdentity), FileAccessError> {
    let target = unaliased_target(root, relative)?;
    if !std::fs::metadata(&target)
        .map_err(|_| FileAccessError::Denied)?
        .is_file()
    {
        return Err(FileAccessError::Denied);
    }
    let mut file = File::open(&target).map_err(|_| FileAccessError::Denied)?;
    let length = single_link_regular_length(&file)?;
    let allowance = budget.allowance()? as u64;
    if length > allowance {
        return Err(FileAccessError::Limit);
    }
    let bytes = read_bounded(&mut file, budget)?;
    let final_length = single_link_regular_length(&file)?;
    if final_length > allowance {
        return Err(FileAccessError::Limit);
    }
    if final_length > bytes.len() as u64 {
        return Err(FileAccessError::Denied);
    }
    Ok((bytes, UNPINNED_IDENTITY))
}

#[cfg(not(target_os = "linux"))]
fn single_link_regular_length(file: &File) -> Result<u64, FileAccessError> {
    let metadata = file.metadata().map_err(|_| FileAccessError::Denied)?;
    if !metadata.is_file() || !has_single_link(&metadata) {
        return Err(FileAccessError::Denied);
    }
    Ok(metadata.len())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn has_single_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink() == 1
}

#[cfg(not(unix))]
fn has_single_link(_metadata: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "Fixture setup and assertions")]
mod tests {
    use super::*;
    use std::io::{self, Cursor};

    #[test]
    fn source_byte_and_attempt_boundaries_do_not_refund() {
        for length in [DOCUMENT_BYTES - 1, DOCUMENT_BYTES, DOCUMENT_BYTES + 1] {
            let mut budget = SourceReadBudget::default();
            let result = read_bounded(&mut io::repeat(0).take(length as u64), &mut budget);
            if length > DOCUMENT_BYTES {
                assert_eq!(result, Err(FileAccessError::Limit));
                assert_eq!(budget.bytes, DOCUMENT_BYTES);
            } else {
                assert_eq!(result.unwrap().len(), length);
                assert_eq!(budget.bytes, length);
            }
        }
        let mut budget = SourceReadBudget::default();
        for _ in 0..4 {
            assert_eq!(
                read_bounded(&mut io::repeat(1).take(DOCUMENT_BYTES as u64), &mut budget)
                    .unwrap()
                    .len(),
                DOCUMENT_BYTES
            );
        }
        assert_eq!(budget.bytes, SOURCE_BYTES);
        assert!(read_bounded(&mut Cursor::new([]), &mut budget)
            .unwrap()
            .is_empty());
        assert_eq!(
            read_bounded(&mut Cursor::new([1]), &mut budget),
            Err(FileAccessError::Limit)
        );
        for _ in 0..SOURCE_ATTEMPTS {
            assert_eq!(budget.reserve_attempt(), Ok(()));
        }
        assert_eq!(budget.reserve_attempt(), Err(FileAccessError::Limit));
        budget.bytes = usize::MAX;
        assert_eq!(budget.debit(1), Err(FileAccessError::Limit));
        assert_eq!(budget.allowance(), Err(FileAccessError::Limit));
        budget.attempts = usize::MAX;
        assert_eq!(budget.reserve_attempt(), Err(FileAccessError::Limit));
    }

    #[test]
    fn failed_reads_retain_charge_but_never_return_a_prefix() {
        struct FailsAfterChunk(bool);
        impl Read for FailsAfterChunk {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if self.0 {
                    return Err(io::Error::other("canary native error"));
                }
                self.0 = true;
                buffer[0] = 42;
                Ok(1)
            }
        }
        let mut budget = SourceReadBudget::default();
        assert_eq!(
            read_bounded(&mut FailsAfterChunk(false), &mut budget),
            Err(FileAccessError::Denied)
        );
        assert_eq!(budget.bytes, 1);
        let mut shrinking = Cursor::new([1, 2, 3]);
        assert_eq!(
            read_bounded(&mut shrinking, &mut budget).unwrap(),
            [1, 2, 3]
        );
    }

    #[test]
    fn poisoned_shared_budget_denies() {
        let budget = SourceReadBudget::shared();
        let _ = std::panic::catch_unwind(|| {
            let _guard = budget.lock().unwrap();
            panic!("poison");
        });
        assert!(matches!(
            FileAccess::deny_all().acquire_shared("canary", &budget),
            Err(FileAccessError::Denied)
        ));
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;
        use std::fs;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Fixture(PathBuf);
        impl Fixture {
            fn new() -> Self {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                let root = std::env::temp_dir().join(format!(
                    "arazzo-acquire-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::create_dir_all(&root).unwrap();
                Self(root)
            }
            fn access(&self) -> FileAccess {
                FileAccess::configure(&[], Some(vec![self.0.to_str().unwrap().to_string()]))
                    .unwrap()
            }
        }

        #[test]
        fn kernel_acquisition_survives_root_and_leaf_replacement() {
            let fixture = Fixture::new();
            fs::write(fixture.0.join("api"), b"authorized").unwrap();
            let access = fixture.access();
            let moved = fixture.0.with_extension("moved");
            fs::rename(&fixture.0, &moved).unwrap();
            fs::create_dir(&fixture.0).unwrap();
            fs::write(fixture.0.join("api"), b"CANARY replacement").unwrap();
            let acquired = access
                .acquire_native(
                    fixture.0.join("api").to_str().unwrap(),
                    &mut SourceReadBudget::default(),
                )
                .unwrap();
            assert_eq!(acquired.bytes, b"authorized");
            assert_eq!(acquired.root_id, 0);
            assert_eq!(
                acquired.file_identity,
                inspect_descriptor(&File::open(moved.join("api")).unwrap())
                    .unwrap()
                    .0
            );
            let root = &access.roots[0].handle;
            let result = acquire_linux(
                root,
                Path::new("api"),
                &mut SourceReadBudget::default(),
                |root, relative| {
                    let file = open_contained(root, relative)?;
                    fs::rename(moved.join("api"), moved.join("old")).unwrap();
                    fs::write(moved.join("api"), b"CANARY swapped leaf").unwrap();
                    Ok(file)
                },
                inspect_descriptor,
                read_bounded,
            )
            .unwrap();
            assert_eq!(result.0, b"authorized");
        }

        #[test]
        fn moved_out_ancestor_new_child_is_never_authorized() {
            let fixture = Fixture::new();
            fs::create_dir(fixture.0.join("sub")).unwrap();
            let access = fixture.access();
            let outside = fixture.0.with_extension("outside");
            let result = acquire_linux(
                &access.roots[0].handle,
                Path::new("sub/new"),
                &mut SourceReadBudget::default(),
                |root, relative| {
                    fs::rename(fixture.0.join("sub"), &outside).unwrap();
                    fs::write(outside.join("new"), b"CANARY new outside child").unwrap();
                    open_contained(root, relative)
                },
                inspect_descriptor,
                read_bounded,
            );
            assert_eq!(result, Err(FileAccessError::Denied));
        }

        #[test]
        fn syscall_and_metadata_failures_never_use_an_alternate_opener() {
            let fixture = Fixture::new();
            fs::write(fixture.0.join("api"), b"authorized").unwrap();
            let access = fixture.access();
            for errno in [
                rustix::io::Errno::NOSYS,
                rustix::io::Errno::PERM,
                rustix::io::Errno::INVAL,
                rustix::io::Errno::AGAIN,
            ] {
                let mut calls = 0;
                let result = acquire_linux(
                    &access.roots[0].handle,
                    Path::new("api"),
                    &mut SourceReadBudget::default(),
                    |root, relative| {
                        open_contained_using(root, relative, |_, _, flags, mode, resolve| {
                            calls += 1;
                            assert_eq!(
                                flags,
                                rustix::fs::OFlags::RDONLY
                                    | rustix::fs::OFlags::CLOEXEC
                                    | rustix::fs::OFlags::NOFOLLOW
                                    | rustix::fs::OFlags::NONBLOCK
                            );
                            assert_eq!(mode, rustix::fs::Mode::empty());
                            assert_eq!(
                                resolve,
                                rustix::fs::ResolveFlags::BENEATH
                                    | rustix::fs::ResolveFlags::NO_SYMLINKS
                                    | rustix::fs::ResolveFlags::NO_MAGICLINKS
                            );
                            Err(errno)
                        })
                    },
                    |_| panic!("metadata after failed syscall"),
                    |_, _| panic!("read after failed syscall"),
                );
                assert_eq!(result, Err(FileAccessError::Denied));
                assert_eq!(calls, 1);
            }
            for failure_at in [1, 2] {
                let mut calls = 0;
                let result = acquire_linux(
                    &access.roots[0].handle,
                    Path::new("api"),
                    &mut SourceReadBudget::default(),
                    open_contained,
                    |file| {
                        calls += 1;
                        if calls == failure_at {
                            Err(FileAccessError::Denied)
                        } else {
                            inspect_descriptor(file)
                        }
                    },
                    read_bounded,
                );
                assert_eq!(result, Err(FileAccessError::Denied));
            }
        }

        #[test]
        fn link_changes_and_growth_at_check_boundaries_deny() {
            let fixture = Fixture::new();
            let leaf = fixture.0.join("api");
            fs::write(&leaf, b"a").unwrap();
            let access = fixture.access();
            let result = acquire_linux(
                &access.roots[0].handle,
                Path::new("api"),
                &mut SourceReadBudget::default(),
                open_contained,
                inspect_descriptor,
                |file, budget| {
                    fs::hard_link(&leaf, fixture.0.join("alias")).unwrap();
                    read_bounded(file, budget)
                },
            );
            assert_eq!(result, Err(FileAccessError::Denied));
            fs::write(fixture.0.join("growing"), b"a").unwrap();
            let mut budget = SourceReadBudget::default();
            let result = acquire_linux(
                &access.roots[0].handle,
                Path::new("growing"),
                &mut budget,
                open_contained,
                inspect_descriptor,
                |file, budget| {
                    let writer = fs::OpenOptions::new()
                        .write(true)
                        .open(fixture.0.join("growing"))
                        .unwrap();
                    writer.set_len((DOCUMENT_BYTES + 1) as u64).unwrap();
                    read_bounded(file, budget)
                },
            );
            assert_eq!(result, Err(FileAccessError::Limit));
            assert_eq!(budget.bytes, DOCUMENT_BYTES);
            fs::write(fixture.0.join("late-growth"), b"a").unwrap();
            let result = acquire_linux(
                &access.roots[0].handle,
                Path::new("late-growth"),
                &mut SourceReadBudget::default(),
                open_contained,
                inspect_descriptor,
                |file, budget| {
                    let bytes = read_bounded(file, budget)?;
                    fs::OpenOptions::new()
                        .write(true)
                        .open(fixture.0.join("late-growth"))
                        .unwrap()
                        .set_len((DOCUMENT_BYTES + 1) as u64)
                        .unwrap();
                    Ok(bytes)
                },
            );
            assert_eq!(result, Err(FileAccessError::Limit));
            fs::write(fixture.0.join("late-alias"), b"a").unwrap();
            let result = acquire_linux(
                &access.roots[0].handle,
                Path::new("late-alias"),
                &mut SourceReadBudget::default(),
                |root, relative| {
                    let file = open_contained(root, relative)?;
                    fs::hard_link(
                        fixture.0.join("late-alias"),
                        fixture.0.join("alias-before-stat"),
                    )
                    .unwrap();
                    Ok(file)
                },
                inspect_descriptor,
                |_, _| panic!("read after initial metadata denial"),
            );
            assert_eq!(result, Err(FileAccessError::Denied));
            fs::write(fixture.0.join("shrinking"), b"abc").unwrap();
            let result = acquire_linux(
                &access.roots[0].handle,
                Path::new("shrinking"),
                &mut SourceReadBudget::default(),
                open_contained,
                inspect_descriptor,
                |file, budget| {
                    fs::OpenOptions::new()
                        .write(true)
                        .open(fixture.0.join("shrinking"))
                        .unwrap()
                        .set_len(0)
                        .unwrap();
                    read_bounded(file, budget)
                },
            )
            .unwrap();
            assert!(result.0.is_empty());
        }

        #[test]
        fn repeated_acquisitions_share_attempts_and_source_bytes() {
            let fixture = Fixture::new();
            fs::write(fixture.0.join("api"), vec![0; DOCUMENT_BYTES]).unwrap();
            let access = fixture.access();
            let path = fixture.0.join("api");
            let path = path.to_str().unwrap();
            let mut budget = SourceReadBudget::default();
            for _ in 0..4 {
                assert_eq!(
                    access
                        .acquire_native(path, &mut budget)
                        .unwrap()
                        .bytes
                        .len(),
                    DOCUMENT_BYTES
                );
            }
            assert!(matches!(
                access.acquire_native(path, &mut budget),
                Err(FileAccessError::Limit)
            ));
            assert_eq!(budget.bytes, SOURCE_BYTES);
            let mut budget = SourceReadBudget::default();
            for _ in 0..16 {
                assert!(matches!(
                    access.acquire_native("../canary", &mut budget),
                    Err(FileAccessError::Denied)
                ));
            }
            assert!(matches!(
                access.acquire_native(path, &mut budget),
                Err(FileAccessError::Limit)
            ));
        }

        #[test]
        fn longest_root_wins_and_lookup_replacements_use_the_kernel() {
            use std::os::unix::fs::symlink;
            let fixture = Fixture::new();
            fs::create_dir_all(fixture.0.join("sub/nested")).unwrap();
            fs::write(fixture.0.join("sub/nested/api"), b"authorized").unwrap();
            let access = FileAccess::configure(
                &[],
                Some(vec![
                    fixture.0.to_str().unwrap().into(),
                    fixture.0.join("sub").to_str().unwrap().into(),
                ]),
            )
            .unwrap();
            let doc = access
                .acquire_native(
                    fixture.0.join("sub/nested/api").to_str().unwrap(),
                    &mut SourceReadBudget::default(),
                )
                .unwrap();
            assert_eq!(doc.root_id, 1);
            let result = access.acquire_using(
                fixture.0.join("sub/nested/api").to_str().unwrap(),
                &mut SourceReadBudget::default(),
                |root, relative, budget| {
                    let moved = fixture.0.join("sub/old-nested");
                    fs::rename(fixture.0.join("sub/nested"), &moved).unwrap();
                    symlink(&moved, fixture.0.join("sub/nested")).unwrap();
                    acquire_from_root(root, relative, budget)
                },
            );
            assert!(matches!(result, Err(FileAccessError::Denied)));
        }

        #[test]
        fn trusted_root_aliases_coalesce_and_remain_pinned() {
            use std::os::unix::fs::symlink;
            let fixture = Fixture::new();
            fs::write(fixture.0.join("api"), b"authorized").unwrap();
            let alias = fixture.0.with_extension("root-alias");
            symlink(&fixture.0, &alias).unwrap();
            let access = FileAccess::configure(
                &[],
                Some(vec![
                    alias.to_str().unwrap().into(),
                    fixture.0.to_str().unwrap().into(),
                ]),
            )
            .unwrap();
            assert_eq!(access.roots.len(), 1);
            let replacement = fixture.0.with_extension("replacement");
            fs::create_dir(&replacement).unwrap();
            fs::write(replacement.join("api"), b"CANARY replacement").unwrap();
            fs::rename(&alias, fixture.0.with_extension("old-alias")).unwrap();
            symlink(replacement, &alias).unwrap();
            let doc = access
                .acquire_native(
                    alias.join("api").to_str().unwrap(),
                    &mut SourceReadBudget::default(),
                )
                .unwrap();
            assert_eq!(doc.bytes, b"authorized");
        }
    }
}
