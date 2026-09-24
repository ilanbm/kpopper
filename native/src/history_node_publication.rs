//! Experimental append publication for node-history frames. No public command routes here.
//! Semantic admission remains the caller's verifier; this module owns only exact byte publication.
use crate::{
    Error, Result, history_node_codec as C, history_transaction_fs as F, identity::sha256, require,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

const FORMAT: &str = "node-publication-experiment/v1";
const JOURNAL: &str = ".kpopper/.history-node-publication.json";
const VIEW: &str = "GROUNDING.yaml";
const MAX_BYTES: usize = 64 * 1024 * 1024;
fn bad(code: &str) -> Error {
    Error(code.into())
}
fn token(s: &str) -> Result<()> {
    crate::history_contract::token(&crate::value::TypedValue::Text(s.into()))
}
fn read(root: &Path, relative: &str) -> Result<Option<Vec<u8>>> {
    F::read(&F::target(root, relative)?)
}
fn digest(raw: &Option<Vec<u8>>) -> Option<String> {
    raw.as_ref().map(|v| sha256(v))
}
fn stream(subject: &str) -> Result<String> {
    Ok(format!(".kpopper/history/{}", C::subject_path(subject)?))
}
fn commit(op: &str) -> Result<String> {
    token(op)?;
    Ok(format!(".kpopper/history-commits/{op}.json"))
}
fn bytes(raw: &Option<Vec<u8>>) -> Option<String> {
    raw.as_ref().map(|v| STANDARD.encode(v))
}
fn unbytes(s: &Option<String>) -> Result<Option<Vec<u8>>> {
    s.as_ref()
        .map(|s| {
            require(s.len() <= MAX_BYTES * 2, "node_publication_limit")?;
            let raw = STANDARD
                .decode(s)
                .map_err(|_| bad("node_publication_base64"))?;
            require(
                raw.len() <= MAX_BYTES && STANDARD.encode(&raw) == *s,
                "node_publication_base64",
            )?;
            Ok(raw)
        })
        .transpose()
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Touch {
    subject: String,
    event: String,
    frame_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    operation: String,
    authority_sha256: String,
    parents: BTreeMap<String, String>,
    touched: Vec<Touch>,
    before_view: Option<String>,
    after_view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    evidence: BTreeMap<String, String>,
    digest: String,
}
impl Manifest {
    fn identity(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.digest.clear();
        Ok(sha256(&serde_json::to_vec(&copy)?))
    }
    fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }
    fn decode(raw: &[u8]) -> Result<Self> {
        require(raw.len() <= MAX_BYTES, "node_publication_limit")?;
        let m: Self = serde_json::from_slice(raw)?;
        require(
            m.format == FORMAT && m.digest == m.identity()? && m.encode()? == raw,
            "node_publication_manifest",
        )?;
        token(&m.operation)?;
        if m.after_view.is_none() {
            let context = m
                .context
                .as_ref()
                .ok_or_else(|| bad("node_publication_view"))?;
            let context = crate::value::TypedValue::from_tagged(context)?;
            require(
                crate::history_node_legacy::kind(&context, crate::history_node_legacy::FORMAT),
                "node_publication_view",
            )?;
        }
        for (path, hash) in &m.evidence {
            evidence_path(path)?;
            require(
                crate::history_paths::object_id(hash),
                "node_publication_evidence",
            )?;
        }
        for parent in m.parents.keys() {
            token(parent)?;
        }
        Ok(m)
    }
}
/// Build a derived legacy transcript or its final materialized copy checkpoint.
/// Historical legacy transactions have no invented node-format current-view bytes.
pub(crate) fn migration_manifest(
    authority_sha: &str,
    operation: &str,
    parents: BTreeMap<String, String>,
    touches: Vec<(String, String, String)>,
    after_view: Option<String>,
    context: &crate::value::TypedValue,
    evidence: BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut manifest = Manifest {
        format: FORMAT.into(),
        operation: operation.into(),
        authority_sha256: authority_sha.into(),
        parents,
        touched: touches
            .into_iter()
            .map(|(subject, event, frame_sha256)| Touch {
                subject,
                event,
                frame_sha256,
            })
            .collect(),
        before_view: None,
        after_view,
        context: Some(context.to_tagged()?),
        evidence,
        digest: String::new(),
    };
    manifest.digest = manifest.identity()?;
    let raw = manifest.encode()?;
    Manifest::decode(&raw)?;
    Ok(raw)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Append {
    subject: String,
    offset: u64,
    prefix_sha256: Option<String>,
    frames: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceFile {
    path: String,
    raw: String,
    existed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format: String,
    manifest: Manifest,
    before_view: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_before: Option<String>,
    after_view: String,
    appends: Vec<Append>,
    retained: Vec<Touch>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    imports: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_parents: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    evidence: Vec<EvidenceFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guard: Option<serde_json::Value>,
    digest: String,
}
impl Journal {
    fn identity(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.digest.clear();
        Ok(sha256(&serde_json::to_vec(&copy)?))
    }
    fn encode(&self) -> Result<Vec<u8>> {
        let raw = serde_json::to_vec(self)?;
        require(raw.len() <= MAX_BYTES, "node_publication_limit")?;
        Ok(raw)
    }
    fn validate(&self) -> Result<()> {
        require(
            self.format == FORMAT && self.digest == self.identity()?,
            "node_publication_journal",
        )?;
        Manifest::decode(&self.manifest.encode()?)?;
        require(
            self.imports.is_empty() || self.target_parents.is_some(),
            "node_import_frontier",
        )?;
        for (op, raw) in &self.imports {
            let m = Manifest::decode(&unbytes(&Some(raw.clone()))?.unwrap())?;
            require(
                m.operation == *op
                    && op != &self.manifest.operation
                    && m.authority_sha256 == self.manifest.authority_sha256,
                "node_import_manifest",
            )?;
        }
        let before = unbytes(&self.before_view)?;
        let after = unbytes(&Some(self.after_view.clone()))?.unwrap();
        require(
            digest(&before) == self.manifest.before_view
                && Some(sha256(&after)) == self.manifest.after_view,
            "node_publication_view",
        )?;
        let mut evidence = BTreeMap::new();
        for file in &self.evidence {
            evidence_path(&file.path)?;
            let raw = unbytes(&Some(file.raw.clone()))?.unwrap();
            require(
                evidence.insert(file.path.clone(), sha256(&raw)).is_none(),
                "node_publication_evidence",
            )?;
        }
        require(
            evidence == self.manifest.evidence,
            "node_publication_evidence",
        )?;
        if self.canonical_before.is_some() {
            let path = format!("evidence/view-edits/{}.yaml", self.manifest.operation);
            require(
                before.is_some() && evidence.get(&path) == self.manifest.before_view.as_ref(),
                "node_edit_evidence_mismatch",
            )?;
        }
        let mut subjects = BTreeSet::new();
        let mut touches = Vec::new();
        for append in &self.appends {
            require(
                subjects.insert(append.subject.clone())
                    && append.offset <= MAX_BYTES as u64
                    && (append.prefix_sha256.is_some() || append.offset == 0),
                "node_publication_append",
            )?;
            let frames = unbytes(&Some(append.frames.clone()))?.unwrap();
            require(!frames.is_empty(), "node_publication_append")?;
            for raw in frames.split_inclusive(|v| *v == b'\n') {
                let event = C::Event::decode(raw, &append.subject)?;
                touches.push(Touch {
                    subject: append.subject.clone(),
                    event: event.id().into(),
                    frame_sha256: sha256(raw),
                });
            }
        }
        let canonical = unbytes(&self.canonical_before)?;
        let before_origins = canonical
            .as_ref()
            .or(before.as_ref())
            .as_ref()
            .map(|raw| current_origins(raw))
            .transpose()?
            .unwrap_or_default();
        for ((subject, id), event) in current_origins(&after)? {
            if before_origins
                .get(&(subject.clone(), id))
                .is_some_and(|old| old.id() == event.id())
            {
                continue;
            }
            let touch = Touch {
                subject,
                event: event.id().into(),
                frame_sha256: sha256(&event.encode()?),
            };
            require(
                !touches.contains(&touch),
                "node_publication_duplicate_event",
            )?;
            touches.push(touch);
        }
        touches.retain(|t| !self.retained.contains(t));
        touches.sort_by(|a, b| (&a.subject, &a.event).cmp(&(&b.subject, &b.event)));
        require(touches == self.manifest.touched, "node_publication_touched")
    }
}
/// The prepared journal binds only touched stream prefixes/tails and current before/after bytes.
#[derive(Clone, Debug)]
pub struct Prepared {
    journal: Journal,
}
impl Prepared {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.journal.validate()?;
        self.journal.encode()
    }
    pub fn from_bytes(raw: &[u8]) -> Result<Self> {
        require(raw.len() <= MAX_BYTES, "node_publication_limit")?;
        let journal: Journal = serde_json::from_slice(raw)?;
        journal.validate()?;
        require(journal.encode()? == raw, "node_publication_journal")?;
        Ok(Self { journal })
    }
    pub(crate) fn canonical_before(&self) -> Result<Option<Vec<u8>>> {
        unbytes(&self.journal.canonical_before)
    }
    pub(crate) fn imports(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        self.journal
            .imports
            .iter()
            .map(|(op, raw)| Ok((op.clone(), unbytes(&Some(raw.clone()))?.unwrap())))
            .collect()
    }
    pub fn digest(&self) -> &str {
        &self.journal.manifest.digest
    }
    pub fn evidence(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        self.journal
            .evidence
            .iter()
            .map(|file| {
                Ok((
                    file.path.clone(),
                    unbytes(&Some(file.raw.clone()))?.unwrap(),
                ))
            })
            .collect()
    }
    pub(crate) fn with_guard(mut self, guard: &crate::value::TypedValue) -> Result<Self> {
        self.journal.guard = Some(guard.to_tagged()?);
        self.journal.digest = self.journal.identity()?;
        self.journal.validate()?;
        Ok(self)
    }
    pub(crate) fn guard(&self) -> Result<Option<crate::value::TypedValue>> {
        self.journal
            .guard
            .as_ref()
            .map(crate::value::TypedValue::from_tagged)
            .transpose()
    }
    pub fn context(&self) -> Result<Option<crate::value::TypedValue>> {
        self.journal
            .manifest
            .context
            .as_ref()
            .map(crate::value::TypedValue::from_tagged)
            .transpose()
    }
    pub fn operation(&self) -> &str {
        &self.journal.manifest.operation
    }
    pub fn before_view(&self) -> Result<Option<Vec<u8>>> {
        unbytes(&self.journal.before_view)
    }
    pub fn after_view(&self) -> Result<Vec<u8>> {
        Ok(unbytes(&Some(self.journal.after_view.clone()))?.unwrap())
    }
    /// Exact touched frames for semantic replay; retained originals are included.
    pub fn frames(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        self.journal
            .appends
            .iter()
            .map(|append| {
                Ok((
                    append.subject.clone(),
                    unbytes(&Some(append.frames.clone()))?.unwrap(),
                ))
            })
            .collect()
    }
}
/// Boundaries are observable for deterministic crash tests; callbacks cannot confer admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Import(usize),
    Journal,
    Append(usize),
    Evidence(usize),
    Commit,
    View,
}

/// Bind the readable view to its publication without embedding a heads map or history closure.
pub fn bind_view(raw: &[u8], operation: &str) -> Result<Vec<u8>> {
    token(operation)?;
    let mut document = crate::history_yaml::decode_document(raw)?;
    let crate::value::TypedValue::Map(ref mut document) = document else {
        return Err(bad("node_publication_view"));
    };
    let meta = document
        .entry("meta".into())
        .or_insert_with(|| crate::value::TypedValue::Map(BTreeMap::new()));
    let crate::value::TypedValue::Map(meta) = meta else {
        return Err(bad("node_publication_view"));
    };
    meta.insert(
        "node_publication".into(),
        crate::value::TypedValue::Text(operation.into()),
    );
    crate::history_yaml::encode_document(&crate::value::TypedValue::Map(document.clone()))
}
fn view_operation(raw: &[u8]) -> Result<Option<String>> {
    let document = crate::history_yaml::decode_document(raw)?;
    let Some(meta) = crate::history_contract::map(&document)?.get("meta") else {
        return Ok(None);
    };
    let Some(operation) = crate::history_contract::map(meta)?.get("node_publication") else {
        return Ok(None);
    };
    let operation = crate::history_contract::text(operation)?;
    token(operation)?;
    Ok(Some(operation.into()))
}
fn verify_view(all: &BTreeMap<String, Manifest>, raw: &[u8]) -> Result<()> {
    let operation = view_operation(raw)?;
    if all.is_empty() {
        return require(operation.is_none(), "node_publication_missing_manifest");
    }
    let operation = operation.ok_or_else(|| bad("node_publication_unbound_view"))?;
    let tips = frontier(all);
    require(
        tips.len() == 1 && tips.contains_key(&operation),
        "node_publication_view_frontier",
    )?;
    require(
        all[&operation].after_view.as_deref() == Some(sha256(raw).as_str()),
        "node_publication_view_mismatch",
    )
}
pub(crate) fn current_origins(raw: &[u8]) -> Result<BTreeMap<(String, String), C::Event>> {
    // Empty/legacy-shaped documents carry no lazy originals. A declared namespace is strict.
    let document = crate::history_yaml::decode_document(raw)?;
    let mut result = BTreeMap::new();
    let Some(meta) = crate::history_contract::map(&document)
        .ok()
        .and_then(|m| m.get("meta"))
    else {
        return Ok(result);
    };
    let Some(history) = crate::history_contract::map(meta)
        .ok()
        .and_then(|m| m.get("node_history"))
    else {
        return Ok(result);
    };
    let history = crate::history_contract::schema(history, &["version", "originals"], &["tails"])?;
    require(
        crate::history_contract::is_int(&history["version"], "1")
            || crate::history_contract::is_int(&history["version"], "2"),
        "node_publication_current_format",
    )?;
    let empty = BTreeMap::new();
    let tails = history
        .get("tails")
        .map(crate::history_contract::map)
        .transpose()?
        .unwrap_or(&empty);
    require(
        tails.is_empty() || crate::history_contract::is_int(&history["version"], "2"),
        "node_publication_current_format",
    )?;
    let originals = crate::history_contract::map(&history["originals"])?;
    require(
        tails.keys().all(|s| originals.contains_key(s)),
        "node_publication_current_tail",
    )?;
    for (subject, binding) in originals {
        let binding = crate::history_node_current::Original::decode(binding)?;
        require(
            binding.subject() == subject,
            "node_publication_current_subject",
        )?;
        let initial = binding.from_document(&document)?;
        let mut previous = initial.id().to_owned();
        let initial_version = initial.reconstruct(None)?;
        let mut stream = initial.encode()?;
        result.insert((subject.clone(), initial.id().into()), initial);
        if let Some(tail) = tails.get(subject) {
            let tail = unbytes(&Some(crate::history_contract::text(tail)?.into()))?.unwrap();
            require(!tail.is_empty(), "node_publication_current_tail")?;
            for frame in tail.split_inclusive(|b| *b == b'\n') {
                let event = C::Event::decode(frame, subject)?;
                require(
                    event.parents() == [previous.clone()],
                    "node_publication_current_tail_parent",
                )?;
                previous = event.id().into();
                require(
                    result
                        .insert((subject.clone(), event.id().into()), event)
                        .is_none(),
                    "node_publication_duplicate_event",
                )?;
            }
            stream.extend(tail);
        }
        let versions = C::decode_stream(&stream, subject)?;
        require(
            versions.values().all(|v| {
                v.operation() == initial_version.operation()
                    || v.state().is_some_and(|state| {
                        crate::history_node_frame::decode(state).is_ok_and(|payload| {
                            !payload.is_semantic
                                && crate::history_node_frame::validate_evidence(
                                    v, &versions, &payload,
                                )
                                .is_ok()
                        })
                    })
            }),
            "node_publication_current_tail_operation",
        )?;
    }
    Ok(result)
}
fn known_touches(all: &BTreeMap<String, Manifest>) -> Result<BTreeMap<String, Touch>> {
    let mut known = BTreeMap::new();
    for manifest in all.values() {
        for touch in &manifest.touched {
            require(
                known.insert(touch.event.clone(), touch.clone()).is_none(),
                "node_publication_duplicate_event",
            )?;
        }
    }
    Ok(known)
}
fn authority(root: &Path) -> Result<String> {
    let raw = read(root, ".kpopper/history.yaml")?
        .ok_or_else(|| bad("node_publication_authority_required"))?;
    let value = crate::history_yaml::decode_document(&raw)?;
    validate_authority(&value)?;
    Ok(sha256(&raw))
}
pub fn validate_authority(value: &crate::value::TypedValue) -> Result<()> {
    let value = crate::history_contract::schema(
        value,
        &[
            "version",
            "profile",
            "authority",
            "record_id",
            "generation",
            "requires",
        ],
        &[],
    )?;
    let text = crate::value::TypedValue::Text;
    require(
        crate::history_contract::is_int(&value["version"], "3")
            && value["profile"] == text(C::FORMAT.into())
            && value["authority"] == text("history".into())
            && value["requires"] == crate::value::TypedValue::List(vec![text(C::FORMAT.into())]),
        "node_publication_authority_required",
    )?;
    crate::history_contract::token(&value["record_id"])?;
    require(
        matches!(&value["generation"],crate::value::TypedValue::Integer(n) if !n.as_str().starts_with('-')),
        "node_publication_authority_required",
    )?;
    Ok(())
}
/// Detect the marked layout without extending admission of any legacy writer.
pub(crate) fn selected(entry: &Path) -> Result<bool> {
    let root = entry.parent().ok_or_else(|| bad("invalid_path"))?;
    let Some(raw) = read(root, ".kpopper/history.yaml")? else {
        return Ok(false);
    };
    let marker = crate::history_yaml::decode_document(&raw)?;
    let fields = crate::history_contract::map(&marker)?;
    if !fields
        .get("profile")
        .is_some_and(|v| crate::history_contract::string_is(v, C::FORMAT))
    {
        return Ok(false);
    }
    validate_authority(&marker)?;
    require(
        entry.file_name().is_some_and(|n| n == VIEW),
        "node_history_entry_unsupported",
    )?;
    Ok(true)
}
pub(crate) fn evidence_path(path: &str) -> Result<()> {
    if let Some(name) = path
        .strip_prefix(".kpopper/hypotheses/")
        .and_then(|p| p.strip_suffix(".yaml").or_else(|| p.strip_suffix(".yml")))
    {
        crate::history_contract::hypothesis_name(&crate::value::TypedValue::Text(name.into()))?;
        return crate::history_branch::portable_path(path);
    }
    if path.starts_with(".kpopper-history-migration/") {
        return crate::history_branch::portable_path(path);
    }

    if crate::history_source_ancestry::path_id(path).is_some() {
        return Ok(());
    }
    for (prefix, suffix) in [
        ("evidence/legacy/", ".zip"),
        ("evidence/migration/", ".json"),
        ("evidence/bootstrap/", ".zip"),
        ("evidence/bootstrap/", ".json"),
    ] {
        if let Some(name) = path
            .strip_prefix(prefix)
            .and_then(|p| p.strip_suffix(suffix))
        {
            token(name)?;
            return require(!name.contains('/'), "node_publication_evidence_path");
        }
    }
    let name = path
        .strip_prefix("evidence/reports/")
        .and_then(|p| p.strip_suffix(".txt"))
        .or_else(|| {
            path.strip_prefix("evidence/view-edits/")
                .and_then(|p| p.strip_suffix(".yaml"))
        })
        .ok_or_else(|| bad("node_publication_evidence_path"))?;
    token(name)?;
    require(!name.contains('/'), "node_publication_evidence_path")
}
fn evidence_directories(root: &Path, journal: &Journal) -> Result<()> {
    if !journal.evidence.is_empty() {
        // Sync each newly linked parent before publishing a durable manifest.
        let mut directories = BTreeSet::from(["evidence"]);
        for file in &journal.evidence {
            evidence_path(&file.path)?;
            let mut parent = file.path.rsplit_once('/').unwrap().0;
            loop {
                directories.insert(parent);
                if let Some((next, _)) = parent.rsplit_once('/') {
                    parent = next;
                } else {
                    break;
                }
            }
        }
        for relative in directories {
            let path = F::target(root, relative)?;
            fs::create_dir_all(&path)?;
            F::sync(path.parent().unwrap())?;
        }
    }
    Ok(())
}
fn evidence_state(root: &Path, file: &EvidenceFile) -> Result<Option<Vec<u8>>> {
    let current = read(root, &file.path)?;
    let expected = unbytes(&Some(file.raw.clone()))?.unwrap();
    require(
        current.as_ref().is_none_or(|raw| *raw == expected) && (!file.existed || current.is_some()),
        "node_publication_evidence_changed",
    )?;
    Ok(current)
}
fn inventory(root: &Path) -> Result<BTreeMap<String, Manifest>> {
    inventory_with(root, &BTreeMap::new())
}
fn inventory_with(
    root: &Path,
    imports: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Manifest>> {
    let authority_sha256 = authority(root)?;
    let mut total_bytes = 0;
    let dir = F::target(root, ".kpopper/history-commits")?;
    let mut all = BTreeMap::new();
    for item in if dir.exists() {
        Some(fs::read_dir(dir)?)
    } else {
        None
    }
    .into_iter()
    .flatten()
    {
        let item = item?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| bad("invalid_path"))?;
        if name.starts_with('.') {
            continue;
        }
        require(name.ends_with(".json"), "node_publication_foreign_format")?;
        let op = name.strip_suffix(".json").unwrap();
        let raw =
            read(root, &commit(op)?)?.ok_or_else(|| bad("node_publication_missing_manifest"))?;
        total_bytes += raw.len();
        require(total_bytes <= MAX_BYTES, "node_publication_limit")?;
        let m = Manifest::decode(&raw)?;
        require(
            m.authority_sha256 == authority_sha256,
            "node_publication_authority_changed",
        )?;
        require(m.operation == op, "node_publication_manifest_path")?;
        all.insert(op.into(), m);
        require(all.len() <= C::MAX_EVENTS, "node_publication_limit")?;
    }
    for (op, raw) in imports {
        let raw = unbytes(&Some(raw.clone()))?.unwrap();
        if !all.contains_key(op) {
            total_bytes += raw.len();
        }
        let m = Manifest::decode(&raw)?;
        require(
            m.operation == *op && m.authority_sha256 == authority_sha256,
            "node_import_manifest",
        )?;
        require(
            all.get(op).is_none_or(|old| old == &m),
            "node_import_collision",
        )?;
        all.insert(op.clone(), m);
    }
    require(
        total_bytes <= MAX_BYTES && all.len() <= C::MAX_EVENTS,
        "node_publication_limit",
    )?;
    validate_inventory(&all)?;
    Ok(all)
}
fn validate_inventory(all: &BTreeMap<String, Manifest>) -> Result<()> {
    for m in all.values() {
        for (op, digest) in &m.parents {
            require(
                all.get(op).is_some_and(|p| p.digest == *digest),
                "node_publication_missing_parent",
            )?;
        }
    }
    let mut remaining = all
        .iter()
        .map(|(op, m)| (op.clone(), m.parents.len()))
        .collect::<BTreeMap<_, _>>();
    let mut children = BTreeMap::<String, Vec<String>>::new();
    for (op, m) in all {
        for parent in m.parents.keys() {
            children.entry(parent.clone()).or_default().push(op.clone());
        }
    }
    let mut ready = remaining
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(op, _)| op.clone())
        .collect::<Vec<_>>();
    let mut count = 0;
    while let Some(op) = ready.pop() {
        count += 1;
        for child in children.get(&op).into_iter().flatten() {
            let n = remaining.get_mut(child).unwrap();
            *n -= 1;
            if *n == 0 {
                ready.push(child.clone());
            }
        }
    }
    require(count == all.len(), "node_publication_parent_cycle")
}
fn verify_imports(root: &Path, journal: &Journal) -> Result<()> {
    for (op, raw) in &journal.imports {
        require(
            read(root, &commit(op)?)? == unbytes(&Some(raw.clone()))?,
            "node_import_missing",
        )?;
    }
    Ok(())
}
fn target_inventory(root: &Path, journal: &Journal) -> Result<BTreeMap<String, Manifest>> {
    let mut all = inventory_with(root, &journal.imports)?;
    if let Some(m) = all.remove(&journal.manifest.operation) {
        require(
            m == journal.manifest,
            "node_publication_operation_collision",
        )?;
    }
    for op in journal.imports.keys() {
        all.remove(op);
    }
    validate_inventory(&all)?;
    require(
        frontier(&all)
            == *journal
                .target_parents
                .as_ref()
                .unwrap_or(&journal.manifest.parents),
        "node_publication_stale_frontier",
    )?;
    Ok(all)
}
fn combined_inventory(root: &Path, journal: &Journal) -> Result<BTreeMap<String, Manifest>> {
    let mut all = inventory_with(root, &journal.imports)?;
    if let Some(m) = all.remove(&journal.manifest.operation) {
        require(
            m == journal.manifest,
            "node_publication_operation_collision",
        )?;
    }
    require(
        frontier(&all) == journal.manifest.parents,
        "node_publication_stale_frontier",
    )?;
    Ok(all)
}
fn frontier(all: &BTreeMap<String, Manifest>) -> BTreeMap<String, String> {
    let non_tips = all
        .values()
        .flat_map(|m| m.parents.keys())
        .collect::<BTreeSet<_>>();
    all.iter()
        .filter(|(k, _)| !non_tips.contains(k))
        .map(|(k, v)| (k.clone(), v.digest.clone()))
        .collect()
}
fn guard(root: &Path) -> Result<()> {
    F::check_reader_journal(
        root,
        &crate::history_transaction::Layout::for_entry(VIEW)?.journal,
    )?;
    require(
        read(root, JOURNAL)?.is_none(),
        "node_publication_recovery_required",
    )
}
fn prefix(current: Option<&[u8]>, append: &Append) -> Result<()> {
    match (&append.prefix_sha256, current) {
        (None, None) => require(append.offset == 0, "node_publication_prefix"),
        (None, Some(_raw)) => require(append.offset == 0, "node_publication_prefix"),
        (Some(expected), Some(raw)) => require(
            raw.len() >= append.offset as usize
                && sha256(&raw[..append.offset as usize]) == *expected,
            "node_publication_prefix",
        ),
        _ => Err(bad("node_publication_prefix")),
    }
}
fn append_state(root: &Path, append: &Append) -> Result<(PathBuf, Option<Vec<u8>>, Vec<u8>)> {
    let path = F::target(root, &stream(&append.subject)?)?;
    let current = F::read(&path)?;
    prefix(current.as_deref(), append)?;
    let frames = unbytes(&Some(append.frames.clone()))?.unwrap();
    let suffix = current
        .as_ref()
        .map(|v| &v[append.offset as usize..])
        .unwrap_or_default();
    require(frames.starts_with(suffix), "node_publication_foreign_tail")?;
    Ok((path, current, frames))
}
fn validate_complete_streams(root: &Path, journal: &Journal) -> Result<()> {
    for append in &journal.appends {
        let (_, current, frames) = append_state(root, append)?;
        let mut complete = current.unwrap_or_default();
        complete.truncate(append.offset as usize);
        complete.extend(frames);
        C::decode_stream(&complete, &append.subject)?;
    }
    Ok(())
}
/// Prepare against a locked, fully byte-validated publication. A semantic verifier is still required.
pub fn prepare(
    root: &Path,
    operation: &str,
    after_view: Vec<u8>,
    frames: BTreeMap<String, Vec<u8>>,
) -> Result<Prepared> {
    prepare_with_context(root, operation, after_view, frames, None)
}

