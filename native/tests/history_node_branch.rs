use kpop_native::{
    history_authoring::Options, history_node_capture::Capture, history_node_codec as C,
    history_node_publication as P, history_node_writer as W, history_paths::Scheme,
    value::TypedValue as V,
};
use serde_json::json;
use std::{collections::BTreeMap, fs};
fn value(v: serde_json::Value) -> V {
    V::from_json(&v).unwrap()
}
fn map(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
fn setup() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".kpopper")).unwrap();
    fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    fs::write(
        root.path().join("GROUNDING.yaml"),
        "meta:\n  purpose: Fixture\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n",
    )
    .unwrap();
    root
}
fn options(op: &str) -> Options {
    Options {
        operation: op.into(),
        recorded_at: "2026-09-24T12:00:00+00:00".into(),
        recording_day: "2026-09-24".into(),
        by: value(json!("writer")),
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    }
}
fn add() -> V {
    value(json!({"kind":"add", "id":"p.a", "body":{"v":1}}))
}
fn set(n: i32) -> V {
    value(json!({"kind":"set", "id":"p.a", "value":n}))
}
fn write(root: &std::path::Path, op: &str, action: &V) -> P::Prepared {
    let p = W::prepare(root, action, &options(op), None).unwrap();
    W::publish(root, &p, None, |_| Ok(())).unwrap();
    p
}

#[test]
fn siblings_preserve_receipts_disputes_and_source_free_union() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(target.path(), "left", &set(2));
    write(sibling.path(), "right", &set(3));
    let left = W::receipt(&P::capture_snapshot(target.path()).unwrap(), "left").unwrap();
    let right = W::receipt(&P::capture_snapshot(sibling.path()).unwrap(), "right").unwrap();
    let bundle = P::export(sibling.path()).unwrap();
    let prepared =
        kpop_native::history_node_branch::prepare(target.path(), &[bundle], "merge").unwrap();
    drop(sibling);
    W::publish(target.path(), &prepared, None, |_| Ok(())).unwrap();
    W::publish(target.path(), &prepared, None, |_| panic!("retry wrote")).unwrap();
    let snapshot = P::capture_snapshot(target.path()).unwrap();
    assert_eq!(snapshot.transactions["merge"].parents, ["left", "right"]);
    assert_eq!(W::receipt(&snapshot, "left").unwrap(), left);
    assert_eq!(W::receipt(&snapshot, "right").unwrap(), right);
    let captured = Capture::read(target.path()).unwrap();
    assert_eq!(
        map(&map(&map(captured.state())["subjects"])["p.a"])["acceptance"],
        value(json!("contested"))
    );
    let copy = P::export(target.path()).unwrap().reconstruct().unwrap();
    drop(target);
    assert_eq!(
        Capture::read(copy.path()).unwrap().state(),
        captured.state()
    );
    // A later explicit choice uses the normal writer after merge.
    let subject = &map(&map(captured.state())["subjects"])["p.a"];
    let heads = map(subject)["heads"].to_json().unwrap();
    let chosen = heads[0].clone();
    let over = heads.as_array().unwrap()[1..].to_vec();
    let accept = value(
        json!({"kind":"accept", "id":"p.a", "of":chosen,"over":over,"because":"explicit choice"}),
    );
    write(copy.path(), "choose", &accept);
}

#[test]
fn new_sibling_nodes_stay_lazy_and_unobserved_by_old_receipts() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(target.path(), "left", &set(2));
    write(
        sibling.path(),
        "right-new",
        &value(json!({"kind":"add","id":"p.b","body":{"v":7}})),
    );
    let p = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    W::publish(target.path(), &p, None, |_| Ok(())).unwrap();
    let snapshot = P::capture_snapshot(target.path()).unwrap();
    let old = W::receipt(&snapshot, "left").unwrap();
    assert!(
        !map(&map(&map(&old)["after"])["document"])["known"]
            .to_json()
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("p.b")
    );
    assert!(
        !target
            .path()
            .join(".kpopper/history")
            .join(C::subject_path("p.b").unwrap())
            .exists()
    );
    write(
        target.path(),
        "change-b",
        &value(json!({"kind":"set","id":"p.b","value":8})),
    );
    Capture::read(target.path()).unwrap();
}

