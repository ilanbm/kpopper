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
fn compact<'a>(snapshot: &'a P::Snapshot, op: &str) -> &'a BTreeMap<String, V> {
    let context = snapshot.transactions[op].context.as_ref().unwrap();
    let fields = map(context);
    assert_eq!(fields["format"], value(json!("node-ledger-authoring/v1")));
    for key in [
        "options", "action", "archive", "evidence", "template", "audits", "result",
    ] {
        map(fields
            .get(key)
            .unwrap_or_else(|| panic!("{op} missing {key}")));
    }
    fields
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
fn writes_compact_context_lazy_creation_exact_retries_and_source_free_pins() {
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
    kpop_native::history_io_metrics::reset();
    let changed = write(root.path(), "set-a", &set(2));
    assert_eq!(
        kpop_native::history_io_metrics::snapshot().semantic_replays,
        1,
        "publication independently replays once before the journal"
    );
    W::publish(root.path(), &changed, None, |_| panic!("retry wrote bytes")).unwrap();
    let snapshot = P::capture_snapshot(root.path()).unwrap();
    let context = compact(&snapshot, "set-a");
    assert!(
        W::receipt(&snapshot, "set-a")
            .unwrap_err()
            .0
            .contains("node_receipt_not_retained")
    );
    assert_eq!(context["action"], set(2));
    assert_eq!(map(&context["options"])["by"], value(json!("writer")));
    assert_eq!(
        map(&map(capture.document())["known"])["p.a"],
        map(&original)["body"]
    );
    assert_eq!(
        map(&map(Capture::read(root.path()).unwrap().document())["known"])["p.a"],
        value(json!({"v":2,"of":"2026-09-24"}))
    );
    let source_state = Capture::read(root.path()).unwrap().state().clone();
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
    let copied = Capture::read(copy.path()).unwrap();
    assert_eq!(
        map(&map(copied.document())["known"])["p.a"],
        value(json!({"v":2,"of":"2026-09-24"}))
    );
    assert_eq!(copied.state(), &source_state);
    let copied_snapshot = P::capture_snapshot(copy.path()).unwrap();
    assert_eq!(compact(&copied_snapshot, "set-a")["action"], set(2));
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
        let creation = P::capture_snapshot(root.path()).unwrap();
        assert_eq!(compact(&creation, "strict-add")["action"], add());
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
            compact(&P::capture_snapshot(root.path()).unwrap(), "strict-add")["action"],
            add()
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
    let tails = history
        .entry("tails".into())
        .or_insert_with(|| V::Map(BTreeMap::new()));
    let V::Map(tails) = tails else { panic!() };
    let mut raw = tails
        .get("p.a")
        .map(|tail| {
            let V::Text(tail) = tail else { panic!() };
            STANDARD.decode(tail).unwrap()
        })
        .unwrap_or_default();
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
    tails.insert("p.a".into(), V::Text(STANDARD.encode(raw)));
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

#[test]
fn public_cli_writes_and_reads_a_marked_record_and_refuses_unsupported_writes() {
    let root = setup();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(args)
            .env_remove("KPOPPER_AGENT_SESSION")
            .output()
            .unwrap()
    };
    for args in [
        vec!["add", "p.a", "v=1"],
        vec!["set", "p.a", "2"],
        vec!["open"],
        vec!["pull", "p.a"],
    ] {
        let out = run(&args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let before = P::export(root.path()).unwrap().encode().unwrap();
    let out = run(&["add", "p.b", "v=3", "--hypothesis", "proposal"]);
    assert!(!out.status.success());
    assert_eq!(P::export(root.path()).unwrap().encode().unwrap(), before);
}

#[test]
fn core_cli_add_set_review_preserves_history_in_open_check_and_pull() {
    let root = setup();
    let doc = value(
        json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{},"judgments":{}}),
    );
    fs::write(
        root.path().join("GROUNDING.yaml"),
        Y::encode_document(&doc).unwrap(),
    )
    .unwrap();
    let resources = tempfile::tempdir().unwrap();
    fs::create_dir(resources.path().join("reasoning")).unwrap();
    let archive = format!(
        "{}.kpopper-runtime",
        kpop_native::reasoning_runtime::target_name().unwrap()
    );
    fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(&archive),
        resources.path().join("reasoning").join(archive),
    )
    .unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(args)
            .env("KPOPPER_NATIVE_RESOURCES", resources.path())
            .env("KPOPPER_NATIVE_CACHE", resources.path().join("cache"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .output()
            .unwrap()
    };
    for args in [
        vec!["add", "p.a", "v=1", "--in", "readings"],
        vec![
            "add",
            "d.b",
            "verdict=ready",
            "rests_on=[p.a]",
            "wrong_if={expr: 'p.a > 3'}",
            "--in",
            "judgments",
        ],
        vec!["set", "p.a", "2"],
        vec!["review", "d.b"],
        vec!["open"],
        vec!["check"],
        vec!["pull", "d.b"],
        vec!["history", "status"],
        vec!["add", "p.c", "v=2", "--in", "readings"],
        vec!["same", "p.a", "p.c", "--keep", "p.a"],
        vec!["add", "p.d", "v=2", "--in", "readings"],
        vec!["distinct", "p.a", "p.d", "different measurements"],
        vec!["open"],
        vec!["check"],
    ] {
        let out = run(&args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(Capture::read(root.path()).unwrap().object_count(), 16);
    for args in [
        vec![
            "add",
            "p.named",
            "v=2",
            "--in",
            "readings",
            "--hypothesis",
            "alpha",
        ],
        vec!["set", "p.named", "3", "--hypothesis", "alpha"],
        vec![
            "add",
            "p.named_alias",
            "v=3",
            "--in",
            "readings",
            "--hypothesis",
            "alpha",
        ],
        vec!["same", "p.named", "p.named_alias", "--keep", "p.named"],
        vec!["open"],
        vec!["check"],
    ] {
        let out = run(&args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let c = Capture::read(root.path()).unwrap();
    assert!(!map(&map(c.document())["readings"]).contains_key("p.named"));
}

#[test]
fn explicit_retirement_retains_lazy_creation_and_replays_after_each_crash() {
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
        let before = Capture::read(root.path()).unwrap();
        let id = map(&map(&map(before.state())["subjects"])["p.a"])["head"].clone();
        let original = before
            .object("p.a", if let V::Text(id) = &id { id } else { panic!() })
            .unwrap();
        assert_eq!(
            compact(&P::capture_snapshot(root.path()).unwrap(), "strict-add")["action"],
            add()
        );
        let stream = root
            .path()
            .join(".kpopper/history")
            .join(C::subject_path("p.a").unwrap());
        assert!(!stream.exists());
        opts.operation = "retire-a".into();
        let action = value(
            json!({"kind":"retire","id":"p.a","of":id.to_json().unwrap(),"over":[],"because":"no longer applies"}),
        );
        let p = W::prepare(root.path(), &action, &opts, None).unwrap();
        assert!(
            W::publish(root.path(), &p, None, |at| if at == phase {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        W::recover(root.path(), None).unwrap();
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        W::publish(root.path(), &p, None, |_| panic!("retry wrote bytes")).unwrap();
        assert!(stream.exists());
        let after = Capture::read(root.path()).unwrap();
        assert!(!map(&map(after.document())["known"]).contains_key("p.a"));
        assert_eq!(after.object_count(), 3);
        assert_eq!(
            after
                .object("p.a", if let V::Text(id) = &id { id } else { panic!() })
                .unwrap(),
            original
        );
        assert_eq!(
            compact(&P::capture_snapshot(root.path()).unwrap(), "strict-add")["action"],
            add()
        );
        let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
        drop(root);
        assert_eq!(Capture::read(copy.path()).unwrap().state(), after.state());
    }
}

#[test]
fn proposals_keep_current_and_hypothetical_objects_until_explicit_acceptance() {
    for existing in [false, true] {
        let root = setup();
        let mut opts = options("proposal-a");
        opts.strict = true;
        if existing {
            write(root.path(), "add-a", &add());
        }
        let request = value(
            json!({"kind":"proposal","id":"p.a","body":{"v":8},"into":"known","because":"possible correction"}),
        );
        let p = W::prepare(root.path(), &request, &opts, None).unwrap();
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        let capture = Capture::read(root.path()).unwrap();
        let doc = map(&map(capture.document())["known"]);
        assert_eq!(doc.contains_key("p.a"), existing);
        if existing {
            assert_eq!(map(&doc["p.a"])["v"], value(json!(1)));
        }
        let snapshot = P::capture_snapshot(root.path()).unwrap();
        assert_eq!(compact(&snapshot, "proposal-a")["action"], request);
        let V::List(proposals) = &map(&map(&map(capture.state())["subjects"])["p.a"])["proposals"]
        else {
            panic!()
        };
        let V::Text(proposal_id) = &proposals[0] else {
            panic!()
        };
        let proposal = capture.object("p.a", proposal_id).unwrap();
        assert_eq!(map(&map(&proposal)["body"])["v"], value(json!(8)));
        let id = json!(proposal_id);
        let over = if existing {
            vec![
                map(&map(&map(capture.state())["subjects"])["p.a"])["head"]
                    .to_json()
                    .unwrap(),
            ]
        } else {
            vec![]
        };
        opts.operation = "accept-proposal".into();
        let action = value(
            json!({"kind":"correct","id":"p.a","of":id,"over":over,"because":"validated correction"}),
        );
        let p = W::prepare(root.path(), &action, &opts, None).unwrap();
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        let after = Capture::read(root.path()).unwrap();
        assert_eq!(
            map(&map(&map(after.document())["known"])["p.a"])["v"],
            value(json!(8))
        );
        let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
        drop(root);
        let copied = Capture::read(copy.path()).unwrap();
        assert_eq!(copied.object("p.a", proposal_id).unwrap(), proposal);
        assert_eq!(
            compact(&P::capture_snapshot(copy.path()).unwrap(), "proposal-a")["action"],
            request
        );
        assert_eq!(Capture::read(copy.path()).unwrap().state(), after.state());
    }
}

#[test]
fn public_cli_explicit_acts_preserve_their_result_and_legacy_pin_identity() {
    let root = setup();
    write(root.path(), "add-a", &add());
    let c = Capture::read(root.path()).unwrap();
    let V::Text(id) = &map(&map(&map(c.state())["subjects"])["p.a"])["head"] else {
        panic!()
    };
    for kind in ["propose", "accept", "refute", "accept", "retire"] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args([
                "history",
                kind,
                "--subject",
                "p.a",
                "--of",
                id,
                "--because",
                "explicit evidence",
            ])
            .env_remove("KPOPPER_AGENT_SESSION")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{kind}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            Capture::read(root.path())
                .unwrap()
                .object("p.a", id)
                .is_ok()
        );
    }
    assert!(
        !map(&map(Capture::read(root.path()).unwrap().document())["known"]).contains_key("p.a")
    );
}

#[test]
fn batch_is_atomic_retains_intermediate_versions_and_final_dependency_pins() {
    let root = setup();
    let action = value(json!({"kind":"batch","actions":[
        {"kind":"add","id":"p.a","body":{"v":1}},
        {"kind":"set","id":"p.a","value":2},
        {"kind":"add","id":"d.b","body":{"claim":"basis","rests_on":["p.a"]}},
        {"kind":"add","id":"p.c","body":{"v":3}}
    ]}));
    let p = W::prepare(root.path(), &action, &options("batch-a"), None).unwrap();
    W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
    W::publish(root.path(), &p, None, |_| panic!("retry wrote")).unwrap();
    let c = Capture::read(root.path()).unwrap();
    assert_eq!(c.object_count(), 8);
    assert_eq!(
        map(&map(&map(c.document())["known"])["p.a"])["v"],
        value(json!(2))
    );
    let states = map(&map(c.state())["subjects"]);
    let V::Text(judgment) = &map(&states["d.b"])["head"] else {
        panic!()
    };
    let judgment = c.object("d.b", judgment).unwrap();
    assert_eq!(
        map(&map(&judgment)["pins"])["p.a"],
        map(&states["p.a"])["head"]
    );
    let streams = root.path().join(".kpopper/history");
    assert!(streams.join(C::subject_path("p.a").unwrap()).exists());
    assert!(!streams.join(C::subject_path("p.c").unwrap()).exists());
    assert_eq!(
        compact(&P::capture_snapshot(root.path()).unwrap(), "batch-a")["action"],
        action
    );
    let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
    drop(root);
    assert_eq!(
        compact(&P::capture_snapshot(copy.path()).unwrap(), "batch-a")["action"],
        action
    );
    assert_eq!(Capture::read(copy.path()).unwrap().state(), c.state());
}

#[test]
fn batch_recovery_commits_all_subjects_or_restores_the_original_view() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = setup();
        write(root.path(), "add-a", &add());
        let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
        let action = value(json!({"kind":"batch","actions":[
            {"kind":"set","id":"p.a","value":4},
            {"kind":"add","id":"p.b","body":{"v":9}}
        ]}));
        let p = W::prepare(root.path(), &action, &options("batch-change"), None).unwrap();
        assert!(
            W::publish(root.path(), &p, None, |at| if at == phase {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        assert!(Capture::read(root.path()).is_err());
        let committed = matches!(phase, P::Phase::Commit | P::Phase::View);
        assert_eq!(
            W::recover(root.path(), None).unwrap(),
            if committed {
                "committed"
            } else {
                "rolled_back"
            }
        );
        assert_eq!(
            fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
            if committed {
                p.after_view().unwrap()
            } else {
                before
            }
        );
        W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
        assert_eq!(Capture::read(root.path()).unwrap().object_count(), 5);
    }
}

#[test]
fn core_batch_forward_pins_and_hypothetical_proposal_survive_source_free_roundtrip() {
    use kpop_native::reasoning_runtime::{OperationalBounds, Runtime};
    let root = setup();
    let doc = value(
        json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{},"judgments":{}}),
    );
    fs::write(
        root.path().join("GROUNDING.yaml"),
        Y::encode_document(&doc).unwrap(),
    )
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            kpop_native::reasoning_runtime::target_name().unwrap()
        ));
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    let mut opts = options("core-batch");
    let action = value(json!({"kind":"batch","actions":[
        {"kind":"add","id":"d.b","body":{"verdict":"ready","rests_on":["p.a"],"wrong_if":{"expr":"p.a > 3"}},"into":"judgments"},
        {"kind":"add","id":"p.a","body":{"v":1},"into":"readings"}
    ]}));
    let p = W::prepare(root.path(), &action, &opts, Some(&runtime)).unwrap();
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let capture = Capture::read(root.path()).unwrap();
    let states = map(&map(capture.state())["subjects"]);
    let V::Text(head) = &map(&states["d.b"])["head"] else {
        panic!()
    };
    let judgment = capture.object("d.b", head).unwrap();
    assert_eq!(
        map(&map(&judgment)["pins"])["p.a"],
        map(&states["p.a"])["head"]
    );
    assert!(map(&map(&judgment)["body"]).contains_key("seen"));
    opts.operation = "core-proposal".into();
    opts.strict = true;
    let action = value(
        json!({"kind":"proposal","id":"d.b","body":{"verdict":"revised","rests_on":["p.a"],"wrong_if":{"expr":"p.a > 5"}},"into":"judgments","because":"check alternate threshold"}),
    );
    let p = W::prepare(root.path(), &action, &opts, Some(&runtime)).unwrap();
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let source = P::capture_snapshot(root.path()).unwrap();
    let contexts =
        ["core-batch", "core-proposal"].map(|op| (op, compact(&source, op)["action"].clone()));
    let source_state = Capture::read(root.path()).unwrap().state().clone();
    let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
    drop(root);
    let restored = P::capture_snapshot(copy.path()).unwrap();
    for (op, expected) in contexts {
        assert_eq!(compact(&restored, op)["action"], expected);
    }
    assert_eq!(Capture::read(copy.path()).unwrap().state(), &source_state);
}

#[test]
fn same_and_distinct_retain_original_claims_and_rewrite_only_current_pins() {
    use kpop_native::reasoning_runtime::{OperationalBounds, Runtime};
    let root = setup();
    let doc = value(
        json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{},"judgments":{}}),
    );
    fs::write(
        root.path().join("GROUNDING.yaml"),
        Y::encode_document(&doc).unwrap(),
    )
    .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            kpop_native::reasoning_runtime::target_name().unwrap()
        ));
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    let mut opts = options("identity-seed");
    opts.strict = true;
    let action = value(json!({"kind":"batch","actions":[
        {"kind":"add","id":"p.a","body":{"v":1},"into":"readings"},
        {"kind":"add","id":"p.b","body":{"v":1},"into":"readings"},
        {"kind":"add","id":"p.c","body":{"v":1},"into":"readings"},
        {"kind":"add","id":"d.j","body":{"verdict":"ready","rests_on":["p.b"],"wrong_if":{"expr":"p.b > 3"}},"into":"judgments"}
    ]}));
    let p = W::prepare(root.path(), &action, &opts, Some(&runtime)).unwrap();
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let c = Capture::read(root.path()).unwrap();
    let V::Text(old_id) = &map(&map(&map(c.state())["subjects"])["p.b"])["head"] else {
        panic!()
    };
    let original = c.object("p.b", old_id).unwrap();
    let frozen = P::export(root.path()).unwrap().encode().unwrap();
    let mut future = opts.clone();
    future.operation = "future-identity".into();
    future.recording_day = "9999-12-31".into();
    let action =
        value(json!({"kind":"same","a":"p.a","b":"p.b","keep":"p.a","as_of":"9999-12-31"}));
    assert_eq!(
        W::prepare(root.path(), &action, &future, Some(&runtime))
            .unwrap_err()
            .0,
        "invalid_identity_as_of"
    );
    assert_eq!(P::export(root.path()).unwrap().encode().unwrap(), frozen);
    opts.operation = "same-ab".into();
    let action = value(json!({"kind":"same","a":"p.a","b":"p.b","keep":"p.a"}));
    let p = W::prepare(root.path(), &action, &opts, Some(&runtime)).unwrap();
    let brief = root.path().join(".kpopper/view.yaml");
    assert!(
        W::publish(root.path(), &p, Some(&runtime), |phase| {
            if phase == P::Phase::Journal {
                fs::write(&brief, "title: concurrent brief\n").unwrap();
            }
            Ok(())
        })
        .unwrap_err()
        .0
        .contains("node_identity_brief_unsupported")
    );
    assert!(
        W::recover(root.path(), Some(&runtime))
            .unwrap_err()
            .0
            .contains("node_identity_brief_unsupported")
    );
    fs::remove_file(&brief).unwrap();
    assert_eq!(
        W::recover(root.path(), Some(&runtime)).unwrap(),
        "rolled_back"
    );
    assert!(
        W::publish(root.path(), &p, Some(&runtime), |phase| {
            if phase == P::Phase::Commit {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert_eq!(
        W::recover(root.path(), Some(&runtime)).unwrap(),
        "committed"
    );
    W::publish(root.path(), &p, Some(&runtime), |_| panic!("retry wrote")).unwrap();
    let after = Capture::read(root.path()).unwrap();
    assert!(!map(&map(after.document())["readings"]).contains_key("p.b"));
    assert_eq!(after.object("p.b", old_id).unwrap(), original);
    let V::Text(judgment_id) = &map(&map(&map(after.state())["subjects"])["d.j"])["head"] else {
        panic!()
    };
    let judgment = after.object("d.j", judgment_id).unwrap();
    assert!(map(&map(&judgment)["pins"]).contains_key("p.a"));
    assert!(!map(&map(&judgment)["pins"]).contains_key("p.b"));
    opts.operation = "distinct-ac".into();
    let action =
        value(json!({"kind":"distinct","a":"p.a","b":"p.c","because":"different measurements"}));
    let p = W::prepare(root.path(), &action, &opts, Some(&runtime)).unwrap();
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
    let after = Capture::read(copy.path()).unwrap();
    assert_eq!(after.object("p.b", old_id).unwrap(), original);
    opts.operation = "forbidden-same".into();
    let action = value(json!({"kind":"same","a":"p.a","b":"p.c","keep":"p.a"}));
    assert!(
        W::prepare(copy.path(), &action, &opts, Some(&runtime))
            .unwrap_err()
            .0
            .contains("identities_declared_distinct")
    );
}

#[test]
fn public_identity_private_second_root_keeps_full_intent_in_private_draft() {
    let root = setup();
    write(root.path(), "add-a", &add());
    write(
        root.path(),
        "private-b",
        &value(json!({"kind":"add","id":"p.b","body":{"v":1,"private":true}})),
    );
    let before = P::export(root.path()).unwrap().encode().unwrap();
    let private = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root.path())
        .args(["same", "p.a", "p.b", "--keep", "p.a"])
        .env("KPOPPER_PRIVATE_HOME", private.path())
        .env_remove("KPOPPER_AGENT_SESSION")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["state"], "private draft");
    assert_eq!(P::export(root.path()).unwrap().encode().unwrap(), before);
    let draft = V::from_tagged(
        &serde_json::from_slice(&fs::read(result["path"].as_str().unwrap()).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(map(&map(&draft)["action"])["a"], value(json!("p.a")));
    assert_eq!(map(&map(&draft)["action"])["b"], value(json!("p.b")));
    assert_eq!(map(&map(&draft)["action"])["keep"], value(json!("p.a")));
}

fn named_setup() -> tempfile::TempDir {
    let root = setup();
    let doc = value(
        json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"known":{},"judgments":{}}),
    );
    fs::write(
        root.path().join("GROUNDING.yaml"),
        Y::encode_document(&doc).unwrap(),
    )
    .unwrap();
    root
}
fn named_runtime() -> (tempfile::TempDir, kpop_native::reasoning_runtime::Runtime) {
    use kpop_native::reasoning_runtime::{OperationalBounds, Runtime};
    let cache = tempfile::tempdir().unwrap();
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            kpop_native::reasoning_runtime::target_name().unwrap()
        ));
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    (cache, runtime)
}
fn hypothesis(action: serde_json::Value, name: Option<&str>) -> V {
    let mut action = json!({"kind":"hypothesis", "action": action});
    if let Some(name) = name {
        action["name"] = json!(name);
    }
    value(action)
}
#[test]
fn named_hypotheses_edit_review_fold_refute_and_source_free_context() {
    let root = named_setup();
    let (_cache, runtime) = named_runtime();
    let write = |root: &std::path::Path, op: &str, action: &V| {
        let p = W::prepare(root, action, &options(op), Some(&runtime))
            .unwrap_or_else(|e| panic!("{op}: {e}"));
        W::publish(root, &p, Some(&runtime), |_| Ok(())).unwrap();
        p
    };
    write(
        root.path(),
        "base",
        &value(json!({"kind":"add","id":"p.a","body":{"v":1,"of":"2026-09-23"}})),
    );
    let original = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    write(
        root.path(),
        "h-set",
        &hypothesis(json!({"kind":"set","id":"p.a","value":2}), Some("alpha")),
    );
    let c = Capture::read(root.path()).unwrap();
    assert_eq!(
        map(&map(&map(c.document())["known"])["p.a"])["v"],
        value(json!(1))
    );
    assert_ne!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        original
    );
    write(
        root.path(),
        "h-add",
        &hypothesis(
            json!({"kind":"add","id":"d.b","into":"judgments","body":{"claim":"bounded","rests_on":["p.a"],"wrong_if":{"expr":"p.a > 5"}}}),
            Some("alpha"),
        ),
    );
    write(
        root.path(),
        "h-review",
        &hypothesis(json!({"kind":"review","id":"d.b"}), Some("alpha")),
    );
    let c = Capture::read(root.path()).unwrap();
    assert!(!map(&map(c.document())["judgments"]).contains_key("d.b"));
    let state = map(&map(c.state())["subjects"]);
    let V::List(proposals) = &map(&state["p.a"])["proposals"] else {
        panic!()
    };
    let V::Text(pin) = &proposals[0] else {
        panic!()
    };
    let claim = c.object("p.a", pin).unwrap();
    let folded = write(
        root.path(),
        "h-fold",
        &hypothesis(
            json!({"kind":"fold","names":["alpha"],"because":"tested"}),
            None,
        ),
    );
    W::publish(root.path(), &folded, Some(&runtime), |_| {
        panic!("retry wrote")
    })
    .unwrap();
    let c = Capture::read(root.path()).unwrap();
    assert_eq!(
        map(&map(&map(c.document())["known"])["p.a"])["v"],
        value(json!(2))
    );
    assert_eq!(c.object("p.a", pin).unwrap(), claim);
    write(
        root.path(),
        "h-new",
        &hypothesis(json!({"kind":"set","id":"p.a","value":4}), Some("beta")),
    );
    write(
        root.path(),
        "h-refute",
        &hypothesis(
            json!({"kind":"refute","names":["beta"],"because":"not supported"}),
            None,
        ),
    );
    let c = Capture::read(root.path()).unwrap();
    assert_eq!(
        map(&map(&map(c.document())["known"])["p.a"])["v"],
        value(json!(2))
    );
    let snapshot = P::capture_snapshot(root.path()).unwrap();
    let contexts = ["h-set", "h-add", "h-review", "h-fold", "h-new", "h-refute"]
        .map(|op| (op, compact(&snapshot, op)["action"].clone()));
    let export = P::export(root.path()).unwrap();
    let copy = export.reconstruct().unwrap();
    drop(root);
    let snapshot = P::capture_snapshot(copy.path()).unwrap();
    for (op, action) in contexts {
        assert_eq!(compact(&snapshot, op)["action"], action);
    }
    assert_eq!(
        Capture::read(copy.path())
            .unwrap()
            .object("p.a", pin)
            .unwrap(),
        claim
    );
}