/// Context contains transaction-local intent and receipt side recipes, never node inventories.
pub fn prepare_with_context(
    root: &Path,
    operation: &str,
    after_view: Vec<u8>,
    frames: BTreeMap<String, Vec<u8>>,
    context: Option<&crate::value::TypedValue>,
) -> Result<Prepared> {
    prepare_with_evidence(
        root,
        operation,
        after_view,
        frames,
        context,
        BTreeMap::new(),
    )
}

pub fn prepare_with_evidence(
    root: &Path,
    operation: &str,
    after_view: Vec<u8>,
    frames: BTreeMap<String, Vec<u8>>,
    context: Option<&crate::value::TypedValue>,
    evidence: BTreeMap<String, Vec<u8>>,
) -> Result<Prepared> {
    prepare_inner(
        root,
        operation,
        after_view,
        frames,
        context,
        evidence,
        None,
        BTreeMap::new(),
    )
}

/// An edited view has two distinct before images: observed bytes for concurrency/rollback,
/// and a manifest-verified canonical view for semantic admission and replay.
pub(crate) fn prepare_edited(
    root: &Path,
    operation: &str,
    after_view: Vec<u8>,
    frames: BTreeMap<String, Vec<u8>>,
    context: &crate::value::TypedValue,
    evidence: BTreeMap<String, Vec<u8>>,
    canonical: &[u8],
) -> Result<Prepared> {
    prepare_inner(
        root,
        operation,
        after_view,
        frames,
        Some(context),
        evidence,
        Some(canonical),
        BTreeMap::new(),
    )
}
pub(crate) fn prepare_union(
    root: &Path,
    operation: &str,
    after: Vec<u8>,
    frames: BTreeMap<String, Vec<u8>>,
    context: &crate::value::TypedValue,
    evidence: BTreeMap<String, Vec<u8>>,
    imports: BTreeMap<String, Vec<u8>>,
) -> Result<Prepared> {
    prepare_inner(
        root,
        operation,
        after,
        frames,
        Some(context),
        evidence,
        None,
        imports
            .into_iter()
            .map(|(op, raw)| (op, STANDARD.encode(raw)))
            .collect(),
    )
}
fn prepare_inner(
    root: &Path,
    operation: &str,
    after_view: Vec<u8>,
    frames: BTreeMap<String, Vec<u8>>,
    context: Option<&crate::value::TypedValue>,
    evidence: BTreeMap<String, Vec<u8>>,
    canonical: Option<&[u8]>,
    imports: BTreeMap<String, String>,
) -> Result<Prepared> {
    let _lock = F::DirectoryGuard::acquire(root, false)?;
    guard(root)?;
    token(operation)?;
    require(after_view.len() <= MAX_BYTES, "node_publication_limit")?;
    require(
        view_operation(&after_view)?.as_deref() == Some(operation),
        "node_publication_unbound_view",
    )?;
    let all = inventory(root)?;
    require(!all.contains_key(operation), "operation_already_prepared")?;
    // Complete closure verification is retained; this is never a scoped fast-read promise.
    verify_history_with(root, &all, &BTreeMap::new(), canonical)?;
    let target_parents = (!imports.is_empty()).then(|| frontier(&all));
    for op in imports.keys() {
        require(!all.contains_key(op), "node_import_existing")?;
    }
    let all = inventory_with(root, &imports)?;
    let before = read(root, VIEW)?;
    if canonical.is_some() {
        require(!all.is_empty(), "history_bootstrap_required")?;
        require(before.is_some(), "node_semantic_missing_view")?;
    }
    let mut appends = Vec::new();
    let mut touched = Vec::new();
    let mut retained = Vec::new();
    let known = known_touches(&all)?;
    for (subject, raw) in frames {
        let old = read(root, &stream(&subject)?)?;
        let append = Append {
            subject: subject.clone(),
            offset: old.as_ref().map_or(0, |v| v.len() as u64),
            prefix_sha256: digest(&old),
            frames: STANDARD.encode(&raw),
        };
        for frame in raw.split_inclusive(|v| *v == b'\n') {
            let event = C::Event::decode(frame, &subject)?;
            let touch = Touch {
                subject: subject.clone(),
                event: event.id().into(),
                frame_sha256: sha256(frame),
            };
            if let Some(old) = known.get(event.id()) {
                require(*old == touch, "node_publication_retained_mismatch")?;
                retained.push(touch);
            } else {
                touched.push(touch);
            }
        }
        appends.push(append);
    }
    for ((subject, _), event) in current_origins(&after_view)? {
        let touch = Touch {
            subject,
            event: event.id().into(),
            frame_sha256: sha256(&event.encode()?),
        };
        if let Some(old) = known.get(event.id()) {
            require(*old == touch, "node_publication_retained_mismatch")?;
            retained.push(touch);
        } else {
            touched.push(touch);
        }
    }
    touched.sort_by(|a, b| (&a.subject, &a.event).cmp(&(&b.subject, &b.event)));
    let mut evidence_files = Vec::new();
    let mut evidence_hashes = BTreeMap::new();
    for (path, raw) in evidence {
        evidence_path(&path)?;
        require(raw.len() <= MAX_BYTES, "node_publication_limit")?;
        let current = read(root, &path)?;
        require(
            current.as_ref().is_none_or(|old| *old == raw),
            "node_publication_evidence_changed",
        )?;
        evidence_hashes.insert(path.clone(), sha256(&raw));
        evidence_files.push(EvidenceFile {
            path,
            raw: STANDARD.encode(raw),
            existed: current.is_some(),
        });
    }
    let mut manifest = Manifest {
        format: FORMAT.into(),
        operation: operation.into(),
        authority_sha256: authority(root)?,
        parents: frontier(&all),
        touched,
        before_view: digest(&before),
        after_view: Some(sha256(&after_view)),
        evidence: evidence_hashes,
        context: context
            .map(crate::value::TypedValue::to_tagged)
            .transpose()?,
        digest: String::new(),
    };
    manifest.digest = manifest.identity()?;
    let mut journal = Journal {
        format: FORMAT.into(),
        manifest,
        before_view: bytes(&before),
        canonical_before: canonical.map(|raw| STANDARD.encode(raw)),
        after_view: STANDARD.encode(after_view),
        appends,
        retained,
        imports,
        target_parents,
        evidence: evidence_files,
        guard: None,
        digest: String::new(),
    };
    journal.digest = journal.identity()?;
    journal.validate()?;
    journal.encode()?;
    validate_complete_streams(root, &journal)?;
    let mut candidate = all;
    candidate.insert(operation.into(), journal.manifest.clone());
    let mut overlay = BTreeMap::new();
    for append in &journal.appends {
        let (_, current, frames) = append_state(root, append)?;
        let mut raw = current.unwrap_or_default();
        raw.truncate(append.offset as usize);
        raw.extend(frames);
        overlay.insert(stream(&append.subject)?, Some(raw));
    }
    for file in &journal.evidence {
        overlay.insert(file.path.clone(), unbytes(&Some(file.raw.clone()))?);
    }
    verify_history_with(
        root,
        &candidate,
        &overlay,
        unbytes(&Some(journal.after_view.clone()))?.as_deref(),
    )?;
    Ok(Prepared { journal })
}
fn verify_history(root: &Path, all: &BTreeMap<String, Manifest>) -> Result<()> {
    verify_history_with(root, all, &BTreeMap::new(), None).map(|_| ())
}
struct Verified {
    versions: BTreeMap<String, BTreeMap<String, C::Version>>,
    source_clocks: crate::history_source_ancestry::Graph,
    raw_evidence: BTreeMap<String, std::sync::Arc<Vec<u8>>>,
    legacy: BTreeMap<String, std::sync::Arc<crate::history_node_legacy::Legacy>>,
}
fn verify_history_with(
    root: &Path,
    all: &BTreeMap<String, Manifest>,
    overlay: &BTreeMap<String, Option<Vec<u8>>>,
    current_view: Option<&[u8]>,
) -> Result<Verified> {
    let mut source_clocks = crate::history_source_ancestry::Graph::default();
    let mut legacy = BTreeMap::new();
    let mut raw_evidence = BTreeMap::new();
    let mut legacy_bytes = 0usize;
    let mut evidence = BTreeMap::new();
    for m in all.values() {
        for (path, hash) in &m.evidence {
            if let Some(prior) = evidence.insert(path, hash) {
                require(prior == hash, "node_publication_evidence_collision")?;
            }
        }
    }
    let mut evidence_bytes = 0usize;
    for (path, hash) in evidence {
        let raw = match overlay.get(path) {
            Some(raw) => raw.clone(),
            None => read(root, path)?,
        }
        .ok_or_else(|| bad("node_publication_evidence_missing"))?;
        evidence_bytes = evidence_bytes.saturating_add(raw.len());
        require(evidence_bytes <= MAX_BYTES, "node_publication_limit")?;
        require(sha256(&raw) == *hash, "node_publication_evidence_changed")?;
        if path.starts_with(crate::history_source_ancestry::PREFIX) {
            source_clocks.insert(path, &raw)?;
        }
        if path.starts_with(crate::history_node_legacy::PREFIX) {
            let value = crate::history_node_legacy::load(&raw)?;
            legacy_bytes = legacy_bytes.saturating_add(value.decoded_bytes);
            require(legacy_bytes <= MAX_BYTES, "node_legacy_archive_limit")?;
            legacy.insert(path.clone(), value);
        }
        raw_evidence.insert(path.clone(), std::sync::Arc::new(raw));
    }
    let mut verified = BTreeMap::new();
    let mut expected = BTreeMap::<String, BTreeMap<String, String>>::new();
    for m in all.values() {
        for t in &m.touched {
            let prior = expected
                .entry(t.subject.clone())
                .or_default()
                .insert(t.event.clone(), t.frame_sha256.clone());
            require(prior.is_none(), "node_publication_duplicate_event")?;
        }
    }
    let stored_view;
    let view = if let Some(raw) = current_view {
        raw
    } else {
        stored_view = read(root, VIEW)?.unwrap_or_default();
        &stored_view
    };
    if !view.is_empty() {
        verify_view(all, view)?;
    }
    let origins = if view.is_empty() {
        BTreeMap::new()
    } else {
        current_origins(view)?
    };
    let known = known_touches(all)?;
    for ((subject, _), event) in &origins {
        require(
            known.get(event.id())
                == Some(&Touch {
                    subject: subject.clone(),
                    event: event.id().into(),
                    frame_sha256: sha256(&event.encode()?),
                }),
            "node_publication_uncommitted_current",
        )?;
    }
    let mut paths = BTreeSet::new();
    let mut total_bytes = 0;
    for (subject, events) in &expected {
        let path = stream(subject)?;
        let raw = match overlay.get(&path) {
            Some(raw) => raw.clone(),
            None => read(root, &path)?,
        };
        if raw.is_none() {
            let mut bytes = Vec::new();
            let mut count = 0;
            for ((s, id), event) in &origins {
                if s != subject {
                    continue;
                }
                let raw = event.encode()?;
                require(
                    events.get(id) == Some(&sha256(&raw)),
                    "node_publication_missing_stream",
                )?;
                bytes.extend(raw);
                count += 1;
            }
            require(
                count > 0 && count == events.len(),
                "node_publication_missing_stream",
            )?;
            total_bytes += bytes.len();
            require(total_bytes <= MAX_BYTES, "node_publication_limit")?;
            verified.insert(subject.clone(), C::decode_stream(&bytes, subject)?);
            continue;
        }
        require(
            !origins.keys().any(|(s, _)| s == subject),
            "node_publication_duplicate_current",
        )?;
        paths.insert(C::subject_path(subject)?);
        let raw = raw.unwrap();
        total_bytes += raw.len();
        require(total_bytes <= MAX_BYTES, "node_publication_limit")?;
        let versions = C::decode_stream(&raw, subject)?;
        require(
            versions.len() == events.len(),
            "node_publication_uncommitted_event",
        )?;
        for (id, version) in &versions {
            require(
                events.get(id).map(String::as_str) == Some(version.frame_sha256()),
                "node_publication_frame_hash",
            )?;
        }
        verified.insert(subject.clone(), versions);
    }
    let directory = F::target(root, ".kpopper/history")?;
    if directory.exists() {
        for item in fs::read_dir(directory)? {
            let name = item?
                .file_name()
                .into_string()
                .map_err(|_| bad("invalid_path"))?;
            if !name.starts_with('.') {
                require(
                    paths.contains(&name)
                        || overlay
                            .get(&format!(".kpopper/history/{name}"))
                            .is_some_and(Option::is_none),
                    "node_publication_uncommitted_stream",
                )?;
            }
        }
    }
    Ok(Verified {
        versions: verified,
        source_clocks,
        raw_evidence,
        legacy,
    })
}
fn revalidate(root: &Path, journal: &Journal) -> Result<()> {
    for file in &journal.evidence {
        let current = evidence_state(root, file)?;
        require(
            current.is_some() == file.existed,
            "node_publication_evidence_changed",
        )?;
    }
    for op in journal.imports.keys() {
        require(read(root, &commit(op)?)?.is_none(), "node_import_existing")?;
    }
    let all = target_inventory(root, journal)?;
    verify_history_with(
        root,
        &all,
        &BTreeMap::new(),
        unbytes(&journal.canonical_before)?.as_deref(),
    )?;
    let known = known_touches(&combined_inventory(root, journal)?)?;
    for touch in &journal.retained {
        require(
            known.get(&touch.event) == Some(touch),
            "node_publication_retained_mismatch",
        )?;
    }
    require(
        read(root, VIEW)? == unbytes(&journal.before_view)?,
        "node_publication_stale_view",
    )?;
    for append in &journal.appends {
        let current = read(root, &stream(&append.subject)?)?;
        require(
            current.as_ref().map_or(0, |v| v.len() as u64) == append.offset
                && digest(&current) == append.prefix_sha256,
            "node_publication_stale_stream",
        )?;
    }
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    F::sync(path)
}

