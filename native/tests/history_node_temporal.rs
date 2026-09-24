use kpop_native::{
    history_authoring::Options,
    history_node_capture::Capture,
    history_node_projection as Projection, history_node_publication as P, history_node_writer as W,
    history_paths::Scheme,
    history_yaml as Y, reasoning_history_assessment as Assessment,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{CaptureOptions, Snapshot},
    value::TypedValue as V,
};
use serde_json::json;
use std::{collections::BTreeMap, fs, path::Path};
fn v(j: serde_json::Value) -> V {
    V::from_json(&j).unwrap()
}
fn m(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
fn l(v: &V) -> &[V] {
    let V::List(l) = v else { panic!() };
    l
}
fn options(op: &str) -> Options {
    Options {
        operation: op.into(),
        recorded_at: "2026-09-24T12:00:00+00:00".into(),
        recording_day: "2026-09-24".into(),
        by: v(json!("writer")),
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    }
}
fn runtime(cache: &Path) -> Runtime {
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            kpop_native::reasoning_runtime::target_name().unwrap()
        ));
    Runtime::open(&archive, cache, OperationalBounds::default()).unwrap()
}
fn setup() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".kpopper")).unwrap();
    fs::write(root.path().join(".kpopper/history.yaml"), "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: fixture\ngeneration: 1\nrequires: [node-history/v1]\n").unwrap();
    let doc = v(
        json!({"meta":{"purpose":"Fixture","reasoning":{"version":2,"profile":"core/v1","requires":["arithmetic/v1"]}},
        "schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"readings":{},"judgments":{}}),
    );
    fs::write(
        root.path().join("GROUNDING.yaml"),
        Y::encode_document(&doc).unwrap(),
    )
    .unwrap();
    root
}
fn write(root: &Path, runtime: &Runtime, op: &str, action: V) -> P::Prepared {
    let p = W::prepare(root, &action, &options(op), Some(runtime)).unwrap();
    W::publish(root, &p, Some(runtime), |_| Ok(())).unwrap();
    p
}
fn seed(root: &Path, runtime: &Runtime, applicability: &str) {
    write(
        root,
        runtime,
        "input",
        v(json!({"kind":"add","id":"p.input","into":"readings","body":{"v":1}})),
    );
    write(
        root,
        runtime,
        "judgment",
        v(
            json!({"kind":"add","id":"d.ready","into":"judgments","body":{
        "verdict":"ready","rests_on":["p.input"],"wrong_if":{"expr":"p.input > 5"},
        "temporal":{"version":1,"applicability":applicability}}}),
        ),
    );
}
fn set(n: i32) -> V {
    v(
        json!({"kind":"set","id":"p.input","value":n,"as_of":if n == 9 { "2026-09-25" } else { "2026-09-26" }}),
    )
}
fn assessment(root: &Path, runtime: &Runtime) -> V {
    let c = Capture::read(root).unwrap();
    let projection = Projection::capture(&c).unwrap();
    let snapshot = Snapshot::from_data(
        projection.document(),
        CaptureOptions {
            context: Some(V::Map(BTreeMap::from([
                ("read_mode".into(), v(json!("supplied"))),
                ("source_collection".into(), v(json!("caller-owned"))),
                ("history".into(), projection.projection().clone()),
            ]))),
            as_of: Some(v(json!("2026-09-24"))),
            ..Default::default()
        },
    )
    .unwrap();
    Assessment::assess(
        &snapshot,
        None,
        "focused-review/v1",
        Some(runtime),
        OperationalBounds::default(),
        None,
    )
    .unwrap()
}

#[test]
fn retained_counterexamples_survive_recovery_and_source_free_export() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    for applicability in ["current", "anchored", "general"] {
        let root = setup();
        seed(root.path(), &runtime, applicability);
        let before = P::capture_snapshot(root.path()).unwrap();
        let original = W::receipt(&before, "judgment").unwrap();
        write(root.path(), &runtime, "fired", set(9));
        write(root.path(), &runtime, "cleared", set(1));
        let report = assessment(root.path(), &runtime);
        let temporal = m(&m(&m(&report)["history_subjects"])["d.ready"])["temporal"].clone();
        assert_eq!(
            m(&temporal)["status"],
            v(json!(if applicability == "current" {
                "recovered"
            } else {
                "counterexample"
            }))
        );
        assert!(
            l(&m(&temporal)["episodes"])
                .iter()
                .any(|e| m(e)["outcome"] == v(json!("counterexample")))
        );
        let bundle = P::export(root.path()).unwrap();
        let copy = bundle.reconstruct().unwrap();
        drop(root);
        assert_eq!(
            W::receipt(&P::capture_snapshot(copy.path()).unwrap(), "judgment").unwrap(),
            original
        );
        assert_eq!(assessment(copy.path(), &runtime), report);
    }
}

