//! Guarded publication of exact prepared images. The mandatory verifier owns
//! runtime/capability and reader-resolved baseline checks, never the byte guard.
use crate::{
    Result, history_authority as A,
    history_contract::*,
    history_transaction::{FileImage, Layout, MAX_TRANSACTION_BYTES, PreparedMutation},
    identity::sha256,
    require,
    value::TypedValue as V,
};
#[cfg(any(unix, windows))]
use fs2::FileExt;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::{ffi::OsStrExt, fs::MetadataExt};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

struct Held {
    _file: File,
    path: PathBuf,
    identity: (u64, u64, u32),
    exclusive: bool,
}
thread_local! {static LOCKS: RefCell<Vec<Weak<Held>>> = const {RefCell::new(Vec::new())};}
struct AuxiliaryPermit {
    path: PathBuf,
    digest: String,
    owner: Rc<Held>,
}
thread_local! {static AUXILIARY: RefCell<Vec<Weak<AuxiliaryPermit>>> = const {RefCell::new(Vec::new())};}
/// Scoped permission for the exact pending auxiliary envelope, retaining the
/// writer's directory lock. It cannot be sent to another thread.
pub struct AuxiliaryOwner {
    _permit: Rc<AuxiliaryPermit>,
}
fn exclusive_owned(root: &Path) -> Result<Option<Rc<Held>>> {
    #[cfg(not(any(unix, windows)))]
    {
        let _ = root;
        Ok(None)
    }
    #[cfg(any(unix, windows))]
    {
        let identity = directory_identity(root)?;
        Ok(LOCKS.with(|locks| {
            locks
                .borrow()
                .iter()
                .filter_map(Weak::upgrade)
                .find(|l| l.identity == identity && l.exclusive)
        }))
    }
}
fn owned_auxiliary(root: &Path, path: &Path, raw: &[u8]) -> Result<bool> {
    let Some(owner) = exclusive_owned(root)? else {
        return Ok(false);
    };
    let digest = sha256(raw);
    Ok(AUXILIARY.with(|permits| {
        let mut permits = permits.borrow_mut();
        permits.retain(|p| p.strong_count() > 0);
        permits
            .iter()
            .filter_map(Weak::upgrade)
            .any(|p| p.path == path && p.digest == digest && p.owner.identity == owner.identity)
    }))
}
pub fn auxiliary_owner(root: &Path, journal: &str, m: &PreparedMutation) -> Result<AuxiliaryOwner> {
    let owner = exclusive_owned(root)?.ok_or_else(|| error("auxiliary_writer_lock_required"))?;
    require(
        journal == Layout::for_entry(&mutation_entry(m)?)?.journal,
        "invalid_auxiliary_journal",
    )?;
    let path = journal_path(root, journal, m)?;
    let raw = m.auxiliary_envelope()?;
    require(
        read(&path)?.is_none_or(|existing| existing == raw),
        "recovery_required",
    )?;
    let permit = Rc::new(AuxiliaryPermit {
        path,
        digest: sha256(&raw),
        owner,
    });
    AUXILIARY.with(|p| p.borrow_mut().push(Rc::downgrade(&permit)));
    Ok(AuxiliaryOwner { _permit: permit })
}
pub fn publish_auxiliary_journal(root: &Path, journal: &str, m: &PreparedMutation) -> Result<()> {
    let path = journal_path(root, journal, m)?;
    let raw = m.auxiliary_envelope()?;
    require(
        owned_auxiliary(root, &path, &raw)?,
        "auxiliary_writer_lock_required",
    )?;
    private_home(root, journal, m)?;
    publish_immutable(root, journal, &raw)
}
pub fn clear_auxiliary_journal(root: &Path, journal: &str, m: &PreparedMutation) -> Result<()> {
    let path = journal_path(root, journal, m)?;
    let raw = m.auxiliary_envelope()?;
    require(
        owned_auxiliary(root, &path, &raw)?,
        "auxiliary_writer_lock_required",
    )?;
    require(
        read(&path)?.is_none_or(|existing| existing == raw),
        "recovery_required",
    )?;
    remove(&path)
}
pub(crate) fn check_reader_journal(root: &Path, journal: &str) -> Result<()> {
    let path = target(root, journal)?;
    if let Some(raw) = read(&path)? {
        require(owned_auxiliary(root, &path, &raw)?, "recovery_required")?;
    }
    Ok(())
}
/// Check every local member guard before reading any participant. A replica
/// contains local basenames only; it never authorizes following a private link.
pub fn check_member_journals(entries: &[PathBuf]) -> Result<()> {
    for entry in entries {
        let parent = entry.parent().ok_or_else(|| error("invalid_path"))?;
        if !parent.is_dir() {
            continue;
        }
        let root = parent.canonicalize()?;
        let name = to_str(Path::new(
            entry.file_name().ok_or_else(|| error("invalid_path"))?,
        ))?;
        check_reader_journal(&root, &Layout::for_entry(name)?.journal)?;
        let mut pending = BTreeSet::new();
        for home in [".kpopper/.history-local", ".history-local"] {
            let path = target(&root, home)?;
            if !path.is_dir() {
                continue;
            }
            for item in fs::read_dir(path)? {
                let item = item?;
                let filename = item.file_name();
                let filename = filename.to_str().ok_or_else(|| error("invalid_path"))?;
                if !filename.starts_with('.') && filename.ends_with(".json") {
                    pending.insert(target(&root, &format!("{home}/{filename}"))?);
                    require(pending.len() <= 1024, "history_limit")?;
                }
            }
        }
        let mut total = 0usize;
        for path in pending {
            let raw = read(&path)?.ok_or_else(|| error("invalid_pending_journal"))?;
            total = total.saturating_add(raw.len());
            require(total <= MAX_TRANSACTION_BYTES, "history_limit")?;
            if owned_auxiliary(&root, &path, &raw)? {
                continue;
            }
            let participating = (|| -> Result<bool> {
                if raw.iter().find(|c| !c.is_ascii_whitespace()) == Some(&b'{') {
                    let json = crate::store::json_input(&raw)?;
                    let guard = V::from_json(&json)?;
                    let g = schema(
                        &guard,
                        &["version", "kind", "operation", "digest", "members"],
                        &[],
                    )?;
                    require(
                        is_int(&g["version"], "1") && string_is(&g["kind"], "member_guard"),
                        "invalid_member_guard",
                    )?;
                    token(&g["operation"])?;
                    let digest = text(&g["digest"])?;
                    require(
                        digest.len() == 64 && crate::history_paths::object_id(digest),
                        "invalid_member_guard",
                    )?;
                    require(
                        path.file_name().and_then(|v| v.to_str())
                            == Some(&format!("{digest}.json")),
                        "invalid_member_guard",
                    )?;
                    let names = crate::history_view::list(&g["members"])?;
                    require(
                        !names.is_empty() && names.len() <= 100_000,
                        "invalid_member_guard",
                    )?;
                    let mut previous = None;
                    for member in names {
                        let member = text(member)?;
                        A::relative_path(member)?;
                        require(
                            !member.contains('/') && previous.is_none_or(|p| p < member),
                            "invalid_member_guard",
                        )?;
                        previous = Some(member);
                    }
                    return Ok(names.iter().any(|v| string_is(v, name)));
                }
                let mutation = PreparedMutation::from_bytes(&raw)?;
                let value = mutation.to_data();
                let baseline = map(&map(&value)?["baseline"])?;
                let namespace = baseline
                    .get("transaction_root")
                    .map(text)
                    .transpose()?
                    .map(PathBuf::from)
                    .unwrap_or(root.clone());
                require(namespace.is_absolute(), "invalid_pending_journal")?;
                let journal = Layout::for_entry(&mutation_entry(&mutation)?)?.journal;
                let primary = target(&namespace, &journal)?;
                let mut expected = replicas(&namespace, &journal, &mutation)?;
                expected.push(primary);
                require(expected.contains(&path), "invalid_pending_journal")?;
                let mut members = participants(&mutation)?;
                members.extend(mutation.files().iter().map(|file| file.path.clone()));
                Ok(members
                    .iter()
                    .map(|p| target(&namespace, p))
                    .collect::<Result<Vec<_>>>()?
                    .contains(&root.join(name)))
            })()
            .map_err(|e| error(&format!("invalid_pending_journal: {e}")))?;
            require(!participating, "recovery_required")?;
        }
    }
    Ok(())
}
/// Thread-bound, reentrant directory lock; clones keep the same kernel lock alive.
pub struct DirectoryGuard {
    _held: Option<Rc<Held>>,
}
impl DirectoryGuard {
    pub fn acquire(root: &Path, exclusive: bool) -> Result<Self> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = root;
            require(!exclusive, "locking_unavailable")?;
            Ok(Self { _held: None })
        }
        #[cfg(unix)]
        {
            let path = root.canonicalize()?;
            let file = File::open(&path)?;
            let stat = file.metadata()?;
            require(stat.is_dir(), "invalid_path")?;
            let identity = (stat.dev(), stat.ino(), std::process::id());
            let held = LOCKS.with(|locks| -> Result<Option<Rc<Held>>> {
                let mut locks = locks.borrow_mut();
                locks.retain(|l| l.strong_count() > 0);
                let active: Vec<_> = locks
                    .iter()
                    .filter_map(Weak::upgrade)
                    .filter(|l| l.identity.2 == identity.2)
                    .collect();
                if let Some(existing) = active.iter().find(|l| l.identity == identity) {
                    require(existing.exclusive || !exclusive, "lock_upgrade_refused")?;
                    return Ok(Some(existing.clone()));
                }
                require(active.iter().all(|l| l.path <= path), "lock_order_refused")?;
                Ok(None)
            })?;
            if let Some(held) = held {
                return Ok(Self { _held: Some(held) });
            }
            if exclusive {
                FileExt::lock_exclusive(&file)?;
            } else {
                FileExt::lock_shared(&file)?;
            }
            let held = Rc::new(Held {
                _file: file,
                path,
                identity,
                exclusive,
            });
            LOCKS.with(|locks| locks.borrow_mut().push(Rc::downgrade(&held)));
            Ok(Self { _held: Some(held) })
        }
        #[cfg(windows)]
        {
            let path = root.canonicalize()?;
            let stat = fs::metadata(&path)?;
            require(stat.is_dir(), "invalid_path")?;
            let identity = windows_identity(&stat)?;
            let held = LOCKS.with(|locks| -> Result<Option<Rc<Held>>> {
                let mut locks = locks.borrow_mut();
                locks.retain(|l| l.strong_count() > 0);
                let active: Vec<_> = locks
                    .iter()
                    .filter_map(Weak::upgrade)
                    .filter(|l| l.identity.2 == identity.2)
                    .collect();
                if let Some(existing) = active.iter().find(|l| l.identity == identity) {
                    require(existing.exclusive || !exclusive, "lock_upgrade_refused")?;
                    return Ok(Some(existing.clone()));
                }
                require(active.iter().all(|l| l.path <= path), "lock_order_refused")?;
                Ok(None)
            })?;
            if let Some(held) = held {
                return Ok(Self { _held: Some(held) });
            }
            let lock_home = std::env::temp_dir().join("kpopper-native-locks");
            fs::create_dir_all(&lock_home)?;
            let mut key = Vec::new();
            for unit in path.as_os_str().encode_wide() {
                key.extend_from_slice(&unit.to_le_bytes());
            }
            let lock_path = lock_home.join(format!("{}.lock", sha256(&key)));
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .open(&lock_path)?;
            if exclusive {
                FileExt::lock_exclusive(&file)?;
            } else {
                FileExt::lock_shared(&file)?;
            }
            require(path == root.canonicalize()?, "directory_replaced")?;
            require(directory_identity(&path)? == identity, "directory_replaced")?;
            require(
                windows_file_identity(&file.metadata()?)?
                    == windows_file_identity(&fs::metadata(&lock_path)?)?,
                "lock_replaced",
            )?;
            let held = Rc::new(Held {
                _file: file,
                path,
                identity,
                exclusive,
            });
            LOCKS.with(|locks| locks.borrow_mut().push(Rc::downgrade(&held)));
            Ok(Self { _held: Some(held) })
        }
    }
}
#[cfg(unix)]
fn directory_identity(root: &Path) -> Result<(u64, u64, u32)> {
    let stat = fs::metadata(root)?;
    Ok((stat.dev(), stat.ino(), std::process::id()))
}
#[cfg(windows)]
fn windows_file_identity(stat: &fs::Metadata) -> Result<(u64, u64)> {
    Ok((
        stat.volume_serial_number()
            .ok_or_else(|| error("locking_unavailable"))? as u64,
        stat.file_index()
            .ok_or_else(|| error("locking_unavailable"))?,
    ))
}
#[cfg(windows)]
fn windows_identity(stat: &fs::Metadata) -> Result<(u64, u64, u32)> {
    let (volume, file) = windows_file_identity(stat)?;
    Ok((volume, file, std::process::id()))
}
#[cfg(windows)]
fn directory_identity(root: &Path) -> Result<(u64, u64, u32)> {
    windows_identity(&fs::metadata(root)?)
}
fn guards(paths: Vec<PathBuf>) -> Result<Vec<DirectoryGuard>> {
    let paths = paths
        .into_iter()
        .map(|p| p.canonicalize())
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    paths
        .iter()
        .map(|p| DirectoryGuard::acquire(p, true))
        .collect()
}
pub fn target(root: &Path, relative: &str) -> Result<PathBuf> {
    A::relative_path(relative)?;
    let mut path = root.canonicalize()?;
    #[cfg(windows)]
    LOCKS.with(|locks| -> Result<()> {
        for held in locks.borrow().iter().filter_map(Weak::upgrade) {
            if held.path == path {
                require(
                    directory_identity(&path)? == held.identity,
                    "directory_replaced",
                )?;
            }
        }
        Ok(())
    })?;
    for segment in relative.split('/') {
        path.push(segment);
        match fs::symlink_metadata(&path) {
            Ok(m) => require(!m.file_type().is_symlink(), "symlink_path")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(path)
}
pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let meta = f.metadata()?;
    require(meta.is_file(), "invalid_path")?;
    require(meta.len() <= MAX_TRANSACTION_BYTES as u64, "history_limit")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut f)
        .take((MAX_TRANSACTION_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    require(bytes.len() <= MAX_TRANSACTION_BYTES, "history_limit")?;
    Ok(Some(bytes))
}
fn sync(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
pub(crate) fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync(path.parent().unwrap()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
pub(crate) fn replace(path: &Path, bytes: Option<&[u8]>) -> Result<()> {
    let Some(bytes) = bytes else {
        return remove(path);
    };
    let parent = path.parent().ok_or_else(|| error("invalid_path"))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".history-")
        .tempfile_in(parent)?;
    #[cfg(unix)]
    if let Ok(m) = fs::metadata(path) {
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(m.permissions().mode() & 0o7777))?;
    }
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|e| crate::Error(format!("io: {}", e.error)))?;
    sync(parent)
}
pub fn publish_immutable(root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    let path = target(root, relative)?;
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".history-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    match fs::hard_link(temporary.path(), &path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => require(
            read(&path)?.as_deref() == Some(bytes),
            "immutable_collision",
        )?,
        Err(e) => return Err(e.into()),
    }
    sync(parent)
}
fn data(m: &PreparedMutation) -> V {
    m.to_data()
}
fn mutation_entry(m: &PreparedMutation) -> Result<String> {
    Ok(text(field(map(&data(m))?, "entry")?)?.into())
}
fn digest(m: &PreparedMutation) -> Result<String> {
    Ok(text(field(map(&data(m))?, "digest")?)?.into())
}
fn participants(m: &PreparedMutation) -> Result<BTreeSet<String>> {
    let value = data(m);
    let base = map(field(map(&value)?, "baseline")?)?;
    let mut result = BTreeSet::new();
    for key in ["record_members", "hypothesis_members"] {
        if let Some(v) = base.get(key) {
            result.extend(map(v)?.keys().cloned());
        }
    }
    Ok(result)
}
pub fn participant_directories(root: &Path, m: &PreparedMutation) -> Result<Vec<PathBuf>> {
    let mut paths = participants(m)?;
    paths.insert(mutation_entry(m)?);
    paths
        .iter()
        .map(|p| Ok(target(root, p)?.parent().unwrap().to_owned()))
        .collect::<Result<BTreeSet<_>>>()
        .map(|v| v.into_iter().collect())
}
pub(crate) fn journal_path(root: &Path, journal: &str, m: &PreparedMutation) -> Result<PathBuf> {
    let path = target(root, journal)?;
    let candidate = Path::new(journal);
    for item in m.files() {
        let member = Path::new(&item.path);
        require(
            !candidate.starts_with(member) && !member.starts_with(candidate),
            "invalid_journal_path",
        )?;
    }
    Ok(path)
}
fn private_home(root: &Path, journal: &str, m: &PreparedMutation) -> Result<()> {
    if journal == Layout::for_entry(&mutation_entry(m)?)?.journal {
        let ignore = Path::new(journal).parent().unwrap().join(".gitignore");
        let path = target(root, to_str(&ignore)?)?;
        if let Some(raw) = read(&path)? {
            require(raw == b"*\n", "journal_ignore_mismatch")?;
        } else {
            publish_immutable(root, to_str(&ignore)?, b"*\n")?;
        }
    }
    Ok(())
}
fn to_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| error("invalid_path"))
}
fn relative<'a>(root: &Path, path: &'a Path) -> Result<&'a str> {
    to_str(
        path.strip_prefix(root.canonicalize()?)
            .map_err(|_| error("invalid_path"))?,
    )
}
pub(crate) fn replicas(root: &Path, journal: &str, m: &PreparedMutation) -> Result<Vec<PathBuf>> {
    let value = data(m);
    let base = map(field(map(&value)?, "baseline")?)?;
    let Some(transaction_root) = base.get("transaction_root") else {
        return Ok(Vec::new());
    };
    let absolute = std::path::absolute(root)?;
    require(
        text(transaction_root)? == to_str(&absolute)?,
        "transaction_root_mismatch",
    )?;
    target(root, journal)?;
    let directories = participants(m)?
        .iter()
        .map(|p| Ok(target(root, p)?.parent().unwrap().to_owned()))
        .collect::<Result<BTreeSet<_>>>()?;
    if directories.len() < 2 {
        return Ok(Vec::new());
    }
    let entry_dir = target(root, &mutation_entry(m)?)?
        .parent()
        .unwrap()
        .to_owned();
    Ok(directories
        .into_iter()
        .filter(|p| *p != entry_dir)
        .map(|p| {
            p.join(".history-local")
                .join(format!("{}.json", digest(m).unwrap()))
        })
        .collect())
}
pub(crate) fn member_guard(root: &Path, m: &PreparedMutation, directory: &Path) -> Result<Vec<u8>> {
    let members = participants(m)?
        .iter()
        .map(|p| target(root, p))
        .collect::<Result<Vec<_>>>()?;
    let names = members
        .iter()
        .filter(|p| p.parent() == Some(directory))
        .map(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .ok_or_else(|| error("invalid_path"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let value = data(m);
    Ok(serde_json::to_vec(
        &serde_json::json!({"version":1,"kind":"member_guard","operation":text(field(map(&value)?,"operation")?)?,"digest":digest(m)?,"members":names}),
    )?)
}
fn prepare_replicas(root: &Path, journal: &str, m: &PreparedMutation) -> Result<Vec<PathBuf>> {
    let paths = replicas(root, journal, m)?;
    let mut images = Vec::new();
    for p in &paths {
        let raw = member_guard(root, m, p.parent().unwrap().parent().unwrap())?;
        let checked = journal_path(root, relative(root, p)?, m)?;
        require(
            read(&checked)?.is_none_or(|existing| existing == raw),
            "journal_replica_mismatch",
        )?;
        images.push(raw);
    }
    for (path, raw) in paths.iter().zip(images) {
        publish_immutable(
            root,
            relative(root, &path.parent().unwrap().join(".gitignore"))?,
            b"*\n",
        )?;
        publish_immutable(root, relative(root, path)?, &raw)?;
    }
    Ok(paths)
}
fn ready_path(primary: &Path, m: &PreparedMutation) -> Result<PathBuf> {
    Ok(primary.with_file_name(format!(
        "{}.{}.ready",
        to_str(Path::new(primary.file_name().unwrap()))?,
        digest(m)?
    )))
}
pub(crate) fn remove_journals(primary: &Path, replicas: &[PathBuf]) -> Result<()> {
    for path in replicas {
        remove(path)?;
    }
    remove(primary)
}
fn preflight(root: &Path, m: &PreparedMutation, recovery: bool) -> Result<Vec<PathBuf>> {
    let paths = m
        .files()
        .iter()
        .map(|i| target(root, &i.path))
        .collect::<Result<Vec<_>>>()?;
    let states = paths.iter().map(|p| read(p)).collect::<Result<Vec<_>>>()?;
    if !recovery
        && states
            .iter()
            .zip(m.files())
            .any(|(raw, i)| *raw != i.before)
        && states.iter().zip(m.files()).all(|(raw, i)| *raw == i.after)
    {
        return Err(error("after_images_match"));
    }
    for (raw, i) in states.iter().zip(m.files()) {
        require(
            *raw == i.before || recovery && *raw == i.after,
            "concurrent_edit",
        )?;
    }
    Ok(paths)
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Before,
    After,
}
fn image(item: &FileImage, direction: Direction) -> Option<&[u8]> {
    match direction {
        Direction::Before => item.before.as_deref(),
        Direction::After => item.after.as_deref(),
    }
}
fn apply(m: &PreparedMutation, paths: &[PathBuf], direction: Direction) -> Result<()> {
    for (i, path) in m.files().iter().zip(paths) {
        let bytes = image(i, direction);
        if read(path)?.as_deref() != bytes {
            replace(path, bytes)?;
        }
    }
    Ok(())
}
fn is_legacy(m: &PreparedMutation) -> Result<()> {
    let value = data(m);
    let value = map(&value)?;
    require(
        string_is(
            field(map(field(value, "authority")?)?, "authority")?,
            "legacy",
        ) && !value.contains_key("transition"),
        "invalid_authority",
    )
}
pub type Verify<'a> = &'a mut dyn FnMut(&V) -> Result<()>;
pub fn publish_legacy(
    root: &Path,
    journal: &str,
    m: &PreparedMutation,
    verify: Verify<'_>,
    mut committed: Option<Verify<'_>>,
) -> Result<()> {
    is_legacy(m)?;
    let directories = participant_directories(root, m)?;
    for dir in &directories {
        fs::create_dir_all(dir)?;
    }
    let _guards = guards(directories)?;
    let primary = journal_path(root, journal, m)?;
    require(read(&primary)?.is_none(), "recovery_required")?;
    let paths = preflight(root, m, false)?;
    verify(&data(m))?;
    private_home(root, journal, m)?;
    publish_immutable(root, journal, &m.to_bytes()?)?;
    let mut copies = prepare_replicas(root, journal, m)?;
    if !copies.is_empty() {
        let ready = ready_path(&primary, m)?;
        publish_immutable(root, relative(root, &ready)?, digest(m)?.as_bytes())?;
        copies.push(ready);
    }
    apply(m, &paths, Direction::After)?;
    if let Some(callback) = committed.as_mut() {
        callback(&data(m))?;
    }
    remove_journals(&primary, &copies)
}
pub fn recover_legacy(
    root: &Path,
    journal: &str,
    direction: Direction,
    verify: Verify<'_>,
    mut committed: Option<Verify<'_>>,
    mut cancel: Option<Verify<'_>>,
) -> Result<PreparedMutation> {
    let primary = target(root, journal)?;
    let raw = read(&primary)?.ok_or_else(|| error("no_recovery_pending"))?;
    let m = PreparedMutation::from_bytes(&raw)?;
    let _guards = guards(participant_directories(root, &m)?)?;
    require(read(&primary)?.as_ref() == Some(&raw), "concurrent_edit")?;
    is_legacy(&m)?;
    journal_path(root, journal, &m)?;
    let copies = replicas(root, journal, &m)?;
    let ready = ready_path(&primary, &m)?;
    let ready_bytes = read(&ready)?;
    let digest = digest(&m)?;
    require(
        ready_bytes
            .as_deref()
            .is_none_or(|raw| raw == digest.as_bytes()),
        "invalid_ready_marker",
    )?;
    if !copies.is_empty() && ready_bytes.is_none() && direction == Direction::Before {
        if let Some(cancel) = cancel.as_mut() {
            cancel(&data(&m))?;
        } else {
            verify(&data(&m))?;
        }
        let mut guarded = BTreeSet::from([target(root, &mutation_entry(&m)?)?
            .parent()
            .unwrap()
            .to_owned()]);
        for p in &copies {
            if read(p)?.is_some() {
                guarded.insert(p.parent().unwrap().parent().unwrap().to_owned());
            }
        }
        for item in m.files() {
            let path = target(root, &item.path)?;
            let current = read(&path)?;
            if current != item.before {
                require(current != item.after, "incomplete_readiness")?;
                require(
                    item.role == "record_member" && !guarded.contains(path.parent().unwrap()),
                    "concurrent_edit",
                )?;
            }
        }
        for p in &copies {
            let expected = member_guard(root, &m, p.parent().unwrap().parent().unwrap())?;
            require(
                read(p)?.is_none_or(|raw| raw == expected),
                "journal_replica_mismatch",
            )?;
        }
        remove_journals(&primary, &copies)?;
        return Ok(m);
    }
    let paths = preflight(root, &m, true)?;
    verify(&data(&m))?;
    let mut copies = prepare_replicas(root, journal, &m)?;
    if !copies.is_empty() {
        publish_immutable(root, relative(root, &ready)?, digest.as_bytes())?;
        copies.push(ready);
    }
    apply(&m, &paths, direction)?;
    if direction == Direction::After
        && let Some(callback) = committed.as_mut()
    {
        callback(&data(&m))?;
    }
    remove_journals(&primary, &copies)?;
    Ok(m)
}
/// A read bracket holds the same directory lock and refuses pending transactions
/// both before and after observation. Auxiliary ownership is not inferred here.
pub fn reader_guard<T>(
    root: &Path,
    journal: &str,
    read_body: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _guard = DirectoryGuard::acquire(root, false)?;
    check_reader_journal(root, journal)?;
    let result = read_body()?;
    check_reader_journal(root, journal)?;
    Ok(result)
}

fn transition_immutable(role: &str) -> bool {
    ["history_object", "history_commit", "history_retained"].contains(&role)
}
pub(crate) fn transition_targets(root: &Path, m: &PreparedMutation) -> Result<Vec<PathBuf>> {
    let value = data(m);
    let value = map(&value)?;
    require(
        value.get("version").is_some_and(|v| is_int(v, "2")) && value.contains_key("transition"),
        "invalid_authority_transition",
    )?;
    let mut paths = Vec::new();
    for item in m.files() {
        let path = target(root, &item.path)?;
        let current = read(&path)?;
        if transition_immutable(&item.role) {
            require(
                current.is_none() || current == item.after,
                "immutable_collision",
            )?;
        } else {
            require(
                current == item.before || current == item.after,
                "concurrent_edit",
            )?;
        }
        paths.push(path);
    }
    Ok(paths)
}
pub(crate) fn mutable_before(m: &PreparedMutation, paths: &[PathBuf]) -> Result<()> {
    for (item, path) in m.files().iter().zip(paths) {
        if !transition_immutable(&item.role) {
            require(read(path)? == item.before, "concurrent_edit")?;
        }
    }
    Ok(())
}
pub(crate) fn apply_transition(
    root: &Path,
    m: &PreparedMutation,
    paths: &[PathBuf],
    direction: Direction,
) -> Result<()> {
    if direction == Direction::After {
        for item in m.files().iter().filter(|i| transition_immutable(&i.role)) {
            publish_immutable(root, &item.path, item.after.as_ref().unwrap())?;
        }
    }
    // Publication switches the authority marker last while the journal protects
    // all readers. Rollback retains immutable evidence for cancellation audit.
    let mut mutable: Vec<_> = m
        .files()
        .iter()
        .zip(paths)
        .filter(|(i, _)| !transition_immutable(&i.role))
        .collect();
    mutable.sort_by_key(|(i, _)| i.role == "history_authority");
    for (item, path) in mutable {
        let bytes = image(item, direction);
        if read(path)?.as_deref() != bytes {
            replace(path, bytes)?;
        }
    }
    Ok(())
}
fn refuse_group(m: &PreparedMutation) -> Result<()> {
    let value = data(m);
    let base = map(field(map(&value)?, "baseline")?)?;
    require(!base.contains_key("group"), "group_recovery_required")
}
pub fn publish_transition(
    root: &Path,
    journal: &str,
    m: &PreparedMutation,
    verify: Verify<'_>,
    mut committed: Option<Verify<'_>>,
) -> Result<()> {
    refuse_group(m)?;
    let _guards = guards(participant_directories(root, m)?)?;
    let primary = journal_path(root, journal, m)?;
    require(read(&primary)?.is_none(), "recovery_required")?;
    let paths = transition_targets(root, m)?;
    mutable_before(m, &paths)?;
    verify(&data(m))?;
    let paths = transition_targets(root, m)?;
    mutable_before(m, &paths)?;
    private_home(root, journal, m)?;
    publish_immutable(root, journal, &m.to_bytes()?)?;
    let mut copies = prepare_replicas(root, journal, m)?;
    if !copies.is_empty() {
        let ready = ready_path(&primary, m)?;
        publish_immutable(root, relative(root, &ready)?, digest(m)?.as_bytes())?;
        copies.push(ready);
    }
    apply_transition(root, m, &paths, Direction::After)?;
    if let Some(callback) = committed.as_mut() {
        callback(&data(m))?;
    }
    remove_journals(&primary, &copies)
}
pub fn recover_transition(
    root: &Path,
    journal: &str,
    direction: Direction,
    verify: Verify<'_>,
    committed: Option<Verify<'_>>,
) -> Result<PreparedMutation> {
    recover_transition_inner(root, journal, direction, verify, committed, false)
}
/// Called only by the guarded lifecycle after terminal cancellation verification.
pub(crate) fn finish_cancelled_transition(
    root: &Path,
    journal: &str,
    m: &PreparedMutation,
) -> Result<()> {
    let primary = journal_path(root, journal, m)?;
    require(
        read(&primary)?.as_ref() == Some(&m.to_bytes()?),
        "concurrent_edit",
    )?;
    let mut copies = replicas(root, journal, m)?;
    for path in &copies {
        let expected = member_guard(root, m, path.parent().unwrap().parent().unwrap())?;
        require(
            read(path)?.is_none_or(|raw| raw == expected),
            "member_guard_mismatch",
        )?;
    }
    if !copies.is_empty() {
        let ready = ready_path(&primary, m)?;
        let expected = digest(m)?;
        require(
            read(&ready)?.is_none_or(|raw| raw == expected.as_bytes()),
            "invalid_ready_marker",
        )?;
        copies.push(ready);
    }
    remove_journals(&primary, &copies)
}
pub(crate) fn rollback_bootstrap(
    root: &Path,
    journal: &str,
    verify: Verify<'_>,
) -> Result<PreparedMutation> {
    recover_transition_inner(root, journal, Direction::Before, verify, None, true)
}
fn recover_transition_inner(
    root: &Path,
    journal: &str,
    direction: Direction,
    verify: Verify<'_>,
    mut committed: Option<Verify<'_>>,
    bootstrap_cleanup: bool,
) -> Result<PreparedMutation> {
    let primary = target(root, journal)?;
    let raw = read(&primary)?.ok_or_else(|| error("no_recovery_pending"))?;
    let m = PreparedMutation::from_bytes(&raw)?;
    refuse_group(&m)?;
    let _guards = guards(participant_directories(root, &m)?)?;
    require(read(&primary)?.as_ref() == Some(&raw), "concurrent_edit")?;
    journal_path(root, journal, &m)?;
    let value = data(&m);
    let value = map(&value)?;
    let base = map(field(value, "baseline")?)?;
    if bootstrap_cleanup {
        let bootstrap = map(field(base, "bootstrap")?)?;
        let authority = map(field(value, "authority")?)?;
        require(
            direction == Direction::Before
                && base.get("source_absent") == Some(&V::Bool(true))
                && bootstrap.get("kind") == Some(&V::Text("new-record-bootstrap/v1".into()))
                && bootstrap.get("version").is_some_and(|v| is_int(v, "1"))
                && string_is(&authority["authority"], "legacy")
                && is_int(&authority["generation"], "0")
                && m.files().iter().all(|i| i.before.is_none()),
            "invalid_history_bootstrap",
        )?;
    }
    let cancellation = if base
        .get("direction")
        .is_some_and(|v| string_is(v, "activate"))
    {
        let layout = Layout::for_entry(&mutation_entry(&m)?)?;
        Some(target(
            root,
            &format!(
                "{}/{}.yaml",
                layout.cancellations,
                text(field(value, "operation")?)?
            ),
        )?)
    } else {
        None
    };
    let check_cancel = || -> Result<()> {
        if let Some(path) = &cancellation {
            require(read(path)?.is_none(), "cancellation_recovery_required")?;
        }
        Ok(())
    };
    check_cancel()?;
    transition_targets(root, &m)?;
    verify(&data(&m))?;
    check_cancel()?;
    let paths = transition_targets(root, &m)?;
    let mut copies = prepare_replicas(root, journal, &m)?;
    let ready = ready_path(&primary, &m)?;
    let digest = digest(&m)?;
    require(
        read(&ready)?
            .as_deref()
            .is_none_or(|raw| raw == digest.as_bytes()),
        "invalid_ready_marker",
    )?;
    if !copies.is_empty() {
        publish_immutable(root, relative(root, &ready)?, digest.as_bytes())?;
        copies.push(ready);
    }
    apply_transition(root, &m, &paths, direction)?;
    if bootstrap_cleanup {
        for (item, path) in m
            .files()
            .iter()
            .zip(&paths)
            .filter(|(i, _)| transition_immutable(&i.role))
        {
            let current = read(path)?;
            require(
                current.is_none() || current == item.after,
                "history_bootstrap_rollback_changed",
            )?;
            if current.is_some() {
                remove(path)?;
            }
        }
    }
    if direction == Direction::After
        && let Some(callback) = committed.as_mut()
    {
        callback(&data(&m))?;
    }
    remove_journals(&primary, &copies)?;
    Ok(m)
}