fn append_frames(root: &Path, append: &Append) -> Result<()> {
    let (path, current, frames) = append_state(root, append)?;
    let already = current
        .as_ref()
        .map_or(0, |v| v.len() - append.offset as usize);
    if already == frames.len() {
        return Ok(());
    }
    let parent = path.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(&frames[already..])?;
    crate::history_io_metrics::write(frames.len() - already);
    file.sync_all()?;
    sync_directory(parent)?;
    Ok(())
}
/// Exact retries after success are recognized by manifest and full-closure verification.
pub fn publish(
    root: &Path,
    prepared: &Prepared,
    verify: impl FnOnce(&Prepared) -> Result<()>,
    mut boundary: impl FnMut(Phase) -> Result<()>,
) -> Result<()> {
    let _lock = F::DirectoryGuard::acquire(root, true)?;
    guard(root)?;
    let journal = &prepared.journal;
    journal.validate()?;
    require(
        authority(root)? == journal.manifest.authority_sha256,
        "node_publication_authority_changed",
    )?;
    if let Some(raw) = read(root, &commit(&journal.manifest.operation)?)? {
        require(
            raw == journal.manifest.encode()?,
            "node_publication_operation_collision",
        )?;
        let all = inventory(root)?;
        verify_history(root, &all)?;
        require(
            digest(&read(root, VIEW)?) == journal.manifest.after_view.clone(),
            "node_publication_retry_view",
        )?;
        verify(prepared)?;
        return Ok(());
    }
    revalidate(root, journal)?;
    verify(prepared)?;
    revalidate(root, journal)?;
    // Create and durably bind both directories before any append or manifest publication.
    for directory in [".kpopper", ".kpopper/history", ".kpopper/history-commits"] {
        let path = F::target(root, directory)?;
        fs::create_dir_all(&path)?;
        sync_directory(path.parent().unwrap())?;
    }
    evidence_directories(root, journal)?;
    F::replace(&F::target(root, JOURNAL)?, Some(&journal.encode()?))?;
    boundary(Phase::Journal)?;
    for (i, append) in journal.appends.iter().enumerate() {
        append_frames(root, append)?;
        boundary(Phase::Append(i))?;
    }
    for (i, file) in journal.evidence.iter().enumerate() {
        F::publish_immutable(
            root,
            &file.path,
            &unbytes(&Some(file.raw.clone()))?.unwrap(),
        )?;
        boundary(Phase::Evidence(i))?;
    }
    for (i, (op, raw)) in journal.imports.iter().enumerate() {
        F::publish_immutable(root, &commit(op)?, &unbytes(&Some(raw.clone()))?.unwrap())?;
        boundary(Phase::Import(i))?;
    }
    verify_imports(root, journal)?;
    combined_inventory(root, journal)?;
    // Callbacks may have changed an earlier evidence file.
    for file in &journal.evidence {
        require(
            evidence_state(root, file)?.is_some(),
            "node_publication_evidence_missing",
        )?;
    }
    F::publish_immutable(
        root,
        &commit(&journal.manifest.operation)?,
        &journal.manifest.encode()?,
    )?;
    boundary(Phase::Commit)?;
    F::replace(
        &F::target(root, VIEW)?,
        unbytes(&Some(journal.after_view.clone()))?.as_deref(),
    )?;
    boundary(Phase::View)?;
    for file in &journal.evidence {
        require(
            evidence_state(root, file)?.is_some(),
            "node_publication_evidence_missing",
        )?;
    }
    verify_imports(root, journal)?;
    F::remove(&F::target(root, JOURNAL)?)
}
/// No manifest means rollback exact journal-owned tails; matching manifest means finish forward.
/// A caller must verify semantic admission of retained journal evidence before recovery mutation.
pub fn recover(root: &Path, verify: impl FnOnce(&Prepared) -> Result<()>) -> Result<&'static str> {
    let _lock = F::DirectoryGuard::acquire(root, true)?;
    let raw = read(root, JOURNAL)?.ok_or_else(|| bad("node_publication_no_journal"))?;
    let journal: Journal = serde_json::from_slice(&raw)?;
    journal.validate()?;
    require(
        authority(root)? == journal.manifest.authority_sha256,
        "node_publication_authority_changed",
    )?;
    require(journal.encode()? == raw, "node_publication_journal")?;
    let committed = read(root, &commit(&journal.manifest.operation)?)?;
    if let Some(raw) = &committed {
        require(
            *raw == journal.manifest.encode()?,
            "node_publication_operation_collision",
        )?;
    }
    let before = unbytes(&journal.before_view)?;
    let after = unbytes(&Some(journal.after_view.clone()))?;
    let current = read(root, VIEW)?;
    require(
        current == before || (committed.is_some() && current == after),
        "node_publication_foreign_view",
    )?;
    // Validate all committed history using the journal's exact before/after overlay, including
    // unrelated nodes, before any recovery mutation. Save observations across the verifier.
    let mut overlay = BTreeMap::new();
    let mut observed = BTreeMap::new();
    for append in &journal.appends {
        let (_, current, frames) = append_state(root, append)?;
        let path = stream(&append.subject)?;
        observed.insert(path.clone(), digest(&current));
        let mut prefix = current.unwrap_or_default();
        prefix.truncate(append.offset as usize);
        let reconstructed = if committed.is_some() {
            prefix.extend(frames);
            Some(prefix)
        } else if append.prefix_sha256.is_some() {
            Some(prefix)
        } else {
            None
        };
        overlay.insert(path, reconstructed);
    }
    for file in &journal.evidence {
        observed.insert(file.path.clone(), digest(&evidence_state(root, file)?));
        if committed.is_some() {
            overlay.insert(file.path.clone(), unbytes(&Some(file.raw.clone()))?);
        }
    }
    validate_complete_streams(root, &journal)?;
    let canonical_before = unbytes(&journal.canonical_before)?;
    let observed_imports = journal
        .imports
        .keys()
        .map(|op| Ok((op.clone(), read(root, &commit(op)?)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let observed_inventory = if committed.is_some() {
        inventory_with(root, &journal.imports)?
    } else {
        target_inventory(root, &journal)?
    };
    verify_history_with(
        root,
        &observed_inventory,
        &overlay,
        if committed.is_some() {
            after.as_deref()
        } else {
            canonical_before.as_deref().or(before.as_deref())
        },
    )?;
    verify(&Prepared {
        journal: journal.clone(),
    })?;
    require(
        read(root, JOURNAL)?.as_deref() == Some(raw.as_slice())
            && read(root, VIEW)? == current
            && read(root, &commit(&journal.manifest.operation)?)? == committed
            && authority(root)? == journal.manifest.authority_sha256,
        "node_publication_verifier_changed_inputs",
    )?;
    for append in &journal.appends {
        let (_, current, _) = append_state(root, append)?;
        require(
            digest(&current) == observed[&stream(&append.subject)?],
            "node_publication_verifier_changed_inputs",
        )?;
    }
    for file in &journal.evidence {
        require(
            digest(&evidence_state(root, file)?) == observed[&file.path],
            "node_publication_verifier_changed_inputs",
        )?;
    }
    for (op, raw) in &observed_imports {
        require(
            read(root, &commit(op)?)? == *raw,
            "node_publication_verifier_changed_inventory",
        )?;
    }
    let current_inventory = if committed.is_some() {
        inventory_with(root, &journal.imports)?
    } else {
        target_inventory(root, &journal)?
    };
    require(
        current_inventory == observed_inventory,
        "node_publication_verifier_changed_inventory",
    )?;
    verify_history_with(
        root,
        &current_inventory,
        &overlay,
        if committed.is_some() {
            after.as_deref()
        } else {
            canonical_before.as_deref().or(before.as_deref())
        },
    )?;
    if committed.is_some() {
        for (op, raw) in &journal.imports {
            F::publish_immutable(root, &commit(op)?, &unbytes(&Some(raw.clone()))?.unwrap())?;
        }
        evidence_directories(root, &journal)?;
        for file in &journal.evidence {
            F::publish_immutable(
                root,
                &file.path,
                &unbytes(&Some(file.raw.clone()))?.unwrap(),
            )?;
        }
        for append in &journal.appends {
            append_frames(root, append)?;
        }
        verify_history_with(root, &inventory(root)?, &BTreeMap::new(), after.as_deref())?;
        F::replace(&F::target(root, VIEW)?, after.as_deref())?;
    } else {
        target_inventory(root, &journal)?;
        for op in journal.imports.keys() {
            if read(root, &commit(op)?)?.is_some() {
                F::remove(&F::target(root, &commit(op)?)?)?;
            }
        }
        for append in &journal.appends {
            let (path, current, _) = append_state(root, append)?;
            if current.is_some() {
                if append.prefix_sha256.is_none() {
                    F::remove(&path)?;
                } else {
                    let file = OpenOptions::new().write(true).open(&path)?;
                    file.set_len(append.offset)?;
                    file.sync_all()?;
                }
            }
        }
        for file in &journal.evidence {
            if !file.existed && evidence_state(root, file)?.is_some() {
                F::remove(&F::target(root, &file.path)?)?;
            }
        }
        F::replace(&F::target(root, VIEW)?, before.as_deref())?;
    }
    F::remove(&F::target(root, JOURNAL)?)?;
    Ok(if committed.is_some() {
        "committed"
    } else {
        "rolled_back"
    })
}
/// Verify the whole committed closure before returning current bytes. Pending publication refuses.
/// One full byte-verified publication. Decoded states are reused by semantic capture.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub source_clocks: crate::history_source_ancestry::Graph,
    pub(crate) raw_evidence: BTreeMap<String, std::sync::Arc<Vec<u8>>>,
    pub(crate) legacy: BTreeMap<String, std::sync::Arc<crate::history_node_legacy::Legacy>>,
    pub authority: crate::value::TypedValue,
    pub revision: String,
    pub current: Option<Vec<u8>>,
    pub versions: BTreeMap<String, BTreeMap<String, C::Version>>,
    pub operations: BTreeMap<String, String>,
    pub transactions: BTreeMap<String, Transaction>,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub digest: String,
    pub parents: Vec<String>,
    pub context: Option<crate::value::TypedValue>,
    pub evidence: BTreeMap<String, String>,
}

fn snapshot(
    root: &Path,
    current: Option<Vec<u8>>,
    verified: Verified,
    all: &BTreeMap<String, Manifest>,
) -> Result<Snapshot> {
    let raw = read(root, ".kpopper/history.yaml")?
        .ok_or_else(|| bad("node_publication_authority_required"))?;
    let authority = crate::history_yaml::decode_document(&raw)?;
    validate_authority(&authority)?;
    let revision = sha256(&serde_json::to_vec(&(
        sha256(&raw),
        digest(&current),
        all.iter()
            .map(|(id, m)| (id, &m.digest))
            .collect::<BTreeMap<_, _>>(),
    ))?);
    Ok(Snapshot {
        source_clocks: verified.source_clocks,
        legacy: verified.legacy,
        raw_evidence: verified.raw_evidence,
        authority,
        revision,
        current,
        versions: verified.versions,
        operations: all
            .values()
            .flat_map(|m| {
                m.touched
                    .iter()
                    .map(|t| (t.event.clone(), m.operation.clone()))
            })
            .collect(),
        transactions: all
            .iter()
            .map(|(id, m)| {
                Ok((
                    id.clone(),
                    Transaction {
                        digest: m.digest.clone(),
                        evidence: m.evidence.clone(),
                        parents: m.parents.keys().cloned().collect(),
                        context: m
                            .context
                            .as_ref()
                            .map(crate::value::TypedValue::from_tagged)
                            .transpose()?,
                    },
                ))
            })
            .collect::<Result<_>>()?,
    })
}

impl Prepared {
    /// Recreate both exact worlds under the publisher/recovery lock. Pending appends and
    /// a committed manifest are overlaid, so semantic replay never reads a mixed world.
    pub(crate) fn snapshots(&self, root: &Path) -> Result<(Snapshot, Snapshot)> {
        let j = &self.journal;
        j.validate()?;
        require(
            authority(root)? == j.manifest.authority_sha256,
            "node_publication_authority_changed",
        )?;
        let before_all = target_inventory(root, j)?;
        let combined = combined_inventory(root, j)?;
        let mut before_overlay = BTreeMap::new();
        let mut after_overlay = BTreeMap::new();
        for append in &j.appends {
            let (_, current, frames) = append_state(root, append)?;
            let mut prefix = current.unwrap_or_default();
            prefix.truncate(append.offset as usize);
            before_overlay.insert(
                stream(&append.subject)?,
                append.prefix_sha256.as_ref().map(|_| prefix.clone()),
            );
            prefix.extend(frames);
            after_overlay.insert(stream(&append.subject)?, Some(prefix));
        }
        for file in &j.evidence {
            evidence_state(root, file)?;
            after_overlay.insert(file.path.clone(), unbytes(&Some(file.raw.clone()))?);
        }
        let before = self.canonical_before()?.or(self.before_view()?);
        let after = self.after_view()?;
        let before_versions = verify_history_with(
            root,
            &before_all,
            &before_overlay,
            Some(before.as_deref().unwrap_or_default()),
        )?;
        let mut after_all = combined;
        after_all.insert(self.operation().into(), j.manifest.clone());
        let after_versions = verify_history_with(root, &after_all, &after_overlay, Some(&after))?;
        Ok((
            snapshot(root, before, before_versions, &before_all)?,
            snapshot(root, Some(after), after_versions, &after_all)?,
        ))
    }
}
pub fn capture(root: &Path) -> Result<Option<Vec<u8>>> {
    Ok(capture_snapshot(root)?.current)
}
pub fn capture_snapshot(root: &Path) -> Result<Snapshot> {
    capture_snapshot_inner(root, None)
}
/// The supplied canonical image must match the committed manifest exactly. It is never
/// inferred from edited bodies or installed on disk by capture.
pub(crate) fn capture_edited_snapshot(root: &Path, canonical: &[u8]) -> Result<Snapshot> {
    capture_snapshot_inner(root, Some(canonical))
}
fn capture_snapshot_inner(root: &Path, canonical: Option<&[u8]>) -> Result<Snapshot> {
    let _lock = F::DirectoryGuard::acquire(root, false)?;
    guard(root)?;
    let all = inventory(root)?;
    if canonical.is_some() {
        require(!all.is_empty(), "history_bootstrap_required")?;
    }
    let versions = verify_history_with(root, &all, &BTreeMap::new(), canonical)?;
    let current = match canonical {
        Some(raw) => Some(raw.to_vec()),
        None => read(root, VIEW)?,
    };
    if !all.is_empty() {
        let tips = frontier(&all);
        require(tips.len() == 1, "node_publication_merge_required")?;
        let tip = &all[tips.first_key_value().unwrap().0];
        require(
            digest(&current) == tip.after_view.clone(),
            "node_publication_view_mismatch",
        )?;
    }
    snapshot(root, current, versions, &all)
}

/// Portable exact bytes for source-free reconstruction. Export remains a full-closure audit.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    format: String,
    files: BTreeMap<String, String>,
    digest: String,
}
impl Bundle {
    pub(crate) fn from_files(files: BTreeMap<String, Vec<u8>>) -> Result<Self> {
        let mut bundle = Self {
            format: "node-history-bundle-experiment/v1".into(),
            files: files
                .into_iter()
                .map(|(p, raw)| (p, STANDARD.encode(raw)))
                .collect(),
            digest: String::new(),
        };
        bundle.digest = bundle.identity()?;
        bundle.encode()?;
        Ok(bundle)
    }
    pub(crate) fn files(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        self.files
            .iter()
            .map(|(path, raw)| Ok((path.clone(), unbytes(&Some(raw.clone()))?.unwrap())))
            .collect()
    }

