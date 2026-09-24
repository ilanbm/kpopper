//! Exact, same-authority branch union. The reducer preserves concurrent disputes; union
//! neither selects a head nor turns publication ancestry into source-clock ancestry.
use crate::{
    Result, history_contract::*, history_node_capture::Capture, history_node_codec as C,
    history_node_current::Original, history_node_frame as Frame, history_node_publication as P,
    history_node_writer as W, history_view::map_mut, history_yaml as Y, require,
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(crate) const FORMAT: &str = "node-branch-union/v1";
fn s(v: &str) -> V {
    V::Text(v.into())
}
pub(crate) fn is_union(context: &V) -> Result<bool> {
    if crate::history_node_transaction::is_context(context) {
        let c = crate::history_node_transaction::validate(context)?;
        return Ok(
            string_is(&c["format"], crate::history_node_transaction::FORMAT)
                && map(&c["action"])?
                    .get("kind")
                    .is_some_and(|v| string_is(v, "branch-union")),
        );
    }
    Ok(map(context)?
        .get("format")
        .is_some_and(|v| string_is(v, FORMAT)))
}
pub(crate) fn evidence(context: &V) -> Result<V> {
    if crate::history_node_transaction::is_context(context) {
        let c = crate::history_node_transaction::validate(context)?;
        require(
            map(&c["action"])?
                .get("kind")
                .is_some_and(|v| string_is(v, "branch-union")),
            "node_transaction_action",
        )?;
        map(&c["evidence"])?;
        return Ok(c["evidence"].clone());
    }
    let c = W::context(context)?;
    let o = schema(&c["options"], &["evidence"], &["adoption"])?;
    map(&o["evidence"])?;
    Ok(o["evidence"].clone())
}
fn tips(versions: &BTreeMap<String, C::Version>) -> Vec<&C::Version> {
    let parents = versions
        .values()
        .flat_map(|v| v.parents())
        .collect::<BTreeSet<_>>();
    versions
        .iter()
        .filter(|(id, _)| !parents.contains(id))
        .map(|(_, v)| v)
        .collect()
}
fn state(version: &C::Version) -> Result<&V> {
    version
        .state()
        .ok_or_else(|| error("node_semantic_absent_unsupported"))
}
fn merge_snapshot(target: &mut P::Snapshot, source: &P::Snapshot) -> Result<()> {
    require(
        target.authority == source.authority,
        "node_branch_authority",
    )?;
    target.source_clocks.merge(&source.source_clocks)?;
    target.legacy.extend(source.legacy.clone());
    target.raw_evidence.extend(source.raw_evidence.clone());
    for (op, tx) in &source.transactions {
        require(
            target
                .transactions
                .get(op)
                .is_none_or(|old| old.digest == tx.digest),
            "node_import_collision",
        )?;
        target.transactions.insert(op.clone(), tx.clone());
    }
    for (id, op) in &source.operations {
        require(
            target.operations.get(id).is_none_or(|old| old == op),
            "node_import_collision",
        )?;
        target.operations.insert(id.clone(), op.clone());
    }
    for (subject, versions) in &source.versions {
        let destination = target.versions.entry(subject.clone()).or_default();
        for (id, version) in versions {
            require(
                destination
                    .get(id)
                    .is_none_or(|old| old.frame_sha256() == version.frame_sha256()),
                "node_import_collision",
            )?;
            destination.insert(id.clone(), version.clone());
        }
    }
    require(
        target.operations.len() <= C::MAX_EVENTS && target.transactions.len() <= C::MAX_EVENTS,
        "node_publication_limit",
    )
}
struct Plan {
    after: Vec<u8>,
    context: V,
    joins: BTreeMap<String, Vec<C::Event>>,
    lazy: BTreeSet<String>,
}
fn plan(
    target: &P::Snapshot,
    union: P::Snapshot,
    operation: &str,
    evidence: &BTreeMap<String, Vec<u8>>,
    adoption: Option<&V>,
) -> Result<Plan> {
    token(&s(operation))?;
    require(
        !union.transactions.contains_key(operation),
        "operation_already_prepared",
    )?;
    let capture = Capture::from_union(union)?;
    let (candidate, objects) = if let Some(adoption) = adoption {
        crate::history_node_adoption::apply(target, &capture, operation, adoption)?
    } else {
        (capture.clone(), vec![])
    };
    let document = crate::history_authoring::destination(candidate.document())?;
    let capabilities = crate::reasoning_fields::capabilities(&document, None)?;
    let profile = text(field(map(&capabilities)?, "profile")?)?;
    let before_side = V::Map(Map::from([(
        "document".into(),
        crate::history_authoring::destination(capture.document())?,
    )]));
    let side = V::Map(Map::from([("document".into(), document.clone())]));
    let diagnostic_receipt =
        crate::history_transaction::semantic_receipt(profile, &capabilities, &before_side, &side)?;
    let mut bases = BTreeMap::new();
    for (subject, versions) in &capture.snapshot.versions {
        let held = tips(versions);
        require(!held.is_empty(), "node_semantic_merge_required")?;
        bases.insert(subject.clone(), held);
    }
    let mut objects_by_subject = BTreeMap::<String, Vec<V>>::new();
    for object in objects {
        let subject = text(&map(&object)?["subject"])?.to_owned();
        objects_by_subject.entry(subject).or_default().push(object);
    }
    let mut joins = BTreeMap::new();
    let mut lazy = BTreeSet::new();
    let mut originals = Map::new();
    let mut tails = Map::new();
    let target_doc = Y::decode_document(target.current.as_deref().unwrap())?;
    let target_history = map(&target_doc)?
        .get("meta")
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("node_history"))
        .and_then(|v| map(v).ok());
    let target_origins = P::current_origins(
        target
            .current
            .as_deref()
            .ok_or_else(|| error("node_semantic_missing_view"))?,
    )?;
    for (subject, held) in bases {
        let base = held[0];
        let mut events = Vec::new();
        let subject_objects = objects_by_subject.get(&subject);
        let mut semantic = Vec::with_capacity(subject_objects.map_or(0, Vec::len));
        if let Some(subject_objects) = subject_objects {
            // Observation ancestry is a deterministic compression choice, separate
            // from the storage event's parent and merge vector.
            let mut observation_chain = capture.history.observations().clone();
            let mut observation_base = capture
                .history
                .objects()
                .iter()
                .filter(|(_, object)| {
                    map(object).is_ok_and(|value| string_is(&value["subject"], &subject))
                })
                .map(|(id, _)| id.clone())
                .next_back();
            for object in subject_objects {
                let o = map(object)?;
                let saw = crate::history_view::list(&o["saw"])?
                    .iter()
                    .map(|v| text(v).map(str::to_owned))
                    .collect::<Result<BTreeSet<_>>>()?;
                let observation = if let Some(old_id) = &observation_base {
                    crate::history_node_observation::ObservationNode::between(
                        text(&o["id"])?,
                        old_id,
                        &observation_chain.replay(old_id)?,
                        &saw,
                    )?
                } else {
                    crate::history_node_observation::ObservationNode::root(text(&o["id"])?, &saw)?
                };
                observation_chain.insert(observation.clone())?;
                observation_base = Some(text(&o["id"])?.to_owned());
                semantic.push((object.clone(), observation));
            }
        }
        if held.len() > 1 || !semantic.is_empty() {
            // A union touches each subject at most once per operation. The selected
            // storage base only controls the ledger delta; all storage heads remain
            // in the deterministic merge parent vector, independent of observation bases.
            let value = crate::history_node_ledger::pack(Some(state(base)?), &semantic)?;
            let event = if held.len() > 1 {
                C::Event::create_merge(
                    &subject,
                    operation,
                    held.iter().map(|v| v.id().into()).collect(),
                    base,
                    Some(value),
                )?
            } else {
                C::Event::create(
                    &subject,
                    operation,
                    vec![base.id().into()],
                    Some(base),
                    Some(value),
                )?
            };
            events.push(event);
        }
        if !events.is_empty() {
            joins.insert(subject, events);
            continue;
        }
        // A singleton initial version whose body is still visible remains in the current view.
        // Existing physical target streams are never removed or rewritten.
        let versions = &capture.snapshot.versions[&subject];
        if target
            .versions
            .get(&subject)
            .is_some_and(|old| old.keys().eq(versions.keys()))
        {
            if let Some(history) = target_history {
                if let Some(binding) = map(&history["originals"])?.get(&subject) {
                    if Original::decode(binding)?.from_document(&document).is_ok() {
                        originals.insert(subject.clone(), binding.clone());
                        if let Some(tail) = history
                            .get("tails")
                            .and_then(|v| map(v).ok())
                            .and_then(|m| m.get(&subject))
                        {
                            tails.insert(subject.clone(), tail.clone());
                        }
                        lazy.insert(subject);
                        continue;
                    }
                }
            }
        }
        let target_physical = target.versions.contains_key(&subject)
            && !target_origins.keys().any(|(s, _)| s == &subject);
        if versions.len() == 1 && base.parents().is_empty() && !target_physical {
            let p = map(state(base)?)?;
            let original = Original::create(
                &subject,
                text(&p["collection"])?,
                base.operation(),
                p["body"].clone(),
                map(&p["context"])?.clone(),
            )?;
            if let Ok(event) = original.from_document(&document) {
                if event.id() == base.id() {
                    originals.insert(subject.clone(), original.encode()?);
                    lazy.insert(subject);
                }
            }
        }
    }
    let mut doc = document;
    map_mut(
        map_mut(&mut doc)?
            .get_mut("meta")
            .ok_or_else(|| error("invalid_document"))?,
    )?
    .insert(
        "node_history".into(),
        V::Map(Map::from([
            ("version".into(), V::from_json(&serde_json::json!(2))?),
            ("originals".into(), V::Map(originals)),
            ("tails".into(), V::Map(tails)),
        ])),
    );
    let mut action = Map::from([("kind".into(), s("branch-union"))]);
    if let Some(adoption) = adoption {
        action.insert("adoption".into(), adoption.clone());
    }
    let mut options = Map::from([(
        "evidence".into(),
        V::Map(
            evidence
                .iter()
                .map(|(p, raw)| (p.clone(), s(&crate::identity::sha256(raw))))
                .collect(),
        ),
    )]);
    if let Some(adoption) = adoption {
        options.insert("adoption".into(), adoption.clone());
    }
    let context = crate::history_node_transaction::create(
        &capture.snapshot,
        &V::Map(action),
        V::Map(options),
        &V::Map(Map::new()),
        evidence,
        &doc,
        &diagnostic_receipt,
    )?;
    Ok(Plan {
        after: P::bind_view(&Y::encode_document(&doc)?, operation)?,
        context,
        joins,
        lazy,
    })
}
fn frames(
    snapshot: &P::Snapshot,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, BTreeMap<String, Vec<u8>>>> {
    let mut result = BTreeMap::<String, BTreeMap<String, Vec<u8>>>::new();
    for subject in snapshot.versions.keys() {
        let path = format!(".kpopper/history/{}", C::subject_path(subject)?);
        if let Some(raw) = files.get(&path) {
            for raw in raw.split_inclusive(|v| *v == b'\n') {
                let event = C::Event::decode(raw, subject)?;
                result
                    .entry(subject.clone())
                    .or_default()
                    .insert(event.id().into(), raw.to_vec());
            }
        }
    }
    for ((subject, id), event) in P::current_origins(snapshot.current.as_deref().unwrap())? {
        result
            .entry(subject)
            .or_default()
            .insert(id, event.encode()?);
    }
    Ok(result)
}
/// Prepare exact local branch union from audited portable captures. Sources and target must
/// share the same authority marker. Conflicts remain disputed and require an explicit later act.
/// No source refs or external paths are reread by publication or recovery.
pub fn prepare(root: &Path, sources: &[P::Bundle], operation: &str) -> Result<P::Prepared> {
    prepare_inner(root, sources, operation, None)
}
pub(crate) fn prepare_adoption(
    root: &Path,
    sources: &[P::Bundle],
    operation: &str,
    adoption: &V,
) -> Result<P::Prepared> {
    prepare_inner(root, sources, operation, Some(adoption))
}
fn prepare_inner(
    root: &Path,
    sources: &[P::Bundle],
    operation: &str,
    adoption: Option<&V>,
) -> Result<P::Prepared> {
    require(
        !sources.is_empty() && sources.len() <= 16,
        "node_branch_source_limit",
    )?;
    let target_bundle = P::export(root)?;
    let target_copy = target_bundle.reconstruct()?;
    let target_capture = Capture::read(target_copy.path())?;
    let target = target_capture.snapshot;
    let target_files = target_bundle.files()?;
    let mut union = target.clone();
    let mut raw = frames(&target, &target_files)?;
    let mut imports = BTreeMap::new();
    let mut evidence = BTreeMap::new();
    let mut input_bytes = target_files.values().map(Vec::len).sum::<usize>();
    for source in sources {
        let files = source.files()?;
        input_bytes = input_bytes.saturating_add(files.values().map(Vec::len).sum::<usize>());
        require(input_bytes <= 64 * 1024 * 1024, "node_branch_input_limit")?;
        let copy = source.reconstruct()?;
        let capture = Capture::read(copy.path())?;
        require(
            files[".kpopper/history.yaml"] == target_files[".kpopper/history.yaml"],
            "node_branch_authority",
        )?;
        for (subject, incoming) in frames(&capture.snapshot, &files)? {
            let held = raw.entry(subject).or_default();
            for (id, bytes) in incoming {
                require(
                    held.get(&id).is_none_or(|old| old == &bytes),
                    "node_import_collision",
                )?;
                held.insert(id, bytes);
            }
        }
        for op in capture
            .snapshot
            .transactions
            .keys()
            .filter(|op| !target.transactions.contains_key(*op))
        {
            let bytes = &files[&format!(".kpopper/history-commits/{op}.json")];
            require(
                imports.get(op).is_none_or(|old| old == bytes),
                "node_import_collision",
            )?;
            imports.insert(op.clone(), bytes.clone());
        }
        for tx in capture.snapshot.transactions.values() {
            for path in tx.evidence.keys() {
                let bytes = &files[path];
                if let Some(old) = target_files.get(path) {
                    require(old == bytes, "node_import_collision")?;
                    continue;
                }
                require(
                    evidence.get(path).is_none_or(|old| old == bytes),
                    "node_import_collision",
                )?;
                evidence.insert(path.clone(), bytes.clone());
            }
        }
        merge_snapshot(&mut union, &capture.snapshot)?;
    }
    require(
        !imports.is_empty() || adoption.is_some(),
        "node_branch_no_new_history",
    )?;
    let built = plan(&target, union, operation, &evidence, adoption)?;
    let mut appends = BTreeMap::new();
    for (subject, versions) in raw {
        if built.lazy.contains(&subject) {
            continue;
        }
        let physical =
            target_files.contains_key(&format!(".kpopper/history/{}", C::subject_path(&subject)?));
        let existing = target.versions.get(&subject);
        let mut bytes = Vec::new();
        for (id, raw) in versions {
            if !physical || existing.is_none_or(|v| !v.contains_key(&id)) {
                bytes.extend(raw);
            }
        }
        if let Some(events) = built.joins.get(&subject) {
            for event in events {
                bytes.extend(event.encode()?);
            }
        }
        if !bytes.is_empty() {
            appends.insert(subject, bytes);
        }
    }
    let prepared = P::prepare_union(
        root,
        operation,
        built.after,
        appends,
        &built.context,
        evidence,
        imports,
    )?;
    verify(root, &prepared)?;
    Ok(prepared)
}
/// Rebuild the deterministic merge using only the journal and verified immutable closure.
/// Imported events remain bound to their original manifests, receipts and semantic identities.
pub(crate) fn verify(root: &Path, prepared: &P::Prepared) -> Result<()> {
    crate::history_node_hypothesis::sources(root)?;
    require(
        prepared.canonical_before()?.is_none(),
        "node_import_edited_view",
    )?;

    let (target, after) = prepared.snapshots(root)?;
    Capture::from_snapshot(target.clone())?;
    Capture::from_snapshot(after.clone())?;
    let mut union = after.clone();
    let operation = prepared.operation();
    union.transactions.remove(operation);
    union.operations.retain(|_, op| op != operation);
    for versions in union.versions.values_mut() {
        versions.retain(|_, v| v.operation() != operation);
    }
    union.versions.retain(|_, v| !v.is_empty());
    union.current = target.current.clone();
    let context = prepared
        .context()?
        .ok_or_else(|| error("node_transaction_missing_context"))?;
    let adoption = if crate::history_node_transaction::is_context(&context) {
        map(&crate::history_node_transaction::validate(&context)?["action"])?.get("adoption")
    } else {
        map(&W::context(&context)?["options"])?.get("adoption")
    };
    require(
        !prepared.imports()?.is_empty() || adoption.is_some(),
        "node_branch_no_new_history",
    )?;
    let expected = plan(&target, union, operation, &prepared.evidence()?, adoption)?;
    let actual = after
        .versions
        .iter()
        .flat_map(|(s, versions)| {
            versions
                .values()
                .filter(|v| v.operation() == operation)
                .map(move |v| ((s.clone(), v.id().to_owned()), v.frame_sha256().to_owned()))
        })
        .collect::<BTreeMap<_, _>>();
    let wanted = expected
        .joins
        .iter()
        .flat_map(|(s, events)| events.iter().map(move |event| (s, event)))
        .map(|(s, event)| {
            Ok((
                (s.clone(), event.id().to_owned()),
                crate::identity::sha256(&event.encode()?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    require(
        actual == wanted
            && prepared.after_view()? == expected.after
            && prepared.context()?.as_ref() == Some(&expected.context),
        "node_branch_replay_mismatch",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history_authoring::Options, history_paths::Scheme};
    use serde_json::json;
    fn value(j: serde_json::Value) -> V {
        V::from_json(&j).unwrap()
    }
    #[test]
    fn compact_ledger_root_can_keep_its_exact_lazy_original_binding() {
        let subject = "p.ledger";
        let payload = crate::history_node_ledger::pack(None, &[]).unwrap();
        let p = map(&payload).unwrap();
        let original = Original::create(
            subject,
            text(&p["collection"]).unwrap(),
            "seed",
            p["body"].clone(),
            map(&p["context"]).unwrap().clone(),
        )
        .unwrap();
        let event = C::Event::create(subject, "seed", vec![], None, Some(payload)).unwrap();
        let document = V::Map(Map::from([(
            "acts".into(),
            V::Map(Map::from([(subject.into(), V::Null)])),
        )]));
        assert_eq!(original.from_document(&document).unwrap().id(), event.id());
    }
    fn write(root: &Path, op: &str, action: V) {
        let options = Options {
            operation: op.into(),
            recorded_at: "2026-09-24T12:00:00+00:00".into(),
            recording_day: "2026-09-24".into(),
            by: s("fixture"),
            strict: false,
            paths: Scheme::Hashed,
            receipt_version: None,
        };
        let p = W::prepare(root, &action, &options, None).unwrap();
        W::publish(root, &p, None, |_| Ok(())).unwrap();
    }
    #[test]
    fn replay_refuses_an_extra_same_subject_evidence_frame() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".kpopper")).unwrap();
        std::fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
        std::fs::write(root.path().join("GROUNDING.yaml"), "meta: {purpose: Fixture}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {}\n").unwrap();
        write(
            root.path(),
            "seed",
            value(json!({"kind":"add","id":"p.a","body":{"v":1}})),
        );
        let source = P::export(root.path()).unwrap().reconstruct().unwrap();
        write(
            root.path(),
            "left",
            value(json!({"kind":"set","id":"p.a","value":2})),
        );
        write(
            source.path(),
            "right",
            value(json!({"kind":"set","id":"p.a","value":3})),
        );
        let bundle = P::export(source.path()).unwrap();
        // Choose a deterministic counterexample where collecting by subject alone would hide
        // the extra frame behind the greater original ID. No random fixture or model call.
        for index in 0..32 {
            let operation = format!("merge-{index}");
            let p = prepare(root.path(), std::slice::from_ref(&bundle), &operation).unwrap();
            let (_, after) = p.snapshots(root.path()).unwrap();
            let base = Frame::tip(&after.versions["p.a"]).unwrap().unwrap();
            let extra = C::Event::create(
                "p.a",
                &operation,
                vec![base.id().into()],
                Some(base),
                base.state().cloned(),
            )
            .unwrap();
            if extra.id() >= base.id() {
                continue;
            }
            let mut frames = p.frames().unwrap();
            frames
                .get_mut("p.a")
                .unwrap()
                .extend(extra.encode().unwrap());
            let forged = P::prepare_union(
                root.path(),
                &operation,
                p.after_view().unwrap(),
                frames,
                p.context().unwrap().as_ref().unwrap(),
                p.evidence().unwrap(),
                p.imports().unwrap(),
            )
            .unwrap();
            assert_eq!(
                verify(root.path(), &forged).unwrap_err().0,
                "node_branch_replay_mismatch"
            );
            assert!(W::publish(root.path(), &forged, None, |_| Ok(())).is_err());
            assert!(
                !root
                    .path()
                    .join(".kpopper/.history-node-publication.json")
                    .exists()
            );
            return;
        }
        panic!("counterexample fixture not found");
    }
}
