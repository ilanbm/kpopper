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
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_FILE: u64 = 64 * 1024 * 1024;
const VERSION: u64 = 1;

#[derive(Clone, Debug)]
pub struct CheckedSessionStore {
    root: PathBuf,
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
        let root = root.as_ref().to_path_buf();
        ensure_dir(&root)?;
        let input = input
            .as_ref()
            .canonicalize()
            .map_err(|e| Error(format!("input path: {e}")))?;
        let profile_hash = navigation_profile
            .as_ref()
            .map(profile_digest)
            .transpose()?;
        let identity = V::Map(std::collections::BTreeMap::from([
            ("project".into(), V::Text(project.into())),
            (
                "input_path".into(),
                V::Text(input.to_string_lossy().into_owned()),
            ),
            (
                "navigation_profile_sha256".into(),
                profile_hash.map_or(V::Null, V::Text),
            ),
            ("encoding".into(), V::Text(encoding.name().into())),
        ]));
        let path = root.join("project.json");
        let held = read_json_if_present(&path)?;
        if let Some(value) = held {
            require(
                value == ordinary(&identity)?,
                "state directory belongs to another project/input/profile",
            )?;
        } else {
            atomic_create(&path, &ordinary(&identity)?)?;
        }
        Ok(Self {
            root,
            identity,
            profile: navigation_profile,
            encoding,
        })
    }

    pub fn revision(&self, context: &CapturedAssessment) -> Result<String> {
        context.session_revision(&self.identity)
    }

    /// Persist a complete context, rejecting a changed freshness snapshot.
    pub fn save(&self, context: &CapturedAssessment, freshness: &Snapshot) -> Result<String> {
        require(
            freshness.snapshot_id() == context.snapshot_id(),
            "project record or history changed; reopen",
        )?;
        let revision = self.revision(context)?;
        let session = CheckedSession::new(
            context.clone(),
            self.identity.clone(),
            self.profile.as_ref(),
        )?;
        require(
            session.revision() == revision,
            "checked session revision mismatch",
        )?;
        let payload = json!({"version": VERSION, "project_identity": ordinary(&self.identity)?,
            "project": self.project()?, "revision": revision, "snapshot_id": context.snapshot_id(),
            "encoding": self.encoding.name(), "context": serde_json::from_str::<J>(&context.to_json()?)?});
        let path = self.context_path(&revision);
        let held = atomic_create(&path, &payload)?;
        require(held == payload, "core context identity collision")?;
        Ok(revision)
    }

    /// Load a retained context using only the caller's already captured snapshot.
    pub fn load(&self, revision: &str, freshness: &Snapshot) -> Result<CheckedSession> {
        require(valid_revision(revision), "invalid core session revision")?;
        let path = self.context_path(revision);
        let payload = read_json(&path)?;
        let map = payload
            .as_object()
            .ok_or_else(|| Error("invalid retained core context".into()))?;
        require(
            map.len() == 7 && map.get("version") == Some(&json!(VERSION)),
            "invalid retained core context",
        )?;
        require(
            map.get("project") == Some(&json!(self.project()?))
                && map.get("revision") == Some(&json!(revision)),
            "core context identity mismatch",
        )?;
        require(
            map.get("project_identity") == Some(&ordinary(&self.identity)?),
            "core context identity mismatch",
        )?;
        require(
            map.get("encoding") == Some(&json!(self.encoding.name())),
            "core context identity mismatch",
        )?;
        require(
            map.get("snapshot_id") == Some(&json!(freshness.snapshot_id())),
            "project record or history changed; reopen",
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
            context.session_revision(&self.identity)? == revision,
            "core context revision mismatch",
        )?;
        CheckedSession::new(context, self.identity.clone(), self.profile.as_ref())
    }

    pub fn context_path(&self, revision: &str) -> PathBuf {
        self.root.join(format!("core-context-{revision}.json"))
    }
    fn project(&self) -> Result<String> {
        match &self.identity {
            V::Map(m) => match &m["project"] {
                V::Text(s) => Ok(s.clone()),
                _ => Err(Error("invalid project identity".into())),
            },
            _ => Err(Error("invalid project identity".into())),
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
fn ordinary(v: &V) -> Result<J> {
    v.to_json().map_err(|_| Error("invalid identity".into()))
}
fn profile_digest(v: &V) -> Result<String> {
    Ok(sha256(serde_json::to_vec(&ordinary(v)?)?.as_slice()))
}
fn ensure_dir(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        require(
            meta.file_type().is_dir() && !meta.file_type().is_symlink(),
            "state root is not a directory",
        )?;
    } else {
        fs::create_dir_all(path).map_err(|e| Error(format!("state directory: {e}")))?;
    }
    Ok(())
}
fn read_json_if_present(path: &Path) -> Result<Option<J>> {
    match fs::symlink_metadata(path) {
        Ok(m) => {
            require(
                m.file_type().is_file() && !m.file_type().is_symlink(),
                "state identity is not a regular file",
            )?;
            Ok(Some(read_json(path)?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error(format!("state identity: {e}"))),
    }
}
fn read_json(path: &Path) -> Result<J> {
    let meta = fs::symlink_metadata(path).map_err(|e| Error(format!("retained context: {e}")))?;
    require(
        meta.file_type().is_file() && !meta.file_type().is_symlink(),
        "retained context is not a regular file",
    )?;
    require(meta.len() <= MAX_FILE, "retained context exceeds limit")?;
    let mut file = File::open(path).map_err(|e| Error(format!("retained context: {e}")))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| Error(e.to_string()))?;
    serde_json::from_slice(&bytes).map_err(|e| Error(format!("invalid retained core context: {e}")))
}
fn atomic_create(path: &Path, value: &J) -> Result<J> {
    let parent = path
        .parent()
        .ok_or_else(|| Error("invalid state path".into()))?;
    let bytes = serde_json::to_vec(value)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".pending-{nonce}-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| Error(format!("temporary state: {e}")))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| Error(format!("temporary state: {e}")))?;
    drop(file);
    if fs::hard_link(&temporary, path).is_err() { /* identical concurrent writers are allowed */ }
    let _ = fs::remove_file(&temporary);
    read_json(path)
}
