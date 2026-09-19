//! Bounded, source-free persistence for checked core sessions.
//!
//! A store only persists an already captured context. Loading never opens the
//! source or evaluates a runtime; callers must provide the freshly captured
//! snapshot to prove that a saved session is still current.
use crate::{
    Error, Result, checked_session::CheckedSession, identity::sha256,
    reasoning_context::CapturedAssessment, reasoning_snapshot::Snapshot, require,
    tokenizer::Encoding, value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

#[cfg(unix)]
use rustix::fs::{AtFlags, Mode, OFlags};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

const MAX_FILE: u64 = 64 * 1024 * 1024;
const VERSION: u64 = 1;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct CheckedSessionStore {
    root: PathBuf,
    root_dir: Arc<File>,
    #[cfg(unix)]
    root_identity: (u64, u64),
    #[cfg(windows)]
    root_identity: Arc<same_file::Handle>,
    identity: V,
    profile: Option<V>,
    encoding: Encoding,
}

impl CheckedSessionStore {
    /// Open (or create) a project-bound state directory and claim its identity.
    pub fn open(
        root: impl AsRef<Path>,
        project: &str,
        input: impl AsRef<Path>,
        navigation_profile: Option<V>,
        encoding: Encoding,
    ) -> Result<Self> {
        require(valid_project(project), "invalid project name")?;
        let root = root.as_ref();
        require(root.is_absolute(), "state root must be absolute")?;
        #[cfg(windows)]
        let root = windows_state_root(root)?;
        #[cfg(not(windows))]
        let root = root.to_path_buf();
        create_state_dir(&root)?;
        let root = root
            .canonicalize()
            .map_err(|e| Error(format!("state root: {e}")))?;
        let root_dir = Arc::new(open_directory(&root)?);
        #[cfg(unix)]
        let root_identity = file_identity(&root_dir)?;
        #[cfg(windows)]
        let root_identity = Arc::new(
            same_file::Handle::from_file(root_dir.try_clone()?)
                .map_err(|e| Error(format!("state root: {e}")))?,
        );
        let input = crate::project_modes::resolved(input.as_ref())?;
        let identity = V::Map(std::collections::BTreeMap::from([
            ("project".into(), V::Text(project.into())),
            (
                "input_path".into(),
                V::Text(input.to_string_lossy().into_owned()),
            ),
        ]));
        let store = Self {
            root,
            root_dir,
            #[cfg(unix)]
            root_identity,
            #[cfg(windows)]
            root_identity,
            identity,
            profile: navigation_profile,
            encoding,
        };
        store.claim_identity()?;
        store.validate_root()?;
        Ok(store)
    }

    pub fn revision(&self, context: &CapturedAssessment) -> Result<String> {
        context.session_revision(&self.session_identity()?)
    }
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Persist a complete context, rejecting a changed freshness snapshot.
    pub fn save(&self, context: &CapturedAssessment, freshness: &Snapshot) -> Result<String> {
        require(
            freshness.snapshot_id() == context.snapshot_id(),
            "project record or history changed; reopen",
        )?;
        self.validate_root()?;
        let revision = self.revision(context)?;
        let base_identity = self.base_session_identity()?;
        let session_identity = self.session_identity()?;
        let session = CheckedSession::new(context.clone(), base_identity, self.profile.as_ref())?;
        require(
            session.revision() == revision,
            "checked session revision mismatch",
        )?;
        let payload = json!({"version": VERSION, "project_identity": ordinary(&session_identity)?,
            "revision": revision, "context": serde_json::from_str::<J>(&context.to_json()?)?});
        let name = context_name(&revision)?;
        let held = self.atomic_create(&name, &payload)?;
        require(held == payload, "core context identity collision")?;
        self.validate_root()?;
        Ok(revision)
    }

    /// Load a retained context using only the caller's already captured snapshot.
    pub fn load(&self, revision: &str, freshness: &Snapshot) -> Result<CheckedSession> {
        let name = context_name(revision)?;
        self.validate_root()?;
        let session_identity = self.session_identity()?;
        let payload = self.read_json(&name)?;
        let map = payload
            .as_object()
            .ok_or_else(|| Error("invalid retained core context".into()))?;
        require(
            map.len() == 4 && map.get("version") == Some(&json!(VERSION)),
            "invalid retained core context",
        )?;
        require(
            map.get("revision") == Some(&json!(revision)),
            "core context identity mismatch",
        )?;
        require(
            map.get("project_identity") == Some(&ordinary(&session_identity)?),
            "core context identity mismatch",
        )?;
        let context = CapturedAssessment::from_data(
            map.get("context")
                .ok_or_else(|| Error("invalid retained core context".into()))?,
        )?;
        require(
            context.snapshot_id() == freshness.snapshot_id(),
            "project record or history changed; reopen",
        )?;
        require(
            context.session_revision(&session_identity)? == revision,
            "core context revision mismatch",
        )?;
        let result = CheckedSession::new(
            context,
            self.base_session_identity()?,
            self.profile.as_ref(),
        )?;
        self.validate_root()?;
        Ok(result)
    }

    pub fn context_path(&self, revision: &str) -> Result<PathBuf> {
        let name = context_name(revision)?;
        self.validate_root()?;
        Ok(self.root.join(name))
    }

    fn base_session_identity(&self) -> Result<V> {
        let identity = match &self.identity {
            V::Map(m) => m,
            _ => return Err(Error("invalid project identity".into())),
        };
        Ok(V::Map(std::collections::BTreeMap::from([
            ("version".into(), V::from_json(&json!(VERSION))?),
            (
                "project".into(),
                identity
                    .get("project")
                    .cloned()
                    .ok_or_else(|| Error("invalid project identity".into()))?,
            ),
            (
                "input_path".into(),
                identity
                    .get("input_path")
                    .cloned()
                    .ok_or_else(|| Error("invalid project identity".into()))?,
            ),
        ])))
    }

    fn session_identity(&self) -> Result<V> {
        let mut identity = self.base_session_identity()?;
        let V::Map(ref mut map) = identity else {
            unreachable!()
        };
        map.insert(
            "navigation_profile_sha256".into(),
            self.profile
                .as_ref()
                .map(profile_digest)
                .transpose()?
                .map_or(V::Null, V::Text),
        );
        Ok(identity)
    }

    fn claim_identity(&self) -> Result<()> {
        let expected = ordinary(&self.identity)?;
        let held = match self.read_json_optional("project.json")? {
            Some(value) => value,
            None => self.atomic_create("project.json", &expected)?,
        };
        require(
            held == expected,
            "state directory belongs to another project/input",
        )
    }

    fn validate_root(&self) -> Result<()> {
        let current = open_directory(&self.root).map_err(|_| Error("state root changed".into()))?;
        #[cfg(unix)]
        require(
            file_identity(&current)? == self.root_identity,
            "state root changed",
        )?;
        #[cfg(windows)]
        require(
            same_file::Handle::from_file(current)
                .map_err(|e| Error(format!("state root changed: {e}")))?
                == *self.root_identity,
            "state root changed",
        )?;
        #[cfg(not(any(unix, windows)))]
        let _ = current;
        let expected = ordinary(&self.identity)?;
        require(
            self.read_json_optional("project.json")? == Some(expected),
            "state directory belongs to another project/input",
        )
    }

    fn read_json_optional(&self, name: &str) -> Result<Option<J>> {
        match open_bounded_at(&self.root_dir, &self.root, name) {
            Ok(file) => read_json_file(file).map(Some),
            Err(e) if e.0 == "state file missing" => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn read_json(&self, name: &str) -> Result<J> {
        read_json_file(open_bounded_at(&self.root_dir, &self.root, name)?)
    }

    fn atomic_create(&self, name: &str, value: &J) -> Result<J> {
        let bytes = serde_json::to_vec(value)?;
        require(bytes.len() as u64 <= MAX_FILE, "state value exceeds limit")?;
        #[cfg(unix)]
        {
            let mut last_error = None;
            for _ in 0..32 {
                let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let temporary = format!(".checked-session-{}-{sequence}.tmp", std::process::id());
                let fd = match rustix::fs::openat(
                    &*self.root_dir,
                    temporary.as_str(),
                    OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                    Mode::from_raw_mode(0o600),
                ) {
                    Ok(fd) => fd,
                    Err(rustix::io::Errno::EXIST) => continue,
                    Err(e) => return Err(Error(format!("temporary state: {e}"))),
                };
                let mut cleanup = TempAt {
                    dir: &self.root_dir,
                    name: temporary.clone(),
                    published: false,
                };
                let mut file = File::from(fd);
                if let Err(e) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
                    return Err(Error(format!("temporary state: {e}")));
                }
                drop(file);
                match rustix::fs::renameat_with(
                    &*self.root_dir,
                    temporary.as_str(),
                    &*self.root_dir,
                    name,
                    rustix::fs::RenameFlags::NOREPLACE,
                ) {
                    Ok(()) => {
                        cleanup.published = true;
                        self.root_dir
                            .sync_all()
                            .map_err(|e| Error(format!("publish state: {e}")))?;
                    }
                    Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => {
                        last_error = Some(Error(format!("publish state: {e}")));
                        continue;
                    }
                }
                return self.read_json(name);
            }
            Err(last_error.unwrap_or_else(|| Error("temporary state name collision".into())))
        }
        #[cfg(not(unix))]
        {
            let path = self.root.join(name);
            let mut file = tempfile::NamedTempFile::new_in(&self.root)
                .map_err(|e| Error(format!("temporary state: {e}")))?;
            file.write_all(&bytes)
                .and_then(|_| file.as_file().sync_all())
                .map_err(|e| Error(format!("temporary state: {e}")))?;
            match file.persist_noclobber(&path) {
                Ok(_) => {}
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(Error(format!("publish state: {}", e.error))),
            }
            self.read_json(name)
        }
    }
}

#[cfg(unix)]
struct TempAt<'a> {
    dir: &'a File,
    name: String,
    published: bool,
}
#[cfg(unix)]
impl Drop for TempAt<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = rustix::fs::unlinkat(self.dir, self.name.as_str(), AtFlags::empty());
        }
    }
}

