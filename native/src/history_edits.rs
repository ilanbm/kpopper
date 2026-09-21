//! Complete generated-view body edits become proposals with exact retained source bytes.
use crate::{
    Result,
    history_authoring::{self as A, Options, Proposal},
    history_authoring_audit::ReplayAudit,
    history_authority as Authority,
    history_capture::Capture,
    history_contract::*,
    history_hypothesis_authoring as HH, history_preparation as P,
    history_store::Store,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_transaction_fs as FS,
    history_view::{self as View, list, map_mut, truth},
    history_yaml as Y,
    identity::sha256,
    reasoning_runtime::Runtime,
    reasoning_snapshot::entries,
    require,
    value::TypedValue as V,
};
use A::{n, obj, s, strings};
use std::collections::{BTreeMap, BTreeSet};

type Selection = (Capture, Vec<String>, BTreeMap<String, (String, V)>);
fn selection(store: &Store, capture: &Capture, subjects: Option<&[String]>) -> Result<Selection> {
    require(!capture.commits.is_empty(), "history_bootstrap_required")?;
    require(
        field(map(field(map(&capture.document)?, "meta")?)?, "history")?.digest()?
            == capture.baseline.digest()?,
        "stale_edit_baseline",
    )?;
    let original = store.render(capture)?;
    let document = Y::decode_document(&original)?;
    let before_template = Authority::document_template(&document)?;
    let after_template = Authority::document_template(&capture.document)?;
    let before_map = map(&before_template)?;
    let after_map = map(&after_template)?;
    for key in before_map.keys().chain(after_map.keys()) {
        require(
            before_map.get(key).unwrap_or(&V::Null).digest()?
                == after_map.get(key).unwrap_or(&V::Null).digest()?,
            "template_disposition_required",
        )?;
    }
    let before = entries(&document)?;
    let after = entries(&capture.document)?;
    require(
        before.keys().all(|id| after.contains_key(id)),
        "deletion_disposition_required",
    )?;
    let mut changed = BTreeSet::new();
    for (id, (collection, body)) in &after {
        if let Some((old_collection, old_body)) = before.get(id) {
            require(
                old_collection == collection,
                "collection_disposition_required",
            )?;
            if old_body.digest()? == body.digest()? {
                continue;
            }
        }
        changed.insert(id.clone());
    }
    require(!changed.is_empty(), "no_body_proposals")?;
    let selected = subjects
        .map(|v| v.to_vec())
        .unwrap_or_else(|| changed.iter().cloned().collect());
    let selected_set = selected.iter().cloned().collect::<BTreeSet<_>>();
    require(
        selected_set.len() == selected.len(),
        "invalid_edit_subjects",
    )?;
    require(selected_set == changed, "unhandled_view_edits")?;
    require(selected.len() <= 64, "history_limit")?;
    let mut canonical = capture.clone();
    canonical.document = document;
    canonical.entry_bytes = original;
    Ok((canonical, selected_set.into_iter().collect(), after))
}

