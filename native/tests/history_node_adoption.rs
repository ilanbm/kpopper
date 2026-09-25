use kpop_native::{
    history_authoring::Options, history_branch_git as Git, history_node_adoption as Adoption,
    history_node_capture::Capture, history_node_publication as P, history_node_writer as W,
    history_paths::Scheme, value::TypedValue as V,
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
    fs::write(root.path().join(".gitattributes"), P::GIT_ATTRIBUTES).unwrap();
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
    let o = std::process::Command::new("git")
        .args([
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.autocrlf=true",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().into()
}
fn commit(root: &std::path::Path) -> String {
    git(root, &["init"]);
    git(root, &["add", "-f", "."]);
    git(root, &["commit", "-m", "Fixture"]);
    git(root, &["rev-parse", "HEAD"])
}
#[test]
fn portable_node_bytes_survive_git_checkout_with_autocrlf() {
    let root = setup();
    write(root.path(), "seed", &add());
    write(root.path(), "change", &set(2));
    let evidence_path = "evidence/reports/line-endings.txt";
    let evidence = b"first line\r\nsecond line\n";
    let view = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    let prepared = P::prepare_with_evidence(
        root.path(),
        "report",
        P::bind_view(&view, "report").unwrap(),
        BTreeMap::new(),
        None,
        BTreeMap::from([(evidence_path.into(), evidence.to_vec())]),
    )
    .unwrap();
    P::publish(root.path(), &prepared, |_| Ok(()), |_| Ok(())).unwrap();

    let portable = P::export(root.path()).unwrap();
    let copy = portable.reconstruct().unwrap();
    assert_eq!(
        fs::read(copy.path().join(".gitattributes")).unwrap(),
        P::GIT_ATTRIBUTES.as_bytes()
    );
    let mut protected = vec!["GROUNDING.yaml".to_owned(), evidence_path.to_owned()];
    for dir in [".kpopper/history", ".kpopper/history-commits"] {
        let entries: Vec<_> = fs::read_dir(copy.path().join(dir))
            .unwrap()
            .map(|entry| format!("{dir}/{}", entry.unwrap().file_name().to_string_lossy()))
            .collect();
        assert!(!entries.is_empty(), "{dir}");
        protected.extend(entries);
    }
    let expected: BTreeMap<_, _> = protected
        .iter()
        .map(|path| (path.clone(), fs::read(copy.path().join(path)).unwrap()))
        .collect();
    assert_eq!(expected[evidence_path].as_slice(), evidence);

    git(copy.path(), &["init"]);
    git(copy.path(), &["commit", "--allow-empty", "-m", "Base"]);
    git(copy.path(), &["branch", "-M", "baseline"]);
    git(copy.path(), &["switch", "-c", "node"]);
    git(copy.path(), &["add", "-f", "."]);
    git(copy.path(), &["commit", "-m", "Node history"]);
    git(copy.path(), &["switch", "baseline"]);
    git(copy.path(), &["switch", "node"]);

    for (path, bytes) in expected {
        assert_eq!(fs::read(copy.path().join(&path)).unwrap(), bytes, "{path}");
    }
    assert_eq!(
        P::export(copy.path()).unwrap().encode().unwrap(),
        portable.encode().unwrap()
    );
    assert!(git(copy.path(), &["status", "--porcelain"]).is_empty());
}
#[test]
fn portable_git_policy_is_derived_and_cannot_import_arbitrary_filters() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let root = setup();
    write(root.path(), "seed", &add());
    let original = P::export(root.path()).unwrap();
    let resign = |mut value: serde_json::Value| {
        value["digest"] = json!("");
        let unsigned: P::Bundle = serde_json::from_value(value.clone()).unwrap();
        value["digest"] = json!(kpop_native::identity::sha256(
            &serde_json::to_vec(&unsigned).unwrap()
        ));
        serde_json::from_value::<P::Bundle>(value).unwrap()
    };
    let mut malicious = serde_json::to_value(&original).unwrap();
    malicious["files"][".gitattributes"] = json!(STANDARD.encode(b"* filter=untrusted\n"));
    assert_eq!(
        resign(malicious).reconstruct().unwrap_err().0,
        "node_publication_bundle_attributes"
    );

    let mut older = serde_json::to_value(&original).unwrap();
    older["files"]
        .as_object_mut()
        .unwrap()
        .remove(".gitattributes");
    let copy = resign(older).reconstruct().unwrap();
    assert_eq!(
        fs::read(copy.path().join(".gitattributes")).unwrap(),
        P::GIT_ATTRIBUTES.as_bytes()
    );
    assert_eq!(
        Capture::read(copy.path()).unwrap().revision(),
        Capture::read(root.path()).unwrap().revision()
    );
}

fn observations() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    kpop_native::history_node_branch_source::Observation,
    String,
) {
    let target = setup();
    write(target.path(), "seed", &add());
    let source = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(target.path(), "left", &set(2));
    write(source.path(), "right", &set(3));
    let id =
        map(&map(&map(Capture::read(source.path()).unwrap().state())["subjects"])["p.a"])["head"]
            .to_json()
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
    let oid = commit(source.path());
    let observation = Git::capture_node(source.path(), &oid, "GROUNDING.yaml").unwrap();
    (target, source, observation, id)
}
fn settings() -> Adoption::Options {
    Adoption::Options {
        operation: "adopt".into(),
        recorded_at: "2026-09-24T12:00:00Z".into(),
        by: "fixture".into(),
    }
}
#[test]
fn pinned_capture_ignores_dirty_worktree_and_survives_source_loss() {
    let (_, source, observation, _) = observations();
    let expected = observation.capture().unwrap().state().clone();
    fs::write(
        source.path().join("GROUNDING.yaml"),
        b"deliberately uncommitted invalid current\n",
    )
    .unwrap();
    let old = Git::capture_node(source.path(), "HEAD", "GROUNDING.yaml").unwrap();
    assert_eq!(old.revision().unwrap(), observation.revision().unwrap());
    drop(source);
    assert_eq!(&expected, observation.capture().unwrap().state());
}
#[test]
fn explicit_overlap_choice_is_atomic_and_preserves_original_context() {
    let (target, source, observation, chosen) = observations();
    assert!(
        Adoption::prepare(
            target.path(),
            std::slice::from_ref(&observation),
            &value(json!({})),
            &settings()
        )
        .is_err()
    );
    let old = P::capture_snapshot(target.path()).unwrap().transactions["left"]
        .context
        .clone()
        .unwrap();
    let source_old = P::capture_snapshot(source.path()).unwrap().transactions["right"]
        .context
        .clone()
        .unwrap();
    assert_eq!(map(&old)["action"], set(2));
    assert_eq!(map(&source_old)["action"], set(3));
    let p = Adoption::prepare(
        target.path(),
        &[observation],
        &value(json!({"p.a":chosen})),
        &settings(),
    )
    .unwrap();
    drop(source);
    W::publish(target.path(), &p, None, |_| Ok(())).unwrap();
    let captured = Capture::read(target.path()).unwrap();
    assert_eq!(
        map(&map(captured.document())["known"])["p.a"]
            .to_json()
            .unwrap()["v"],
        3
    );
    let snapshot = P::capture_snapshot(target.path()).unwrap();
    assert_eq!(snapshot.transactions["left"].context.as_ref(), Some(&old));
    assert_eq!(
        snapshot.transactions["right"].context.as_ref(),
        Some(&source_old)
    );
    W::publish(target.path(), &p, None, |_| panic!("retry wrote")).unwrap();
    let copy = P::export(target.path()).unwrap().reconstruct().unwrap();
    drop(target);
    assert_eq!(
        Capture::read(copy.path()).unwrap().state(),
        captured.state()
    );
    let restored = P::capture_snapshot(copy.path()).unwrap();
    assert_eq!(restored.transactions["left"].context.as_ref(), Some(&old));
    assert_eq!(
        restored.transactions["right"].context.as_ref(),
        Some(&source_old)
    );
}
#[test]
fn explicit_adoption_recovers_before_and_after_commit_without_git() {
    let (root, source, observation, chosen) = observations();
    let baseline = P::export(root.path()).unwrap();
    drop(source);
    for stop in [
        P::Phase::Journal,
        P::Phase::Import(0),
        P::Phase::Append(0),
        P::Phase::Commit,
        P::Phase::View,
    ] {
        let target = baseline.reconstruct().unwrap();
        let p = Adoption::prepare(
            target.path(),
            std::slice::from_ref(&observation),
            &value(json!({"p.a":chosen})),
            &settings(),
        )
        .unwrap();
        assert!(
            W::publish(target.path(), &p, None, |phase| if phase == stop {
                Err(kpop_native::Error("stop".into()))
            } else {
                Ok(())
            })
            .is_err()
        );
        W::recover(target.path(), None).unwrap();
        let capture = Capture::read(target.path()).unwrap();
        assert_eq!(
            map(&map(capture.document())["known"])["p.a"]
                .to_json()
                .unwrap()["v"],
            if matches!(stop, P::Phase::Commit | P::Phase::View) {
                3
            } else {
                2
            }
        );
    }
}

