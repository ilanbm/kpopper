//! Explicit identity intents over immutable current claims and named proposals.
use crate::{
    Result, history_adapter as D,
    history_authoring::{self as A, Options, empty, n, obj, s, strings},
    history_authoring_audit::ReplayAudit,
    history_capture::Capture,
    history_contract::*,
    history_hypotheses as HH, history_hypothesis_authoring as HA,
    history_preparation as Preparation,
    history_store::Store,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_transaction_fs as FS,
    history_view::{self as View, map_mut},
    history_yaml::{self as Y, SourceValue as Source},
    identity::sha256,
    reasoning_fields as F,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use std::collections::BTreeSet;

#[derive(Clone)]
pub enum Action {
    Same { keep: Option<String> },
    Distinct { because: String },
}
#[derive(Clone)]
pub struct Intent {
    pub a: String,
    pub b: String,
    pub action: Action,
    pub as_of: Option<String>,
}
impl crate::history_identity_core::Input for Capture {
    fn source_body(&self, object: &V) -> Result<Source> {
        let m = map(object)?;
        let key = (text(&m["subject"])?.into(), text(&m["id"])?.into());
        let raw = self
            .object_bytes
            .get(&key)
            .ok_or_else(|| error("incomplete_closure"))?;
        Y::decode_source_document(raw)?
            .get("body")
            .cloned()
            .ok_or_else(|| error("invalid_object"))
    }
    fn template(&self) -> Result<V> {
        View::template(&self.commits)
    }
}
#[cfg(test)]
use crate::history_identity_core::{Builder, Spec};

fn context(
    store: &Store,
    captured: &Capture,
    intent: &Intent,
    options: &Options,
) -> Result<HA::Context> {
    let context = HA::capture(store, captured, options)?;
    crate::history_identity_core::validate(captured, &context, intent, options)?;
    Ok(context)
}
fn survivor(intent: &Intent) -> Result<(&str, &str)> {
    let Action::Same { keep } = &intent.action else {
        return Err(error("invalid_identity_receipt"));
    };
    match keep.as_deref() {
        None => Ok((&intent.a, &intent.b)),
        Some(k) if k == "a" || k == intent.a => Ok((&intent.a, &intent.b)),
        Some(k) if k == "b" || k == intent.b => Ok((&intent.b, &intent.a)),
        _ => Err(error("invalid_identity_keep")),
    }
}
pub fn prepare(
    store: &Store,
    captured: &Capture,
    intent: &Intent,
    options: &Options,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    let mut options = options.clone();
    options.recording_day = crate::source_clock::latest_day();
    prepare_inner(store, captured, intent, &options, runtime, None, None, None)
}
#[allow(clippy::too_many_arguments)]
fn prepare_inner(
    store: &Store,
    captured: &Capture,
    intent: &Intent,
    options: &Options,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
    brief_before: Option<Option<&[u8]>>,
    receipt_version: Option<u8>,
) -> Result<PreparedMutation> {
    let context = context(store, captured, intent, options)?;
    let plan = crate::history_identity_core::plan(captured, &context, intent, options, runtime)?;
    let objects = plan.objects;
    let mut template = plan.template;
    let after = plan.after_document;
    let archive = A::archive(store)?;
    let physical = HA::physical_evidence(store)?;
    let observed = FS::read(&FS::target(&store.root, &store.layout.view)?)?;
    let view = brief_before.unwrap_or(observed.as_deref());
    let view_after = if matches!(intent.action, Action::Same { .. }) {
        let (survivor, retired) = survivor(intent)?;
        crate::history_identity_brief::rewrite(
            view,
            retired,
            survivor,
            &F::snapshot_fields(&context.base)?,
        )?
    } else {
        view.map(Vec::from)
    };
    let changed = view_after.as_deref() != view;
    require(
        receipt_version != Some(1) || !changed,
        "identity_brief_requires_migration",
    )?;
    require(
        receipt_version.is_none_or(|v| v == 1 || v == 2) && (receipt_version != Some(2) || changed),
        "invalid_identity_receipt",
    )?;
    let mut data = obj([
        ("version", n(if changed { "2" } else { "1" })),
        (
            "kind",
            s(if matches!(intent.action, Action::Same { .. }) {
                "same"
            } else {
                "distinct"
            }),
        ),
        ("a", s(&intent.a)),
        ("b", s(&intent.b)),
        ("by", options.by.clone()),
        ("operation", s(&options.operation)),
        ("recorded_at", s(&options.recorded_at)),
    ]);
    match &intent.action {
        Action::Same { keep } => {
            map_mut(&mut data)?.insert("keep".into(), keep.as_deref().map(s).unwrap_or(V::Null));
        }
        Action::Distinct { because } => {
            map_mut(&mut data)?.insert("because".into(), s(because));
        }
    }
    if let Some(day) = &intent.as_of {
        map_mut(&mut data)?.insert("as_of".into(), s(day));
    }
    let mut extra = vec![];
    if changed {
        let raw = view.ok_or_else(|| error("invalid_brief_encoding"))?;
        let next = view_after.clone().unwrap();
        let utf8 = std::str::from_utf8(raw).map_err(|_| error("invalid_brief_encoding"))?;
        map_mut(&mut data)?.insert(
            "brief".into(),
            obj([
                ("path", s(&store.layout.view)),
                ("before_utf8", s(utf8)),
                ("before_sha256", s(&sha256(raw))),
                ("after_sha256", s(&sha256(&next))),
            ]),
        );
        extra.push(FileImage {
            role: "view".into(),
            path: store.layout.view.clone(),
            before: Some(raw.to_vec()),
            after: Some(next),
        });
    }
    map_mut(&mut data)?.extend([
        ("baseline".into(), captured.baseline.clone()),
        ("archive".into(), archive.clone()),
        ("physical".into(), physical.clone()),
        (
            "view_sha256".into(),
            view.map(|v| s(&sha256(v))).unwrap_or(V::Null),
        ),
    ]);
    let mut before = A::evidence(
        &context.base,
        &mut HA::world(&context.base, None, runtime)?,
        audit,
    )?;
    map_mut(&mut before)?.insert("identity_authoring".into(), data);
    let mut after_evidence = A::evidence(&after, &mut HA::world(&after, None, runtime)?, audit)?;
    let mut output = obj([(
        "objects",
        strings(
            objects
                .iter()
                .map(|o| text(&map(o).unwrap()["id"]).unwrap().to_owned())
                .collect::<BTreeSet<_>>(),
        ),
    )]);
    if changed {
        map_mut(&mut output)?.insert(
            "view_sha256".into(),
            s(&sha256(view_after.as_ref().unwrap())),
        );
    }
    map_mut(&mut after_evidence)?.insert("identity_authoring".into(), output);
    let cap = F::capabilities(&context.base, None)?;
    let receipt = T::semantic_receipt(
        text(&map(&cap)?["profile"])?,
        &cap,
        &before,
        &after_evidence,
    )?;
    for o in &objects {
        let o = map(o)?;
        if !string_is(&o["kind"], "act") {
            map_mut(&mut template)?
                .entry(text(&map(&o["authored"])?["collection"])?.into())
                .or_insert_with(empty);
        }
    }
    let result = Preparation::prepare_commit_with_files(
        captured,
        &options.operation,
        &objects,
        &template,
        &receipt,
        options.requires().as_ref(),
        &extra,
    )?;
    let candidate = A::candidate(captured, &result)?;
    let adapted = D::from_store_capture(&candidate)?;
    HH::layers(adapted.projection(), adapted.document())?;
    require(
        A::archive(store)? == archive
            && HA::physical_evidence(store)? == physical
            && FS::read(&FS::target(&store.root, &store.layout.view)?)? == observed,
        "identity_source_changed",
    )?;
    Ok(result)
}
pub(crate) fn sources(store: &Store, mutation: &PreparedMutation) -> Result<()> {
    let data = mutation.to_data();
    let intent = map(field(
        map(&map(&map(&data)?["receipt"])?["before"])?,
        "identity_authoring",
    )?)?;
    require(
        A::archive(store)? == *field(intent, "archive")?
            && HA::physical_evidence(store)? == *field(intent, "physical")?,
        "identity_source_changed",
    )?;
    let view = FS::read(&FS::target(&store.root, &store.layout.view)?)?;
    let auxiliary = mutation.auxiliary_view()?;
    if is_int(field(intent, "version")?, "2") {
        let a = auxiliary.ok_or_else(|| error("invalid_identity_receipt"))?;
        require(
            view == a.before || view == a.after,
            "identity_source_changed",
        )?;
    } else {
        require(
            is_int(field(intent, "version")?, "1") && auxiliary.is_none(),
            "invalid_identity_receipt",
        )?;
        require(
            view.map(|v| s(&sha256(&v))).unwrap_or(V::Null) == *field(intent, "view_sha256")?,
            "identity_source_changed",
        )?;
    }
    Ok(())
}
pub fn verify_prepared(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
) -> Result<()> {
    sources(store, mutation)?;
    let (capture, mut options, intent, audit) =
        HA::replay_context(store, mutation, "identity_authoring", true)?;
    options.recording_day = crate::source_clock::latest_day();
    let i = map(&intent)?;
    let version = field(i, "version")?;
    require(
        is_int(version, "1") || is_int(version, "2"),
        "invalid_identity_receipt",
    )?;
    let action = match text(field(i, "kind")?)? {
        "same" => Action::Same {
            keep: match field(i, "keep")? {
                V::Null => None,
                v => Some(text(v)?.into()),
            },
        },
        "distinct" => Action::Distinct {
            because: text(field(i, "because")?)?.into(),
        },
        _ => return Err(error("invalid_identity_receipt")),
    };
    let intent = Intent {
        a: text(field(i, "a")?)?.into(),
        b: text(field(i, "b")?)?.into(),
        action,
        as_of: i.get("as_of").map(text).transpose()?.map(str::to_owned),
    };
    let brief = mutation.auxiliary_view()?.map(|v| v.before.as_deref());
    let expected = prepare_inner(
        store,
        &capture,
        &intent,
        &options,
        runtime,
        Some(&audit),
        brief,
        Some(if is_int(version, "2") { 2 } else { 1 }),
    )?;
    require(
        expected.to_bytes()? == mutation.to_bytes()?,
        "identity_receipt_mismatch",
    )
}
pub fn commit(
    store: &Store,
    mutation: &PreparedMutation,
    runtime: Option<&Runtime>,
    verify: FS::Verify<'_>,
) -> Result<V> {
    store.commit_identity(mutation, runtime, verify)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_identity_merge as M;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde_json::Value as J;
    use std::collections::BTreeMap;
    use std::path::Path;
    fn runtime(cache: &Path) -> Runtime {
        Runtime::open(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
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
    fn corpus() -> J {
        let mut base: J =
            serde_json::from_str(include_str!("../tests/fixtures/history-identity.json")).unwrap();
        let candidate: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-identity-candidate.json"
        ))
        .unwrap();
        base["cases"]
            .as_array_mut()
            .unwrap()
            .extend(candidate["cases"].as_array().unwrap().iter().cloned());
        let edges: J = serde_json::from_str(include_str!(
            "../tests/fixtures/history-identity-edges.json"
        ))
        .unwrap();
        base["cases"]
            .as_array_mut()
            .unwrap()
            .extend(edges["cases"].as_array().unwrap().iter().cloned());
        base
    }
    fn write(root: &Path, case: &J) {
        for (path, raw) in case["files"].as_object().unwrap() {
            let p = root.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, STANDARD.decode(raw.as_str().unwrap()).unwrap()).unwrap();
        }
    }
    fn intent(case: &J) -> Intent {
        Intent {
            a: case["a"].as_str().unwrap().into(),
            b: case["b"].as_str().unwrap().into(),
            action: if case["kind"] == "same" {
                Action::Same {
                    keep: case["keep"].as_str().map(str::to_owned),
                }
            } else {
                Action::Distinct {
                    because: case["because"].as_str().unwrap().into(),
                }
            },
            as_of: case["as_of"].as_str().map(str::to_owned),
        }
    }
    fn options(case: &J) -> Options {
        Options {
            operation: case["operation"].as_str().unwrap().into(),
            recorded_at: case["recorded_at"].as_str().unwrap().into(),
            recording_day: case["today"].as_str().unwrap().into(),
            by: V::from_json(&case["by"]).unwrap(),
            strict: true,
            paths: crate::history_paths::Scheme::Hashed,
            receipt_version: None,
        }
    }
    #[test]
    fn identity_actions_match_python_mutation_bytes() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let mut failures = vec![];
        for case in corpus()["cases"].as_array().unwrap() {
            let root = tempfile::tempdir().unwrap();
            write(root.path(), case);
            let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
            let audit = case
                .get("receipt")
                .map(|r| ReplayAudit::oracle(&V::from_tagged(r).unwrap()).unwrap());
            let result = if !case["as_of"].is_null() && !case["as_of"].is_string() {
                Err(error("invalid_identity_as_of"))
            } else {
                store.capture().and_then(|c| {
                    prepare_inner(
                        &store,
                        &c,
                        &intent(case),
                        &options(case),
                        Some(&runtime),
                        audit.as_ref(),
                        None,
                        None,
                    )
                })
            };
            match (case.get("output"), result) {
                (Some(expected), Ok(actual)) => {
                    if actual.to_bytes().unwrap() != expected.as_str().unwrap().as_bytes() {
                        std::fs::write(
                            std::env::temp_dir().join(format!(
                                "identity-{}-actual.json",
                                case["operation"].as_str().unwrap()
                            )),
                            actual.to_bytes().unwrap(),
                        )
                        .unwrap();
                        failures.push(format!("{}: bytes differ", case["name"]));
                    }
                }
                (None, Err(_)) => {}
                (Some(_), Err(e)) => {
                    failures.push(format!("{}: unexpected refusal {}", case["name"], e))
                }
                (None, Ok(_)) => failures.push(format!("{}: unexpected acceptance", case["name"])),
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
    fn selected(prefix: &str) -> J {
        corpus()["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"].as_str().unwrap().starts_with(prefix))
            .unwrap()
            .clone()
    }
    #[test]
    fn identity_replay_retains_originals_and_requires_fresh_runtime() {
        let case = selected("same_rewrites");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let mutation = prepare(
            &store,
            &before,
            &intent(&case),
            &options(&case),
            Some(&runtime),
        )
        .unwrap();
        assert_eq!(store.capture().unwrap().inventory, before.inventory);
        assert!(verify_prepared(&store, &mutation, None).is_err());
        assert_eq!(
            store.commit(&mutation, &mut |_| Ok(())).unwrap_err().0,
            "unsupported_identity_replay"
        );
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        let after = store.capture().unwrap();
        for (k, v) in &before.object_bytes {
            assert_eq!(after.object_bytes.get(k), Some(v));
        }
        assert_eq!(
            map(&map(&map(&after.state).unwrap()["subjects"]).unwrap()["p.other"]).unwrap()["acceptance"],
            s("retired")
        );
        commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
        assert_eq!(
            store.capture().unwrap().commits.len(),
            before.commits.len() + 1
        );
    }
    #[test]
    fn identity_auxiliary_recovery_covers_each_publication_frontier() {
        let case = selected("brief_references_join");
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        for phase in 0..4 {
            let root = tempfile::tempdir().unwrap();
            write(root.path(), &case);
            let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
            let before = store.capture().unwrap();
            let mutation = prepare(
                &store,
                &before,
                &intent(&case),
                &options(&case),
                Some(&runtime),
            )
            .unwrap();
            let auxiliary = mutation.auxiliary_view().unwrap().unwrap();
            {
                let _lock = FS::DirectoryGuard::acquire(&store.root, true).unwrap();
                let _owner =
                    FS::auxiliary_owner(&store.root, &store.layout.journal, &mutation).unwrap();
                FS::publish_auxiliary_journal(&store.root, &store.layout.journal, &mutation)
                    .unwrap();
                FS::check_member_journals(std::slice::from_ref(&store.entry)).unwrap();
                if phase > 0 {
                    for f in mutation
                        .files()
                        .iter()
                        .filter(|f| f.role == "history_object" || f.role == "history_commit")
                    {
                        FS::publish_immutable(&store.root, &f.path, f.after.as_ref().unwrap())
                            .unwrap();
                    }
                }
                if phase > 1 {
                    let record = mutation
                        .files()
                        .iter()
                        .find(|f| f.role == "record")
                        .unwrap();
                    FS::replace(&store.entry, record.after.as_deref()).unwrap();
                }
                if phase > 2 {
                    FS::replace(
                        &store.root.join(&auxiliary.path),
                        auxiliary.after.as_deref(),
                    )
                    .unwrap();
                }
            }
            assert_eq!(store.capture().unwrap_err().0, "recovery_required");
            assert_eq!(
                FS::check_member_journals(std::slice::from_ref(&store.entry))
                    .unwrap_err()
                    .0,
                "recovery_required"
            );
            if phase == 0 {
                store
                    .recover_auxiliary("before", Some(&runtime), &mut |_| Ok(()))
                    .unwrap();
                assert_eq!(store.capture().unwrap().inventory, before.inventory);
            } else {
                assert_eq!(
                    store
                        .recover_auxiliary("before", Some(&runtime), &mut |_| Ok(()))
                        .unwrap_err()
                        .0,
                    "history_already_committed"
                );
                store
                    .recover_auxiliary("after", Some(&runtime), &mut |_| Ok(()))
                    .unwrap();
                assert_eq!(
                    std::fs::read(store.root.join(&auxiliary.path)).unwrap(),
                    *auxiliary.after.as_ref().unwrap()
                );
                assert_eq!(
                    store.capture().unwrap().commits.len(),
                    before.commits.len() + 1
                );
                verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
            }
            assert!(!store.root.join(&store.layout.journal).exists());
        }
    }
    #[test]
    fn identity_final_callback_failure_keeps_recoverable_guard() {
        let case = selected("brief_references_join");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let mutation = prepare(
            &store,
            &before,
            &intent(&case),
            &options(&case),
            Some(&runtime),
        )
        .unwrap();
        let mut calls = 0;
        assert_eq!(
            commit(&store, &mutation, Some(&runtime), &mut |_| {
                calls += 1;
                if calls == 2 {
                    Err(error("callback_failed"))
                } else {
                    Ok(())
                }
            })
            .unwrap_err()
            .0,
            "callback_failed"
        );
        assert_eq!(store.capture().unwrap_err().0, "recovery_required");
        store
            .recover_auxiliary("after", Some(&runtime), &mut |_| Ok(()))
            .unwrap();
        assert_eq!(
            store.capture().unwrap().commits.len(),
            before.commits.len() + 1
        );
    }
    #[test]
    fn identity_storage_rejects_rehashed_receipts_and_late_source_changes() {
        let case = selected("brief_references_join");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let before = store.capture().unwrap();
        let mutation = prepare(
            &store,
            &before,
            &intent(&case),
            &options(&case),
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
        for f in &mut files {
            if f.role == "history_commit" {
                let mut m = Y::decode_document(f.after.as_ref().unwrap()).unwrap();
                map_mut(&mut m)
                    .unwrap()
                    .insert("receipt".into(), receipt.clone());
                f.after = Some(crate::history_emit::encode_document(&m).unwrap());
            }
        }
        let forged = PreparedMutation::prepare(
            &options(&case).operation,
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
                .commit_identity(&forged, Some(&runtime), &mut |_| Ok(()))
                .unwrap_err()
                .0,
            "identity_receipt_mismatch"
        );
        let archive = store.root.join(&store.layout.replaced);
        assert_eq!(
            commit(&store, &mutation, Some(&runtime), &mut |_| {
                std::fs::write(&archive, b"changed archive")?;
                Ok(())
            })
            .unwrap_err()
            .0,
            "identity_source_changed"
        );
        assert_eq!(std::fs::read(&store.entry).unwrap(), before.entry_bytes);
        assert!(!store.root.join(&store.layout.journal).exists());
        std::fs::remove_file(archive).unwrap();
        let brief = store.root.join(&store.layout.view);
        assert_eq!(
            commit(&store, &mutation, Some(&runtime), &mut |_| {
                std::fs::write(&brief, b"changed brief")?;
                Ok(())
            })
            .unwrap_err()
            .0,
            "identity_source_changed"
        );
        assert_eq!(store.capture().unwrap().commits.len(), before.commits.len());
    }

    #[test]
    fn identity_clock_cannot_be_overridden_by_recording_metadata_or_replay() {
        let case = selected("same_rewrites");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let captured = store.capture().unwrap();
        let mut intent = intent(&case);
        intent.as_of = Some("9999-12-31".into());
        let mut options = options(&case);
        options.recording_day = "9999-12-31".into();
        assert_eq!(
            prepare(&store, &captured, &intent, &options, Some(&runtime))
                .unwrap_err()
                .0,
            "invalid_identity_as_of"
        );
        options.recording_day.clear();
        assert_eq!(
            prepare(&store, &captured, &intent, &options, Some(&runtime))
                .unwrap_err()
                .0,
            "invalid_identity_as_of"
        );
        // Manufacture a semantically consistent receipt under a false test clock;
        // current replay must reject it even after all envelope hashes are rebuilt.
        options.recording_day = "9999-12-31".into();
        let forged = prepare_inner(
            &store,
            &captured,
            &intent,
            &options,
            Some(&runtime),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            store
                .commit_identity(&forged, Some(&runtime), &mut |_| Ok(()))
                .unwrap_err()
                .0,
            "invalid_identity_as_of"
        );
        assert_eq!(store.capture().unwrap().inventory, captured.inventory);
    }
    #[test]
    fn inferred_judgments_cannot_use_reading_date_supersession() {
        for deps in [empty(), V::List(vec![])] {
            let kept = obj([("v", n("1")), ("of", s("2026-01-01")), ("rests_on", deps)]);
            let removed = obj([("v", n("2")), ("of", s("2026-01-02"))]);
            let document = obj([
                (
                    "meta",
                    obj([(
                        "reasoning",
                        obj([
                            ("version", n("2")),
                            ("profile", s("core/v1")),
                            ("requires", strings(vec!["arithmetic/v1".into()])),
                        ]),
                    )]),
                ),
                (
                    "schema",
                    obj([
                        ("deps", s("rests_on")),
                        ("snapshot", s("seen")),
                        ("predicate", s("wrong_if")),
                    ]),
                ),
                (
                    "readings",
                    obj([("p.left", kept.clone()), ("p.right", removed.clone())]),
                ),
            ]);
            let mut world = HA::world(&document, None, None).unwrap();
            assert_eq!(
                M::merge(
                    "p.left",
                    "p.right",
                    &Source::from_typed(&kept),
                    &Source::from_typed(&removed),
                    &mut world
                )
                .unwrap_err()
                .0,
                "identity_conflicting_readings"
            );
        }
    }

    #[test]
    fn identity_pin_rebuild_handles_long_chains_without_call_stack_recursion() {
        let case = selected("same_rewrites");
        let root = tempfile::tempdir().unwrap();
        write(root.path(), &case);
        let store = Store::new(&root.path().join("GROUNDING.yaml")).unwrap();
        let mut captured = store.capture().unwrap();
        let options = options(&case);
        let authored = obj([
            ("profile", s("core/v1")),
            ("collection", s("decisions")),
            (
                "fields",
                obj([
                    ("deps", s("rests_on")),
                    ("snapshot", s("seen")),
                    ("predicate", s("wrong_if")),
                    ("value", s("v")),
                ]),
            ),
        ]);
        let mut specs = BTreeMap::new();
        let mut mapped = BTreeMap::new();
        let mut previous: Option<(String, String)> = None;
        for i in 0..300 {
            let subject = format!("d.chain{i:03}");
            let key = (String::new(), subject.clone());
            let (deps, pins) = if let Some((id, version)) = &previous {
                (
                    strings(vec![id.clone()]),
                    V::Map(Map::from([(id.clone(), s(version))])),
                )
            } else {
                (strings(vec![]), empty())
            };
            let body = obj([("verdict", s("old")), ("rests_on", deps)]);
            let prior = A::make_object(
                &subject,
                "judgment",
                body.clone(),
                strings(vec![]),
                Some(authored.clone()),
                pins,
                empty(),
                &options,
            )
            .unwrap();
            let id = text(&map(&prior).unwrap()["id"]).unwrap().to_owned();
            captured.objects.insert(id.clone(), prior.clone());
            let mut changed = body;
            map_mut(&mut changed)
                .unwrap()
                .insert("verdict".into(), s("new"));
            specs.insert(
                key.clone(),
                Spec {
                    body: changed,
                    authored: authored.clone(),
                    prior,
                    olds: vec![id.clone()],
                },
            );
            mapped.insert((String::new(), id.clone()), key);
            previous = Some((subject, id));
        }
        let last = specs.keys().last().unwrap().clone();
        let mut builder = Builder {
            captured: &captured,
            specs: &specs,
            mapped: &mapped,
            created: BTreeMap::new(),
            visiting: BTreeSet::new(),
            survivor: "p.input",
            retired: "p.other",
            options: &options,
        };
        builder.build(&last).unwrap();
        assert_eq!(builder.created.len(), 300);
        assert!(builder.visiting.is_empty());
        for (key, value) in &builder.created {
            let prior = map(&specs[key].prior).unwrap();
            assert_ne!(map(value).unwrap()["id"], prior["id"]);
            for old in map(&prior["pins"]).unwrap().values() {
                let dependency = &mapped[&(String::new(), text(old).unwrap().into())];
                assert!(
                    map(&map(value).unwrap()["pins"])
                        .unwrap()
                        .values()
                        .any(|v| *v == map(&builder.created[dependency]).unwrap()["id"])
                );
            }
        }
    }
}