fn valid_project(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
        && s.as_bytes()[0].is_ascii_alphanumeric()
}
fn valid_revision(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
fn context_name(revision: &str) -> Result<String> {
    require(valid_revision(revision), "invalid core session revision")?;
    Ok(format!("core-context-{revision}.json"))
}
fn ordinary(v: &V) -> Result<J> {
    v.to_json().map_err(|_| Error("invalid identity".into()))
}
fn profile_digest(v: &V) -> Result<String> {
    Ok(sha256(serde_json::to_vec(&ordinary(v)?)?.as_slice()))
}

fn create_state_dir(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        return require(
            meta.file_type().is_dir() && !meta.file_type().is_symlink(),
            "state root is not a directory",
        );
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error(format!("state directory: {e}")))?;
    }
    match fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|e| Error(format!("state directory: {e}")))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta =
                fs::symlink_metadata(path).map_err(|e| Error(format!("state directory: {e}")))?;
            require(
                meta.file_type().is_dir() && !meta.file_type().is_symlink(),
                "state root is not a directory",
            )
        }
        Err(e) => Err(Error(format!("state directory: {e}"))),
    }
}

fn open_directory(path: &Path) -> Result<File> {
    #[cfg(unix)]
    {
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| Error(format!("state root: {e}")))?;
        Ok(File::from(fd))
    }
    #[cfg(windows)]
    {
        require(
            windows_local_path(path),
            "network state root is unsupported",
        )?;
        let file = fs::OpenOptions::new()
            .access_mode(0x8000_0000) // GENERIC_READ
            .share_mode(0x0000_0001 | 0x0000_0002) // deny FILE_SHARE_DELETE
            .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
            .open(path)
            .map_err(|e| Error(format!("state root: {e}")))?;
        require(
            file.metadata().map_err(|e| Error(e.to_string()))?.is_dir(),
            "state root is not a directory",
        )?;
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let file = File::open(path).map_err(|e| Error(format!("state root: {e}")))?;
        require(
            file.metadata().map_err(|e| Error(e.to_string()))?.is_dir(),
            "state root is not a directory",
        )?;
        Ok(file)
    }
}