#[test]
fn named_hypothesis_crashes_and_physical_authority_are_fail_closed() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = named_setup();
        let (_cache, runtime) = named_runtime();
        let base = W::prepare(root.path(), &add(), &options("base"), Some(&runtime)).unwrap();
        W::publish(root.path(), &base, Some(&runtime), |_| Ok(())).unwrap();
        let action = hypothesis(json!({"kind":"set","id":"p.a","value":2}), Some("alpha"));
        let p = W::prepare(root.path(), &action, &options("h-set"), Some(&runtime)).unwrap();
        assert!(
            W::publish(root.path(), &p, Some(&runtime), |at| if at == phase {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        W::recover(root.path(), Some(&runtime)).unwrap();
        W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
        let c = Capture::read(root.path()).unwrap();
        assert_eq!(
            map(&map(&map(c.document())["known"])["p.a"])["v"],
            value(json!(1))
        );
        fs::create_dir(root.path().join(".kpopper/hypotheses")).unwrap();
        fs::write(
            root.path().join(".kpopper/hypotheses/alpha.yaml"),
            "known: {}\n",
        )
        .unwrap();
        assert!(
            W::prepare(root.path(), &action, &options("h-next"), Some(&runtime))
                .unwrap_err()
                .0
                .contains("physical")
        );
    }
}

#[test]
fn public_named_consolidation_dry_run_fold_and_refute() {
    let root = named_setup();
    let (_cache, runtime) = named_runtime();
    let action = hypothesis(
        json!({"kind":"add","id":"p.new","body":{"v":2}}),
        Some("alpha"),
    );
    let p = W::prepare(root.path(), &action, &options("proposal"), Some(&runtime)).unwrap();
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let resources = tempfile::tempdir().unwrap();
    fs::create_dir(resources.path().join("reasoning")).unwrap();
    let archive = format!(
        "{}.kpopper-runtime",
        kpop_native::reasoning_runtime::target_name().unwrap()
    );
    fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(&archive),
        resources.path().join("reasoning").join(archive),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(args)
            .env("KPOPPER_NATIVE_RESOURCES", resources.path())
            .env("KPOPPER_NATIVE_CACHE", resources.path().join("cache"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let before = P::export(root.path()).unwrap().encode().unwrap();
    assert!(run(&["consolidate", "alpha", "--dry-run"]).contains("candidate"));
    assert_eq!(P::export(root.path()).unwrap().encode().unwrap(), before);
    assert!(run(&["consolidate", "alpha"]).contains("folded: alpha"));
    assert_eq!(
        map(&map(&map(Capture::read(root.path()).unwrap().document())["known"])["p.new"])["v"],
        value(json!(2))
    );
    run(&["add", "p.refuted", "v=3", "--hypothesis", "beta"]);
    assert!(
        run(&["consolidate", "--refute", "beta", "insufficient evidence"])
            .contains("refuted: beta")
    );
    assert!(
        !map(&map(Capture::read(root.path()).unwrap().document())["known"])
            .contains_key("p.refuted")
    );
}

#[test]
fn named_fold_assesses_untouched_judgments_and_sources_at_each_phase() {
    let root = named_setup();
    let (_cache, runtime) = named_runtime();
    let write = |op: &str, action: &V| {
        let p = W::prepare(root.path(), action, &options(op), Some(&runtime)).unwrap();
        W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    };
    write(
        "base",
        &value(json!({"kind":"add","id":"p.a","body":{"v":0,"of":"2026-09-23"}})),
    );
    write(
        "guard",
        &value(
            json!({"kind":"add","id":"d.guard","into":"judgments","body":{"verdict":"safe","rests_on":["p.a"],"wrong_if":{"expr":"p.a > 1"}}}),
        ),
    );
    let edit = hypothesis(json!({"kind":"set","id":"p.a","value":2}), Some("alpha"));
    let p = W::prepare(root.path(), &edit, &options("h-edit"), Some(&runtime)).unwrap();
    let physical = root.path().join(".kpopper/hypotheses/alpha.yaml");
    assert!(
        W::publish(root.path(), &p, Some(&runtime), |phase| {
            if phase == P::Phase::Journal {
                fs::create_dir(physical.parent().unwrap()).unwrap();
                fs::write(&physical, "broken: [\n").unwrap();
            }
            Ok(())
        })
        .unwrap_err()
        .0
        .contains("physical")
    );
    assert!(
        W::recover(root.path(), Some(&runtime))
            .unwrap_err()
            .0
            .contains("physical")
    );
    fs::remove_file(physical).unwrap();
    assert_eq!(
        W::recover(root.path(), Some(&runtime)).unwrap(),
        "rolled_back"
    );
    W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let before = P::export(root.path()).unwrap().encode().unwrap();
    let fold = hypothesis(
        json!({"kind":"fold","names":["alpha"],"because":"attempt"}),
        None,
    );
    assert!(
        W::prepare(root.path(), &fold, &options("h-fold"), Some(&runtime))
            .unwrap_err()
            .0
            .contains("hypothesis_candidate_not_clean")
    );
    assert_eq!(P::export(root.path()).unwrap().encode().unwrap(), before);
}

#[test]
fn public_identity_and_named_edit_inspect_private_named_worlds() {
    let root = named_setup();
    let (_cache, runtime) = named_runtime();
    for (op, action) in [
        ("a", value(json!({"kind":"add","id":"p.a","body":{"v":1}}))),
        ("b", value(json!({"kind":"add","id":"p.b","body":{"v":1}}))),
        (
            "private-b",
            hypothesis(
                json!({"kind":"add","id":"p.b","body":{"v":1,"private":true}}),
                Some("alpha"),
            ),
        ),
    ] {
        let p = W::prepare(root.path(), &action, &options(op), Some(&runtime)).unwrap();
        W::publish(root.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    }
    let before = P::export(root.path()).unwrap().encode().unwrap();
    let private = tempfile::tempdir().unwrap();
    for args in [
        vec!["same", "p.a", "p.b", "--keep", "p.a"],
        vec!["set", "p.b", "2", "--hypothesis", "alpha"],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(&args)
            .env("KPOPPER_PRIVATE_HOME", private.path())
            .env_remove("KPOPPER_AGENT_SESSION")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("private"));
        assert_eq!(P::export(root.path()).unwrap().encode().unwrap(), before);
    }
}

#[test]
fn public_cli_update_retains_source_quote_and_reader_evidence() {
    let root = setup();
    write(
        root.path(),
        "source",
        &value(
            json!({"kind":"add","id":"s.old","into":"known","body":{"url":"https://example.test","read":"2026-09-23"}}),
        ),
    );
    write(
        root.path(),
        "reading",
        &value(
            json!({"kind":"add","id":"p.a","as_of":"2026-09-23","into":"known","body":{"v":1,"from":"s.old","at":"table"}}),
        ),
    );
    fs::write(root.path().join("report.json"), serde_json::to_vec(&json!({"event_id":"cli-report","date":"2026-09-24","source_quote":"a is now 2","target":"p.a","value":2})).unwrap()).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(args)
            .env_remove("KPOPPER_AGENT_SESSION")
            .output()
            .unwrap()
    };
    let out = run(&[
        "update",
        "--file",
        "report.json",
        "--state-dir",
        root.path().join("reports").to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt["state"], "applied");
    let event = receipt["event_id"].as_str().unwrap();
    assert_eq!(
        fs::read(root.path().join(format!("evidence/reports/{event}.txt"))).unwrap(),
        b"a is now 2"
    );
    for args in [["open", "--json"], ["pull", "p.a"]] {
        let out = run(&args);
        assert!(
            out.status.success(),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
    drop(root);
    assert_eq!(
        map(&map(Capture::read(copy.path()).unwrap().document())["known"])["p.a"]
            .to_json()
            .unwrap()["v"],
        2
    );
}

#[test]
fn edited_view_proposals_keep_exact_raw_bytes_and_canonical_before() {
    let root = setup();
    write(root.path(), "add-a", &add());
    let canonical = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    let mut doc = Y::decode_document(&canonical).unwrap().to_json().unwrap();
    doc["known"]["p.a"]["v"] = json!(2);
    let mut edited = Y::encode_document(&value(doc)).unwrap();
    edited.extend(b"# raw human evidence\r\n");
    fs::write(root.path().join("GROUNDING.yaml"), &edited).unwrap();
    assert!(Capture::read(root.path()).is_err());
    let mut op = options("edited-a");
    op.strict = true;
    let p = W::prepare_edits(root.path(), &canonical, "human edit", None, &op, None).unwrap();
    assert_eq!(p.before_view().unwrap().unwrap(), edited);
    assert_eq!(
        p.evidence().unwrap()["evidence/view-edits/edited-a.yaml"],
        edited
    );
    W::publish(root.path(), &p, None, |_| Ok(())).unwrap();
    W::publish(root.path(), &p, None, |_| panic!("retry wrote")).unwrap();
    let capture = Capture::read(root.path()).unwrap();
    let state = map(&map(capture.state())["subjects"]);
    let head = match &map(&state["p.a"])["head"] {
        V::Text(id) => id,
        _ => panic!(),
    };
    assert_eq!(
        map(&capture.object("p.a", head).unwrap())["body"],
        value(json!({"v":1}))
    );
    assert_eq!(
        fs::read(root.path().join("evidence/view-edits/edited-a.yaml")).unwrap(),
        edited
    );
    // A complete export remains independently readable without the submitted baseline file.
    let bundle = P::export(root.path()).unwrap();
    let dest = bundle.reconstruct().unwrap();
    Capture::read(dest.path()).unwrap();
}

#[test]
fn edited_view_refuses_wrong_baseline_partial_selection_and_changed_template() {
    let root = setup();
    write(root.path(), "add-a", &add());
    let canonical = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    let mut doc = Y::decode_document(&canonical).unwrap().to_json().unwrap();
    doc["known"]["p.a"]["v"] = json!(2);
    let edited = Y::encode_document(&value(doc.clone())).unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), &edited).unwrap();
    let mut op = options("edited-a");
    op.strict = true;
    assert!(W::prepare_edits(root.path(), &edited, "human edit", None, &op, None).is_err());
    assert!(W::prepare_edits(root.path(), &canonical, "human edit", Some(&[]), &op, None).is_err());
    doc["meta"]["purpose"] = json!("changed");
    fs::write(
        root.path().join("GROUNDING.yaml"),
        Y::encode_document(&value(doc)).unwrap(),
    )
    .unwrap();
    assert!(W::prepare_edits(root.path(), &canonical, "human edit", None, &op, None).is_err());
    assert!(
        !root
            .path()
            .join(".kpopper/history-publication.json")
            .exists()
    );
}

#[test]
fn edited_view_crashes_preserve_human_bytes_and_retry_without_duplicate_proposals() {
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Evidence(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = setup();
        write(root.path(), "add-a", &add());
        let canonical = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
        let mut doc = Y::decode_document(&canonical).unwrap().to_json().unwrap();
        doc["known"]["p.a"]["v"] = json!(2);
        doc["known"]["p.b"] = json!({"v":3});
        let mut edited = Y::encode_document(&value(doc)).unwrap();
        edited.extend(b"# retain exactly\r\n");
        fs::write(root.path().join("GROUNDING.yaml"), &edited).unwrap();
        let mut op = options("edited-a");
        op.strict = true;
        let prepared =
            W::prepare_edits(root.path(), &canonical, "human edit", None, &op, None).unwrap();
        assert!(
            W::publish(root.path(), &prepared, None, |at| if at == phase {
                Err(kpop_native::Error("crash".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        let committed = matches!(phase, P::Phase::Commit | P::Phase::View);
        assert_eq!(
            W::recover(root.path(), None).unwrap(),
            if committed {
                "committed"
            } else {
                "rolled_back"
            }
        );
        assert_eq!(
            fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
            if committed {
                prepared.after_view().unwrap()
            } else {
                edited.clone()
            }
        );
        W::publish(root.path(), &prepared, None, |_| Ok(())).unwrap();
        assert_eq!(Capture::read(root.path()).unwrap().object_count(), 5);
        assert_eq!(
            fs::read(root.path().join("evidence/view-edits/edited-a.yaml")).unwrap(),
            edited
        );
    }
}

#[test]
fn edited_view_refuses_concurrent_edit_and_unrelated_corruption() {
    let root = setup();
    write(root.path(), "add-a", &add());
    write(
        root.path(),
        "add-b",
        &value(json!({"kind":"add","id":"p.b","body":{"v":7}})),
    );
    write(
        root.path(),
        "change-b",
        &value(json!({"kind":"set","id":"p.b","value":8})),
    );
    let canonical = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    let mut doc = Y::decode_document(&canonical).unwrap().to_json().unwrap();
    doc["known"]["p.a"]["v"] = json!(2);
    let edited = Y::encode_document(&value(doc)).unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), &edited).unwrap();
    let mut op = options("edited-a");
    op.strict = true;
    let prepared =
        W::prepare_edits(root.path(), &canonical, "human edit", None, &op, None).unwrap();
    let mut concurrent = edited.clone();
    concurrent.extend(b"# later edit\n");
    fs::write(root.path().join("GROUNDING.yaml"), &concurrent).unwrap();
    assert!(W::publish(root.path(), &prepared, None, |_| Ok(())).is_err());
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        concurrent
    );
    fs::write(root.path().join("GROUNDING.yaml"), &edited).unwrap();
    fs::write(
        root.path()
            .join(".kpopper/history")
            .join(C::subject_path("p.b").unwrap()),
        b"corrupt\n",
    )
    .unwrap();
    assert!(W::prepare_edits(root.path(), &canonical, "human edit", None, &op, None).is_err());
    assert!(W::publish(root.path(), &prepared, None, |_| Ok(())).is_err());
}

#[test]
fn public_cli_reconcile_requires_exact_baseline_and_records_proposals() {
    let root = setup();
    write(root.path(), "add-a", &add());
    let canonical = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    fs::write(root.path().join("saved.yaml"), &canonical).unwrap();
    let mut doc = Y::decode_document(&canonical).unwrap().to_json().unwrap();
    doc["known"]["p.a"]["v"] = json!(2);
    let edited = Y::encode_document(&value(doc)).unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), &edited).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(args)
            .env_remove("KPOPPER_AGENT_SESSION")
            .output()
            .unwrap()
    };
    // Without a local checkpoint or a Git preimage, an explicit baseline is
    // still necessary. A normal successful publisher now retains the checkpoint.
    fs::remove_file(root.path().join(".kpopper/.history-local/accepted-node-view.yaml")).unwrap();
    let fail = run(&[
        "history",
        "reconcile",
        "--record-proposals",
        "--because",
        "human edit",
    ]);
    assert!(!fail.status.success());
    assert!(String::from_utf8_lossy(&fail.stdout).contains("node_edit_baseline_required"));
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        edited
    );
    let out = run(&[
        "history",
        "reconcile",
        "--record-proposals",
        "--because",
        "human edit",
        "--baseline",
        "saved.yaml",
    ]);
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("proposed"));
    assert_eq!(Capture::read(root.path()).unwrap().object_count(), 3);
}
