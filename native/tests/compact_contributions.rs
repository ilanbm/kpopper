//! Explicit contributions and pending readings beside compact node history.
use kpop_native::value::TypedValue as V;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    private: PathBuf,
}

fn fixture(config: Option<&str>) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("repo");
    let private = temp.path().canonicalize().unwrap().join("private");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.test"]);
    if let Some(config) = config {
        let state = root.join(".git/kpopper/project");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("project.json"), config).unwrap();
    }
    Fixture {
        _temp: temp,
        root,
        private,
    }
}

fn run(fixture: &Fixture, args: &[&str]) -> Output {
    let root = &fixture.root;
    let resources = root.join(".test-runtime");
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"))
        .env("KPOPPER_PRIVATE_HOME", &fixture.private)
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git(root: &Path, args: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn pending_head(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--verify",
            "-q",
            "refs/kpopper/pending_grounding",
        ])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

fn profile(fixture: &Fixture) -> Value {
    let status: Value =
        serde_json::from_str(&success(run(fixture, &["history", "status"]))).unwrap();
    status["authority"]["profile"].clone()
}

fn compact_body(root: &Path, id: &str) -> serde_json::Value {
    let capture = kpop_native::history_node_capture::Capture::read(root).unwrap();
    let V::Map(document) = capture.document() else {
        panic!("record mapping")
    };
    let V::Map(known) = &document["known"] else {
        panic!("known mapping")
    };
    known[id].to_json().unwrap()
}

/// Every file under the record root except test runtime caches.
fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![];
    let mut todo = vec![root.to_owned()];
    while let Some(dir) = todo.pop() {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let name = path.strip_prefix(root).unwrap();
            if [".git", ".test-runtime", ".test-cache"]
                .iter()
                .any(|skip| name.starts_with(skip))
            {
                continue;
            }
            if path.is_dir() {
                todo.push(path);
            } else {
                out.push((name.to_owned(), fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn compact_private_and_local_scopes_keep_their_routes() {
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    let before = tree(&fixture.root);
    let draft: Value = serde_json::from_str(&success(run(
        &fixture,
        &[
            "add",
            "p.private",
            "v=3",
            "--shareability",
            "private",
            "--scope",
            "project",
            "--environment",
            "workspace",
        ],
    )))
    .unwrap();
    assert_eq!(draft["state"], "private draft");
    assert!(Path::new(draft["path"].as_str().unwrap()).starts_with(&fixture.private));
    assert_eq!(tree(&fixture.root), before);

    let committed = success(run(
        &fixture,
        &[
            "add",
            "p.feature",
            "v=4",
            "--shareability",
            "project",
            "--scope",
            "feature",
            "--environment",
            "checkout",
        ],
    ));
    assert!(committed.contains("history committed:"), "{committed}");
    assert_eq!(
        compact_body(&fixture.root, "p.feature"),
        serde_json::json!({"v": 4, "scope": {"kind": "feature", "environment": "checkout"}})
    );
    assert_eq!(profile(&fixture), "node-history/v1");
    assert_eq!(pending_head(&fixture.root), None);
}

#[test]
fn pending_reading_captured_before_the_record_stays_an_alternative() {
    let fixture = fixture(None);
    let receipt: Value = serde_json::from_str(&success(run(
        &fixture,
        &[
            "add",
            "p.base",
            "v=9",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
            "--event-id",
            "before-record",
        ],
    )))
    .unwrap();
    assert_eq!(receipt["state"], "captured");
    assert!(!fixture.root.join("GROUNDING.yaml").exists());
    let head = pending_head(&fixture.root).unwrap();
    success(run(&fixture, &["add", "p.base", "v=1"]));
    assert_eq!(profile(&fixture), "node-history/v1");

    let pulled: Value = serde_json::from_str(&success(run(&fixture, &["pull", "p.base"]))).unwrap();
    let contention = &pulled["nodes"]["p.base"]["state"]["contention"];
    let pending = format!("pending-{}", receipt["revision"].as_str().unwrap());
    assert_eq!(contention["status"], "detected", "{contention}");
    assert_eq!(
        contention["witnesses"],
        serde_json::json!([
            ["checkout", {"v": 1}],
            [pending, {"v": 9, "scope": {"kind": "project", "environment": "workspace"}}],
        ])
    );
    // Reading neither folds the pending body nor advances the ledger.
    assert_eq!(
        compact_body(&fixture.root, "p.base"),
        serde_json::json!({"v": 1})
    );
    assert_eq!(pending_head(&fixture.root).unwrap(), head);
    // After birth the identical request replays by recomputing its original plain
    // meaning; different content under the same event is refused.
    let replay: Value = serde_json::from_str(&success(run(
        &fixture,
        &[
            "add",
            "p.base",
            "v=9",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
            "--event-id",
            "before-record",
        ],
    )))
    .unwrap();
    assert_eq!(replay["replay"], true);
    assert_eq!(replay["revision"], receipt["revision"]);
    let changed = run(
        &fixture,
        &[
            "add",
            "p.base",
            "v=8",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
            "--event-id",
            "before-record",
        ],
    );
    assert!(!changed.status.success());
    assert!(String::from_utf8_lossy(&changed.stderr).contains("event ID was already used"));
    assert_eq!(pending_head(&fixture.root).unwrap(), head);
}

#[test]
fn legacy_subset_transport_cannot_name_a_compact_source() {
    // Why compact records do not reuse v3: its origin binds a history/v1 authority,
    // so naming the real compact source is refused and anything else would misstate it.
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    let marker = kpop_native::history_yaml::decode_document(
        &fs::read(fixture.root.join(".kpopper/history.yaml")).unwrap(),
    )
    .unwrap();
    let digest = "0".repeat(64);
    let origin = |authority: V| {
        let mut origin = V::from_json(&serde_json::json!({
            "version": 1, "kind": "selected-subject-observation",
            "operation": "contribution-subset-e", "recorded_at": "2026-09-25T00:00:00Z",
            "source_capture_digest": digest, "source_entry": "GROUNDING.yaml",
            "roots": ["p.base"], "disclosed_locators": [], "prepared_digest": digest,
            "subjects": {"p.base": {"source_state": "prepared_candidate",
                "objects_digest": digest, "reduction_digest": digest}},
        }))
        .unwrap();
        let V::Map(fields) = &mut origin else {
            panic!("origin mapping")
        };
        fields.insert("source_authority".into(), authority);
        origin
    };
    assert_eq!(
        kpop_native::history_projection::validate_subset_origin(&origin(marker))
            .unwrap_err()
            .0,
        "unsupported_authority"
    );
    // The same origin naming a history/v1 source passes the authority check.
    let legacy = V::from_json(&serde_json::json!({"version": 1, "record_id": "r",
        "authority": "history", "generation": 1, "profile": "history/v1"}))
    .unwrap();
    kpop_native::history_projection::validate_subset_origin(&origin(legacy)).unwrap();
}

const PUBLICATION: &str = r#"{"version":1,"mode":"advanced","record":"GROUNDING.yaml","generation":0,"publication":{"remote":"origin","repository":"test/test","target":"main","branch":"pending_grounding","standing_permission":false}}"#;

fn json(output: Output) -> Value {
    serde_json::from_str(&success(output)).unwrap()
}

fn ledger_bundle(root: &Path, revision: &str) -> (V, kpop_native::history_authority::Files) {
    let project = kpop_native::project_modes::Project::open(root).unwrap();
    let ledger = kpop_native::pending_state::Ledger::capture(&project).unwrap();
    let bundle = &ledger.bundles[revision];
    (bundle.value.clone(), bundle.files.clone())
}

fn field<'a>(value: &'a V, path: &[&str]) -> &'a V {
    path.iter().fold(value, |value, key| {
        let V::Map(m) = value else {
            panic!("{key}: not a mapping")
        };
        &m[*key]
    })
}

fn set_field(value: &mut V, path: &[&str], new: V) {
    let (last, parents) = path.split_last().unwrap();
    let mut current = value;
    for key in parents {
        let V::Map(m) = current else {
            panic!("mapping")
        };
        current = m.get_mut(*key).unwrap();
    }
    let V::Map(m) = current else {
        panic!("mapping")
    };
    m.insert((*last).into(), new);
}

/// Re-seal a tampered manifest so refusals come from semantic checks, not identity.
fn reseal(bundle: &mut V, files: &kpop_native::history_authority::Files) {
    let hashes = V::Map(
        files
            .iter()
            .map(|(p, b)| (p.clone(), V::Text(kpop_native::identity::sha256(b))))
            .collect(),
    );
    set_field(bundle, &["manifest", "evidence"], hashes);
    let revision = field(bundle, &["manifest"]).digest().unwrap();
    set_field(bundle, &["revision"], V::Text(revision));
}

fn subjects(bundle: &V) -> Vec<String> {
    let V::Map(m) = field(bundle, &["manifest", "closure", "subjects"]) else {
        panic!("subjects")
    };
    m.keys().cloned().collect()
}

#[test]
fn scoped_add_and_set_capture_v4_selected_closures_and_replay() {
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    success(run(&fixture, &["add", "p.other", "v=7"]));
    let before = tree(&fixture.root);
    let args = [
        "add",
        "p.derived",
        "v=2",
        "from=p.base",
        "--shareability",
        "project",
        "--scope",
        "external",
        "--environment",
        "vendor",
        "--event-id",
        "derived",
    ];
    let receipt = json(run(&fixture, &args));
    assert_eq!(receipt["state"], "captured");
    assert_eq!(receipt["replay"], false);
    // Capture never writes accepted local state.
    assert_eq!(tree(&fixture.root), before);
    let revision = receipt["revision"].as_str().unwrap();
    let (bundle, files) = ledger_bundle(&fixture.root, revision);
    assert_eq!(
        field(&bundle, &["manifest", "version"]).to_json().unwrap(),
        4
    );
    assert_eq!(
        field(&bundle, &["manifest", "format"]),
        &V::Text("node-contribution/v1".into())
    );
    let source = field(&bundle, &["manifest", "source"]);
    assert_eq!(field(source, &["kind"]), &V::Text("node-history/v1".into()));
    assert_eq!(
        field(source, &["authority", "profile"]),
        &V::Text("node-history/v1".into())
    );
    // No author is synthesized: the prepared claim keeps a null actor.
    let V::Map(source_fields) = source else {
        panic!("source")
    };
    assert!(!source_fields.contains_key("by"));
    let prepared = files
        .values()
        .map(|raw| kpop_native::history_yaml::decode_document(raw).unwrap())
        .find(|o| field(o, &["subject"]) == &V::Text("p.derived".into()))
        .unwrap();
    assert_eq!(field(&prepared, &["by"]), &V::Null);
    // The selected closure follows `from`, never unrelated subjects.
    assert_eq!(subjects(&bundle), ["p.base", "p.derived"]);
    assert!(files.keys().all(|p| p.starts_with("node-closure/objects/")));
    // Every retained version of each selected subject travels; nothing else does.
    let carried = files
        .values()
        .map(|raw| {
            let object = kpop_native::history_yaml::decode_document(raw).unwrap();
            field(&object, &["subject"])
                .to_json()
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        carried,
        ["p.base".to_owned(), "p.derived".to_owned()].into()
    );
    kpop_native::pending_bundle::validate(&bundle, &files).unwrap();
    // Literal replay returns the same event; changed content is refused.
    let replay = json(run(&fixture, &args));
    assert_eq!(replay["replay"], true);
    assert_eq!(replay["revision"], receipt["revision"]);
    let mut changed = args.to_vec();
    changed[2] = "v=3";
    let changed = run(&fixture, &changed);
    assert!(!changed.status.success());
    assert!(String::from_utf8_lossy(&changed.stderr).contains("event ID was already used"));

    let set = json(run(
        &fixture,
        &[
            "set",
            "p.base",
            "5",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
            "--event-id",
            "base-set",
        ],
    ));
    let (bundle, _) = ledger_bundle(&fixture.root, set["revision"].as_str().unwrap());
    assert_eq!(subjects(&bundle), ["p.base"]);
    assert_eq!(
        field(
            &bundle,
            &["manifest", "closure", "subjects", "p.base", "source_state"]
        ),
        &V::Text("prepared_candidate".into())
    );
    assert_eq!(tree(&fixture.root), before);
    // The pending set is shown beside the local reading with its own scope.
    let pulled = json(run(&fixture, &["pull", "p.base"]));
    let contention = &pulled["nodes"]["p.base"]["state"]["contention"];
    assert_eq!(contention["status"], "detected");
    assert_eq!(
        contention["witnesses"][0],
        serde_json::json!(["checkout", {"v": 1}])
    );
    // Both pending closures hold p.base: the dependency at v=1 and the set at v=5.
    let witnesses = contention["witnesses"].as_array().unwrap();
    assert_eq!(witnesses.len(), 3, "{contention}");
    assert!(
        witnesses
            .iter()
            .any(|w| w[0].as_str().unwrap().starts_with("pending-") && w[1]["v"] == 5)
    );
}

#[test]
fn v4_tamper_unknown_versions_and_masquerade_refuse() {
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    let receipt = json(run(
        &fixture,
        &[
            "set",
            "p.base",
            "2",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
        ],
    ));
    let (bundle, files) = ledger_bundle(&fixture.root, receipt["revision"].as_str().unwrap());
    let validate = |bundle: &V, files: &kpop_native::history_authority::Files| {
        kpop_native::pending_bundle::validate(bundle, files)
            .err()
            .map(|e| e.0)
            .unwrap_or_default()
    };
    assert_eq!(validate(&bundle, &files), "");
    // Rendered meaning must follow from the carried objects.
    let mut tampered = bundle.clone();
    set_field(
        &mut tampered,
        &["manifest", "document", "known", "p.base", "v"],
        V::from_json(&serde_json::json!(9)).unwrap(),
    );
    reseal(&mut tampered, &files);
    assert_eq!(
        validate(&tampered, &files),
        "invalid_node_contribution_document"
    );
    // A reduction the objects do not produce is refused.
    let mut tampered = bundle.clone();
    set_field(
        &mut tampered,
        &[
            "manifest",
            "closure",
            "subjects",
            "p.base",
            "reduction_digest",
        ],
        V::Text("0".repeat(64)),
    );
    reseal(&mut tampered, &files);
    assert_eq!(validate(&tampered, &files), "subset_reduction_mismatch");
    // Dropping a retained version breaks the closure.
    let mut fewer = files.clone();
    let first = fewer.keys().next().unwrap().clone();
    fewer.remove(&first);
    let mut tampered = bundle.clone();
    reseal(&mut tampered, &fewer);
    assert!(!validate(&tampered, &fewer).is_empty());
    // An extra, unselected file is not accepted as evidence.
    let mut extra = files.clone();
    extra.insert("notes/private.txt".into(), b"x".to_vec());
    let mut tampered = bundle.clone();
    reseal(&mut tampered, &extra);
    assert_eq!(
        validate(&tampered, &extra),
        "contribution_evidence_allowlist"
    );
    // A legacy history/v1 source is refused rather than read as compact.
    let mut tampered = bundle.clone();
    set_field(
        &mut tampered,
        &["manifest", "source", "authority"],
        V::from_json(&serde_json::json!({"version": 1, "record_id": "r", "authority": "history", "generation": 1, "profile": "history/v1"})).unwrap(),
    );
    reseal(&mut tampered, &files);
    assert!(!validate(&tampered, &files).is_empty());
    // Unknown versions never reach a plain-document branch.
    let mut tampered = bundle.clone();
    set_field(
        &mut tampered,
        &["manifest", "version"],
        V::from_json(&serde_json::json!(5)).unwrap(),
    );
    reseal(&mut tampered, &files);
    assert_eq!(validate(&tampered, &files), "invalid_contribution_identity");
    // Changed bytes under the original identity are refused.
    let mut changed = files.clone();
    changed.insert(first, b"subject: p.base\n".to_vec());
    assert_eq!(
        validate(&bundle, &changed),
        "contribution_evidence_mismatch"
    );
}

#[test]
fn private_evidence_in_the_retained_closure_becomes_a_private_draft() {
    for private_field in ["from=/Users/someone/notes.txt", "private=true"] {
        let fixture = fixture(None);
        if private_field == "private=true" {
            // The public authoring route correctly drafts a newly private input.
            // Seed retained private data through the native local writer so this
            // case exercises the export boundary of an already populated record.
            success(run(&fixture, &["add", "p.anchor", "v=0"]));
            use kpop_native::{
                history_authoring::Options,
                history_node_writer as W,
                reasoning_runtime::{OperationalBounds, Runtime},
            };
            let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!(
                    "{}.kpopper-runtime",
                    kpop_native::reasoning_runtime::target_name().unwrap()
                ));
            let runtime = Runtime::open(
                &archive,
                &fixture.root.join(".test-cache"),
                OperationalBounds::default(),
            )
            .unwrap();
            let action = V::from_json(
                &serde_json::json!({"kind":"add", "id":"p.base", "body":{"v":1, "private":true}}),
            )
            .unwrap();
            let options = Options {
                operation: "retained-private".into(),
                recorded_at: "2026-09-25T00:00:00+00:00".into(),
                recording_day: "2026-09-25".into(),
                by: V::Text("writer".into()),
                strict: true,
                paths: kpop_native::history_paths::Scheme::Hashed,
                receipt_version: None,
            };
            let prepared = W::prepare(&fixture.root, &action, &options, Some(&runtime)).unwrap();
            W::publish(&fixture.root, &prepared, Some(&runtime), |_| Ok(())).unwrap();
        } else {
            success(run(&fixture, &["add", "p.base", "v=1", private_field]));
        }
        let before = tree(&fixture.root);
        let draft = json(run(
            &fixture,
            &[
                "set",
                "p.base",
                "2",
                "--shareability",
                "project",
                "--scope",
                "project",
                "--environment",
                "workspace",
            ],
        ));
        assert_eq!(draft["state"], "private draft");
        assert!(Path::new(draft["path"].as_str().unwrap()).starts_with(&fixture.private));
        assert_eq!(tree(&fixture.root), before);
        assert_eq!(pending_head(&fixture.root), None);
    }
}

#[test]
fn a_v4_contribution_refuses_a_legacy_target_without_mutating_it() {
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    let receipt = json(run(
        &fixture,
        &[
            "add",
            "p.shared",
            "v=2",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
        ],
    ));
    let revision = receipt["revision"].as_str().unwrap();
    // Retain the pending Git contribution while replacing only this disposable
    // fixture's active record with an earlier release's legacy authority.
    fs::remove_dir_all(fixture.root.join(".kpopper")).unwrap();
    fs::remove_file(fixture.root.join("GROUNDING.yaml")).unwrap();
    legacy_history_record(&fixture.root);
    let before = tree(&fixture.root);
    let head = pending_head(&fixture.root);
    for extra in [vec!["--preview"], vec!["--by", "reviewer"]] {
        let mut args = vec!["history", "adopt", "--revision", revision];
        args.extend(extra);
        let output = run(&fixture, &args);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("requires a compact target record"),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(tree(&fixture.root), before);
        assert_eq!(pending_head(&fixture.root), head);
    }
}

#[test]
fn v4_materializes_compactly_and_adopts_with_explicit_choices() {
    let fixture = fixture(None);
    success(run(&fixture, &["add", "p.base", "v=1"]));
    let added = json(run(
        &fixture,
        &[
            "add",
            "p.new",
            "v=2",
            "--shareability",
            "project",
            "--scope",
            "external",
            "--environment",
            "vendor",
        ],
    ));
    let set = json(run(
        &fixture,
        &[
            "set",
            "p.base",
            "3",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
        ],
    ));
    let added = added["revision"].as_str().unwrap();
    let set = set["revision"].as_str().unwrap();
    let out = fixture.root.parent().unwrap().join("snapshot");
    let materialized = json(run(
        &fixture,
        &[
            "knowledge",
            "materialize",
            added,
            "--out",
            out.to_str().unwrap(),
        ],
    ));
    assert_eq!(materialized["state"], "materialized");
    let marker = fs::read_to_string(out.join(".kpopper/history.yaml")).unwrap();
    assert!(marker.contains("node-history/v1"), "{marker}");
    assert!(!out.join(".kpopper/history").exists() || !out.join("node-closure").exists());
    assert!(
        out.join(format!("evidence/contributions/{added}.json"))
            .exists()
    );
    assert_eq!(
        compact_body(&out, "p.new"),
        serde_json::json!({"v": 2, "scope": {"kind": "external", "environment": "vendor"}})
    );

    let preview = json(run(
        &fixture,
        &["history", "adopt", "--revision", set, "--preview"],
    ));
    assert_eq!(preview["subjects"]["p.base"]["requires_choice"], true);
    let incoming = preview["subjects"]["p.base"]["incoming_heads"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = run(
        &fixture,
        &["history", "adopt", "--revision", set, "--by", "reviewer"],
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stdout).contains("adoption_choice_required"));
    let no_actor = run(&fixture, &["history", "adopt", "--revision", added]);
    assert!(!no_actor.status.success());
    let adopted = json(run(
        &fixture,
        &["history", "adopt", "--revision", added, "--by", "reviewer"],
    ));
    assert_eq!(adopted["state"], "adopted");
    let choice = format!("p.base={incoming}");
    json(run(
        &fixture,
        &[
            "history",
            "adopt",
            "--revision",
            set,
            "--by",
            "reviewer",
            "--choose",
            &choice,
        ],
    ));
    let status = json(run(&fixture, &["history", "status"]));
    assert_eq!(status["authority"]["profile"], "node-history/v1");
    assert_eq!(status["subjects"]["p.base"]["heads"][0], incoming.as_str());
    assert_eq!(status["subjects"]["p.new"]["acceptance"], "accepted");
    assert!(success(run(&fixture, &["check"])).contains("0 problems"));
    // The adopter recorded the explicit acceptance; the contribution kept no author.
    let capture = kpop_native::history_node_capture::Capture::read(&fixture.root).unwrap();
    let text = format!("{:?}", capture.state());
    assert!(text.contains("accepted"));
}

#[test]
fn compact_target_is_compared_and_pending_conflicts_are_reported() {
    let fixture = fixture(Some(PUBLICATION));
    success(run(&fixture, &["add", "p.base", "v=1"]));
    git(&fixture.root, &["add", "-A"]);
    git(
        &fixture.root,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "record"],
    );
    git(
        &fixture.root,
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    json(run(
        &fixture,
        &[
            "set",
            "p.base",
            "4",
            "--shareability",
            "project",
            "--scope",
            "project",
            "--environment",
            "workspace",
        ],
    ));
    let captured = kpop_native::source_capture::capture_source(
        &[fixture.root.join("GROUNDING.yaml")],
        &fixture.root,
        kpop_native::source_capture::ReadMode::Live,
        None,
    )
    .unwrap();
    let data = captured.snapshot().unwrap().to_data().to_json().unwrap();
    let context = &data["context"];
    assert_eq!(
        context["target"]["status"], "observed",
        "{}",
        context["target"]
    );
    let holders = context["conflicts"]["p.base"].as_array().unwrap();
    let names = holders
        .iter()
        .map(|h| h[0].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names[0], "checkout");
    assert!(names.iter().any(|n| n.starts_with("target:")), "{names:?}");
    assert!(names.iter().any(|n| n.starts_with("pending-")), "{names:?}");
    captured.verify().unwrap();
}

/// A core/v1 active-history record as earlier releases created it. Only fixtures
/// build this format; new records are compact.
fn legacy_history_record(root: &Path) {
    use kpop_native::{
        history_bootstrap as B,
        reasoning_runtime::{OperationalBounds, Runtime},
    };
    let entry = root.join("GROUNDING.yaml");
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            kpop_native::reasoning_runtime::target_name().unwrap()
        ));
    let runtime = Runtime::open(
        &archive,
        &root.join(".test-cache"),
        OperationalBounds::default(),
    )
    .unwrap();
    let policy = kpop_native::project_modes::Project::open(root)
        .unwrap()
        .config()
        .unwrap();
    let action = V::from_json(
        &serde_json::json!({"kind":"add","id":"p.base","body":{"v":1},"as_of":"2026-09-19"}),
    )
    .unwrap();
    let mutation = B::prepare(
        &entry,
        &action,
        &policy,
        &B::BootstrapOptions {
            operation: "first-legacy".into(),
            recorded_at: "2026-09-19T12:00:00+00:00".into(),
            recording_day: "2026-09-19".into(),
            record_id: "legacy-fixture".into(),
            by: V::Null,
        },
        Some(&runtime),
    )
    .unwrap();
    B::publish(&entry, &mutation, &policy, Some(&runtime), &mut |_| Ok(())).unwrap();
}

#[test]
fn legacy_v3_stays_readable_and_materializes_and_adopts_compactly() {
    let fixture = fixture(None);
    legacy_history_record(&fixture.root);
    let receipt = json(run(
        &fixture,
        &[
            "add",
            "p.legacy",
            "v=2",
            "--shareability",
            "project",
            "--scope",
            "external",
            "--environment",
            "vendor",
        ],
    ));
    let revision = receipt["revision"].as_str().unwrap().to_owned();
    let (bundle, files) = ledger_bundle(&fixture.root, &revision);
    assert_eq!(
        field(&bundle, &["manifest", "version"]).to_json().unwrap(),
        3
    );
    kpop_native::pending_bundle::validate(&bundle, &files).unwrap();
    let out = fixture.root.parent().unwrap().join("snapshot");
    json(run(
        &fixture,
        &[
            "knowledge",
            "materialize",
            &revision,
            "--out",
            out.to_str().unwrap(),
        ],
    ));
    assert!(
        fs::read_to_string(out.join(".kpopper/history.yaml"))
            .unwrap()
            .contains("node-history/v1")
    );
    assert!(!out.join("history-closure").exists());
    assert_eq!(
        compact_body(&out, "p.legacy"),
        serde_json::json!({"v": 2, "scope": {"kind": "external", "environment": "vendor"}})
    );
    // Replace the legacy record with a compact one; the retained v3 adopts compactly.
    for path in [".kpopper", "GROUNDING.yaml"] {
        let path = fixture.root.join(path);
        if path.is_dir() {
            fs::remove_dir_all(&path).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
    }
    for leftover in fs::read_dir(&fixture.root).unwrap() {
        let path = leftover.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if name.starts_with(".kpopper") || name == "evidence" {
            let _ = fs::remove_dir_all(&path);
        }
    }
    success(run(&fixture, &["add", "p.fresh", "v=1"]));
    assert_eq!(profile(&fixture), "node-history/v1");
    let adopted = json(run(
        &fixture,
        &[
            "history",
            "adopt",
            "--revision",
            &revision,
            "--by",
            "reviewer",
        ],
    ));
    assert_eq!(adopted["state"], "adopted");
    assert_eq!(
        compact_body(&fixture.root, "p.legacy"),
        serde_json::json!({"v": 2, "scope": {"kind": "external", "environment": "vendor"}})
    );
}
