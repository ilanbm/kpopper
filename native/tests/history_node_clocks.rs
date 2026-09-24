use kpop_native::{
    history_authoring::Options,
    history_node_capture::{self as N, Capture},
    history_node_clocks as Clocks, history_node_codec as C,
    history_node_observation::ObservationNode,
    history_node_publication as P, history_node_writer as W,
    history_paths::Scheme,
    history_source_ancestry::Proof,
    history_yaml as Y,
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

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgSign=false",
        ])
        .args(args)
        .env("GIT_AUTHOR_DATE", "2026-09-24T12:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2026-09-24T12:00:00+00:00")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn source(algorithm: &str) -> (tempfile::TempDir, String, String) {
    let root = tempfile::tempdir().unwrap();
    git(
        root.path(),
        &["init", &format!("--object-format={algorithm}")],
    );
    git(root.path(), &["commit", "--allow-empty", "-m", "older"]);
    let a = git(root.path(), &["rev-parse", "HEAD"]);
    git(root.path(), &["commit", "--allow-empty", "-m", "newer"]);
    let b = git(root.path(), &["rev-parse", "HEAD"]);
    (root, a, b)
}
fn seed_clock(root: &std::path::Path, operation: &str, commit: &str, n: i32) {
    // Retained/imported source clocks are object metadata, not a body locator.
    let mut object = json!({"id_scheme":"typed-history/v2","schema_version":2,"subject":"p.clock",
        "kind":"reading","by":"fixture","on":"2026-09-24","op":operation,"at":{"commit":commit},
        "body":{"v":n,"from":"source.repository"},"saw":[],"pins":{},
        "authored":{"collection":"known","profile":"ordinary-reader/v1","fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}});
    let id = kpop_native::identity::typed_object_identity(&value(object.clone())).unwrap();
    object["id"] = json!(id);
    let object = value(object);
    let observation = ObservationNode::root(&id, &Default::default()).unwrap();
    let payload = N::payload(&object, &observation).unwrap();
    let event = C::Event::create("p.clock", operation, vec![], None, Some(payload)).unwrap();
    let doc = value(
        json!({"meta":{"purpose":"Fixture"},"schema":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"},"known":{"p.clock":{"v":n,"from":"source.repository"}}}),
    );
    let p = P::prepare(
        root,
        operation,
        P::bind_view(&Y::encode_document(&doc).unwrap(), operation).unwrap(),
        BTreeMap::from([("p.clock".into(), event.encode().unwrap())]),
    )
    .unwrap();
    P::publish(root, &p, |_| Ok(()), |_| Ok(())).unwrap();
    Capture::read(root).unwrap();
}
fn contested(a: &str, b: &str) -> tempfile::TempDir {
    let target = setup();
    let sibling = setup();
    seed_clock(target.path(), "left-clock", a, 1);
    seed_clock(sibling.path(), "right-clock", b, 2);
    write(target.path(), "left-seed", &add());
    write(
        sibling.path(),
        "right-seed",
        &value(json!({"kind":"add","id":"p.b","body":{"v":1}})),
    );
    let p = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(sibling.path()).unwrap()],
        "merge",
    )
    .unwrap();
    W::publish(target.path(), &p, None, |_| Ok(())).unwrap();
    assert_eq!(acceptance(target.path()), value(json!("contested")));
    target
}
fn acceptance(root: &std::path::Path) -> V {
    let c = Capture::read(root).unwrap();
    map(&map(&map(c.state())["subjects"])["p.clock"])["acceptance"].clone()
}
#[test]
fn verified_git_clocks_resolve_after_admission_and_survive_source_loss() {
    for algorithm in ["sha1", "sha256"] {
        let (repo, a, b) = source(algorithm);
        let proof = Proof::capture(repo.path(), &a, &b).unwrap();
        let target = contested(&a, &b);
        let before = P::capture_snapshot(target.path()).unwrap();
        let old = before
            .transactions
            .iter()
            .filter(|(_, tx)| tx.context.is_some())
            .map(|(op, _)| (op.clone(), W::receipt(&before, op).unwrap()))
            .collect::<BTreeMap<_, _>>();
        let object_count = Capture::read(target.path()).unwrap().object_count();
        let p = Clocks::prepare(target.path(), "proof", &proof).unwrap();
        assert!(
            p.frames().unwrap().is_empty(),
            "clock evidence must not rewrite node history"
        );
        drop(repo);
        W::publish(target.path(), &p, None, |_| Ok(())).unwrap();
        W::publish(target.path(), &p, None, |_| panic!("retry wrote")).unwrap();
        assert_eq!(acceptance(target.path()), value(json!("accepted")));
        let c = Capture::read(target.path()).unwrap();
        assert_eq!(c.object_count(), object_count);
        assert_eq!(
            map(&map(c.document())["known"])["p.clock"]
                .to_json()
                .unwrap()["v"],
            2
        );
        let after = P::capture_snapshot(target.path()).unwrap();
        for (op, receipt) in old {
            assert_eq!(W::receipt(&after, &op).unwrap(), receipt);
        }
        let receipt = W::receipt(&after, "proof").unwrap().to_json().unwrap();
        assert!(
            receipt["before"]["document"]["known"]
                .get("p.clock")
                .is_none()
        );
        assert_eq!(receipt["before"]["authoring"]["action"]["kind"], "source-clocks");
        assert!(receipt["after"]["document"]["known"].get("p.clock").is_none(),
            "compact receipt projection must not copy the current world");
        let copy = P::export(target.path()).unwrap().reconstruct().unwrap();
        drop(target);
        assert_eq!(acceptance(copy.path()), value(json!("accepted")));
        write(copy.path(), "later", &set(4));
        Capture::read(copy.path()).unwrap();
    }
}
#[test]
fn ancestry_refuses_unproven_reverse_and_mutable_endpoints() {
    let (repo, a, b) = source("sha1");
    assert!(Proof::capture(repo.path(), &b, &a).is_err());
    assert!(Proof::capture(repo.path(), &a[..12], &b).is_err());
    assert!(Proof::capture(repo.path(), &a, "HEAD").is_err());
    git(repo.path(), &["checkout", "--detach", &a]);
    git(repo.path(), &["commit", "--allow-empty", "-m", "sibling"]);
    let sibling = git(repo.path(), &["rev-parse", "HEAD"]);
    assert!(Proof::capture(repo.path(), &sibling, &b).is_err());
    // A replacement claiming sibling ancestry must not create a proof.
    git(repo.path(), &["replace", &b, &sibling]);
    assert!(Proof::capture(repo.path(), &sibling, &b).is_err());
    Proof::capture(repo.path(), &a, &b).unwrap();
}
#[test]
fn source_clock_proof_recovers_at_every_durable_boundary() {
    let (repo, a, b) = source("sha1");
    let proof = Proof::capture(repo.path(), &a, &b).unwrap();
    let root = contested(&a, &b);
    let baseline = P::export(root.path()).unwrap();
    for stop in [
        P::Phase::Journal,
        P::Phase::Evidence(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let target = baseline.reconstruct().unwrap();
        let p = Clocks::prepare(target.path(), "proof", &proof).unwrap();
        assert!(
            p.frames().unwrap().is_empty(),
            "clock evidence must not rewrite node history"
        );
        assert!(
            W::publish(target.path(), &p, None, |phase| if phase == stop {
                Err(kpop_native::Error("stop".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        assert!(Capture::read(target.path()).is_err());
        let result = W::recover(target.path(), None).unwrap();
        let committed = matches!(stop, P::Phase::Commit | P::Phase::View);
        assert_eq!(
            result,
            if committed {
                "committed"
            } else {
                "rolled_back"
            }
        );
        assert_eq!(
            acceptance(target.path()),
            value(json!(if committed { "accepted" } else { "contested" }))
        );
    }
}

#[test]
fn imported_proofs_survive_union_and_corrupt_bytes_block_recovery() {
    let (repo, a, b) = source("sha1");
    let proof = Proof::capture(repo.path(), &a, &b).unwrap();
    let target = contested(&a, &b);
    let source = P::export(target.path()).unwrap().reconstruct().unwrap();
    let p = Clocks::prepare(source.path(), "source-proof", &proof).unwrap();
    W::publish(source.path(), &p, None, |_| Ok(())).unwrap();
    write(target.path(), "target-edit", &set(8));
    let p = kpop_native::history_node_branch::prepare(
        target.path(),
        &[P::export(source.path()).unwrap()],
        "proof-merge",
    )
    .unwrap();
    drop(source);
    drop(repo);
    W::publish(target.path(), &p, None, |_| Ok(())).unwrap();
    assert_eq!(acceptance(target.path()), value(json!("accepted")));
    let copy = P::export(target.path()).unwrap().reconstruct().unwrap();
    let p = Clocks::prepare(copy.path(), "repeat-proof", &proof).unwrap();
    assert!(
        W::publish(copy.path(), &p, None, |phase| {
            if phase == P::Phase::Journal {
                Err(kpop_native::Error("stop".into()))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    let evidence = p.evidence().unwrap();
    let path = copy.path().join(evidence.first_key_value().unwrap().0);
    let before = fs::read(copy.path().join("GROUNDING.yaml")).unwrap();
    let mut raw = fs::read(&path).unwrap();
    raw.push(b'!');
    fs::write(&path, &raw).unwrap();
    assert!(W::recover(copy.path(), None).is_err());
    assert_eq!(fs::read(&path).unwrap(), raw);
    assert_eq!(
        fs::read(copy.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
}

#[test]
fn ordinary_batch_evidence_cannot_silently_admit_source_clocks() {
    let (repo, a, b) = source("sha1");
    let proof = Proof::capture(repo.path(), &a, &b).unwrap();
    let target = setup();
    write(target.path(), "seed", &add());
    let p = Clocks::prepare(target.path(), "proof", &proof).unwrap();
    let batch = kpop_native::history_authoring_batch::BatchOptions {
        authoring: options("batch"),
        receipt_version: 8,
        context: value(json!({})),
        evidence: p.evidence().unwrap(),
    };
    let error = W::prepare_batch(
        target.path(),
        &[value(json!({"kind":"add","id":"p.c","body":{"v":3}}))],
        &batch,
        None,
    )
    .unwrap_err();
    assert_eq!(error.0, "source_ancestry_admission");
    assert!(
        target
            .path()
            .join("evidence/source-clocks")
            .try_exists()
            .unwrap()
            == false
    );
}
