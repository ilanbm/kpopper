//! Exact, bounded observations of a source closure, independent of YAML meaning.
use crate::{Result, history_contract::error, history_sources, identity::sha256, require};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

pub(crate) const MAX_FILE: usize = 16 * 1024 * 1024;
const MAX_TOTAL: usize = 64 * 1024 * 1024;
const MAX_MEMBERS: usize = 100_000;

pub(crate) fn absolute(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let mut result = PathBuf::new();
    for part in path.components() {
        if part == Component::ParentDir {
            result.pop();
        } else {
            result.push(part);
        }
    }
    Ok(result)
}
pub(crate) fn name(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| error("invalid_path"))
}
fn magic(text: &str) -> bool {
    text.contains(['*', '?', '['])
}
pub(crate) fn escaped(path: &Path) -> Result<PathBuf> {
    let mut out = String::new();
    for c in name(path)?.chars() {
        if "[*?".contains(c) {
            out.push('[');
            out.push(c);
            out.push(']');
        } else {
            out.push(c);
        }
    }
    Ok(PathBuf::from(out))
}
/// Python glob semantics: component-local patterns, sorted results and explicit
/// leading dots. Pointer strings are literal; only caller patterns use this.
pub(crate) fn glob(pattern: &Path) -> Result<Vec<PathBuf>> {
    let pattern = absolute(pattern)?;
    if !magic(name(&pattern)?) {
        return Ok(if pattern.symlink_metadata().is_ok() {
            vec![pattern]
        } else {
            vec![]
        });
    }
    let mut paths = vec![PathBuf::new()];
    let mut visits = 0;
    for component in pattern.components() {
        let Component::Normal(part) = component else {
            for path in &mut paths {
                path.push(component);
            }
            continue;
        };
        let part = part.to_str().ok_or_else(|| error("invalid_path"))?;
        let mut next = Vec::new();
        for parent in paths {
            if !magic(part) {
                let path = parent.join(part);
                if path.symlink_metadata().is_ok() {
                    next.push(path);
                }
            } else if parent.is_dir() {
                for item in fs::read_dir(&parent)? {
                    let item = item?;
                    visits += 1;
                    require(visits <= MAX_MEMBERS, "source_limit")?;
                    let filename = item.file_name();
                    let filename = filename.to_str().ok_or_else(|| error("invalid_path"))?;
                    if (!filename.starts_with('.') || part.starts_with('.'))
                        && history_sources::matches(filename, part)?
                    {
                        next.push(parent.join(filename));
                    }
                }
            }
            require(next.len() <= MAX_MEMBERS, "source_limit")?;
        }
        paths = next;
    }
    paths.sort();
    Ok(paths)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Observation {
    Bytes(String),
    Unreadable(String),
    Exists(bool),
    Directory(bool),
    File(bool),
    Glob(Vec<PathBuf>),
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Inventory {
    pub events: BTreeMap<(String, PathBuf), Observation>,
    pub files: BTreeMap<PathBuf, Vec<u8>>,
    total: usize,
}
fn failure(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::IsADirectory => "IsADirectoryError",
        std::io::ErrorKind::NotADirectory => "NotADirectoryError",
        _ => "OSError",
    }
}
fn bytes(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut raw = Vec::new();
    file.take((MAX_FILE + 1) as u64).read_to_end(&mut raw)?;
    Ok(raw)
}
impl Inventory {
    pub fn event(&mut self, kind: &str, path: &Path, value: Observation) -> Result<()> {
        let key = (kind.into(), absolute(path)?);
        require(
            self.events.get(&key).is_none_or(|old| *old == value),
            "snapshot_changed",
        )?;
        self.events.insert(key, value);
        require(self.events.len() <= MAX_MEMBERS, "source_limit")
    }
    pub fn exists(&mut self, path: &Path) -> Result<bool> {
        let value = path.exists();
        self.event("exists", path, Observation::Exists(value))?;
        Ok(value)
    }
    pub fn file(&mut self, path: &Path) -> Result<bool> {
        let value = path.is_file();
        self.event("file", path, Observation::File(value))?;
        Ok(value)
    }
    pub fn directory(&mut self, path: &Path) -> Result<bool> {
        let value = path.is_dir();
        self.event("directory", path, Observation::Directory(value))?;
        Ok(value)
    }
    pub fn glob(&mut self, path: &Path) -> Result<Vec<PathBuf>> {
        let paths = glob(path)?;
        self.event("glob", path, Observation::Glob(paths.clone()))?;
        Ok(paths)
    }
    pub fn retain(&mut self, path: &Path, raw: Vec<u8>) -> Result<()> {
        require(raw.len() <= MAX_FILE, "source_limit")?;
        let path = absolute(path)?;
        self.event("bytes", &path, Observation::Bytes(sha256(&raw)))?;
        if !self.files.contains_key(&path) {
            self.total = self.total.saturating_add(raw.len());
        }
        require(self.total <= MAX_TOTAL, "source_limit")?;
        self.files.insert(path, raw);
        Ok(())
    }
    pub fn read(&mut self, path: &Path) -> Result<Vec<u8>> {
        match bytes(path) {
            Ok(raw) => {
                self.retain(path, raw.clone())?;
                Ok(raw)
            }
            Err(e) => {
                self.event(
                    "unreadable",
                    path,
                    Observation::Unreadable(failure(&e).into()),
                )?;
                Err(error(failure(&e)))
            }
        }
    }
    /// Bind administration bytes without retaining them in a portable source copy.
    pub fn observe_bytes(&mut self, path: &Path) -> Result<()> {
        let raw = bytes(path)?;
        require(raw.len() <= MAX_FILE, "source_limit")?;
        self.event("bytes", path, Observation::Bytes(sha256(&raw)))
    }
    pub fn verify(&self) -> Result<()> {
        for ((_, path), expected) in &self.events {
            let actual = match expected {
                Observation::Bytes(_) => {
                    let raw = bytes(path).map_err(|_| error("snapshot_changed"))?;
                    require(raw.len() <= MAX_FILE, "snapshot_changed")?;
                    Observation::Bytes(sha256(&raw))
                }
                Observation::Unreadable(_) => match bytes(path) {
                    Ok(_) => return Err(error("snapshot_changed")),
                    Err(e) => Observation::Unreadable(failure(&e).into()),
                },
                Observation::Exists(_) => Observation::Exists(path.exists()),
                Observation::Directory(_) => Observation::Directory(path.is_dir()),
                Observation::File(_) => Observation::File(path.is_file()),
                Observation::Glob(_) => Observation::Glob(glob(path)?),
            };
            require(actual == *expected, "snapshot_changed")?;
        }
        Ok(())
    }
}