fn prepare_inner(
    store: &Store,
    capture: &Capture,
    because: &str,
    subjects: Option<&[String]>,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    require(!because.trim().is_empty(), "act_reason_required")?;
    require(
        !capture.commits.contains_key(&options.operation),
        "operation_already_prepared",
    )?;
    require(
        capture.entry_bytes.len() <= 16 * 1024 * 1024,
        "history_limit",
    )?;
    require(options.strict, "explicit_root_disposition_required")?;
    let (canonical, subjects, authored) = selection(store, capture, subjects)?;
    let archive = A::archive(store)?;
    let mut objects = vec![];
    let mut steps = vec![];
    let mut first = None;
    for (index, subject) in subjects.iter().enumerate() {
        let (collection, body) = &authored[subject];
        let operation = format!(
            "edit-step-{}",
            obj([
                ("operation", s(&options.operation)),
                ("index", n(&index.to_string()))
            ])
            .digest()?
        );
        let mut step_options = options.clone();
        step_options.operation = operation.clone();
        // Edited-view receipt1 retains historical user-authored seen. New direct
        // proposals use receipt9 and do not accept supplied snapshots.
        step_options.receipt_version = Some(5);
        let proposal = Proposal {
            subject: subject.clone(),
            body: body.clone(),
            collection: collection.clone(),
            because: because.into(),
            hypothesis: None,
        };
        let mutation =
            A::prepare_proposal_inner(store, &canonical, &proposal, &step_options, runtime, audit)?;
        let receipt = map(&mutation.to_data())?["receipt"].clone();
        if first.is_none() {
            first = Some(receipt.clone());
        }
        steps.push(obj([
            ("operation", s(&operation)),
            ("receipt_digest", map(&receipt)?["digest"].clone()),
        ]));
        for file in mutation
            .files()
            .iter()
            .filter(|f| f.role == "history_object")
        {
            objects.push(Y::decode_document(
                file.after
                    .as_ref()
                    .ok_or_else(|| error("invalid_mutation"))?,
            )?);
        }
    }
    let path = if store.layout.home.is_empty() {
        format!("evidence/view-edits/{}.yaml", options.operation)
    } else {
        format!(
            "{}/evidence/view-edits/{}.yaml",
            store.layout.home, options.operation
        )
    };
    let first = first.ok_or_else(|| error("no_body_proposals"))?;
    let first = map(&first)?;
    let mut before = first["before"].clone();
    let text =
        std::str::from_utf8(&capture.entry_bytes).map_err(|_| error("invalid_edit_evidence"))?;
    map_mut(&mut before)?.insert(
        "authoring".into(),
        obj([
            ("version", n("1")),
            ("kind", s("view-edit-proposals")),
            ("because", s(because)),
            ("by", options.by.clone()),
            ("recorded_at", s(&options.recorded_at)),
            ("subjects", strings(subjects)),
            ("baseline", capture.baseline.clone()),
            ("archive", archive.clone()),
            ("original_view_sha256", s(&sha256(&canonical.entry_bytes))),
            ("edited_view_sha256", s(&sha256(&capture.entry_bytes))),
            ("edited_view_utf8", s(text)),
            (
                "evidence",
                V::Map(Map::from([(
                    path.clone(),
                    s(&sha256(&capture.entry_bytes)),
                )])),
            ),
            ("template_disposition", s("unchanged")),
        ]),
    );
    map_mut(&mut before)?.insert(
        "history_edit".into(),
        obj([("version", n("1")), ("kind", s("view-edit-proposals"))]),
    );
    let mut after = first["before"].clone();
    let ids = objects
        .iter()
        .map(text_id)
        .collect::<Result<BTreeSet<_>>>()?;
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([
            ("steps", V::List(steps)),
            (
                "objects",
                strings(
                    ids.into_iter()
                        .filter(|id| !canonical.objects.contains_key(id)),
                ),
            ),
            ("disposition", s("proposed")),
            ("complete_body_capture", V::Bool(true)),
        ]),
    );
    let receipt = T::semantic_receipt(
        text_profile(first)?,
        &first["capabilities"],
        &before,
        &after,
    )?;
    let evidence = FileImage {
        path,
        role: "history_evidence".into(),
        before: None,
        after: Some(capture.entry_bytes.clone()),
    };
    let prepared = P::prepare_commit_with_files(
        &canonical,
        &options.operation,
        &objects,
        &View::template(&canonical.commits)?,
        &receipt,
        options.requires().as_ref(),
        &[evidence],
    )?;
    let mut files = prepared.files().to_vec();
    for file in &mut files {
        if file.role == "record" {
            file.before = Some(capture.entry_bytes.clone());
        }
    }
    require(A::archive(store)? == archive, "concurrent_archive_edit")?;
    PreparedMutation::prepare(
        &options.operation,
        &capture.marker,
        &capture.baseline,
        files,
        &receipt,
        &store.layout.entry,
        None,
    )
}
fn text_id(value: &V) -> Result<String> {
    text(&map(value)?["id"]).map(str::to_owned)
}
fn text_profile(value: &Map) -> Result<&str> {
    text(&value["profile"])
}

pub fn prepare_proposals(
    store: &Store,
    because: &str,
    subjects: Option<&[String]>,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    let capture = store.capture_reconciliation(true)?;
    let description = store.prepare_reconciliation(Some(&capture), true, &[])?;
    require(
        !truth(field(map(&description)?, "conflicted")?),
        "conflict_disposition_required",
    )?;
    let prepared = prepare_inner(store, &capture, because, subjects, options, runtime, None)?;
    require(
        store.capture()?.inventory == capture.inventory,
        "stale_baseline",
    )?;
    Ok(prepared)
}

