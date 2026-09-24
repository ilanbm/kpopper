//! Final-world admission and immutable final pins; legacy sequential receipts replay exactly.
use crate::{
    Result,
    history_authoring::{self as A, Options},
    history_authoring_audit::ReplayAudit,
    history_capture::Capture,
    history_contract::*,
    history_preparation as P,
    history_store::Store,
    history_transaction::{self as T, FileImage, PreparedMutation},
    history_view::{map_mut, truth},
    identity::sha256,
    reasoning_runtime::Runtime,
    require,
    value::TypedValue as V,
};
use A::{empty, n, obj, s, strings};

#[derive(Clone)]
pub struct BatchOptions {
    pub authoring: Options,
    pub receipt_version: u8,
    pub context: V,
    pub evidence: crate::history_authority::Files,
}
fn evidence_files(options: &BatchOptions) -> Result<Vec<FileImage>> {
    options
        .evidence
        .iter()
        .map(|(path, raw)| {
            crate::history_authority::relative_path(path)?;
            Ok(FileImage {
                path: path.clone(),
                role: "history_evidence".into(),
                before: None,
                after: Some(raw.clone()),
            })
        })
        .collect()
}
pub fn prepare_batch(
    store: &Store,
    capture: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
) -> Result<PreparedMutation> {
    prepare_inner(store, capture, actions, options, runtime, None)
}
pub(crate) fn prepare_inner(
    store: &Store,
    capture: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    require(!actions.is_empty() && actions.len() <= 64, "invalid_batch")?;
    require(
        [2, 3, 6, 8].contains(&options.receipt_version),
        "invalid_authoring_receipt",
    )?;
    let mut actions = actions.to_vec();
    for action in &mut actions {
        let a = map_mut(action).map_err(|_| error("invalid_batch_action"))?;
        if !a.get("as_of").is_some_and(truth) {
            require(
                !options.authoring.recording_day.is_empty(),
                "missing_recording_time",
            )?;
            a.insert("as_of".into(), s(&options.authoring.recording_day));
        }
    }
    if options.receipt_version < 6 {
        sequential(store, capture, &actions, options, runtime, audit)
    } else {
        final_world(store, capture, &actions, options, runtime, audit)
    }
}
fn final_world(
    store: &Store,
    original: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    let mut op = options.authoring.clone();
    op.strict = true;
    A::guards(store, original, &op)?;
    crate::history_sources::capture(&store.root, &store.layout.entry, &original.document)?;
    let frozen = A::archive(store)?;
    let plan = crate::history_authoring_batch_core::prepare(
        original, actions, options, runtime, audit, &frozen,
    )?;
    let files = evidence_files(options)?;
    let mutation = P::prepare_commit_with_files(
        original,
        &op.operation,
        &plan.objects,
        &A::template(original, &plan.document)?,
        &plan.receipt,
        op.requires_for(&plan.document)?.as_ref(),
        &files,
    )?;
    let adapted = A::document(&A::candidate(original, &mutation)?)?;
    require(
        adapted.digest()? == plan.document.digest()?,
        "batch_final_projection_mismatch",
    )?;
    require(A::archive(store)? == frozen, "concurrent_archive_edit")?;
    crate::history_sources::capture(&store.root, &store.layout.entry, &original.document)?;
    Ok(mutation)
}

