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
fn siblings_preserve_context_disputes_and_source_free_union() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(target.path(), "left", &set(2));
    write(sibling.path(), "right", &set(3));
    let left = P::capture_snapshot(target.path()).unwrap().transactions["left"]
        .context
        .clone()
        .unwrap();
    let right = P::capture_snapshot(sibling.path()).unwrap().transactions["right"]
        .context
        .clone()
        .unwrap();
    assert_eq!(map(&left)["action"], set(2));
    assert_eq!(map(&right)["action"], set(3));
    let bundle = P::export(sibling.path()).unwrap();
    let prepared =
        kpop_native::history_node_branch::prepare(target.path(), &[bundle], "merge").unwrap();
    drop(sibling);
    W::publish(target.path(), &prepared, None, |_| Ok(())).unwrap();
    W::publish(target.path(), &prepared, None, |_| panic!("retry wrote")).unwrap();
    let snapshot = P::capture_snapshot(target.path()).unwrap();
    assert_eq!(snapshot.transactions["merge"].parents, ["left", "right"]);
    assert_eq!(snapshot.transactions["left"].context.as_ref(), Some(&left));
    assert_eq!(
        snapshot.transactions["right"].context.as_ref(),
        Some(&right)
    );
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
    let restored = P::capture_snapshot(copy.path()).unwrap();
    assert_eq!(restored.transactions["left"].context.as_ref(), Some(&left));
    assert_eq!(
        restored.transactions["right"].context.as_ref(),
        Some(&right)
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
fn new_sibling_nodes_stay_lazy_and_unobserved_by_old_context() {
    let target = setup();
    write(target.path(), "seed", &add());
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(target.path(), "left", &set(2));
    let old = P::capture_snapshot(target.path()).unwrap().transactions["left"]
        .context
        .clone()
        .unwrap();
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
    assert_eq!(snapshot.transactions["left"].context.as_ref(), Some(&old));
    assert_eq!(map(&old)["action"], set(2));
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

#[test]
fn wide_branch_union_bounds_lazy_metadata_and_preserves_original_events() {
    fn count(v: &V) -> usize {
        1 + match v {
            V::Map(values) => values.values().map(count).sum(),
            V::List(values) => values.iter().map(count).sum(),
            _ => 0,
        }
    }
    let base = setup();
    write(base.path(), "seed", &add());
    let base_bundle = P::export(base.path()).unwrap();
    let mut branches = vec![];
    let mut originals = BTreeMap::new();
    let mut original_frames = BTreeMap::new();
    let mut lazy_total = 0;
    for branch in 0..6 {
        let copy = base_bundle.reconstruct().unwrap();
        for batch in 0..2 {
            let actions = (batch * 64..(batch + 1) * 64)
                .map(|i| json!({"kind":"add", "id":format!("p.branch{branch}.item{i:03}"), "body":{"v":i}}))
                .collect::<Vec<_>>();
            write(
                copy.path(),
                &format!("branch{branch}-batch{batch}"),
                &value(json!({"kind":"batch", "actions":actions})),
            );
        }
        let capture = Capture::read(copy.path()).unwrap();
        assert_eq!(map(&map(capture.state())["subjects"]).len(), 129);
        let document = kpop_native::history_yaml::decode_document(
            &fs::read(copy.path().join("GROUNDING.yaml")).unwrap(),
        )
        .unwrap();
        let bindings = &map(&map(&map(&document)["meta"])["node_history"])["originals"];
        let cost = map(bindings).values().map(count).sum::<usize>();
        assert!(
            cost <= kpop_native::value::MAX_VALUES / 4,
            "each writer-produced branch is already within its lazy budget: {cost}"
        );
        lazy_total += cost;
        for (subject, state) in map(&map(capture.state())["subjects"]) {
            let V::Text(head) = &map(state)["head"] else {
                panic!()
            };
            originals.insert(
                subject.clone(),
                (head.clone(), capture.object(subject, head).unwrap()),
            );
        }
        for (subject, versions) in P::capture_snapshot(copy.path()).unwrap().versions {
            let frames = versions
                .iter()
                .map(|(id, version)| (id.clone(), version.frame_sha256().to_owned()))
                .collect::<BTreeMap<_, _>>();
            if let Some(previous) = original_frames.insert(subject, frames.clone()) {
                assert_eq!(previous, frames);
            }
        }
        branches.push(copy);
    }
    assert!(
        lazy_total > kpop_native::value::MAX_VALUES,
        "fixture must overflow the old combined lazy view: {lazy_total}"
    );
    // Put the lexicographically last subjects in the target so the budget also
    // materializes originals that were already lazy in the destination.
    let target = branches.last().unwrap();
    assert!(
        !target
            .path()
            .join(".kpopper/history")
            .join(C::subject_path("p.branch5.item000").unwrap())
            .exists()
    );
    let sources = branches
        .iter()
        .take(branches.len() - 1)
        .map(|branch| P::export(branch.path()).unwrap())
        .collect::<Vec<_>>();
    let prepared =
        kpop_native::history_node_branch::prepare(target.path(), &sources, "wide-union").unwrap();
    let mut reverse = sources.clone();
    reverse.reverse();
    assert_eq!(
        kpop_native::history_node_branch::prepare(target.path(), &reverse, "wide-union")
            .unwrap()
            .to_bytes()
            .unwrap(),
        prepared.to_bytes().unwrap()
    );
    W::publish(target.path(), &prepared, None, |_| Ok(())).unwrap();
    let document = kpop_native::history_yaml::decode_document(
        &fs::read(target.path().join("GROUNDING.yaml")).unwrap(),
    )
    .unwrap();
    let bindings = map(&map(&map(&map(&document)["meta"])["node_history"])["originals"]);
    assert!(bindings.values().map(count).sum::<usize>() <= kpop_native::value::MAX_VALUES / 4);
    let captured = Capture::read(target.path()).unwrap();
    assert_eq!(map(&map(captured.state())["subjects"]).len(), 769);
    for (subject, (id, object)) in &originals {
        assert_eq!(captured.object(subject, id).unwrap(), *object);
        assert_eq!(
            map(&map(captured.state())["subjects"])[subject]
                .to_json()
                .unwrap()["head"],
            id.as_str()
        );
    }
    for (subject, versions) in P::capture_snapshot(target.path()).unwrap().versions {
        assert_eq!(
            versions
                .iter()
                .map(|(id, version)| (id.clone(), version.frame_sha256().to_owned()))
                .collect::<BTreeMap<_, _>>(),
            original_frames[&subject]
        );
    }
    assert!(bindings.contains_key("p.branch0.item000"));
    assert!(!bindings.contains_key("p.branch5.item000"));
    assert!(
        target
            .path()
            .join(".kpopper/history")
            .join(C::subject_path("p.branch5.item000").unwrap())
            .is_file()
    );
    let portable = P::export(target.path()).unwrap().reconstruct().unwrap();
    drop(branches);
    assert_eq!(
        Capture::read(portable.path()).unwrap().state(),
        captured.state()
    );
    for subject in ["p.branch0.item000", "p.branch5.item000"] {
        write(
            portable.path(),
            &subject.replace('.', "-"),
            &value(json!({"kind":"set", "id":subject, "value":-1})),
        );
        let now = Capture::read(portable.path()).unwrap();
        let (id, original) = &originals[subject];
        assert_eq!(now.object(subject, id).unwrap(), *original);
        assert_eq!(
            map(&map(now.document())["known"])[subject]
                .to_json()
                .unwrap()["v"],
            -1
        );
    }
}