fn public_fixture() -> (tempfile::TempDir, String) {
    let root = setup();
    write(root.path(), "seed", &add());
    commit(root.path());
    git(root.path(), &["branch", "-M", "target"]);
    git(root.path(), &["switch", "-c", "source"]);
    write(root.path(), "right", &set(3));
    commit(root.path());
    let capture = Capture::read(root.path()).unwrap();
    let chosen = map(&map(&map(capture.state())["subjects"])["p.a"])["head"]
        .to_json()
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    git(root.path(), &["switch", "target"]);
    write(root.path(), "left", &set(2));
    commit(root.path());
    (root, chosen)
}
#[test]
fn public_cli_requires_choices_and_preview_revision_then_adopts() {
    let (root, chosen) = public_fixture();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root.path())
            .args(args)
            .env("KPOPPER_NATIVE_ONLY", "1")
            .output()
            .unwrap()
    };
    let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    let pull = run(&["pull", "p.a", "--from", "source"]);
    assert!(
        pull.status.success(),
        "{}",
        String::from_utf8_lossy(&pull.stderr)
    );
    let pulled = String::from_utf8(pull.stdout).unwrap();
    assert!(
        pulled.contains("BRANCH OBSERVATION")
            && pulled.contains("source revision:")
            && pulled.contains(&chosen),
        "{pulled}"
    );
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
    let preview = run(&["consolidate", "--from", "source", "--dry-run"]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let out = String::from_utf8(preview.stdout).unwrap();
    assert!(out.contains("requires_choice: true"), "{out}");
    let doc =
        kpop_native::history_yaml::decode_document(out.split_once('\n').unwrap().1.as_bytes())
            .unwrap();
    let revision = map(&doc)["source_revision"]
        .to_json()
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
    let missing = run(&["consolidate", "--from", "source", "--by", "fixture"]);
    assert!(!missing.status.success());
    let choose = format!("p.a={chosen}");
    let stale = run(&[
        "consolidate",
        "--from",
        "source",
        "--by",
        "fixture",
        "--choose",
        &choose,
        "--source-revision",
        &"0".repeat(64),
    ]);
    assert!(!stale.status.success());
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
    let adopted = run(&[
        "consolidate",
        "--from",
        "source",
        "--by",
        "fixture",
        "--choose",
        &choose,
        "--source-revision",
        &revision,
    ]);
    assert!(
        adopted.status.success(),
        "{}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert!(String::from_utf8_lossy(&adopted.stdout).contains("BRANCH ADOPTED"));
    let capture = Capture::read(root.path()).unwrap();
    assert_eq!(
        map(&map(capture.document())["known"])["p.a"]
            .to_json()
            .unwrap()["v"],
        3
    );
}
#[test]
fn public_private_branch_retains_a_draft_without_shared_publication() {
    let (root, chosen) = public_fixture();
    git(root.path(), &["switch", "source"]);
    write(
        root.path(),
        "private",
        &value(
            json!({"kind":"add","id":"p.secret","body":{"v":"private-source-marker","privacy":"private"}}),
        ),
    );
    commit(root.path());
    git(root.path(), &["switch", "target"]);
    let before = fs::read(root.path().join("GROUNDING.yaml")).unwrap();
    let before_bundle = P::export(root.path()).unwrap().encode().unwrap();
    let read = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root.path())
        .args(["pull", "p.secret", "--from", "source"])
        .env("KPOPPER_NATIVE_ONLY", "1")
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(String::from_utf8_lossy(&read.stdout).contains("private-source-marker"));
    assert_eq!(
        P::export(root.path()).unwrap().encode().unwrap(),
        before_bundle
    );
    let home = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root.path())
        .args([
            "consolidate",
            "--from",
            "source",
            "--by",
            "fixture",
            "--choose",
            &format!("p.a={chosen}"),
        ])
        .env("KPOPPER_NATIVE_ONLY", "1")
        .env("KPOPPER_PRIVATE_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("RETAINED PRIVATELY"));
    assert_eq!(
        fs::read(root.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
    assert!(
        !root
            .path()
            .join(".kpopper/history-commits/private.json")
            .exists()
    );
}

#[test]
fn multiple_sources_require_each_overlap_and_can_readopt_retained_history() {
    let (target, source, first, chosen) = observations();
    let other = P::export(target.path()).unwrap().reconstruct().unwrap();
    write(
        other.path(),
        "another",
        &value(json!({"kind":"add","id":"p.b","body":{"v":4}})),
    );
    let oid = commit(other.path());
    let second = Git::capture_node(other.path(), &oid, "GROUNDING.yaml").unwrap();
    let sources = vec![first, second];
    let preview = Adoption::preview(target.path(), &sources).unwrap();
    assert!(map(&preview).contains_key("source_set_revision"));
    let choices = value(json!({"p.a":chosen}));
    let prepared = Adoption::prepare(target.path(), &sources, &choices, &settings()).unwrap();
    W::publish(target.path(), &prepared, None, |_| Ok(())).unwrap();
    drop(source);
    drop(other);
    let capture = Capture::read(target.path()).unwrap();
    assert!(map(&map(capture.document())["known"]).contains_key("p.b"));
    Adoption::preview(target.path(), &sources).unwrap();
    let mut options = settings();
    options.operation = "readopt".into();
    let again=Adoption::prepare(target.path(),&sources,&value(json!({"p.a":chosen,"p.b":map(&map(&map(capture.state())["subjects"])["p.b"])["head"].to_json().unwrap()})),&options).unwrap();
    W::publish(target.path(), &again, None, |_| Ok(())).unwrap();
}

#[test]
fn advanced_node_reads_overlay_an_existing_ledger_and_configured_target() {
    let (root, _) = public_fixture();
    let paths = [root.path().join("GROUNDING.yaml")];
    let live = || {
        kpop_native::source_capture::capture_source(
            &paths,
            root.path(),
            kpop_native::source_capture::ReadMode::Live,
            None,
        )
    };
    live().unwrap();
    git(
        root.path(),
        &["update-ref", "refs/kpopper/pending_grounding", "HEAD"],
    );
    let project = kpop_native::project_modes::Project::open(root.path()).unwrap();
    let ledger = kpop_native::pending_state::Ledger::capture(&project).unwrap();
    assert!(ledger.head.is_some() && ledger.events.is_empty());
    live().unwrap().verify().unwrap();
    git(
        root.path(),
        &["update-ref", "-d", "refs/kpopper/pending_grounding"],
    );
    let mut config = project.config().unwrap().to_json().unwrap();
    config["publication"] = json!({"remote":"origin","repository":"fixture/repo","target":"main","branch":"observations","standing_permission":false});
    fs::create_dir_all(project.config_path.parent().unwrap()).unwrap();
    fs::write(&project.config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let captured = live().unwrap();
    let data = captured.snapshot().unwrap().to_data().to_json().unwrap();
    assert_eq!(data["context"]["target"]["status"], "unavailable");
    captured.verify().unwrap();
}