#[cfg(unix)]
fn file_identity(file: &File) -> Result<(u64, u64)> {
    let metadata = file
        .metadata()
        .map_err(|e| Error(format!("state root: {e}")))?;
    require(metadata.is_dir(), "state root is not a directory")?;
    Ok((metadata.dev(), metadata.ino()))
}

fn open_bounded_at(root: &File, _root_path: &Path, name: &str) -> Result<File> {
    #[cfg(unix)]
    {
        let fd = rustix::fs::openat(
            root,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| {
            if e == rustix::io::Errno::NOENT {
                Error("state file missing".into())
            } else {
                Error(format!("retained context: {e}"))
            }
        })?;
        Ok(File::from(fd))
    }
    #[cfg(windows)]
    {
        let _ = root;
        let root_path = _root_path;
        require(
            Path::new(name).components().count() == 1
                && matches!(
                    Path::new(name).components().next(),
                    Some(std::path::Component::Normal(_))
                ),
            "invalid retained context name",
        )?;
        let path = root_path.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error("state file missing".into())
            } else {
                Error(format!("retained context: {e}"))
            }
        })?;
        require(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "retained context is not a regular file",
        )?;
        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(0x0000_0001 | 0x0000_0002) // deny FILE_SHARE_DELETE
            .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
            .open(&path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error("state file missing".into())
                } else {
                    Error(format!("retained context: {e}"))
                }
            })?;
        require(
            file.metadata()
                .map_err(|e| Error(format!("retained context: {e}")))?
                .is_file(),
            "retained context is not a regular file",
        )?;
        let opened = same_file::Handle::from_file(file.try_clone()?)
            .map_err(|e| Error(format!("retained context: {e}")))?;
        let named = same_file::Handle::from_path(&path)
            .map_err(|e| Error(format!("retained context: {e}")))?;
        require(opened == named, "retained context path changed")?;
        Ok(file)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = root;
        File::open(Path::new(name)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error("state file missing".into())
            } else {
                Error(format!("retained context: {e}"))
            }
        })
    }
}

