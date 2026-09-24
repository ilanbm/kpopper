//! Semantic preparation and exact replay of immutable history authoring.
//! Callers capture operation, recording day and timestamp once; retry retains bytes.
use crate::{
    Result, history_adapter as Adapter,
    history_authoring_audit::ReplayAudit,
    history_authority as A,
    history_capture::{self as H, Capture},
    history_contract::*,
    history_paths as P, history_preparation as Preparation, history_reduce,
    history_store::Store,
    history_transaction::{self as T, PreparedMutation},
    history_transaction_fs as FS,
    history_view::{self as View, list, map_mut, truth},
    history_yaml as Y,
    identity::{sha256, typed_object_identity},
    reasoning_authoring::{self as Authoring, World},
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    reasoning_snapshot::entries,
    require,
    value::{Integer, TypedValue as V},
};
use std::collections::BTreeSet;
pub(crate) fn s(v: &str) -> V {
    V::Text(v.into())
}
pub(crate) fn n(v: &str) -> V {
    V::Integer(Integer::new(v).unwrap())
}
pub(crate) fn obj(v: impl IntoIterator<Item = (&'static str, V)>) -> V {
    V::Map(v.into_iter().map(|(k, v)| (k.into(), v)).collect())
}
pub(crate) fn strings(v: impl IntoIterator<Item = String>) -> V {
    V::List(v.into_iter().map(V::Text).collect())
}
pub(crate) fn empty() -> V {
    V::Map(Map::new())
}

/// Values supplied by the operation boundary, not inferred from computational time.
#[derive(Clone)]
pub struct Options {
    pub operation: String,
    pub recorded_at: String,
    pub recording_day: String,
    pub by: V,
    pub strict: bool,
    pub paths: P::Scheme,
    /// Retained direct receipts (or sequential batch children) choose their original
    /// semantics. Fresh judgments use 7.
    pub receipt_version: Option<u8>,
}
impl Options {
    pub(crate) fn requires_for(&self, document: &V) -> Result<Option<V>> {
        let mut required = self
            .requires()
            .map(|v| list(&v).unwrap().to_vec())
            .unwrap_or_default();
        if !temporal_subjects(document)?.is_empty() {
            required.push(s(A::TEMPORAL_APPLICABILITY));
        }
        required.sort_by(|a, b| text(a).unwrap().cmp(text(b).unwrap()));
        required.dedup();
        Ok((!required.is_empty()).then_some(V::List(required)))
    }
    pub(crate) fn requires(&self) -> Option<V> {
        let mut requires = BTreeSet::new();
        if self.strict {
            requires.insert(A::ROOT_DISPOSITION.into());
        }
        if self.paths == P::Scheme::Hashed {
            requires.insert(P::CAPABILITY.into());
        }
        if requires.is_empty() {
            None
        } else {
            Some(strings(requires))
        }
    }
}
pub(crate) fn document(capture: &Capture) -> Result<V> {
    let mut doc = Adapter::from_store_capture(capture)?.document().clone();
    map_mut(
        map_mut(&mut doc)?
            .get_mut("meta")
            .ok_or_else(|| error("invalid_document"))?,
    )?
    .remove("history");
    Ok(doc)
}
pub(crate) fn destination(document: &V) -> Result<V> {
    if !Authoring::selected(document, None)? {
        return Ok(document.clone());
    }
    let cap = F::capabilities(document, None)?;
    let desired = Authoring::declaration(document)?;
    let have = list(&map(&cap)?["requires"])?;
    if list(&map(&desired)?["requires"])?
        .iter()
        .any(|v| !have.contains(v))
    {
        let mut doc = document.clone();
        map_mut(
            map_mut(&mut doc)?
                .get_mut("meta")
                .ok_or_else(|| error("invalid_document"))?,
        )?
        .insert("reasoning".into(), desired);
        Ok(doc)
    } else {
        Ok(document.clone())
    }
}
pub(crate) fn archive(store: &Store) -> Result<V> {
    let path = FS::target(&store.root, &store.layout.replaced)?;
    let raw = FS::read(&path)?;
    require(
        raw.as_ref().is_none_or(|v| v.len() <= 16 * 1024 * 1024),
        "history_limit",
    )?;
    Ok(obj([
        ("path", s(&store.layout.replaced)),
        ("sha256", raw.map(|v| s(&sha256(&v))).unwrap_or(V::Null)),
    ]))
}
pub(crate) fn guards(store: &Store, capture: &Capture, options: &Options) -> Result<()> {
    require(!capture.commits.is_empty(), "history_bootstrap_required")?;
    require(
        store.root.canonicalize()? == capture.root.canonicalize()?
            && store.layout.entry == capture.layout.entry,
        "capture_entry_mismatch",
    )?;
    let meta = map(field(map(&capture.document)?, "meta")?)?;
    require(
        field(meta, "history")?.digest()? == capture.baseline.digest()?,
        "stale_view",
    )?;
    require(
        store.render(capture)? == capture.entry_bytes,
        "unresolved_view_edit",
    )?;
    require(
        !capture.commits.contains_key(&options.operation),
        "operation_already_prepared",
    )?;
    require(!options.recorded_at.is_empty(), "missing_recording_time")?;
    Ok(())
}
#[cfg(test)]
pub(crate) fn head<'a>(capture: &'a Capture, subject: &str) -> Result<&'a V> {
    crate::history_authoring_core::Input::head(capture, subject)
}

