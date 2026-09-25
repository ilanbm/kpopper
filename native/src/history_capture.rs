//! Read-only capture of exact on-disk history membership and bytes.
use crate::{
    Result,
    history_authority::{self as A, Files, Generation, ObjectBytes, ObjectPaths},
    history_contract::*,
    history_paths, history_reduce, history_yaml,
    identity::sha256,
    require,
    source_clock::Ancestry,
    value::{Integer, TypedValue as V},
};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBJECT_BYTES: usize = 1024 * 1024;
fn s(v: &str) -> V {
    V::Text(v.into())
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub entry: String,
    pub authority: String,
    pub commits: String,
    pub objects: String,
    pub cancellations: String,
    pub journal: String,
}
impl Layout {
    pub fn for_entry(entry: &str) -> Result<Self> {
        A::relative_path(entry)?;
        require(!entry.contains('/'), "invalid_path")?;
        let modern = entry == "GROUNDING.yaml";
        let prefix = if modern {
            ".kpopper/history"
        } else {
            "PROVENANCE.history"
        };
        Ok(Self {
            entry: entry.into(),
            authority: format!("{prefix}.yaml"),
            commits: format!("{prefix}-commits"),
            objects: prefix.into(),
            cancellations: format!("{prefix}-cancellations"),
            journal: format!(
                "{}.history-local/{}.json",
                if modern { ".kpopper/" } else { "" },
                &sha256(entry.as_bytes())[..24]
            ),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    Exists(bool),
    Directory(bool),
    Bytes { sha256: String, maximum: usize },
    Members { names: Vec<String>, hidden: bool },
}
#[derive(Clone, Debug)]
pub struct Capture {
    pub root: PathBuf,
    pub layout: Layout,
    pub entry_bytes: Vec<u8>,
    pub document: V,
    pub view_alternatives: Vec<crate::history_view::Alternative>,
    pub authority_bytes: Vec<u8>,
    pub marker: V,
    pub commits: Files,
    pub object_bytes: ObjectBytes,
    pub objects: Map,
    pub state: V,
    pub baseline: V,
    pub inactive_generations: BTreeMap<String, Generation>,
    pub cancellation_bytes: Files,
    pub storage_bytes: Files,
    pub object_paths: ObjectPaths,
    pub inventory: BTreeMap<(String, String), Observation>,
}
impl Capture {
    pub fn verify_current(&self) -> Result<()> {
        let mut reader = Reader::open(&self.root)?;
        reader.journal_guard(&self.layout.journal)?;
        reader.inventory = self.inventory.clone();
        reader.verify()?;
        reader.journal_guard(&self.layout.journal)
    }
    /// Portable detached evidence; raw bytes remain available on the capture.
    pub fn evidence(&self) -> V {
        let hashes = |files: &Files| {
            V::Map(
                files
                    .iter()
                    .map(|(k, v)| (k.clone(), s(&sha256(v))))
                    .collect(),
            )
        };
        let object_bytes = self
            .object_bytes
            .iter()
            .map(|((subject, id), raw)| V::List(vec![s(subject), s(id), s(&sha256(raw))]))
            .collect();
        let paths = self
            .object_paths
            .iter()
            .map(|((subject, id), path)| V::List(vec![s(subject), s(id), s(path)]))
            .collect();
        V::Map(Map::from([
            ("entry_sha256".into(), s(&sha256(&self.entry_bytes))),
            ("authority_sha256".into(), s(&sha256(&self.authority_bytes))),
            ("document".into(), self.document.clone()),
            ("authority".into(), self.marker.clone()),
            ("baseline".into(), self.baseline.clone()),
            ("state".into(), self.state.clone()),
            ("objects".into(), V::Map(self.objects.clone())),
            ("commits".into(), hashes(&self.commits)),
            ("object_bytes".into(), V::List(object_bytes)),
            ("object_paths".into(), V::List(paths)),
            ("storage".into(), hashes(&self.storage_bytes)),
            ("cancellations".into(), hashes(&self.cancellation_bytes)),
            (
                "inactive".into(),
                V::Map(
                    self.inactive_generations
                        .iter()
                        .map(|(k, g)| (k.clone(), g.evidence()))
                        .collect(),
                ),
            ),
        ]))
    }
}
struct Reader {
    root: PathBuf,
    total: usize,
    inventory: BTreeMap<(String, String), Observation>,
    // The directory flock is the existing POSIX reader/writer coordination boundary.
    _lock: crate::history_transaction_fs::DirectoryGuard,
}
impl Reader {
    fn open(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        require(root.is_dir(), "invalid_path")?;
        let lock = crate::history_transaction_fs::DirectoryGuard::acquire(&root, false)?;
        Ok(Self {
            root,
            total: 0,
            inventory: BTreeMap::new(),
            _lock: lock,
        })
    }
    fn target(&self, relative: &str) -> Result<PathBuf> {
        A::relative_path(relative)?;
        let mut path = self.root.clone();
        for component in relative.split('/') {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Ok(meta) => require(!meta.file_type().is_symlink(), "symlink_path")?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(path)
    }
    fn event(&mut self, kind: &str, path: &str, value: Observation) -> Result<()> {
        let key = (kind.into(), path.into());
        if let Some(old) = self.inventory.get(&key) {
            require(*old == value, "snapshot_changed")?;
        }
        self.inventory.insert(key, value);
        Ok(())
    }
    fn exists(&mut self, path: &str) -> Result<bool> {
        let exists = self.target(path)?.exists();
        self.event("exists", path, Observation::Exists(exists))?;
        Ok(exists)
    }
    fn read(&mut self, relative: &str, maximum: usize) -> Result<Vec<u8>> {
        let path = self.target(relative)?;
        require(
            self.exists(relative)? && path.is_file(),
            "missing_history_file",
        )?;
        let raw = bounded_read(&path, maximum)?;
        self.total += raw.len();
        require(self.total <= MAX_CAPTURE_BYTES, "history_limit")?;
        self.event(
            "bytes",
            relative,
            Observation::Bytes {
                sha256: sha256(&raw),
                maximum,
            },
        )?;
        Ok(raw)
    }
    fn listing(&mut self, relative: &str, hidden: bool) -> Result<Vec<String>> {
        let path = self.target(relative)?;
        if !hidden {
            self.event("directory", relative, Observation::Directory(path.is_dir()))?;
            require(
                !self.exists(relative)? || path.is_dir(),
                "invalid_history_path",
            )?;
        }
        let names = directory_names(&path, hidden)?;
        self.event(
            if hidden { "hidden" } else { "glob" },
            relative,
            Observation::Members {
                names: names.clone(),
                hidden,
            },
        )?;
        for name in &names {
            self.target(&format!("{relative}/{name}"))?;
        }
        Ok(names)
    }
    fn journal_guard(&self, path: &str) -> Result<()> {
        crate::history_transaction_fs::check_reader_journal(&self.root, path)
    }
    fn verify(&self) -> Result<()> {
        for ((_, relative), expected) in &self.inventory {
            let path = self.target(relative)?;
            let actual = match expected {
                Observation::Exists(_) => Observation::Exists(path.exists()),
                Observation::Directory(_) => Observation::Directory(path.is_dir()),
                Observation::Bytes { maximum, .. } => Observation::Bytes {
                    sha256: sha256(
                        &bounded_read(&path, *maximum).map_err(|_| error("snapshot_changed"))?,
                    ),
                    maximum: *maximum,
                },
                Observation::Members { hidden, .. } => Observation::Members {
                    names: directory_names(&path, *hidden)
                        .map_err(|_| error("snapshot_changed"))?,
                    hidden: *hidden,
                },
            };
            require(actual == *expected, "snapshot_changed")?;
        }
        Ok(())
    }
}
fn bounded_read(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    require(path.is_file(), "missing_history_file")?;
    require(path.metadata()?.len() <= maximum as u64, "history_limit")?;
    let mut raw = Vec::new();
    File::open(path)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut raw)?;
    require(raw.len() <= maximum, "history_limit")?;
    Ok(raw)
}
fn directory_names(path: &Path, hidden: bool) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    require(path.is_dir(), "invalid_history_path")?;
    let mut names = Vec::new();
    for member in fs::read_dir(path)? {
        let name = member?
            .file_name()
            .into_string()
            .map_err(|_| error("invalid_path"))?;
        if name.starts_with('.') != hidden {
            continue;
        }
        names.push(name);
        require(names.len() <= MAX_OBJECTS, "history_limit")?;
    }
    names.sort();
    Ok(names)
}
pub fn baseline(marker: &V, commits: &Files, state: &V) -> Result<V> {
    let marker = map(marker)?;
    let subjects = map(&map(state)?["subjects"])?;
    let mut heads = Map::new();
    let mut open_acts = Map::new();
    for (subject, entry) in subjects {
        let entry = map(entry)?;
        heads.insert(subject.clone(), entry["heads"].clone());
        let mut acts = std::collections::BTreeSet::new();
        for ids in map(&entry["open_acts"])?.values() {
            let V::List(ids) = ids else {
                return Err(error("invalid_baseline"));
            };
            for id in ids {
                acts.insert(text(id)?.to_owned());
            }
        }
        open_acts.insert(
            subject.clone(),
            V::List(acts.iter().map(|id| s(id)).collect()),
        );
    }
    let value = V::Map(Map::from([
        ("version".into(), V::Integer(Integer::new("1")?)),
        ("record_id".into(), marker["record_id"].clone()),
        ("authority_generation".into(), marker["generation"].clone()),
        (
            "committed_set_digest".into(),
            s(&A::committed_set_digest(commits)?),
        ),
        ("heads".into(), V::Map(heads)),
        ("open_acts".into(), V::Map(open_acts)),
    ]));
    A::validate_baseline(&value)?;
    Ok(value)
}
/// Public semantic evidence follows the record's verified storage authority.
/// Existing legacy captures retain their original envelope.
pub fn evidence_for_entry(entry: &Path) -> Result<V> {
    if crate::history_node_publication::selected(entry)? {
        return crate::history_node_capture::public_evidence(entry);
    }
    let captured = capture(entry, None, None)?;
    let evidence = captured.evidence();
    captured.verify_current()?;
    Ok(evidence)
}

pub fn capture(
    entry: &Path,
    rules: Option<&Map>,
    ancestry: Option<&Ancestry<'_>>,
) -> Result<Capture> {
    capture_options(entry, rules, ancestry, false)
}
pub fn capture_reconciliation(entry: &Path, allow_conflicts: bool) -> Result<Capture> {
    capture_options(entry, None, None, allow_conflicts)
}
fn capture_options(
    entry: &Path,
    rules: Option<&Map>,
    ancestry: Option<&Ancestry<'_>>,
    allow_conflicts: bool,
) -> Result<Capture> {
    let absolute = if entry.is_absolute() {
        entry.to_owned()
    } else {
        std::env::current_dir()?.join(entry)
    };
    let name = absolute
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| error("invalid_path"))?;
    let layout = Layout::for_entry(name)?;
    let mut reader = Reader::open(absolute.parent().ok_or_else(|| error("invalid_path"))?)?;
    reader.journal_guard(&layout.journal)?;
    let entry_bytes = reader.read(&layout.entry, MAX_REQUEST_BYTES)?;
    require(reader.exists(&layout.authority)?, "history_not_active")?;
    let authority_bytes = reader.read(&layout.authority, MAX_OBJECT_BYTES)?;
    let marker = history_yaml::decode_document(&authority_bytes)?;
    A::validate_authority(&marker)?;
    require(
        string_is(&map(&marker)?["authority"], "history"),
        "history_not_active",
    )?;
    let alternatives = if allow_conflicts {
        crate::history_view::alternatives(&entry_bytes)?
    } else {
        vec![crate::history_view::Alternative {
            name: "view".into(),
            bytes: entry_bytes.clone(),
            document: history_yaml::decode_document(&entry_bytes)?,
        }]
    };
    let document = alternatives[0].document.clone();
    for alternative in &alternatives {
        let supplied = map(&alternative.document)?
            .get("meta")
            .and_then(|m| map(m).ok())
            .and_then(|m| m.get("history"))
            .ok_or_else(|| error("baseline_mismatch"))?;
        A::bind_authority(&marker, supplied)?;
    }
    let supplied = map(&document)?
        .get("meta")
        .and_then(|m| map(m).ok())
        .and_then(|m| m.get("history"))
        .ok_or_else(|| error("baseline_mismatch"))?;
    A::bind_authority(&marker, supplied)?;
    let mut commits = Files::new();
    let mut storage = Files::new();
    for name in reader.listing(&layout.commits, false)? {
        let operation = name
            .strip_suffix(".yaml")
            .ok_or_else(|| error("invalid_history_path"))?;
        token(&s(operation))?;
        commits.insert(
            operation.into(),
            reader.read(&format!("{}/{name}", layout.commits), MAX_REQUEST_BYTES)?,
        );
    }
    for directory in reader.listing(&layout.objects, false)? {
        let directory_path = format!("{}/{directory}", layout.objects);
        require(
            reader.target(&directory_path)?.is_dir(),
            "invalid_history_path",
        )?;
        history_paths::parse_object_path(&format!("{directory}/{}.yaml", "a".repeat(40)))
            .map_err(|_| error("invalid_history_path"))?;
        for name in reader.listing(&directory_path, false)? {
            let relative = format!("{directory}/{name}");
            history_paths::parse_object_path(&relative)?;
            require(storage.len() < MAX_OBJECTS, "history_limit")?;
            storage.insert(
                relative,
                reader.read(&format!("{directory_path}/{name}"), MAX_OBJECT_BYTES)?,
            );
        }
    }
    let (object_bytes, object_paths) = A::objects_from_storage(&commits, &storage)?;
    let mut cancellations = Files::new();
    let members = reader.listing(&layout.cancellations, false)?;
    require(
        reader.listing(&layout.cancellations, true)?.is_empty(),
        "cancellation_membership_mismatch",
    )?;
    for name in members {
        require(name.ends_with(".yaml"), "invalid_history_path")?;
        cancellations.insert(
            name.clone(),
            reader.read(
                &format!("{}/{name}", layout.cancellations),
                MAX_REQUEST_BYTES,
            )?,
        );
    }
    let mut generations =
        A::committed_generations(&marker, &commits, &object_bytes, &cancellations)?;
    let generation = if let V::Integer(n) = &map(&marker)?["generation"] {
        n.as_str()
    } else {
        unreachable!("generation")
    };
    let active = generations.remove(generation);
    let (commits, objects, selected_bytes) = match active {
        Some(g) => (g.commits, g.objects, g.object_bytes),
        None => (Files::new(), Map::new(), ObjectBytes::new()),
    };
    for alternative in &alternatives {
        let supplied = &map(&map(&alternative.document)?["meta"])?["history"];
        for (role, act) in [("heads", false), ("open_acts", true)] {
            for (subject, versions) in map(&map(supplied)?[role])? {
                let V::List(versions) = versions else {
                    unreachable!("baseline")
                };
                for version in versions {
                    let obj = objects
                        .get(text(version)?)
                        .ok_or_else(|| error("incomplete_view_baseline"))?;
                    let obj = map(obj)?;
                    require(
                        string_is(&obj["subject"], subject)
                            && string_is(&obj["kind"], "act") == act,
                        "baseline_reference_mismatch",
                    )?;
                }
            }
        }
    }
    let state = history_reduce::reduce_bytes(&selected_bytes, rules, ancestry)?;
    let baseline = baseline(&marker, &commits, &state)?;
    reader.verify()?;
    reader.journal_guard(&layout.journal)?;
    Ok(Capture {
        root: reader.root.clone(),
        layout,
        entry_bytes,
        document,
        view_alternatives: alternatives,
        authority_bytes,
        marker,
        commits,
        object_bytes,
        objects,
        state,
        baseline,
        inactive_generations: generations,
        cancellation_bytes: cancellations,
        storage_bytes: storage,
        object_paths,
        inventory: reader.inventory.clone(),
    })
}