pub fn raw_edit_evidence(receipt: &V) -> Result<Vec<u8>> {
    T::validate_receipt(receipt)?;
    let before = map(&map(receipt)?["before"])?;
    require(
        before.get("history_edit")
            == Some(&obj([
                ("version", n("1")),
                ("kind", s("view-edit-proposals")),
            ])),
        "invalid_edit_receipt",
    )?;
    let intent = map(field(before, "authoring")?)?;
    let raw = text(field(intent, "edited_view_utf8")?)
        .map_err(|_| error("invalid_edit_evidence"))?
        .as_bytes();
    require(raw.len() <= 16 * 1024 * 1024, "history_limit")?;
    let evidence = map(field(intent, "evidence")?).map_err(|_| error("edit_evidence_mismatch"))?;
    require(
        field(intent, "edited_view_sha256")? == &s(&sha256(raw))
            && evidence.len() == 1
            && evidence.values().next() == Some(&s(&sha256(raw))),
        "edit_evidence_mismatch",
    )?;
    Ok(raw.to_vec())
}
pub fn verify_prepared(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    let (capture, options, intent, audit) =
        HH::replay_context(store, mutation, "authoring", false)?;
    let intent = map(&intent)?;
    require(
        is_int(field(intent, "version")?, "1")
            && string_is(field(intent, "kind")?, "view-edit-proposals"),
        "invalid_edit_receipt",
    )?;
    require(
        raw_edit_evidence(&map(&mutation.to_data())?["receipt"])? == capture.entry_bytes,
        "edit_evidence_mismatch",
    )?;
    let subjects = list(field(intent, "subjects")?)?
        .iter()
        .map(|v| text(v).map(str::to_owned))
        .collect::<Result<Vec<_>>>()?;
    let expected = prepare_inner(
        store,
        &capture,
        text(field(intent, "because")?)?,
        Some(&subjects),
        &options,
        runtime,
        Some(&audit),
    )?;
    require(
        expected.to_bytes()? == mutation.to_bytes()?,
        "edit_receipt_mismatch",
    )
}
pub fn commit(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
    verify: FS::Verify<'_>,
) -> Result<V> {
    store.commit_edits(mutation, runtime, &mut |data| {
        verify(data)?;
        let intent = map(field(
            map(&map(&map(data)?["receipt"])?["before"])?,
            "authoring",
        )?)?;
        require(
            A::archive(store)? == *field(intent, "archive")?,
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
mod tests {
    use super::*;
    use serde_json::Value as J;
    fn runtime(cache: &std::path::Path) -> Runtime {
        Runtime::open(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.kpopper-runtime",
                    crate::reasoning_runtime::target_name().unwrap()
                )),
            cache,
            crate::reasoning_runtime::OperationalBounds::default(),
        )
        .unwrap()
    }
    fn write(root: &std::path::Path, case: &J) {
        for (path, raw) in case["files"].as_object().unwrap() {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw.as_str().unwrap()).unwrap();
        }
    }
    fn options() -> Options {
        Options {
            operation: "edit-1".into(),
            recorded_at: "2026-09-19T09:30:00+00:00".into(),
            recording_day: "2026-09-19".into(),
            by: s("writer"),
            strict: true,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        }
    }
    fn case(name: &str) -> J {
        let data: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-edits-candidate.json"
        ))
        .unwrap();
        data["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
            .clone()
    }
    #[test]
    fn edited_view_proposals_match_retained_and_candidate_receipt5_envelopes() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let mut failures = vec![];
        for raw in [
            include_str!("../tests/fixtures/history-edits.json"),
            include_str!("../tests/fixtures/history-edits-candidate.json"),
        ] {
            let data: J = serde_json::from_str(raw).unwrap();
            for case in data["cases"].as_array().unwrap() {
                let root = tempfile::tempdir().unwrap();
                write(root.path(), case);
                let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
                let capture = store.capture_reconciliation(true).unwrap();
                let subjects = case["subjects"].as_array().map(|v| {
                    v.iter()
                        .map(|s| s.as_str().unwrap().into())
                        .collect::<Vec<_>>()
                });
                let audit = case
                    .get("receipt")
                    .map(|r| ReplayAudit::oracle(&V::from_tagged(r).unwrap()).unwrap());
                let result = prepare_inner(
                    &store,
                    &capture,
                    case["because"].as_str().unwrap(),
                    subjects.as_deref(),
                    &options(),
                    Some(&runtime),
                    audit.as_ref(),
                );
                match (case.get("output"), result) {
                    (Some(expected), Ok(actual)) => {
                        assert_eq!(
                            raw_edit_evidence(&map(&actual.to_data()).unwrap()["receipt"]).unwrap(),
                            case["evidence"].as_str().unwrap().as_bytes()
                        );
                        if actual.to_bytes().unwrap() != expected.as_str().unwrap().as_bytes() {
                            let label = case["name"].as_str().unwrap();
                            std::fs::write(
                                std::env::temp_dir().join(format!("edits-{label}-actual.json")),
                                actual.to_bytes().unwrap(),
                            )
                            .unwrap();
                            std::fs::write(
                                std::env::temp_dir().join(format!("edits-{label}-expected.json")),
                                expected.as_str().unwrap(),
                            )
                            .unwrap();
                            failures.push(format!("{label}: byte mismatch"));
                        }
                    }
                    (Some(_), Err(e)) => failures.push(format!("{}: {e}", case["name"])),
                    (None, Ok(_)) => {
                        failures.push(format!("{}: unexpectedly accepted", case["name"]))
                    }
                    (None, Err(_)) => {}
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    #[test]
    fn edited_bytes_remain_evidence_and_replay_never_accepts_their_claims() {
        let case = case("two-bodies");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let raw = std::fs::read(&store.entry).unwrap();
        let before = store.capture().unwrap();
        let prepared = prepare_proposals(
            &store,
            "capture exact authored edits",
            None,
            &options(),
            Some(&runtime),
        )
        .unwrap();
        assert_eq!(std::fs::read(&store.entry).unwrap(), raw);
        verify_prepared(&store, &prepared, Some(&runtime)).unwrap();
        assert!(verify_prepared(&store, &prepared, None).is_err());
        assert_eq!(
            store.commit(&prepared, &mut |_| Ok(())).unwrap_err().0,
            "unsupported_view_edit_replay"
        );
        assert!(
            store
                .commit_edits(&prepared, None, &mut |_| Ok(()))
                .is_err()
        );
        commit(&store, &prepared, Some(&runtime), &mut |_| Ok(())).unwrap();
        let accepted = store.capture().unwrap();
        let document = A::document(&accepted).unwrap();
        assert!(!entries(&document).unwrap().contains_key("p.new"));
        assert_eq!(
            map(&entries(&document).unwrap()["p.input"].1).unwrap()["v"],
            n("1")
        );
        for (key, bytes) in before.object_bytes {
            assert_eq!(accepted.object_bytes[&key], bytes);
        }
        let evidence = prepared
            .files()
            .iter()
            .find(|f| f.role == "history_evidence")
            .unwrap();
        assert_eq!(std::fs::read(store.root.join(&evidence.path)).unwrap(), raw);
        commit(&store, &prepared, Some(&runtime), &mut |_| Ok(())).unwrap();
        std::fs::write(&store.entry, &raw).unwrap();
        commit(&store, &prepared, Some(&runtime), &mut |_| Ok(())).unwrap();
        assert_eq!(store.capture().unwrap().entry_bytes, accepted.entry_bytes);
    }
    #[test]
    fn edited_evidence_refuses_a_self_consistent_receipt_with_a_different_source_hash() {
        let case = case("supplied-seen");
        let receipt = V::from_tagged(&case["receipt"]).unwrap();
        let old = map(&receipt).unwrap();
        let mut before = old["before"].clone();
        map_mut(map_mut(&mut before).unwrap().get_mut("authoring").unwrap())
            .unwrap()
            .insert("edited_view_utf8".into(), s("forged bytes"));
        let forged =
            T::semantic_receipt("core/v1", &old["capabilities"], &before, &old["after"]).unwrap();
        assert_eq!(
            raw_edit_evidence(&forged).unwrap_err().0,
            "edit_evidence_mismatch"
        );
    }
    #[test]
    fn storage_replays_rehashed_edits_and_rechecks_archive_after_callback() {
        let case = case("changed-reading");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let original = std::fs::read(&store.entry).unwrap();
        let mutation = prepare_proposals(
            &store,
            "capture exact authored edits",
            None,
            &options(),
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
                let mut manifest = Y::decode_document(file.after.as_ref().unwrap()).unwrap();
                map_mut(&mut manifest)
                    .unwrap()
                    .insert("receipt".into(), receipt.clone());
                file.after = Some(crate::history_emit::encode_document(&manifest).unwrap());
            }
        }
        let forged = PreparedMutation::prepare(
            "edit-1",
            &data["authority"],
            &data["baseline"],
            files,
            &receipt,
            "GROUNDING.yaml",
            None,
        )
        .unwrap();
        assert_eq!(
            store
                .commit_edits(&forged, Some(&runtime), &mut |_| Ok(()))
                .unwrap_err()
                .0,
            "edit_receipt_mismatch"
        );
        let err = commit(&store, &mutation, Some(&runtime), &mut |_| {
            std::fs::write(store.root.join(&store.layout.replaced), b"changed archive")?;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.0, "concurrent_archive_edit");
        assert_eq!(std::fs::read(&store.entry).unwrap(), original);
        assert_eq!(store.capture().unwrap().commits.len(), 1);
    }
}