#[cfg(windows)]
fn windows_local_path(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    !matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::DeviceNS(_) | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)
            )
    )
}

#[cfg(windows)]
fn windows_state_root(path: &Path) -> Result<PathBuf> {
    require(
        windows_local_path(path),
        "network state root is unsupported",
    )?;
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    let mut ancestor = path;
    let mut tail = Vec::new();
    while !ancestor.exists() {
        tail.push(
            ancestor
                .file_name()
                .ok_or_else(|| Error("state root is not a directory".into()))?
                .to_owned(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| Error("state root is not a directory".into()))?;
    }
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|e| Error(format!("state root: {e}")))?;
    require(
        windows_local_path(&resolved),
        "network state root is unsupported",
    )?;
    for part in tail.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn read_json_file(mut file: File) -> Result<J> {
    let meta = file
        .metadata()
        .map_err(|e| Error(format!("retained context: {e}")))?;
    require(meta.is_file(), "retained context is not a regular file")?;
    require(meta.len() <= MAX_FILE, "retained context exceeds limit")?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    Read::by_ref(&mut file)
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error(format!("retained context: {e}")))?;
    require(
        bytes.len() as u64 <= MAX_FILE,
        "retained context exceeds limit",
    )?;
    serde_json::from_slice(&bytes).map_err(|e| Error(format!("invalid retained core context: {e}")))
}
