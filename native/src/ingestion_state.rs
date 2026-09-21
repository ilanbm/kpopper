//! Private durable state for asynchronous ingestion.
use crate::{Result, history_contract::error, history_transaction_fs as F, require};
#[cfg(any(unix, windows))]
use fs2::FileExt;
use serde_json::{Value as J, json};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub const TERMINAL: &[&str] = &[
    "applied",
    "project_captured",
    "needs_primary",
    "superseded",
    "error",
];
pub const ACTIVE: &[&str] = &["captured", "processing", "recovery_required"];
pub const AUTO_STATES: &[&str] = &["captured", "processing"];
pub const DIRECTORIES: &[&str] = &[
    "envelopes",
    "sources",
    "events",
    "journals",
    "drafts",
    "results",
    "signals",
    "receipts",
    "delivery-jobs",
    "delivery",
    "watchers",
];

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub fn resolve_root(record: &Path, selected: Option<&Path>) -> Result<PathBuf> {
    resolve_root_from(record, selected, &std::env::current_dir()?)
}

pub fn resolve_root_from(record: &Path, selected: Option<&Path>, cwd: &Path) -> Result<PathBuf> {
    let record = record.canonicalize()?;
    let root = if let Some(path) = selected {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }
    } else {
        let base = std::env::var_os("XDG_STATE_HOME")
            .filter(|path| Path::new(path).is_absolute())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|path| PathBuf::from(path).join(".local/state"))
            })
            .ok_or_else(|| error("state home is unavailable"))?;
        base.join("kpopper/ingestion")
            .join(crate::identity::sha256(record.to_string_lossy().as_bytes()))
    };
    let root = crate::source_inventory::absolute(&root)?;
    require(
        !record.starts_with(&root),
        "state directory cannot contain the provenance record",
    )?;
    Ok(root)
}

pub(crate) fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub struct Layout {
    pub record: PathBuf,
    pub root: PathBuf,
}

pub struct FileLock {
    #[cfg(any(unix, windows))]
    file: File,
}
impl FileLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        private_directory(path.parent().ok_or_else(|| error("invalid_path"))?)?;
        #[cfg(any(unix, windows))]
        {
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            file.lock_exclusive()?;
            Ok(Self { file })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = path;
            Err(error("durable ingestion writes require file locking"))
        }
    }
}
#[cfg(any(unix, windows))]
impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl Layout {
    pub fn create(record: &Path, state_dir: Option<&Path>) -> Result<Self> {
        Self::create_from(record, state_dir, &std::env::current_dir()?)
    }
    pub fn create_from(record: &Path, state_dir: Option<&Path>, cwd: &Path) -> Result<Self> {
        let record = record.canonicalize()?;
        let root = resolve_root_from(&record, state_dir, cwd)?;
        require(
            !root.exists() || root.is_dir(),
            "state path exists and is not a directory",
        )?;
        let expected = json!({"record":record});
        if root.exists() {
            match read_json(&root.join("record.json"))? {
                Some(actual) => require(
                    actual == expected,
                    "state directory belongs to another provenance record",
                )?,
                None => require(
                    fs::read_dir(&root)?.next().is_none(),
                    "existing non-empty state directory is not owned by ingestion",
                )?,
            }
        }
        private_directory(&root)?;
        let root = root.canonicalize()?;
        let _lock = FileLock::acquire(&root.join("state.lock"))?;
        let marker = root.join("record.json");
        match read_json(&marker)? {
            Some(actual) => require(
                actual == expected,
                "state directory belongs to another provenance record",
            )?,
            None => {
                require(
                    fs::read_dir(&root)?
                        .all(|entry| entry.is_ok_and(|entry| entry.file_name() == "state.lock")),
                    "existing non-empty state directory is not owned by ingestion",
                )?;
                save_json(&marker, &expected)?;
            }
        }
        for directory in DIRECTORIES {
            private_directory(&root.join(directory))?;
        }
        Ok(Self { record, root })
    }

    pub fn existing(record: &Path, state_dir: Option<&Path>) -> Result<Option<Self>> {
        Self::existing_from(record, state_dir, &std::env::current_dir()?)
    }
    pub fn existing_from(record: &Path, state_dir: Option<&Path>, cwd: &Path) -> Result<Option<Self>> {
        let record = record.canonicalize()?;
        let root = resolve_root_from(&record, state_dir, cwd)?;
        let root = if root.exists() {
            root.canonicalize()?
        } else {
            root
        };
        let Some(marker) = read_json(&root.join("record.json"))? else {
            return Ok(None);
        };
        require(
            marker == json!({"record":record}),
            "state directory belongs to another provenance record",
        )?;
        Ok(Some(Self { record, root }))
    }

    pub fn path(&self, kind: &str, id: &str) -> PathBuf {
        self.root.join(kind).join(format!("{id}.json"))
    }
    pub fn source_path(&self, id: &str) -> PathBuf {
        self.root.join("sources").join(format!("{id}.txt"))
    }
}

pub fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    F::read(path)
}
pub fn read_json(path: &Path) -> Result<Option<J>> {
    read(path)?
        .map(|raw| {
            crate::json_ingress::parse_slice(&raw, crate::json_ingress::DuplicateKeys::Reject)
                .map_err(|_| error("invalid ingestion state"))
        })
        .transpose()
}
pub fn save_json(path: &Path, value: &J) -> Result<()> {
    let mut raw = serde_json::to_vec(value)?;
    raw.push(b'\n');
    F::replace(path, Some(&raw))
}
pub fn save_bytes(path: &Path, value: &[u8]) -> Result<()> {
    F::replace(path, Some(value))
}
pub fn remove(path: &Path) -> Result<()> {
    F::remove(path)
}

pub fn summary(event: &J) -> J {
    let mut out = serde_json::Map::new();
    for key in [
        "event_id",
        "state",
        "target",
        "captured_at",
        "finished_at",
        "reason",
        "record",
        "state_dir",
    ] {
        out.insert(key.into(), event.get(key).cloned().unwrap_or(J::Null));
    }
    J::Object(out)
}

pub fn canonical_json(value: &J) -> Result<Vec<u8>> {
    let mut raw = serde_json::to_vec(value)?;
    raw.push(b'\n');
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_state_dir_uses_supplied_workspace_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let record = workspace.join("GROUNDING.yaml");
        std::fs::write(&record, "known: {p.a: {v: 1}}\n").unwrap();
        let root = resolve_root_from(&record, Some(Path::new("relstate")), &workspace).unwrap();
        assert_eq!(root, workspace.join("relstate"));
        assert_ne!(root, elsewhere.join("relstate"));
    }
}