#[test]
fn temporal_writer_recovers_each_publication_phase_with_exact_receipts() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    for phase in [
        P::Phase::Journal,
        P::Phase::Append(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let root = setup();
        seed(root.path(), &runtime, "current");
        let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
        let prepared = W::prepare(root.path(), &set(9), &options("fired"), Some(&runtime)).unwrap();
        assert!(
            W::publish(root.path(), &prepared, Some(&runtime), |at| {
                if at == phase {
                    Err(kpop_native::Error("crash".into()))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        let committed = matches!(phase, P::Phase::Commit | P::Phase::View);
        assert_eq!(
            W::recover(root.path(), Some(&runtime)).unwrap(),
            if committed {
                "committed"
            } else {
                "rolled_back"
            }
        );
        if !committed {
            assert_eq!(
                fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
                before
            );
        }
        W::publish(root.path(), &prepared, Some(&runtime), |_| Ok(())).unwrap();
        let captured = Capture::read(root.path()).unwrap();
        let projection = Projection::capture(&captured).unwrap();
        assert_eq!(
            m(&m(projection.projection())["temporal"])["complete"],
            V::Bool(true)
        );
        W::publish(root.path(), &prepared, Some(&runtime), |_| {
            panic!("retry wrote")
        })
        .unwrap();
    }
}

#[test]
fn public_cli_authors_and_reads_temporal_history() {
    let root = setup();
    let resources = tempfile::tempdir().unwrap();
    fs::create_dir(resources.path().join("reasoning")).unwrap();
    let archive = format!(
        "{}.kpopper-runtime",
        kpop_native::reasoning_runtime::target_name().unwrap()
    );
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(&archive),
        resources.path().join("reasoning").join(archive),
    )
    .unwrap();
    for args in [
        vec!["add", "p.input", "v=1", "of=2026-09-20", "--in", "readings"],
        vec![
            "add",
            "d.ready",
            "verdict=ready",
            "rests_on=[p.input]",
            "wrong_if={expr: 'p.input > 5'}",
            "temporal={version: 1, applicability: current}",
            "--in",
            "judgments",
        ],
        vec!["set", "p.input", "9", "--as-of", "2026-09-21"],
        vec!["set", "p.input", "1", "--as-of", "2026-09-22"],
        vec!["open"],
        vec!["check"],
        vec!["pull", "d.ready"],
        vec!["history", "status"],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(&args)
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
    }
    let projection = Projection::capture(&Capture::read(root.path()).unwrap()).unwrap();
    assert_eq!(
        m(&m(projection.projection())["temporal"])["complete"],
        V::Bool(true)
    );
}

#[test]
fn temporal_batch_and_nested_proposal_keep_accepted_world_and_distinct_snapshots() {
    let root = setup();
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    let write_strict = |operation: &str, action: V| {
        let mut opts = options(operation);
        opts.strict = true;
        let prepared = W::prepare(root.path(), &action, &opts, Some(&runtime)).unwrap();
        W::publish(root.path(), &prepared, Some(&runtime), |_| Ok(())).unwrap();
    };
    write_strict(
        "batch",
        v(json!({"kind":"batch","actions":[
            {"kind":"add","id":"d.ready","into":"judgments","body":{
                "verdict":"ready","rests_on":["p.input"],"wrong_if":{"expr":"p.input > 5"},
                "temporal":{"version":1,"applicability":"current"}}},
            {"kind":"add","id":"p.input","into":"readings","body":{"v":1}}
        ]})),
    );
    let before = Capture::read(root.path()).unwrap();
    let prior_head = m(&m(&m(before.state())["subjects"])["d.ready"])["head"].clone();
    write_strict(
        "proposal",
        v(json!({"kind":"proposal","id":"d.ready",
        "into":"judgments","because":"different threshold","body":{
            "verdict":"ready","rests_on":["p.input"],"wrong_if":{"expr":"p.input > 7"},
            "temporal":{"version":1,"applicability":"anchored"}}})),
    );
    let after = Capture::read(root.path()).unwrap();
    assert_eq!(
        m(&m(&m(after.state())["subjects"])["d.ready"])["head"],
        prior_head
    );
    let snapshot = P::capture_snapshot(root.path()).unwrap();
    let receipt = W::receipt(&snapshot, "proposal").unwrap();
    let evidence = m(&m(&receipt)["after"]);
    let accepted = m(&evidence["temporal_replay"]);
    let proposed = m(&m(&evidence["proposal"])["temporal_replay"]);
    assert_ne!(accepted["snapshot"], proposed["snapshot"]);
    assert_eq!(m(&accepted["claims"])["d.ready"], prior_head);
    // Retained proposal receipts name the accepted input versions in their hypothetical
    // evidence. The proposed identity is explicit in authoring, never a committed observation.
    assert_eq!(proposed["claims"], accepted["claims"]);
    assert_ne!(m(&evidence["authoring"])["proposal"], prior_head);
    let projection = Projection::capture(&after).unwrap();
    assert!(
        l(&m(&m(projection.projection())["temporal"])["observations"])
            .iter()
            .all(|observation| m(observation)["snapshot"] != proposed["snapshot"])
    );
    let copy = P::export(root.path()).unwrap().reconstruct().unwrap();
    drop(root);
    assert_eq!(
        W::receipt(&P::capture_snapshot(copy.path()).unwrap(), "proposal").unwrap(),
        receipt
    );
    assert_eq!(
        m(&m(Projection::capture(&Capture::read(copy.path()).unwrap())
            .unwrap()
            .projection())["temporal"])["complete"],
        V::Bool(true)
    );
}

#[test]
fn first_temporal_snapshot_keeps_unchanged_dependency_lazy() {
    let root = setup();
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    seed(root.path(), &runtime, "current");
    let stream = root
        .path()
        .join(".kpopper/history")
        .join(kpop_native::history_node_codec::subject_path("p.input").unwrap());
    assert!(
        !stream.exists(),
        "first temporal evidence materialized an unchanged dependency"
    );
}

#[test]
fn branch_siblings_keep_exact_temporal_worlds_and_merge_both_parents() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    let target = setup();
    seed(target.path(), &runtime, "general");
    let sibling = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(target.path(), &runtime, "left-fired", set(9));
    write(
        sibling.path(),
        &runtime,
        "right-only",
        v(json!({"kind":"add","id":"p.sibling","into":"readings","body":{"v":7}})),
    );
    let left_receipt =
        W::receipt(&P::capture_snapshot(target.path()).unwrap(), "left-fired").unwrap();
    let p = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    drop(sibling);
    W::publish(target.path(), &p, Some(&runtime), |_| Ok(())).unwrap();
    let copy = P::export(target.path()).unwrap().reconstruct().unwrap();
    drop(target);
    let c = Capture::read(copy.path()).unwrap();
    let snapshot = P::capture_snapshot(copy.path()).unwrap();
    assert_eq!(
        snapshot.transactions["merge"].parents,
        ["left-fired", "right-only"]
    );
    assert_eq!(W::receipt(&snapshot, "left-fired").unwrap(), left_receipt);
    let projected = Projection::capture(&c).unwrap();
    let temporal = m(&m(projected.projection())["temporal"]);
    let observations = l(&temporal["observations"]);
    let mut left_seen = false;
    let mut merged_seen = false;
    for observation in observations {
        let o = m(observation);
        let V::Text(raw) = &o["snapshot"] else {
            panic!()
        };
        let data = Snapshot::from_json(raw.as_bytes()).unwrap().to_data();
        let nodes = m(&m(&data)["nodes"]);
        if o["operation"] == v(json!("left-fired")) {
            assert!(!nodes.contains_key("p.sibling"));
            left_seen = true;
        }
        if o["operation"] == v(json!("merge")) && o["phase"] == v(json!("after")) {
            assert!(nodes.contains_key("p.sibling"));
            assert_eq!(m(&m(&nodes["p.input"])["body"])["v"], v(json!(9)));
            merged_seen = true;
        }
    }
    assert!(left_seen && merged_seen);
    let report = assessment(copy.path(), &runtime);
    assert_eq!(
        m(&m(&m(&m(&report)["history_subjects"])["d.ready"])["temporal"])["status"],
        v(json!("counterexample"))
    );
}