fn intent(
    original: &Capture,
    actions: &[V],
    options: &BatchOptions,
    archive: &V,
    evidence: bool,
) -> V {
    let mut value = obj([
        ("version", n(&options.receipt_version.to_string())),
        ("kind", s("batch")),
        ("actions", V::List(actions.to_vec())),
        ("by", options.authoring.by.clone()),
        ("recorded_at", s(&options.authoring.recorded_at)),
        ("archive", archive.clone()),
        ("baseline", original.baseline.clone()),
        (
            "context",
            if truth(&options.context) {
                options.context.clone()
            } else {
                empty()
            },
        ),
    ]);
    if evidence {
        map_mut(&mut value).unwrap().insert(
            "evidence".into(),
            V::Map(
                options
                    .evidence
                    .iter()
                    .map(|(p, raw)| (p.clone(), s(&sha256(raw))))
                    .collect(),
            ),
        );
    }
    value
}
fn sequential(
    store: &Store,
    original: &Capture,
    actions: &[V],
    options: &BatchOptions,
    runtime: Option<&Runtime>,
    audit: Option<&ReplayAudit>,
) -> Result<PreparedMutation> {
    require(
        options.receipt_version == 3 || options.evidence.is_empty(),
        "invalid_authoring_receipt",
    )?;
    require(
        !original.commits.contains_key(&options.authoring.operation),
        "operation_already_prepared",
    )?;
    let frozen = A::archive(store)?;
    let mut virtual_capture = original.clone();
    let mut steps = vec![];
    let mut first = None;
    let mut last = None;
    for (index, action) in actions.iter().enumerate() {
        let mut child = options.authoring.clone();
        child.operation = format!(
            "batch-step-{}",
            obj([
                ("operation", s(&options.authoring.operation)),
                ("index", n(&index.to_string()))
            ])
            .digest()?
        );
        // Fresh sequential batches use the current direct receipt version. A
        // retained old batch can explicitly replay its v1 child semantics.
        child.receipt_version = options.authoring.receipt_version;
        let mutation = A::prepare_inner(store, &virtual_capture, action, &child, runtime, audit)?;
        let data = mutation.to_data();
        let receipt = map(&data)?["receipt"].clone();
        if first.is_none() {
            first = Some(receipt.clone());
        }
        last = Some(receipt.clone());
        steps.push(obj([
            ("operation", s(&child.operation)),
            (
                if options.receipt_version == 2 {
                    "receipt"
                } else {
                    "receipt_digest"
                },
                if options.receipt_version == 2 {
                    receipt.clone()
                } else {
                    map(&receipt)?["digest"].clone()
                },
            ),
        ]));
        virtual_capture = A::candidate(&virtual_capture, &mutation)?;
    }
    let first = first.unwrap();
    let last = last.unwrap();
    let first = map(&first)?;
    let last = map(&last)?;
    let mut before = first["before"].clone();
    map_mut(&mut before)?.insert(
        "authoring".into(),
        intent(
            original,
            actions,
            options,
            &frozen,
            options.receipt_version == 3,
        ),
    );
    let new = virtual_capture
        .objects
        .iter()
        .filter(|(id, _)| !original.objects.contains_key(*id))
        .map(|(_, o)| o.clone())
        .collect::<Vec<_>>();
    let mut after = last["after"].clone();
    map_mut(&mut after)?.insert(
        "authoring".into(),
        obj([
            ("steps", V::List(steps)),
            (
                "objects",
                strings(
                    new.iter()
                        .map(|o| text(&map(o).unwrap()["id"]).unwrap().to_owned()),
                ),
            ),
        ]),
    );
    let receipt = T::semantic_receipt(
        text(&first["profile"])?,
        &first["capabilities"],
        &before,
        &after,
    )?;
    let mutation = P::prepare_commit_with_files(
        original,
        &options.authoring.operation,
        &new,
        &crate::history_view::template(&virtual_capture.commits)?,
        &receipt,
        options
            .authoring
            .requires_for(field(map(&last["after"])?, "document")?)?
            .as_ref(),
        &evidence_files(options)?,
    )?;
    require(A::archive(store)? == frozen, "concurrent_archive_edit")?;
    Ok(mutation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        history_view::list, reasoning_authoring::World, reasoning_runtime::OperationalBounds,
    };
    use serde_json::Value as J;
    fn open_runtime(cache: &std::path::Path) -> Runtime {
        let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                crate::reasoning_runtime::target_name().unwrap()
            ));
        Runtime::open(&archive, cache, OperationalBounds::default()).unwrap()
    }
    #[test]
    fn admission_compares_the_prior_formula_using_final_inputs() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = open_runtime(cache.path());
        let final_doc=V::from_json(&serde_json::json!({"meta":{"reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"known":{"p.input":{"v":6},"p.ratio":{"rule":{"expr":"p.input / 2"}}}})).unwrap();
        let mut prior = final_doc.clone();
        map_mut(
            map_mut(map_mut(&mut prior).unwrap().get_mut("known").unwrap())
                .unwrap()
                .get_mut("p.ratio")
                .unwrap(),
        )
        .unwrap()
        .insert("rule".into(), obj([("expr", s("p.input / 3"))]));
        let mut world = World::batch_admission(
            &final_doc,
            &prior,
            "p.ratio",
            Some(&runtime),
            OperationalBounds::default(),
        )
        .unwrap();
        assert_eq!(
            world.result("p.ratio").unwrap()["value"],
            serde_json::json!({"type":"number","numerator":"2","denominator":"1"})
        );
        assert!(!world.same_value("p.ratio", &n("3")).unwrap());
    }
    #[test]
    fn native_batch_replay_preserves_final_pins_evidence_and_retry() {
        let cache = tempfile::tempdir().unwrap();
        let runtime = open_runtime(cache.path());
        for (version, name, raw) in [
            (
                2,
                "2-set",
                include_str!("../tests/fixtures/history-authoring-batch.json"),
            ),
            (
                3,
                "3-set",
                include_str!("../tests/fixtures/history-authoring-batch.json"),
            ),
            (
                6,
                "6-forward-dependency",
                include_str!("../tests/fixtures/history-authoring-batch.json"),
            ),
            (
                8,
                "8-forward-dependency",
                include_str!("../tests/fixtures/history-authoring-batch-candidate.json"),
            ),
        ] {
            let data: J = serde_json::from_str(raw).unwrap();
            let case = data["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == name)
                .unwrap();
            let temp = tempfile::tempdir().unwrap();
            for (name, raw) in case["files"].as_object().unwrap() {
                let p = temp.path().join(name);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, raw.as_str().unwrap()).unwrap();
            }
            let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
            let capture = store.capture().unwrap();
            let actions = V::from_tagged(&case["actions"]).unwrap();
            let mut options = BatchOptions {
                authoring: Options {
                    operation: "batch-1".into(),
                    recorded_at: "2026-09-19T09:30:00+00:00".into(),
                    recording_day: "2026-09-19".into(),
                    by: s("writer"),
                    strict: version >= 3,
                    paths: crate::history_paths::Scheme::Hashed,
                    receipt_version: None,
                },
                receipt_version: version,
                context: empty(),
                evidence: Default::default(),
            };
            if version >= 3 {
                options.evidence.insert(
                    ".kpopper/evidence/reports/batch-source.txt".into(),
                    b"source report".to_vec(),
                );
            }
            let mutation = prepare_batch(
                &store,
                &capture,
                list(&actions).unwrap(),
                &options,
                Some(&runtime),
            )
            .unwrap();
            A::verify_prepared(&store, &mutation, Some(&runtime)).unwrap();
            assert!(
                A::verify_prepared(&store, &mutation, None).is_err(),
                "replay skipped a fresh evaluator for {version}"
            );
            A::commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
            A::commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
            let after = store.capture().unwrap();
            assert_eq!(after.commits.len(), 2);
            for (key, raw) in &capture.object_bytes {
                assert_eq!(&after.object_bytes[key], raw);
            }
            if version >= 6 {
                let judgment = map(A::head(&after, "d.ready").unwrap()).unwrap();
                let input = map(A::head(&after, "p.new").unwrap()).unwrap();
                assert_eq!(map(&judgment["pins"]).unwrap()["p.new"], input["id"]);
            }
            if version >= 3 {
                assert_eq!(
                    std::fs::read(
                        store
                            .root
                            .join(".kpopper/evidence/reports/batch-source.txt")
                    )
                    .unwrap(),
                    b"source report"
                );
            }
        }
    }
    #[test]
    fn final_and_sequential_batches_match_exact_python_envelopes() {
        let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                crate::reasoning_runtime::target_name().unwrap()
            ));
        let cache = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
        let mut failures = vec![];
        for raw in [
            include_str!("../tests/fixtures/history-authoring-batch.json"),
            include_str!("../tests/fixtures/history-authoring-batch-candidate.json"),
        ] {
            let data: J = serde_json::from_str(raw).unwrap();
            for case in data["cases"].as_array().unwrap() {
                let temp = tempfile::tempdir().unwrap();
                for (name, raw) in case["files"].as_object().unwrap() {
                    let path = temp.path().join(name);
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(path, raw.as_str().unwrap()).unwrap();
                }
                let store = Store::new(&temp.path().join("GROUNDING.yaml")).unwrap();
                let capture = store.capture().unwrap();
                let actions = V::from_tagged(&case["actions"]).unwrap();
                let version = case["version"].as_u64().unwrap() as u8;
                let options = BatchOptions {
                    authoring: Options {
                        operation: "batch-1".into(),
                        recorded_at: "2026-09-19T09:30:00+00:00".into(),
                        recording_day: "2026-09-19".into(),
                        by: s("writer"),
                        strict: version >= 3,
                        paths: crate::history_paths::Scheme::Hashed,
                        receipt_version: Some(1),
                    },
                    receipt_version: version,
                    context: V::from_tagged(&case["context"]).unwrap(),
                    evidence: Default::default(),
                };
                let audit = case
                    .get("receipt")
                    .map(|v| ReplayAudit::oracle(&V::from_tagged(v).unwrap()).unwrap());
                let got = prepare_inner(
                    &store,
                    &capture,
                    list(&actions).unwrap(),
                    &options,
                    Some(&runtime),
                    audit.as_ref(),
                );
                if let Some(expected) = case.get("output") {
                    match got {
                        Ok(m) => {
                            if m.to_bytes().unwrap() != expected.as_str().unwrap().as_bytes() {
                                std::fs::write(
                                    temp.path().join("actual.json"),
                                    m.to_bytes().unwrap(),
                                )
                                .unwrap();
                                let path = temp.keep();
                                failures.push(format!(
                                    "{} mismatch {}",
                                    case["name"],
                                    path.display()
                                ));
                            }
                        }
                        Err(e) => failures.push(format!("{} refused {e}", case["name"])),
                    }
                } else if got.is_ok() {
                    failures.push(format!(
                        "{} accepted refusal: {}",
                        case["name"], case["refused"]
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
