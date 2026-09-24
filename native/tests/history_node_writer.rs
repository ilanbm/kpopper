use kpop_native::{
    history_authoring::Options, history_node_capture::Capture, history_node_codec as C,
    history_node_publication as P, history_node_writer as W, history_paths::Scheme,
    history_yaml as Y, value::TypedValue as V,
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
fn writes_receipts_lazy_creation_exact_retries_and_source_free_pins() {
    let root = setup();
    let initial = write(root.path(), "add-a", &add());
    assert!(
        !root
            .path()
            .join(".kpopper/history")
            .join(C::subject_path("p.a").unwrap())
            .exists()
    );
    W::publish(root.path(), &initial, None, |_| panic!("retry wrote bytes")).unwrap();
    let capture = Capture::read(root.path()).unwrap();
    let state = map(&map(capture.state())["subjects"]);
    let id = match &map(&state["p.a"])["head"] {
        V::Text(s) => s.clone(),
        _ => panic!(),
    };
    let original = capture.object("p.a", &id).unwrap();
    let changed = write(root.path(), "set-a", &set(2));
    W::publish(root.path(), &changed, None, |_| panic!("retry wrote bytes")).unwrap();
    let snapshot = P::capture_snapshot(root.path()).unwrap();
    let receipt = W::receipt(&snapshot, "set-a").unwrap();
    assert_eq!(
        map(&map(&map(&receipt)["after"])["document"])["known"],
        value(json!({"p.a":{"v":2,"of":"2026-09-24"}}))
    );
    let bundle = P::export(root.path()).unwrap();
    let copy = bundle.reconstruct().unwrap();
    drop(root);
    assert_eq!(
        Capture::read(copy.path())
            .unwrap()
            .object("p.a", &id)
            .unwrap(),
        original
    );
    assert_eq!(
        W::receipt(&P::capture_snapshot(copy.path()).unwrap(), "set-a").unwrap(),
        receipt
    );
}
#[test]
fn real_verifier_recovers_every_boundary_and_torn_append() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = setup();
        write(root.path(), "add-a", &add());
        let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
        let p = W::prepare(root.path(), &set(2), &options("set-a"), None).unwrap();
        let result = W::publish(root.path(), &p, None, |at| {
            if at == phase {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert!(Capture::read(root.path()).is_err());
        if phase == P::Phase::Append(0) {
            let path = root
                .path()
                .join(".kpopper/history")
                .join(C::subject_path("p.a").unwrap());
            let bytes = fs::read(&path).unwrap();
            fs::write(path, &bytes[..bytes.len() - 9]).unwrap();
        }
        let committed = matches!(phase, P::Phase::Commit | P::Phase::View);
        assert_eq!(
            W::recover(root.path(), None).unwrap(),
            if committed {
                "committed"
            } else {
                "rolled_back"
            }
        );
        Capture::read(root.path()).unwrap();
        assert_eq!(
            fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
            if committed {
                p.after_view().unwrap()
            } else {
                before
            }
        );
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
    }
}
#[test]
fn stale_snapshot_archive_and_unsupported_intent_refuse_without_writes() {
    let root = setup();
    write(root.path(), "add-a", &add());
    let before = P::export(root.path()).unwrap().encode().unwrap();
    for action in [
        value(json!({"kind":"retire", "id":"p.a"})),
        value(json!({"kind":"set","id":"p.a","value":2,"expected_record_sha256":"0".repeat(64)})),
    ] {
        assert!(W::prepare(root.path(), &action, &options("refuse"), None).is_err());
        assert_eq!(P::export(root.path()).unwrap().encode().unwrap(), before);
    }
    let p = W::prepare(root.path(), &set(2), &options("set-a"), None).unwrap();
    fs::write(
        root.path().join(".kpopper/replaced.yaml"),
        "foreign archive",
    )
    .unwrap();
    assert!(W::publish(root.path(), &p, None, |_| Ok(())).is_err());
    assert!(
        !root
            .path()
            .join(".kpopper/.history-node-publication.json")
            .exists()
    );
}
#[test]
fn rehashed_false_current_is_rejected_before_journal_creation() {
    let root = setup();
    write(root.path(), "add-a", &add());
    let p = W::prepare(root.path(), &set(2), &options("set-a"), None).unwrap();
    let mut doc = Y::decode_document(&p.after_view().unwrap()).unwrap();
    let V::Map(m) = &mut doc else { panic!() };
    m.insert("known".into(), value(json!({"p.a":{"v":999}})));
    let forged = P::prepare_with_context(
        root.path(),
        "set-a",
        Y::encode_document(&doc).unwrap(),
        p.frames().unwrap(),
        p.context().unwrap().as_ref(),
    )
    .unwrap();
    assert!(W::publish(root.path(), &forged, None, |_| Ok(())).is_err());
    assert!(
        !root
            .path()
            .join(".kpopper/.history-node-publication.json")
            .exists()
    );
}

#[test]
fn forged_rehashed_intent_cannot_publish_or_recover() {
    for phase in [P::Phase::Journal, P::Phase::Commit] {
        let root = setup();
        write(root.path(), "add-a", &add());
        let p = W::prepare(root.path(), &set(2), &options("set-a"), None).unwrap();
        let mut context = p.context().unwrap().unwrap();
        let V::Map(c) = &mut context else { panic!() };
        let V::Map(options) = c.get_mut("options").unwrap() else {
            panic!()
        };
        options.insert("by".into(), value(json!("forged-author")));
        let forged = P::prepare_with_context(
            root.path(),
            "set-a",
            p.after_view().unwrap(),
            p.frames().unwrap(),
            Some(&context),
        )
        .unwrap();
        assert!(W::publish(root.path(), &forged, None, |_| Ok(())).is_err());
        assert!(
            !root
                .path()
                .join(".kpopper/.history-node-publication.json")
                .exists()
        );
        // Simulate untrusted bytes that pass storage hashing, without semantic admission.
        assert!(
            P::publish(
                root.path(),
                &forged,
                |_| Ok(()),
                |at| if at == phase {
                    Err(kpop_native::Error("crash".into()))
                } else {
                    Ok(())
                }
            )
            .is_err()
        );
        let journal = root.path().join(".kpopper/.history-node-publication.json");
        let old_journal = fs::read(&journal).unwrap();
        let old_view = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
        assert!(W::recover(root.path(), None).is_err());
        assert_eq!(fs::read(journal).unwrap(), old_journal);
        assert_eq!(
            fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
            old_view
        );
    }
}

#[test]
fn growing_catalog_has_no_history_files_and_edits_only_materialize_the_changed_node() {
    let root = setup();
    for n in 0..5 {
        write(
            root.path(),
            &format!("add-{n}"),
            &value(json!({"kind":"add","id":format!("p.{n}"),"body":{"v":n}})),
        );
        assert_eq!(
            fs::read_dir(root.path().join(".kpopper/history"))
                .unwrap()
                .count(),
            0
        );
        let manifest = fs::read(
            root.path()
                .join(format!(".kpopper/history-commits/add-{n}.json")),
        )
        .unwrap();
        assert!(
            manifest.len() < 8192,
            "transaction metadata grew to {} bytes",
            manifest.len()
        );
    }
    write(
        root.path(),
        "edit-0",
        &value(json!({"kind":"set","id":"p.0","value":100})),
    );
    let files = fs::read_dir(root.path().join(".kpopper/history"))
        .unwrap()
        .map(|f| f.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(files, vec![C::subject_path("p.0").unwrap()]);
    Capture::read(root.path()).unwrap();
}

#[test]
fn strict_creation_retains_acceptance_in_current_until_the_first_later_change() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = setup();
        let mut opts = options("strict-add");
        opts.strict = true;
        let p = W::prepare(root.path(), &add(), &opts, None).unwrap();
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        assert_eq!(Capture::read(root.path()).unwrap().object_count(), 2);
        let stream = root
            .path()
            .join(".kpopper/history")
            .join(C::subject_path("p.a").unwrap());
        assert!(!stream.exists());
        let receipt = W::receipt(&P::capture_snapshot(root.path()).unwrap(), "strict-add").unwrap();
        opts.operation = "strict-set".into();
        let p = W::prepare(root.path(), &set(2), &opts, None).unwrap();
        assert!(
            W::publish(root.path(), &p, None, |at| if at == phase {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        W::recover(root.path(), None).unwrap();
        if matches!(phase, P::Phase::Journal | P::Phase::Append(0)) {
            assert!(!stream.exists());
        }
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        assert!(stream.exists());
        assert_eq!(Capture::read(root.path()).unwrap().object_count(), 4);
        assert_eq!(
            W::receipt(&P::capture_snapshot(root.path()).unwrap(), "strict-add").unwrap(),
            receipt
        );
    }
}

#[test]
fn lazy_creation_tail_cannot_hide_a_disconnected_same_operation_root() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let root = setup();
    let mut opts = options("strict-add");
    opts.strict = true;
    let p = W::prepare(root.path(), &add(), &opts, None).unwrap();
    let mut doc = Y::decode_document(&p.after_view().unwrap()).unwrap();
    let V::Map(doc_map) = &mut doc else { panic!() };
    let V::Map(meta) = doc_map.get_mut("meta").unwrap() else {
        panic!()
    };
    let V::Map(history) = meta.get_mut("node_history").unwrap() else {
        panic!()
    };
    let V::Map(tails) = history.get_mut("tails").unwrap() else {
        panic!()
    };
    let V::Text(tail) = tails.get_mut("p.a").unwrap() else {
        panic!()
    };
    let mut raw = STANDARD.decode(&*tail).unwrap();
    raw.extend(
        C::Event::create(
            "p.a",
            "strict-add",
            vec![],
            None,
            Some(value(json!({"extra":1}))),
        )
        .unwrap()
        .encode()
        .unwrap(),
    );
    *tail = STANDARD.encode(raw);
    assert!(
        P::prepare_with_context(
            root.path(),
            "strict-add",
            Y::encode_document(&doc).unwrap(),
            p.frames().unwrap(),
            p.context().unwrap().as_ref()
        )
        .is_err()
    );
}
