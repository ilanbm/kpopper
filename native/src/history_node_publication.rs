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
    after_view: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<serde_json::Value>,
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
        for parent in m.parents.keys() {
            token(parent)?;
        }
        Ok(m)
    }
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
struct Journal {
    format: String,
    manifest: Manifest,
    before_view: Option<String>,
    after_view: String,
    appends: Vec<Append>,
    retained: Vec<Touch>,
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
        let before = unbytes(&self.before_view)?;
        let after = unbytes(&Some(self.after_view.clone()))?.unwrap();
        require(
            digest(&before) == self.manifest.before_view
                && sha256(&after) == self.manifest.after_view,
            "node_publication_view",
        )?;
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
        let before_origins = before
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
    Journal,
    Append(usize),
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
        all[&operation].after_view == sha256(raw),
        "node_publication_view_mismatch",
    )
}
fn current_origins(raw: &[u8]) -> Result<BTreeMap<(String, String), C::Event>> {
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
            versions
                .values()
                .all(|v| v.operation() == initial_version.operation()),
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
        crate::history_contract::is_int(&value["generation"], "1"),
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
fn inventory(root: &Path) -> Result<BTreeMap<String, Manifest>> {
    let authority_sha256 = authority(root)?;
    let mut total_bytes = 0;
    let dir = F::target(root, ".kpopper/history-commits")?;
    let mut all = BTreeMap::new();
    if !dir.exists() {
        return Ok(all);
    }
    for item in fs::read_dir(dir)? {
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
    for m in all.values() {
        for (op, digest) in &m.parents {
            require(
                all.get(op).is_some_and(|p| p.digest == *digest),
                "node_publication_missing_parent",
            )?;
        }
    }
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
    verify_history(root, &all)?;
    let before = read(root, VIEW)?;
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
        } else {
            touched.push(touch);
        }
    }
    touched.sort_by(|a, b| (&a.subject, &a.event).cmp(&(&b.subject, &b.event)));
    let mut manifest = Manifest {
        format: FORMAT.into(),
        operation: operation.into(),
        authority_sha256: authority(root)?,
        parents: frontier(&all),
        touched,
        before_view: digest(&before),
        after_view: sha256(&after_view),
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
        after_view: STANDARD.encode(after_view),
        appends,
        retained,
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
fn verify_history_with(
    root: &Path,
    all: &BTreeMap<String, Manifest>,
    overlay: &BTreeMap<String, Option<Vec<u8>>>,
    current_view: Option<&[u8]>,
) -> Result<BTreeMap<String, BTreeMap<String, C::Version>>> {
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
    Ok(verified)
}
fn revalidate(root: &Path, journal: &Journal) -> Result<()> {
    let all = inventory(root)?;
    require(
        frontier(&all) == journal.manifest.parents,
        "node_publication_stale_frontier",
    )?;
    verify_history(root, &all)?;
    let known = known_touches(&all)?;
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
            digest(&read(root, VIEW)?) == Some(journal.manifest.after_view.clone()),
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
    F::replace(&F::target(root, JOURNAL)?, Some(&journal.encode()?))?;
    boundary(Phase::Journal)?;
    for (i, append) in journal.appends.iter().enumerate() {
        append_frames(root, append)?;
        boundary(Phase::Append(i))?;
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
    validate_complete_streams(root, &journal)?;
    let observed_inventory = inventory(root)?;
    verify_history_with(
        root,
        &observed_inventory,
        &overlay,
        if committed.is_some() {
            after.as_deref()
        } else {
            before.as_deref()
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
    let current_inventory = inventory(root)?;
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
            before.as_deref()
        },
    )?;
    if committed.is_some() {
        for append in &journal.appends {
            append_frames(root, append)?;
        }
        verify_history_with(root, &inventory(root)?, &BTreeMap::new(), after.as_deref())?;
        F::replace(&F::target(root, VIEW)?, after.as_deref())?;
    } else {
        require(
            frontier(&inventory(root)?) == journal.manifest.parents,
            "node_publication_stale_frontier",
        )?;
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
}

fn snapshot(
    root: &Path,
    current: Option<Vec<u8>>,
    versions: BTreeMap<String, BTreeMap<String, C::Version>>,
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
        authority,
        revision,
        current,
        versions,
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
        let mut before_all = inventory(root)?;
        if let Some(committed) = before_all.remove(self.operation()) {
            require(
                committed == j.manifest,
                "node_publication_operation_collision",
            )?;
        }
        require(
            frontier(&before_all) == j.manifest.parents,
            "node_publication_stale_frontier",
        )?;
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
        let before = self.before_view()?;
        let after = self.after_view()?;
        let before_versions = verify_history_with(
            root,
            &before_all,
            &before_overlay,
            Some(before.as_deref().unwrap_or_default()),
        )?;
        let mut after_all = before_all.clone();
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
    let _lock = F::DirectoryGuard::acquire(root, false)?;
    guard(root)?;
    let all = inventory(root)?;
    let versions = verify_history_with(root, &all, &BTreeMap::new(), None)?;
    let current = read(root, VIEW)?;
    if !all.is_empty() {
        let tips = frontier(&all);
        require(tips.len() == 1, "node_publication_merge_required")?;
        let tip = &all[tips.first_key_value().unwrap().0];
        require(
            digest(&current) == Some(tip.after_view.clone()),
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
                path == VIEW || path == ".kpopper/history.yaml" || stream_path || manifest_path,
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
    capture(root)?;
    let all = inventory(root)?;
    let mut paths = BTreeSet::from([VIEW.to_owned(), ".kpopper/history.yaml".into()]);
    for (operation, manifest) in all {
        paths.insert(commit(&operation)?);
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