pub(crate) fn evidence(doc: &V, world: &mut World<'_>, audit: Option<&ReplayAudit>) -> Result<V> {
    let report = if let Some(audit) = audit {
        audit.assessment(world)?
    } else {
        world.assessment()?
    };
    Ok(obj([
        ("kind", s("authored-computational-projection/v1")),
        ("document", doc.clone()),
        ("assessment", report),
    ]))
}
fn temporal_subjects(doc: &V) -> Result<BTreeSet<String>> {
    Ok(entries(doc)?
        .into_iter()
        .filter_map(|(subject, (_, body))| {
            map(&body)
                .ok()
                .and_then(|b| b.get("temporal"))
                .is_some_and(|v| matches!(v, V::Map(_)))
                .then_some(subject)
        })
        .collect())
}

pub(crate) fn attach_temporal_replay(
    evidence: &mut V,
    doc: &V,
    world: &World<'_>,
    versions: &Map,
) -> Result<()> {
    let claims = temporal_subjects(doc)?
        .into_iter()
        .filter_map(|subject| {
            versions
                .get(&subject)
                .map(|version| (subject, version.clone()))
        })
        .collect::<Map>();
    if !claims.is_empty() {
        map_mut(evidence)?.insert(
            "temporal_replay".into(),
            obj([
                ("version", n("1")),
                ("snapshot", s(&world.snapshot().to_json()?)),
                ("claims", V::Map(claims)),
            ]),
        );
    }
    Ok(())
}
pub(crate) fn evidence_with_versions(
    doc: &V,
    world: &mut World<'_>,
    audit: Option<&ReplayAudit>,
    versions: &Map,
) -> Result<V> {
    let mut value = evidence(doc, world, audit)?;
    attach_temporal_replay(&mut value, doc, world, versions)?;
    Ok(value)
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_object(
    subject: &str,
    kind: &str,
    body: V,
    saw: V,
    authored: Option<V>,
    pins: V,
    gaps: V,
    options: &Options,
) -> Result<V> {
    let mut value = obj([
        ("schema_version", n("2")),
        ("id_scheme", s("typed-history/v2")),
        ("subject", s(subject)),
        ("kind", s(kind)),
        ("body", body),
        ("saw", saw),
        ("by", options.by.clone()),
        ("on", s(&options.recorded_at)),
        ("op", s(&options.operation)),
    ]);
    if let Some(authored) = authored {
        map_mut(&mut value)?.extend([("authored".into(), authored), ("pins".into(), pins)]);
    }
    if truth(&gaps) {
        map_mut(&mut value)?.insert("pin_gaps".into(), gaps);
    }
    let id = typed_object_identity(&value)?;
    map_mut(&mut value)?.insert("id".into(), s(&id));
    validate_object(&value)?;
    Ok(value)
}
pub(crate) fn saw(capture: &Capture, subject: &str) -> Result<Vec<String>> {
    capture
        .objects
        .iter()
        .filter_map(|(id, o)| match map(o) {
            Ok(m) if string_is(&m["subject"], subject) => Some(Ok(id.clone())),
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        })
        .collect()
}
pub(crate) fn template(capture: &Capture, doc: &V) -> Result<V> {
    let mut template = View::template(&capture.commits)?;
    for c in F::collections(doc)?.keys() {
        map_mut(&mut template)?
            .entry(c.clone())
            .or_insert_with(empty);
    }
    let meta = map(&map(doc)?["meta"])?;
    let target = map_mut(
        map_mut(&mut template)?
            .get_mut("meta")
            .ok_or_else(|| error("invalid_document"))?,
    )?;
    if let Some(updated) = meta.get("updated") {
        target.insert("updated".into(), updated.clone());
    }
    if let Some(reasoning) = meta.get("reasoning") {
        target.insert("reasoning".into(), reasoning.clone());
    }
    Ok(template)
}
pub(crate) fn candidate(capture: &Capture, mutation: &PreparedMutation) -> Result<Capture> {
    let mut candidate = capture.clone();
    for file in mutation.files() {
        let raw = file
            .after
            .as_ref()
            .ok_or_else(|| error("invalid_mutation"))?;
        if file.role == "history_commit" {
            let m = Y::decode_document(raw)?;
            candidate
                .commits
                .insert(text(&map(&m)?["operation"])?.into(), raw.clone());
        } else if file.role == "history_object" {
            let o = Y::decode_document(raw)?;
            let m = map(&o)?;
            let id = text(&m["id"])?;
            let subject = text(&m["subject"])?;
            candidate
                .object_bytes
                .insert((subject.into(), id.into()), raw.clone());
            candidate.objects.insert(id.into(), o);
        } else if file.role == "record" {
            candidate.entry_bytes = raw.clone();
            candidate.document = Y::decode_document(raw)?;
        }
    }
    let raw = candidate
        .objects
        .iter()
        .map(|(id, o)| {
            let subject = text(&map(o)?["subject"])?;
            let key = (subject.into(), id.clone());
            Ok((
                key.clone(),
                candidate
                    .object_bytes
                    .get(&key)
                    .ok_or_else(|| error("incomplete_closure"))?
                    .clone(),
            ))
        })
        .collect::<Result<_>>()?;
    candidate.state =
        history_reduce::reduce_bytes(&raw, Some(map(&map(&capture.state)?["rules"])?), None)?;
    candidate.baseline = H::baseline(&candidate.marker, &candidate.commits, &candidate.state)?;
    Adapter::from_store_capture(&candidate)?;
    Ok(candidate)
}

impl crate::history_authoring_core::Input for Capture {
    fn document(&self) -> Result<V> {
        document(self)
    }
    fn state(&self) -> &V {
        &self.state
    }
    fn object(&self, id: &str) -> Result<&V> {
        self.objects
            .get(id)
            .ok_or_else(|| error("incomplete_closure"))
    }
    fn saw(&self, subject: &str) -> Result<Vec<String>> {
        saw(self, subject)
    }
    fn baseline(&self) -> &V {
        &self.baseline
    }
}

/// Prepare a core add, set or review from one detached store capture.
/// No files are written. Other authoring families refuse until their adapter is selected.
pub fn prepare(
    store: &Store,
    capture: &Capture,
    action: &V,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    prepare_inner(store, capture, action, options, runtime, None)
}
pub(crate) fn prepare_inner(
    store: &Store,
    capture: &Capture,
    action: &V,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    guards(store, capture, options)?;
    if let Some(expected) = map(action)?.get("expected_record_sha256") {
        use sha2::Digest;
        let expected = text(expected)?;
        require(
            expected.len() == 64 && expected.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid_expected_record_sha256",
        )?;
        let actual = format!("{:x}", sha2::Sha256::digest(&capture.entry_bytes));
        require(
            actual == expected,
            "record changed since the caller read it",
        )?;
    }
    crate::history_sources::capture(&store.root, &store.layout.entry, &capture.document)?;
    let frozen_archive = archive(store)?;
    let plan = crate::history_authoring_core::prepare(capture, action, options, runtime)?;
    let receipt = crate::history_authoring_core::receipt(
        capture,
        &plan,
        options,
        runtime,
        audit,
        &frozen_archive,
    )?;
    let new = plan.objects;
    let doc = plan.document;
    let template = template(capture, &doc)?;
    let mutation = Preparation::prepare_commit(
        capture,
        &options.operation,
        &new,
        &template,
        &receipt,
        options.requires_for(&doc)?.as_ref(),
    )?;
    candidate(capture, &mutation)?;
    require(archive(store)? == frozen_archive, "concurrent_archive_edit")?;
    crate::history_sources::capture(&store.root, &store.layout.entry, &capture.document)?;
    Ok(mutation)
}

/// Prepare an explicit acceptance, correction, refutation, proposal act or retirement.
/// The target claim already exists; no computed outcome grants an acceptance act.
pub fn prepare_act(
    store: &Store,
    capture: &Capture,
    action: &V,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    prepare_act_inner(store, capture, action, options, runtime, None)
}
fn prepare_act_inner(
    store: &Store,
    capture: &Capture,
    action: &V,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    guards(store, capture, options)?;
    let frozen_archive = archive(store)?;
    let plan = crate::history_authoring_act::prepare(capture, action, options)?;
    let new = &plan.objects;
    let before_doc = document(capture)?;
    let profile = plan.profile.as_str();
    let cap = F::capabilities(&before_doc, Some(profile))?;
    let placeholder = T::semantic_receipt(profile, &cap, &empty(), &empty())?;
    let initial_template = View::template(&capture.commits)?;
    let requires = options.requires_for(&before_doc)?;
    let draft = Preparation::prepare_commit(
        capture,
        &options.operation,
        &new,
        &initial_template,
        &placeholder,
        requires.as_ref(),
    )?;
    let projected = destination(&document(&candidate(capture, &draft)?)?)?;
    let template = template(capture, &projected)?;
    let requires = options.requires_for(&projected)?;
    let draft = Preparation::prepare_commit(
        capture,
        &options.operation,
        &new,
        &template,
        &placeholder,
        requires.as_ref(),
    )?;
    let projected = candidate(capture, &draft)?;
    let after_doc = document(&projected)?;
    let receipt = crate::history_authoring_act::receipt(
        capture,
        &projected,
        &plan,
        options,
        runtime,
        audit,
        &frozen_archive,
    )?;
    let mutation = Preparation::prepare_commit(
        capture,
        &options.operation,
        &new,
        &template,
        &receipt,
        options.requires_for(&after_doc)?.as_ref(),
    )?;
    require(
        mutation
            .files()
            .iter()
            .find(|f| f.role == "record")
            .and_then(|f| f.after.as_ref())
            == Some(&projected.entry_bytes),
        "view_projection_mismatch",
    )?;
    require(archive(store)? == frozen_archive, "concurrent_archive_edit")?;
    Ok(mutation)
}

/// A hypothetical claim with an explicit proposal disposition.
#[derive(Clone)]
pub struct Proposal {
    pub subject: String,
    pub body: V,
    pub collection: String,
    pub because: String,
    pub hypothesis: Option<V>,
}
pub fn prepare_proposal(
    store: &Store,
    capture: &Capture,
    proposal: &Proposal,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    prepare_proposal_inner(store, capture, proposal, options, runtime, None)
}
pub(crate) fn prepare_proposal_inner(
    store: &Store,
    capture: &Capture,
    proposal: &Proposal,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    guards(store, capture, options)?;
    let frozen_archive = archive(store)?;
    let plan = crate::history_authoring_proposal::prepare(capture, proposal, options, runtime)?;
    let new = &plan.objects;
    let doc = &plan.document;
    let collection = proposal.collection.as_str();
    let mut selected = capture.objects.clone();
    for o in new {
        selected.insert(text(&map(o)?["id"])?.into(), o.clone());
    }
    validate_closure(&selected)?;
    let receipt = crate::history_authoring_proposal::receipt(
        capture,
        proposal,
        &plan,
        options,
        runtime,
        audit,
        &frozen_archive,
    )?;
    let mut template = template(capture, &doc)?;
    map_mut(&mut template)?
        .entry(collection.into())
        .or_insert_with(empty);
    let mutation = Preparation::prepare_commit(
        capture,
        &options.operation,
        &new,
        &template,
        &receipt,
        options.requires_for(&plan.hypothetical)?.as_ref(),
    )?;
    candidate(capture, &mutation)?;
    require(archive(store)? == frozen_archive, "concurrent_archive_edit")?;
    Ok(mutation)
}

/// Replay from the validated causal parents, excluding siblings and the incoming commit.
pub fn verify_prepared(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let live = store.capture()?;
    let data = mutation.to_data();
    let data = map(&data)?;
    let receipt = &data["receipt"];
    let intent = map(field(map(&map(receipt)?["before"])?, "authoring")?)?;
    let version = field(intent, "version")?;
    require(
        ["1", "2", "3", "4", "5", "6", "7", "8", "9"]
            .iter()
            .any(|v| is_int(version, v)),
        "invalid_authoring_receipt",
    )?;
    require(
        archive(store)? == *field(intent, "archive")?,
        "concurrent_archive_edit",
    )?;
    let manifest = mutation
        .files()
        .iter()
        .find(|f| f.role == "history_commit")
        .and_then(|f| f.after.as_deref())
        .ok_or_else(|| error("invalid_mutation"))?;
    let manifest = Y::decode_document(manifest)?;
    A::validate_commit(&manifest)?;
    let manifest = map(&manifest)?;
    let mut commits = A::Files::new();
    let mut todo = map(&manifest["parents"])?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    while let Some(op) = todo.pop() {
        if commits.contains_key(&op) {
            continue;
        }
        let raw = live
            .commits
            .get(&op)
            .ok_or_else(|| error("incomplete_closure"))?;
        let parent = Y::decode_document(raw)?;
        todo.extend(map(&map(&parent)?["parents"])?.keys().cloned());
        commits.insert(op, raw.clone());
    }
    let objects = A::committed_objects(&live.marker, &commits, &live.object_bytes)?;
    let mut captured = live.clone();
    captured.commits = commits;
    captured.objects = objects;
    let raw = captured
        .object_bytes
        .iter()
        .filter(|((_, id), _)| captured.objects.contains_key(id))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    captured.state =
        history_reduce::reduce_bytes(&raw, Some(map(&map(&live.state)?["rules"])?), None)?;
    captured.baseline = H::baseline(&live.marker, &captured.commits, &captured.state)?;
    captured.entry_bytes = mutation
        .files()
        .iter()
        .find(|f| f.role == "record")
        .and_then(|f| f.before.clone())
        .ok_or_else(|| error("invalid_mutation"))?;
    captured.document = Y::decode_document(&captured.entry_bytes)?;
    let audit = ReplayAudit::from_parents(receipt, &captured.commits)?;
    let requires = manifest
        .get("requires")
        .map(list)
        .transpose()?
        .unwrap_or(&[]);
    let empty_action = empty();
    let action = intent.get("action").unwrap_or(&empty_action);
    let options = Options {
        operation: text(&data["operation"])?.into(),
        recorded_at: text(field(intent, "recorded_at")?)?.into(),
        recording_day: String::new(),
        by: field(intent, "by")?.clone(),
        strict: requires.contains(&s(A::ROOT_DISPOSITION)),
        paths: if requires.contains(&s(P::CAPABILITY)) {
            P::Scheme::Hashed
        } else {
            P::Scheme::Legacy
        },
        receipt_version: Some(if is_int(version, "7") {
            7
        } else if is_int(version, "9") {
            9
        } else if is_int(version, "5") {
            5
        } else {
            1
        }),
    };
    let expected = if ["2", "3", "6", "8"].iter().any(|v| is_int(version, v)) {
        require(
            intent.get("kind").is_some_and(|v| string_is(v, "batch")),
            "invalid_authoring_receipt",
        )?;
        let version = [2u8, 3, 6, 8]
            .into_iter()
            .find(|v| is_int(version, &v.to_string()))
            .unwrap();
        let actions = list(field(intent, "actions")?)?;
        let evidence = mutation
            .files()
            .iter()
            .filter(|f| f.role == "history_evidence")
            .map(|f| {
                Ok((
                    f.path.clone(),
                    f.after.clone().ok_or_else(|| error("invalid_mutation"))?,
                ))
            })
            .collect::<Result<_>>()?;
        let mut batch = crate::history_authoring_batch::BatchOptions {
            authoring: options,
            receipt_version: version,
            context: field(intent, "context")?.clone(),
            evidence,
        };
        if [2, 3].contains(&version) {
            batch.authoring.receipt_version = None;
            let current = crate::history_authoring_batch::prepare_inner(
                store,
                &captured,
                actions,
                &batch,
                runtime,
                Some(&audit),
            );
            if let Ok(current) = current
                && current.to_bytes()? == mutation.to_bytes()?
            {
                return Ok(());
            }
            // Old sequential envelopes did not capture typed seen values. The
            // compatibility candidate is independently evaluated and must match
            // the complete immutable mutation; caller evidence grants no result.
            batch.authoring.receipt_version = Some(1);
        }
        crate::history_authoring_batch::prepare_inner(
            store,
            &captured,
            actions,
            &batch,
            runtime,
            Some(&audit),
        )?
    } else if is_int(version, "5") || is_int(version, "9") {
        require(
            intent.get("kind").is_some_and(|v| string_is(v, "proposal")) && options.strict,
            "invalid_authoring_receipt",
        )?;
        let proposal = Proposal {
            subject: text(field(intent, "subject")?)?.into(),
            body: field(intent, "body")?.clone(),
            collection: text(field(intent, "collection")?)?.into(),
            because: text(field(intent, "because")?)?.into(),
            hypothesis: Some(field(intent, "hypothesis")?)
                .filter(|v| **v != V::Null)
                .cloned(),
        };
        prepare_proposal_inner(store, &captured, &proposal, &options, runtime, Some(&audit))?
    } else if is_int(version, "4") {
        require(
            intent.get("kind").is_some_and(|v| string_is(v, "act")),
            "invalid_authoring_receipt",
        )?;
        prepare_act_inner(store, &captured, action, &options, runtime, Some(&audit))?
    } else {
        prepare_inner(store, &captured, action, &options, runtime, Some(&audit))?
    };
    require(
        expected.to_bytes()? == mutation.to_bytes()?,
        "authoring_receipt_mismatch",
    )
}

/// Semantic replay is mandatory; caller verification adds routing and policy checks.
pub fn commit(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
    verify: FS::Verify<'_>,
) -> Result<V> {
    store.commit(mutation, &mut |data| {
        verify_prepared(store, mutation, runtime)?;
        verify(data)?;
        let receipt = field(map(data)?, "receipt")?;
        let intent = field(map(field(map(receipt)?, "before")?)?, "authoring")?;
        require(
            archive(store)? == *field(map(intent)?, "archive")?,
            "concurrent_archive_edit",
        )?;
        crate::history_sources::capture(
            &store.root,
            &store.layout.entry,
            &store.capture()?.document,
        )?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "../tests/support/history_temporal_authoring.rs"]
mod temporal_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    mod ordinary {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/ordinary_authoring.rs"
        ));
    }
    use crate::reasoning_runtime::OperationalBounds;
    use serde_json::Value as J;
    pub(crate) fn runtime(cache: &std::path::Path) -> Runtime {
        let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                crate::reasoning_runtime::target_name().unwrap()
            ));
        Runtime::open(&archive, cache, OperationalBounds::default()).unwrap()
    }
    pub(super) fn write(root: &std::path::Path, case: &J) {
        for (name, raw) in case["files"].as_object().unwrap() {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw.as_str().unwrap()).unwrap();
        }
    }
    pub(super) fn options(case: &J) -> Options {
        Options {
            operation: "write-1".into(),
            recorded_at: "2026-09-19T09:30:00+00:00".into(),
            recording_day: "2026-09-19".into(),
            by: s("writer"),
            strict: case["strict"].as_bool().unwrap(),
            paths: if case["legacy"] == true {
                P::Scheme::Legacy
            } else {
                P::Scheme::Hashed
            },
            receipt_version: if case["candidate"] == true {
                None
            } else {
                Some(1)
            },
        }
    }
    #[test]
    fn prepared_direct_actions_match_exact_python_envelopes() {
        let data: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-authoring.json")).unwrap();
        direct_corpus(&data, "direct");
    }
    #[test]
    fn prepared_direct_v7_captures_seen_and_missing_pin_evidence() {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-authoring-candidate.json"
        ))
        .unwrap();
        direct_corpus(&data, "direct");
    }
    #[test]
    fn prepared_targeted_acts_match_python_envelopes() {
        let data: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-authoring-act.json"))
                .unwrap();
        direct_corpus(&data, "act");
    }
    #[test]
    fn prepared_proposals_preserve_v5_and_capture_v9_seen() {
        for raw in [
            include_str!("../tests/fixtures/history-authoring-proposal.json"),
            include_str!("../tests/fixtures/history-authoring-proposal-candidate.json"),
        ] {
            let data: J = serde_json::from_str(raw).unwrap();
            direct_corpus(&data, "proposal");
        }
    }
    fn direct_corpus(data: &J, family: &str) {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let mut failures = vec![];
        for case in data["cases"].as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            write(temp.path(), case);
            let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
            let captured = store.capture().unwrap();
            let action = case
                .get("action")
                .map(|a| V::from_tagged(a).unwrap())
                .unwrap_or_else(empty);
            // The oracle fixture names Python's adapter. Rebind only that audited field
            // after actual native evaluation, so every other envelope byte is compared.
            let audit = case
                .get("receipt")
                .map(|v| ReplayAudit::oracle(&V::from_tagged(v).unwrap()).unwrap());
            let got = if family == "proposal" {
                let p = V::from_tagged(&case["proposal"]).unwrap();
                let p = map(&p).unwrap();
                let proposal = Proposal {
                    subject: text(&p["subject"]).unwrap().into(),
                    body: p["body"].clone(),
                    collection: text(&p["collection"]).unwrap().into(),
                    because: text(&p["because"]).unwrap().into(),
                    hypothesis: None,
                };
                let mut opts = options(case);
                opts.receipt_version = Some(if case["candidate"] == true { 9 } else { 5 });
                prepare_proposal_inner(
                    &store,
                    &captured,
                    &proposal,
                    &opts,
                    Some(&runtime),
                    audit.as_ref(),
                )
            } else if family == "act" {
                prepare_act_inner(
                    &store,
                    &captured,
                    &action,
                    &options(case),
                    Some(&runtime),
                    audit.as_ref(),
                )
            } else {
                prepare_inner(
                    &store,
                    &captured,
                    &action,
                    &options(case),
                    Some(&runtime),
                    audit.as_ref(),
                )
            };
            if let Some(expected) = case.get("output") {
                match got {
                    Ok(m) => {
                        if m.to_bytes().unwrap() != expected.as_str().unwrap().as_bytes() {
                            std::fs::write(temp.path().join("actual.json"), m.to_bytes().unwrap())
                                .unwrap();
                            let path = temp.keep();
                            failures.push(format!("{} mismatch: {}", case["name"], path.display()));
                        }
                    }
                    Err(e) => failures.push(format!("{} refused: {e}", case["name"])),
                }
            } else if got.is_ok() {
                failures.push(format!("{} accepted refusal", case["name"]));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn proposal_replay_retains_acceptance_until_an_explicit_act() {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-authoring-proposal-candidate.json"
        ))
        .unwrap();
        let case = data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "existing")
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let proposal = Proposal {
            subject: "p.input".into(),
            collection: "readings".into(),
            body: obj([("v", n("3"))]),
            because: "explicit proposal".into(),
            hypothesis: None,
        };
        let mut opts = options(case);
        opts.receipt_version = Some(9);
        let mutation = prepare_proposal(&store, &before, &proposal, &opts, Some(&runtime)).unwrap();
        verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        let captured = store.capture().unwrap();
        assert_eq!(
            head(&captured, "p.input").unwrap(),
            head(&before, "p.input").unwrap()
        );
        let data = mutation.to_data();
        let receipt = &map(&data).unwrap()["receipt"];
        let proposed =
            map(&map(&map(receipt).unwrap()["after"]).unwrap()["authoring"]).unwrap()["proposal"]
                .clone();
        let old = map(head(&before, "p.input").unwrap()).unwrap()["id"].clone();
        let action = obj([
            ("kind", s("accept")),
            ("id", s("p.input")),
            ("of", proposed),
            ("over", V::List(vec![old])),
            ("because", s("chosen explicitly")),
        ]);
        opts.operation = "accept-1".into();
        opts.receipt_version = None;
        let acceptance = prepare_act(&store, &captured, &action, &opts, Some(&runtime)).unwrap();
        verify_prepared(&store, &acceptance, Some(&runtime)).unwrap();
        commit(&store, &acceptance, Some(&runtime), &mut |_| Ok(())).unwrap();
        assert_eq!(
            map(&map(head(&store.capture().unwrap(), "p.input").unwrap()).unwrap()["body"])
                .unwrap()["v"],
            n("3")
        );
    }

    #[test]
    fn proposal_commit_rechecks_original_sources_after_caller_verification() {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-authoring-proposal-candidate.json"
        ))
        .unwrap();
        let case = data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "imported")
            .unwrap();
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let proposal = Proposal {
            subject: "p.new".into(),
            collection: "known".into(),
            body: obj([("v", n("2"))]),
            because: "explicit proposal".into(),
            hypothesis: None,
        };
        let mutation =
            prepare_proposal(&store, &before, &proposal, &options(case), Some(&runtime)).unwrap();
        let err = commit(&store, &mutation, Some(&runtime), &mut |_| {
            std::fs::write(store.root.join("old/a.yaml"), b"changed")?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.0, "retained_history_mismatch");
        assert_eq!(store.capture().unwrap().commits.len(), 1);
    }

    #[test]
    fn typed_recording_dates_replay_without_setting_computational_time() {
        let data: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-authoring.json")).unwrap();
        let case = &data["cases"][1];
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let capture = store.capture().unwrap();
        let mut action = V::from_tagged(&case["action"]).unwrap();
        let date = V::from_tagged(&serde_json::json!(["date", "2026-09-18"])).unwrap();
        map_mut(&mut action)
            .unwrap()
            .insert("as_of".into(), date.clone());
        let mut opts = options(case);
        opts.recording_day.clear();
        let mutation = prepare(&store, &capture, &action, &opts, Some(&runtime)).unwrap();
        verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
        let projected = candidate(&capture, &mutation).unwrap();
        assert_eq!(
            map(&map(head(&projected, "p.input").unwrap()).unwrap()["body"]).unwrap()["of"],
            date
        );
        let world = World::new(
            &document(&capture).unwrap(),
            None,
            Some(&runtime),
            OperationalBounds::default(),
        )
        .unwrap();
        assert_eq!(map(world.snapshot().data()).unwrap()["as_of"], V::Null);
    }
    #[test]
    fn rehashed_computational_forgery_fails_whole_mutation_replay() {
        let data: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-authoring.json")).unwrap();
        let case = &data["cases"][1];
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let capture = store.capture().unwrap();
        let mutation = prepare(
            &store,
            &capture,
            &V::from_tagged(&case["action"]).unwrap(),
            &options(case),
            Some(&runtime),
        )
        .unwrap();
        let data = mutation.to_data();
        let data = map(&data).unwrap();
        let old = map(&data["receipt"]).unwrap();
        let mut after = old["after"].clone();
        map_mut(&mut after)
            .unwrap()
            .insert("forged_evidence".into(), V::Bool(true));
        let receipt =
            T::semantic_receipt("core/v1", &old["capabilities"], &old["before"], &after).unwrap();
        let mut files = mutation.files().to_vec();
        for file in &mut files {
            if file.role == "history_commit" {
                let mut m = Y::decode_document(file.after.as_ref().unwrap()).unwrap();
                map_mut(&mut m)
                    .unwrap()
                    .insert("receipt".into(), receipt.clone());
                file.after = Some(crate::history_emit::encode_document(&m).unwrap());
            }
        }
        let forged = PreparedMutation::prepare(
            "write-1",
            &data["authority"],
            &data["baseline"],
            files,
            &receipt,
            "GROUNDING.yaml",
            None,
        )
        .unwrap();
        assert_eq!(
            verify_prepared(&store, &forged, Some(&runtime))
                .unwrap_err()
                .0,
            "authoring_receipt_mismatch"
        );
        assert_eq!(store.capture().unwrap().commits.len(), 1);
    }

    #[test]
    fn malformed_capture_and_callback_archive_changes_refuse() {
        let data: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-authoring.json")).unwrap();
        let case = &data["cases"][1];
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let captured = store.capture().unwrap();
        let action = V::from_tagged(&case["action"]).unwrap();
        let mut bad = captured.clone();
        map_mut(
            map_mut(
                map_mut(&mut bad.state)
                    .unwrap()
                    .get_mut("subjects")
                    .unwrap(),
            )
            .unwrap()
            .get_mut("p.input")
            .unwrap(),
        )
        .unwrap()
        .insert("head".into(), s(&"a".repeat(64)));
        assert!(prepare(&store, &bad, &action, &options(case), Some(&runtime)).is_err());
        let mutation = prepare(&store, &captured, &action, &options(case), Some(&runtime)).unwrap();
        let err = commit(&store, &mutation, Some(&runtime), &mut |_| {
            std::fs::write(store.root.join(&store.layout.replaced), b"changed")?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.0, "concurrent_archive_edit");
        assert_eq!(store.capture().unwrap().commits.len(), 1);
    }

    #[test]
    fn direct_replay_evaluates_again_and_survives_committed_retry() {
        let data: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-authoring.json")).unwrap();
        let case = &data["cases"][1];
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
        let captured = store.capture().unwrap();
        let action = V::from_tagged(&case["action"]).unwrap();
        let mutation = prepare(&store, &captured, &action, &options(case), Some(&runtime)).unwrap();
        verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
        assert!(
            verify_prepared(&store, &mutation, None).is_err(),
            "replay without a fresh evaluator accepted"
        );
        let old_bytes = captured.object_bytes;
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        let after = store.capture().unwrap();
        for (key, raw) in old_bytes {
            assert_eq!(after.object_bytes[&key], raw);
        }
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        assert_eq!(store.capture().unwrap().commits.len(), 2);
        let before = mutation
            .files()
            .iter()
            .find(|f| f.role == "record")
            .unwrap()
            .before
            .as_ref()
            .unwrap();
        std::fs::write(&store.entry, before).unwrap();
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        assert_eq!(store.capture().unwrap().entry_bytes, after.entry_bytes);
    }
}