#[test]
fn branch_union_recovers_each_boundary_without_sources() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Import(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let target = setup();
        write(target.path(), "seed", &add());
        let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
        write(target.path(), "left", &set(2));
        write(sibling.path(), "right", &set(3));
        let before = P::export(target.path()).unwrap().encode().unwrap();
        let p = kpop_native::history_node_branch::prepare(
            target.path(),
            &[P::export(sibling.path()).unwrap()],
            "merge",
        )
        .unwrap();
        drop(sibling);
        assert!(
            W::publish(target.path(), &p, None, |at| if at == phase {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        assert!(Capture::read(target.path()).is_err());
        let committed = matches!(phase, P::Phase::Commit | P::Phase::View);
        assert_eq!(
            W::recover(target.path(), None).unwrap(),
            if committed {
                "committed"
            } else {
                "rolled_back"
            }
        );
        if !committed {
            assert_eq!(P::export(target.path()).unwrap().encode().unwrap(), before);
        }
        W::publish(target.path(), &p, None, |_| Ok(())).unwrap();
        Capture::read(target.path()).unwrap();
    }
}

#[test]
fn foreign_authority_and_stale_target_refuse_without_writes() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(sibling.path(), "right", &set(3));
    let p = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    write(target.path(), "moved", &set(4));
    let before = P::export(target.path()).unwrap().encode().unwrap();
    assert!(W::publish(target.path(), &p, None, |_| Ok(())).is_err());
    assert_eq!(P::export(target.path()).unwrap().encode().unwrap(), before);
    let foreign = setup();
    let marker = foreign.path().join(".kpopper/history.yaml");
    fs::write(
        &marker,
        fs::read_to_string(&marker)
            .unwrap()
            .replace("fixture", "foreign"),
    )
    .unwrap();
    write(foreign.path(), "foreign", &add());
    assert!(
        kpop_native::history_node_branch::prepare(
            target.path(),
            &[P::export(foreign.path()).unwrap()],
            "foreign-merge"
        )
        .is_err()
    );
    assert_eq!(P::export(target.path()).unwrap().encode().unwrap(), before);
}

#[test]
fn partial_import_rolls_back_all_source_manifests_and_retries() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(sibling.path(), "incoming-a", &set(2));
    write(
        sibling.path(),
        "incoming-b",
        &value(json!({"kind":"add","id":"p.b","body":{"v":7}})),
    );
    let before = P::export(target.path()).unwrap().encode().unwrap();
    let prepared = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    drop(sibling);
    assert!(
        W::publish(target.path(), &prepared, None, |at| {
            if at == P::Phase::Import(0) {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert_eq!(W::recover(target.path(), None).unwrap(), "rolled_back");
    assert_eq!(P::export(target.path()).unwrap().encode().unwrap(), before);
    W::publish(target.path(), &prepared, None, |_| Ok(())).unwrap();
    Capture::read(target.path()).unwrap();
}

#[test]
fn changed_import_is_refused_before_commit_and_recovery_mutations() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(sibling.path(), "right", &set(3));
    let prepared = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    let path = target.path().join(".kpopper/history-commits/right.json");
    assert!(
        W::publish(target.path(), &prepared, None, |at| {
            if at == P::Phase::Import(0) {
                fs::write(&path, b"tampered").unwrap();
            }
            Ok(())
        })
        .is_err()
    );
    assert!(
        !target
            .path()
            .join(".kpopper/history-commits/merge.json")
            .exists()
    );
    assert!(W::recover(target.path(), None).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"tampered");
    assert!(
        target
            .path()
            .join(".kpopper/.history-node-publication.json")
            .exists()
    );
}

#[test]
fn physical_hypotheses_are_not_silently_lost_by_export_or_union() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(sibling.path(), "right", &set(3));
    let p = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    fs::create_dir(target.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(
        target.path().join(".kpopper/hypotheses/physical.yaml"),
        "known: {}\n",
    )
    .unwrap();
    assert!(P::export(target.path()).unwrap_err().0.contains("physical"));
    assert!(
        W::publish(target.path(), &p, None, |_| Ok(()))
            .unwrap_err()
            .0
            .contains("physical")
    );
    assert!(
        !target
            .path()
            .join(".kpopper/.history-node-publication.json")
            .exists()
    );
}

#[test]
fn noncanonical_import_reencoding_refuses_before_recovery_mutation() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(sibling.path(), "right", &set(3));
    let prepared = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    assert!(
        W::publish(target.path(), &prepared, None, |at| {
            if at == P::Phase::Import(0) {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    let path = target.path().join(".kpopper/history-commits/right.json");
    let mut bytes = fs::read(&path).unwrap();
    bytes.push(b'\n'); // Identical JSON value, different immutable bytes.
    fs::write(&path, &bytes).unwrap();
    let journal_path = target
        .path()
        .join(".kpopper/.history-node-publication.json");
    let journal = fs::read(&journal_path).unwrap();
    let current = fs::read(target.path().join("GROUNDING.yaml")).unwrap();
    assert!(W::recover(target.path(), None).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(&journal_path).unwrap(), journal);
    assert_eq!(
        fs::read(target.path().join("GROUNDING.yaml")).unwrap(),
        current
    );
}

#[test]
fn publication_union_does_not_authenticate_source_commit_ancestry() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    for (root, op, commit, reading) in [
        (target.path(), "left-clock", "a".repeat(40), 1),
        (sibling.path(), "right-clock", "b".repeat(40), 2),
    ] {
        write(
            root,
            op,
            &value(json!({"kind":"add","id":"p.clock", "body":{
                "v":reading,"from":"source.repository","at":{"commit":commit}
            }})),
        );
    }
    let prepared = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    W::publish(target.path(), &prepared, None, |_| Ok(())).unwrap();
    let c = Capture::read(target.path()).unwrap();
    assert_eq!(
        map(&map(&map(c.state())["subjects"])["p.clock"])["acceptance"],
        value(json!("contested"))
    );
    assert_eq!(
        map(&map(&map(c.state())["subjects"])["p.clock"])["heads"]
            .to_json()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn nonnegative_authority_generations_work_without_merging_distinct_epochs() {
    for generation in ["0", "7", "4294967297"] {
        let root = setup();
        let marker = root.path().join(".kpopper/history.yaml");
        let raw = fs::read_to_string(&marker)
            .unwrap()
            .replace("generation: 1", &format!("generation: {generation}"));
        fs::write(&marker, &raw).unwrap();
        write(root.path(), "seed", &add());
        let sibling = P::export(root.path()).unwrap().reconstruct().unwrap();
        write(root.path(), "left", &set(2));
        write(sibling.path(), "right", &set(3));
        let p = kpop_native::history_node_branch::prepare(
            root.path(),
            &[P::export(sibling.path()).unwrap()],
            "merge",
        )
        .unwrap();
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        let foreign = setup();
        fs::write(
            foreign.path().join(".kpopper/history.yaml"),
            raw.replace(&format!("generation: {generation}"), "generation: 91"),
        )
        .unwrap();
        write(foreign.path(), "foreign", &add());
        assert!(
            kpop_native::history_node_branch::prepare(
                root.path(),
                &[P::export(foreign.path()).unwrap()],
                "bad-epoch"
            )
            .is_err()
        );
    }
}