    fn identity(&self) -> Result<String> {
        let mut copy = self.clone();
        copy.digest.clear();
        Ok(sha256(&serde_json::to_vec(&copy)?))
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        let raw = serde_json::to_vec(self)?;
        require(raw.len() <= MAX_BYTES, "node_publication_limit")?;
        Ok(raw)
    }
    pub fn decode(raw: &[u8]) -> Result<Self> {
        require(raw.len() <= MAX_BYTES, "node_publication_limit")?;
        let bundle: Self = serde_json::from_slice(raw)?;
        require(
            bundle.format == "node-history-bundle-experiment/v1"
                && bundle.identity()? == bundle.digest
                && bundle.encode()? == raw,
            "node_publication_bundle",
        )?;
        Ok(bundle)
    }
    /// A new isolated temporary copy. Existing record paths are never overwritten.
    pub fn reconstruct(&self) -> Result<tempfile::TempDir> {
        Self::decode(&self.encode()?)?;
        require(
            self.files.contains_key(VIEW) && self.files.contains_key(".kpopper/history.yaml"),
            "node_publication_bundle",
        )?;
        let root = tempfile::tempdir()?;
        let mut total = 0;
        for (path, raw) in &self.files {
            let relative = Path::new(path);
            let name = relative
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or_else(|| bad("invalid_path"))?;
            let stream_path = relative.parent() == Some(Path::new(".kpopper/history"))
                && name
                    .strip_suffix(".jsonl")
                    .is_some_and(|s| s.len() == 64 && crate::history_paths::object_id(s));
            let manifest_path = relative.parent() == Some(Path::new(".kpopper/history-commits"))
                && name.strip_suffix(".json").is_some_and(|s| token(s).is_ok());
            require(
                path == VIEW
                    || path == ".kpopper/history.yaml"
                    || stream_path
                    || manifest_path
                    || evidence_path(path).is_ok(),
                "node_publication_bundle_path",
            )?;
            let bytes = unbytes(&Some(raw.clone()))?.unwrap();
            total += bytes.len();
            require(total <= MAX_BYTES, "node_publication_limit")?;
            F::publish_immutable(root.path(), path, &bytes)?;
        }
        capture(root.path())?;
        // Reject files that are structurally allowed but do not belong to the verified export.
        require(
            export(root.path())?.files == self.files,
            "node_publication_bundle_extra",
        )?;
        Ok(root)
    }
}
pub fn export(root: &Path) -> Result<Bundle> {
    let _lock = F::DirectoryGuard::acquire(root, false)?;
    // Physical layers cannot silently disappear from a portable branch capture.
    crate::history_node_hypothesis::sources(root)?;
    capture(root)?;
    let all = inventory(root)?;
    let mut paths = BTreeSet::from([VIEW.to_owned(), ".kpopper/history.yaml".into()]);
    let physical = crate::history_hypothesis_authoring::physical_files(
        &crate::history_store::Store::new(&root.join(VIEW))?,
    )?;
    paths.extend(physical.keys().cloned());
    for (operation, manifest) in all {
        paths.insert(commit(&operation)?);
        paths.extend(manifest.evidence.keys().cloned());
        for touch in manifest.touched {
            let path = stream(&touch.subject)?;
            if read(root, &path)?.is_some() {
                paths.insert(path);
            }
        }
    }
    let mut files = BTreeMap::new();
    let mut total = 0;
    for path in paths {
        let raw = read(root, &path)?.ok_or_else(|| bad("node_publication_bundle_missing"))?;
        total += raw.len();
        require(total <= MAX_BYTES, "node_publication_limit")?;
        files.insert(path, STANDARD.encode(raw));
    }
    let mut bundle = Bundle {
        format: "node-history-bundle-experiment/v1".into(),
        files,
        digest: String::new(),
    };
    bundle.digest = bundle.identity()?;
    bundle.encode()?;
    Ok(bundle)
}
