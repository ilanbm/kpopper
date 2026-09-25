//! `history migrate` creates compact node history from an ordinary record while
//! keeping the established import's claims, acts, topology and source evidence.
use kpop_native::{
    history_migration as M, history_node_archive::Archive, history_node_capture::Capture,
    history_node_import as I, history_yaml as Y, source_capture::ReadMode, value::TypedValue as V,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_READ_MODE")
        .args(args)
        .output()
        .unwrap()
}
fn ok(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn refused(output: Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["state"], "refused");
    value["code"].as_str().unwrap().to_owned()
}
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            if name == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(name, fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    if root.exists() {
        walk(root, root, &mut out);
    }
    out
}
fn write(root: &Path, files: &[(&str, &str)]) {
    for (name, raw) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw).unwrap();
    }
}
fn map(value: &V) -> &BTreeMap<String, V> {
    let V::Map(map) = value else {
        panic!("expected map: {value:?}")
    };
    map
}
fn text(value: &V) -> &str {
    let V::Text(text) = value else {
        panic!("expected text: {value:?}")
    };
    text
}
fn options(operation: &str) -> M::Options {
    M::Options {
        operation: operation.into(),
        recorded_at: "2026-09-25T00:00:00+00:00".into(),
        record_id: Some("record-fixture".into()),
        read_mode: ReadMode::Frozen,
        route: false,
        as_of: None,
    }
}
/// The import map and archive retained by the copy's single import transaction.
fn retained(copy: &Path) -> (V, Archive) {
    let mut mapping = None;
    let mut archive = None;
    for (name, raw) in tree(copy) {
        if let Some(rest) = name.strip_prefix("evidence/bootstrap/") {
            if rest.ends_with(".zip") {
                assert!(archive.replace(Archive::decode(&raw).unwrap()).is_none());
            } else if !rest.ends_with("-live.json") {
                let tagged: Value = serde_json::from_slice(&raw).unwrap();
                assert!(mapping.replace(V::from_tagged(&tagged).unwrap()).is_none());
            }
        }
    }
    (mapping.unwrap(), archive.unwrap())
}

const RECORD: &str = "meta: {purpose: Fixture}\nknown:\n  p.x: {v: 1, of: 2026-09-19}\n  p.src: {v: 3, from: {file: docs/source.txt}}\njudgments:\n  d.j: {verdict: ok, rests_on: [p.x]}\n";
const REPLACED: &str = "d.j:\n- verdict: older\n  rests_on: [p.x]\n  ended: superseded\n  day: 2026-09-20\n- verdict: middle\n  rests_on: [p.x]\n  day: 2026-09-21\nd.gone:\n- verdict: retired\n  ended: no longer relevant\n";
const HYPOTHESIS: &str =
    "hypothesis: {claim: another reading, born: 2026-09-12}\nknown:\n  p.alt: {v: 5}\n";

#[test]
fn compact_import_keeps_the_established_import_objects_and_their_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    write(
        &source,
        &[
            ("GROUNDING.yaml", RECORD),
            (".kpopper/replaced.yaml", REPLACED),
            (".kpopper/hypotheses/alternative.yaml", HYPOTHESIS),
            ("docs/source.txt", "exact evidence\n"),
        ],
    );
    let before = tree(&source);
    let entry = Path::new("GROUNDING.yaml");
    let legacy = M::Plan::prepare(entry, &source, options("import-same"), None).unwrap();
    let compact = I::Plan::prepare(entry, &source, options("import-same"), None).unwrap();
    let legacy_objects = legacy
        .files()
        .iter()
        .filter(|(path, _)| path.starts_with(".kpopper/history/"))
        .map(|(_, raw)| {
            let object = Y::decode_document(raw).unwrap();
            (text(&map(&object)["id"]).to_owned(), object)
        })
        .collect::<BTreeMap<_, _>>();
    let copy = root.join("copy");
    compact.publish(&copy).unwrap();
    assert_eq!(tree(&source), before, "the source was written");
    let captured = Capture::read(&copy).unwrap();
    let (mapping, archive) = retained(&copy);
    let imported = map(&mapping)["objects"].to_json().unwrap();
    let ids = imported
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["semantic_id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    // Every claim and act of the established import, with the same semantic ID.
    assert_eq!(ids.len(), 9);
    for id in &ids {
        let expected = &legacy_objects[id];
        let subject = text(&map(expected)["subject"]);
        assert_eq!(&captured.object(subject, id).unwrap(), expected);
    }
    // The rest of the legacy objects are the named-hypothesis proposal, which the
    // compact copy imports through the established physical-import transaction.
    let hypothesis = legacy_objects
        .iter()
        .filter(|(id, _)| !ids.contains(*id))
        .map(|(_, o)| text(&map(o)["subject"]).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(hypothesis, BTreeSet::from(["p.alt".to_owned()]));
    assert_eq!(captured.object_count(), legacy_objects.len());
    let state = map(&map(captured.state())["subjects"]);
    for (subject, acceptance) in [
        ("p.x", "accepted"),
        ("d.j", "accepted"),
        ("d.gone", "retired"),
        ("p.alt", "proposed"),
    ] {
        assert_eq!(
            text(&map(&state[subject])["acceptance"]),
            acceptance,
            "{subject}"
        );
    }
    // The archive retains every original byte; nothing legacy is active.
    for (name, raw) in &before {
        assert_eq!(archive.files().get(name), Some(raw), "{name}");
    }
    assert_eq!(archive.files().len(), before.len());
    let copied = tree(&copy);
    assert_eq!(copied["docs/source.txt"], b"exact evidence\n");
    assert_eq!(
        copied[".kpopper/hypotheses/alternative.yaml"],
        HYPOTHESIS.as_bytes()
    );
    assert!(!copied.contains_key(".kpopper/replaced.yaml"));
    assert!(
        !copied
            .keys()
            .any(|p| p.starts_with(".kpopper-history-migration")
                || p.starts_with(".kpopper/history-commits/") && p.ends_with(".yaml"))
    );
    let view = Y::decode_document(&copied["GROUNDING.yaml"]).unwrap();
    let meta = map(&map(&view)["meta"]);
    assert!(meta.contains_key("node_history") && !meta.contains_key("history"));
    assert!(!meta.contains_key("history_import"));
}

#[test]
fn public_migrate_preview_and_copy_are_compact_and_leave_the_source_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    write(
        &source,
        &[
            ("GROUNDING.yaml", RECORD),
            (".kpopper/replaced.yaml", REPLACED),
            (".kpopper/hypotheses/alternative.yaml", HYPOTHESIS),
            ("docs/source.txt", "exact evidence\n"),
        ],
    );
    let before = tree(&source);
    let preview = ok(run(&source, &["history", "migrate", "--read-mode", "live"]));
    assert_eq!(tree(&source), before);
    assert_eq!(preview["state"], "preview");
    assert_eq!(preview["complete"], true);
    assert_eq!(preview["history_format"], "node-history/v1");
    assert_eq!(preview["source_read_mode"], "live");
    assert_eq!(preview["copied_read_mode"], "frozen");
    assert_eq!(preview["captured_pending_context"], true);
    assert_eq!(preview["entry_renamed"], false);
    assert_eq!(preview["hypotheses"], serde_json::json!(["alternative"]));
    assert_eq!(
        preview["materialized_members"],
        serde_json::json!(["docs/source.txt"])
    );
    assert_eq!(
        preview["archived_only_members"],
        serde_json::json!([".kpopper/replaced.yaml", "GROUNDING.yaml"])
    );
    assert_eq!(
        preview["manifest"]["original_provenance"]["writer"],
        Value::Null
    );
    assert_eq!(preview["manifest"]["observation"]["read_mode"], "live");
    let copy = root.join("copy");
    let published = ok(run(
        &source,
        &[
            "history",
            "migrate",
            "--read-mode",
            "live",
            "--to",
            copy.to_str().unwrap(),
        ],
    ));
    assert_eq!(published["state"], "materialized");
    assert_eq!(tree(&source), before);
    let status = ok(run(&copy, &["history", "status"]));
    assert_eq!(status["authority"]["profile"], "node-history/v1");
    // One import transaction and the physical hypothesis import.
    assert_eq!(status["commits"], 2);
    assert_eq!(status["subjects"]["d.gone"]["acceptance"], "retired");
    assert_eq!(status["subjects"]["p.alt"]["acceptance"], "proposed");
    let (mapping, _) = retained(&copy);
    let observation = map(&map(&mapping)["observation"]);
    let live = &tree(&copy)[text(&observation["path"])];
    assert_eq!(
        text(&observation["sha256"]),
        kpop_native::identity::sha256(live)
    );
    let observed = kpop_native::reasoning_snapshot::Snapshot::from_json(live).unwrap();
    assert_eq!(observed.snapshot_id(), text(&observation["snapshot_id"]));
    // The alias selects the same compact import; it is not another backend.
    // Both earlier copy formats displayed the exact replaced member; so does this one.
    let shown = run(&copy, &["--json", "--frozen", "pull", "d.j", "--history"]);
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let wrapper: Value = serde_json::from_slice(&shown.stdout).unwrap();
    let payload: Value = serde_json::from_str(wrapper["output"].as_str().unwrap()).unwrap();
    let section = &payload["historical_section"];
    let archived = section["legacy_archive_evidence"].as_array().unwrap();
    assert_eq!(archived.len(), 2, "{section}");
    assert_eq!(archived[0]["source"], "verified_import_archive");
    assert_eq!(archived[0]["archive_member"], ".kpopper/replaced.yaml");
    assert_eq!(
        archived[0]["member_sha256"],
        kpop_native::identity::sha256(REPLACED.as_bytes())
    );
    assert_eq!(
        section["versions"].as_array().unwrap().len(),
        5,
        "{section}"
    );
    let alias = root.join("alias");
    ok(run(
        &source,
        &[
            "history",
            "migrate",
            "--node-history",
            "--to",
            alias.to_str().unwrap(),
        ],
    ));
    let (alias_mapping, _) = retained(&alias);
    assert_eq!(
        text(&map(&alias_mapping)["format"]),
        "node-history-import-map/v1"
    );
    assert_eq!(tree(&source), before);
}

#[test]
fn custom_entry_and_external_members_keep_exact_topology_for_replay() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    let files = [
        (
            "project/notes.yml",
            "# entry\nalso: [../shared/extra.yaml]\nknown: {p.first: {v: 1}}\n",
        ),
        ("shared/extra.yaml", "known: {p.second: {v: 2}}\n"),
        (
            "project/PROVENANCE.replaced.yaml",
            "p.first:\n- v: 0\n  ended: remeasured\n",
        ),
        ("project/PROVENANCE.d/other.yaml", HYPOTHESIS),
    ];
    write(&source, &files);
    let project = source.join("project");
    let before = tree(&source);
    let preview = ok(run(
        &project,
        &["history", "migrate", "--record", "notes.yml"],
    ));
    assert_eq!(preview["state"], "preview");
    assert_eq!(preview["entry_renamed"], true);
    assert_eq!(preview["source_entry"], "notes.yml");
    assert_eq!(preview["destination_entry"], "GROUNDING.yaml");
    assert_eq!(
        preview["manifest"]["topology"]["entry"],
        "project/notes.yml"
    );
    let copy = root.join("copy");
    ok(run(
        &project,
        &[
            "history",
            "migrate",
            "--record",
            "notes.yml",
            "--to",
            copy.to_str().unwrap(),
        ],
    ));
    assert_eq!(tree(&source), before);
    let status = ok(run(&copy, &["history", "status"]));
    assert_eq!(status["subjects"]["p.first"]["acceptance"], "accepted");
    assert_eq!(status["subjects"]["p.second"]["acceptance"], "accepted");
    assert_eq!(status["subjects"]["p.alt"]["acceptance"], "proposed");
    assert_eq!(
        status["subjects"]["p.first"]["heads"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let copied = tree(&copy);
    assert_eq!(
        copied[".kpopper/hypotheses/other.yaml"],
        HYPOTHESIS.as_bytes()
    );
    assert!(!copied.contains_key("notes.yml") && !copied.contains_key("PROVENANCE.replaced.yaml"));
    // Exact rollback evidence: every original, at its original relative location.
    let (mapping, archive) = retained(&copy);
    let topology = map(&map(&mapping)["topology"]);
    let originals = map(&topology["originals"]);
    let mut restored = BTreeMap::new();
    for (member, raw) in archive.files() {
        restored.insert(text(&originals[member]).to_owned(), raw.clone());
    }
    let expected = files
        .iter()
        .map(|(name, raw)| (name.to_string(), raw.as_bytes().to_vec()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(restored, expected);
    // Reads replay from the archive, with no access to the deleted source.
    fs::remove_dir_all(&source).unwrap();
    Capture::read(&copy).unwrap();
}

#[test]
fn incomplete_and_raced_imports_refuse_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();

    // An unresolved archive alias is reported, never published as a partial copy.
    let blocked = root.join("blocked");
    write(
        &blocked,
        &[
            ("GROUNDING.yaml", "known: {p.x: {v: 2}}\n"),
            (".kpopper/replaced.yaml", "p.x:\n- same_as: 9\n"),
        ],
    );
    let before = tree(&blocked);
    let preview = ok(run(&blocked, &["history", "migrate"]));
    assert_eq!(preview["state"], "blocked");
    assert_eq!(preview["complete"], false);
    assert_eq!(
        refused(run(
            &blocked,
            &[
                "history",
                "migrate",
                "--to",
                root.join("b").to_str().unwrap()
            ]
        )),
        "incomplete_history_import"
    );
    assert!(!root.join("b").exists());
    assert_eq!(tree(&blocked), before);

    // Source changes after planning and a destination that appears first both refuse.
    let source = root.join("source");
    write(&source, &[("GROUNDING.yaml", "known: {p.x: {v: 1}}\n")]);
    let entry = Path::new("GROUNDING.yaml");
    let plan = I::Plan::prepare(entry, &source, options("import-race"), None).unwrap();
    let raced = root.join("raced");
    fs::create_dir(&raced).unwrap();
    fs::write(raced.join("mine.txt"), "someone else's\n").unwrap();
    assert!(plan.publish(&raced).is_err());
    assert_eq!(tree(&raced).len(), 1);
    fs::write(source.join("GROUNDING.yaml"), "known: {p.x: {v: 2}}\n").unwrap();
    assert!(plan.publish(&root.join("changed")).is_err());
    assert!(!root.join("changed").exists());
    assert!(!root.read_dir().unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".kpopper-import-")
    }));
}

#[test]
fn tampered_archive_or_map_refuses_every_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    write(
        &source,
        &[
            ("GROUNDING.yaml", RECORD),
            (".kpopper/replaced.yaml", REPLACED),
            ("docs/source.txt", "x\n"),
        ],
    );
    let copy = root.join("copy");
    ok(run(
        &source,
        &["history", "migrate", "--to", copy.to_str().unwrap()],
    ));
    Capture::read(&copy).unwrap();
    for suffix in [".zip", ".json"] {
        let target = root.join(format!("tampered{suffix}"));
        for (name, raw) in tree(&copy) {
            let path = target.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, raw).unwrap();
        }
        let name = tree(&target)
            .into_keys()
            .find(|p| p.starts_with("evidence/bootstrap/") && p.ends_with(suffix))
            .unwrap();
        let mut raw = fs::read(target.join(&name)).unwrap();
        let last = raw.len() - 2;
        raw[last] ^= 1;
        fs::write(target.join(&name), raw).unwrap();
        assert!(Capture::read(&target).is_err(), "{name}");
        refused(run(&target, &["history", "status"]));
    }
}

#[test]
fn active_legacy_history_converts_by_default_with_its_original_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    write(
        &source,
        &[
            ("GROUNDING.yaml", RECORD),
            (".kpopper/replaced.yaml", REPLACED),
            ("docs/source.txt", "x\n"),
        ],
    );
    let legacy = root.join("legacy");
    M::Plan::prepare(
        Path::new("GROUNDING.yaml"),
        &source,
        options("import-legacy"),
        None,
    )
    .unwrap()
    .publish(&legacy)
    .unwrap();
    let before = tree(&legacy);
    let old = ok(run(&legacy, &["history", "status"]));
    assert_eq!(old["authority"]["profile"], "history/v1");
    let preview = ok(run(&legacy, &["history", "migrate"]));
    assert_eq!(preview["state"], "preview");
    assert_eq!(preview["history_format"], "node-history/v1");
    let node = root.join("node");
    ok(run(
        &legacy,
        &["history", "migrate", "--to", node.to_str().unwrap()],
    ));
    assert_eq!(tree(&legacy), before);
    let new = ok(run(&node, &["history", "status"]));
    assert_eq!(new["authority"]["profile"], "node-history/v1");
    assert_eq!(new["subjects"], old["subjects"]);
}

#[test]
fn many_current_subjects_stay_within_the_bounded_view() {
    // Each lazy original repeats a tagged header in the one readable document.
    // Past a fixed budget, subjects keep the identical initial event in a stream.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    let mut record = String::from("meta: {purpose: many}\nknown:\n");
    for i in 0..700 {
        record.push_str(&format!("  p.s{i}: {{v: {i}}}\n"));
    }
    write(&source, &[("GROUNDING.yaml", &record)]);
    let copy = root.join("copy");
    I::Plan::prepare(
        Path::new("GROUNDING.yaml"),
        &source,
        options("import-many"),
        None,
    )
    .unwrap()
    .publish(&copy)
    .unwrap();
    let captured = Capture::read(&copy).unwrap();
    assert_eq!(captured.object_count(), 700);
    let streams = tree(&copy)
        .into_keys()
        .filter(|p| p.starts_with(".kpopper/history/"))
        .count();
    assert!(streams > 0 && streams < 700, "{streams}");
}

#[test]
fn a_live_copy_retains_the_pending_observation_without_folding_or_advancing_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join("repo");
    fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .current_dir(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "Fixture"]);
    git(&["config", "user.email", "fixture@example.test"]);
    // Ordinary capture needs the ordinary program as well as the core runtime.
    let resources = std::env::var_os("KPOP_CONSOLIDATION_RESOURCES")
        .or_else(|| std::env::var_os("KPOPPER_NATIVE_RESOURCES"))
        .map(std::path::PathBuf::from)
        .expect("set KPOP_CONSOLIDATION_RESOURCES to verified native resources");
    let kpop = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&repo)
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("KPOPPER_READ_MODE")
            .env("KPOPPER_NATIVE_RESOURCES", &resources)
            .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
            .env("KPOPPER_PRIVATE_HOME", root.join("private"))
            .args(args)
            .output()
            .unwrap()
    };
    // A project reading captured as pending before the ordinary record exists.
    let captured = ok(kpop(&[
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
    ]));
    assert_eq!(captured["state"], "captured");
    fs::write(repo.join("GROUNDING.yaml"), "known: {p.base: {v: 1}}\n").unwrap();
    git(&["add", "GROUNDING.yaml"]);
    git(&["-c", "commit.gpgsign=false", "commit", "-qm", "record"]);
    let pending = git(&["rev-parse", "refs/kpopper/pending_grounding"]);
    let before = tree(&repo);
    let copy = root.join("copy");
    let published = ok(kpop(&[
        "history",
        "migrate",
        "--read-mode",
        "live",
        "--to",
        copy.to_str().unwrap(),
    ]));
    assert_eq!(published["captured_pending_context"], true);
    assert_eq!(tree(&repo), before);
    assert_eq!(
        git(&["rev-parse", "refs/kpopper/pending_grounding"]),
        pending
    );
    let (mapping, _) = retained(&copy);
    let observation = map(&map(&mapping)["observation"]);
    let live = &tree(&copy)[text(&observation["path"])];
    let observed = kpop_native::reasoning_snapshot::Snapshot::from_json(live).unwrap();
    let data = observed.to_data().to_json().unwrap();
    assert!(
        !data["context"]["pending"]["bundles"]
            .as_object()
            .unwrap()
            .is_empty(),
        "{}",
        data["context"]["pending"]
    );
    // The copied record is the frozen source; the pending reading stays evidence.
    let body = Capture::read(&copy).unwrap();
    let V::Map(document) = body.document() else {
        panic!("record")
    };
    assert_eq!(
        map(&document["known"])["p.base"].to_json().unwrap(),
        serde_json::json!({"v": 1})
    );
}

fn status(copy: &Path) -> Value {
    ok(run(copy, &["history", "status"]))
}

#[test]
fn absolute_pointer_layouts_replay_through_the_recorded_inverse() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    let project = source.join("project");
    let member = project.join("member.yaml");
    let extra = source.join("shared/extra.yaml");
    let entry = format!("# entry\nrecord: {}\n", member.display());
    let member_raw = format!(
        "record: {}\nknown: {{p.first: {{v: 1}}}}\n",
        extra.display()
    );
    write(
        &source,
        &[
            ("project/GROUNDING.yaml", &entry),
            ("project/member.yaml", &member_raw),
            ("shared/extra.yaml", "known: {p.second: {v: 2}}\n"),
        ],
    );
    let before = tree(&source);
    let preview = ok(run(&project, &["history", "migrate"]));
    assert_eq!(preview["state"], "preview");
    assert_eq!(preview["inverse"]["representable"], false);
    assert_eq!(preview["inverse"]["reason"], "absolute_original_pointer");
    assert_eq!(
        preview["inverse"]["members"],
        serde_json::json!(["GROUNDING.yaml", "member.yaml"])
    );
    let copy = root.join("copy");
    ok(run(
        &project,
        &["history", "migrate", "--to", copy.to_str().unwrap()],
    ));
    assert_eq!(tree(&source), before);
    let original = status(&copy);
    assert_eq!(original["subjects"]["p.first"]["acceptance"], "accepted");
    assert_eq!(original["subjects"]["p.second"]["acceptance"], "accepted");
    // The archive keeps the exact bytes, absolute pointers included.
    let (_, archive) = retained(&copy);
    assert_eq!(archive.files()["GROUNDING.yaml"], entry.as_bytes());
    assert_eq!(archive.files()["member.yaml"], member_raw.as_bytes());
    // Replay never follows the original absolute locations: a different file
    // there now changes nothing, and neither does their absence.
    fs::rename(&source, root.join("moved")).unwrap();
    assert_eq!(status(&copy)["subjects"], original["subjects"]);
    write(
        &source,
        &[
            ("project/member.yaml", "known: {p.first: {v: 99}}\n"),
            ("shared/extra.yaml", "known: {p.other: {v: 7}}\n"),
        ],
    );
    assert_eq!(status(&copy)["subjects"], original["subjects"]);
    // An absolute layout cannot be restored elsewhere, as for history/v1 copies.
    let error = I::restore(&copy, &root.join("restored")).unwrap_err();
    assert_eq!(error.0, "nonrepresentable_inverse");
    assert!(!root.join("restored").exists());
}

#[test]
fn a_keyed_dependency_map_is_refused_before_any_write() {
    // History reads a dependency map as version pins. Without resolvable pins it
    // becomes a list of names, so no compact copy can keep this reading unchanged.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    let record = "known:\n  p.input: {v: 2}\njudgments:\n  d.stable:\n    rests_on: {p.input: unknown, p.gone: earlier reason}\n    verdict: stable\n";
    write(&source, &[("GROUNDING.yaml", record)]);
    let before = tree(&source);
    let entry = Path::new("GROUNDING.yaml");
    let copy = root.join("copy");
    assert_eq!(
        refused(run(
            &source,
            &["history", "migrate", "--to", copy.to_str().unwrap()]
        )),
        "unsupported_legacy_dependency_mapping"
    );
    assert!(!copy.exists());
    assert_eq!(tree(&source), before);
    assert_eq!(
        M::Plan::prepare(entry, &source, options("import-map"), None)
            .err()
            .unwrap()
            .0,
        "unsupported_legacy_dependency_mapping"
    );
    // The retired bootstrap accepted this shape, and its reading differs from the source.
    let boot = root.join("boot");
    kpop_native::history_node_bootstrap::Plan::prepare(&source.join("GROUNDING.yaml"))
        .unwrap()
        .publish(&boot)
        .unwrap();
    let captured = Capture::read(&boot).unwrap();
    let judgments = map(&map(captured.document())["judgments"]);
    assert_eq!(
        map(&judgments["d.stable"])["rests_on"].to_json().unwrap(),
        serde_json::json!(["p.gone", "p.input"])
    );
}

/// A copy imported from a custom entry: its rebuilt directory has no GROUNDING.yaml.
fn topology_copy(root: &Path) -> std::path::PathBuf {
    let source = root.join("topology");
    write(
        &source,
        &[
            (
                "project/notes.yml",
                "also: [../shared/extra.yaml]\nknown: {p.first: {v: 1}}\n",
            ),
            ("shared/extra.yaml", "known: {p.second: {v: 2}}\n"),
        ],
    );
    let copy = root.join("topology-copy");
    ok(run(
        &source.join("project"),
        &[
            "history",
            "migrate",
            "--record",
            "notes.yml",
            "--to",
            copy.to_str().unwrap(),
        ],
    ));
    copy
}

#[test]
fn replay_ignores_a_foreign_project_above_the_temporary_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let copy = topology_copy(&root);
    let source = root.join("plain");
    write(&source, &[("GROUNDING.yaml", "known: {p.x: {v: 1}}\n")]);
    let boot = root.join("boot");
    kpop_native::history_node_bootstrap::Plan::prepare(&source.join("GROUNDING.yaml"))
        .unwrap()
        .publish(&boot)
        .unwrap();
    // Unrelated, malformed project configuration above TMPDIR, with and without Git.
    let foreign = root.join("foreign");
    write(
        &foreign,
        &[(".kpopper/project.json", "{not json"), ("tmp/.keep", "")],
    );
    let repository = root.join("repository");
    write(&repository, &[("tmp/.keep", "")]);
    let init = Command::new("git")
        .current_dir(&repository)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(init.status.success());
    write(
        &repository,
        &[(".git/kpopper/project/project.json", "{not json")],
    );
    for parent in [&foreign, &repository] {
        for record in [&copy, &boot] {
            let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
                .current_dir(record)
                .env("TMPDIR", parent.join("tmp"))
                .args(["history", "status"])
                .output()
                .unwrap();
            ok(output);
        }
    }
}

#[test]
fn restore_rebuilds_exact_originals_and_refuses_later_history() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let copy = topology_copy(&root);
    let restored = root.join("restored");
    let result = I::restore(&copy, &restored).unwrap().to_json().unwrap();
    assert_eq!(result["state"], "restored_copy");
    assert_eq!(tree(&restored), tree(&root.join("topology")));
    assert_eq!(
        I::restore(&copy, &restored).unwrap_err().0,
        "migration_destination_exists"
    );
    // A hypothesis import belongs to the migration; exact files include it.
    let source = root.join("source");
    write(
        &source,
        &[
            ("GROUNDING.yaml", RECORD),
            (".kpopper/replaced.yaml", REPLACED),
            (".kpopper/hypotheses/alternative.yaml", HYPOTHESIS),
            ("docs/source.txt", "exact evidence\n"),
        ],
    );
    let full = root.join("full");
    ok(run(
        &source,
        &["history", "migrate", "--to", full.to_str().unwrap()],
    ));
    I::restore(&full, &root.join("full-restored")).unwrap();
    assert_eq!(tree(&root.join("full-restored")), tree(&source));
    // Later history in the copy is not representable in the original files.
    let head = status(&full)["subjects"]["p.x"]["heads"][0]
        .as_str()
        .unwrap()
        .to_owned();
    ok(run(
        &full,
        &[
            "history",
            "retire",
            "--subject",
            "p.x",
            "--of",
            &head,
            "--because",
            "later",
        ],
    ));
    assert_eq!(
        I::restore(&full, &root.join("late")).unwrap_err().0,
        "newer_history_not_representable"
    );
    assert!(!root.join("late").exists());
}

#[cfg(unix)]
mod prior_generation {
    use super::*;
    use kpop_native::{
        Result,
        history_activation::{self as A, Deployment, DeploymentGuard, Selection},
    };
    use serde_json::json;
    use std::cell::Cell;

    struct Exclusion(Cell<bool>);
    struct Held<'a>(&'a Exclusion);
    impl Drop for Held<'_> {
        fn drop(&mut self) {
            self.0.0.set(false);
        }
    }
    impl DeploymentGuard for Held<'_> {
        fn verify(&self) -> Result<()> {
            assert!(self.0.0.get());
            Ok(())
        }
    }
    impl Deployment for Exclusion {
        fn exclude<'a>(&'a self, _: &Value, _: &Value) -> Result<Box<dyn DeploymentGuard + 'a>> {
            assert!(!self.0.replace(true));
            Ok(Box::new(Held(self)))
        }
    }
    /// The managed-runtime declaration used by the activation lifecycle tests.
    fn selection(root: &Path) -> Selection {
        let corpus: Value =
            serde_json::from_str(include_str!("fixtures/history-runtime.json")).unwrap();
        let mut declaration = corpus["cases"][0]["declaration"].clone();
        let shell = Path::new("/bin/sh").canonicalize().unwrap();
        fs::write(root.join("cli.py"), b"fixture").unwrap();
        declaration["nonce"] = json!("NONCE_PLACEHOLDER");
        declaration["resolved"]["executable"] = json!(shell);
        declaration["resolved"]["cli"] = json!(root.join("cli.py"));
        declaration["resolved"]["package_root"] = json!(root);
        let response = root.join("response.json");
        fs::write(&response, serde_json::to_vec(&declaration).unwrap()).unwrap();
        Selection {
            inventory: json!([{"id":"managed","argv":[shell,"-c","sed \"s/NONCE_PLACEHOLDER/$5/g\" \"$1\"","probe",response],"executable":shell,"package_root":root}]),
            expected_digests: json!({"managed":{"sources":declaration["sources"]["digest"],"native":declaration["native"]["digest"],"schemas":declaration["schemas"]["digest"]}}),
        }
    }

    #[test]
    fn a_deactivated_generation_stays_archived_evidence_under_the_same_record() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        write(
            &source,
            &[
                ("GROUNDING.yaml", RECORD),
                (".kpopper/replaced.yaml", REPLACED),
                ("docs/source.txt", "x\n"),
            ],
        );
        let entry = source.join("GROUNDING.yaml");
        let runtime = root.join("runtime");
        fs::create_dir(&runtime).unwrap();
        let guard = Exclusion(Cell::new(false));
        let activation = A::prepare_activation(
            &entry,
            &A::Options {
                operation: "activate-test".into(),
                recorded_at: "2026-09-19T12:00:00+00:00".into(),
                record_id: Some("record-test".into()),
            },
            &selection(&runtime),
            &guard,
            None,
        )
        .unwrap();
        A::publish(&entry, &activation, &guard, None).unwrap();
        let data = activation.to_data();
        let deployment = map(&map(&map(&data)["baseline"])["deployment"]);
        let deployed = Selection {
            inventory: deployment["inventory"].to_json().unwrap(),
            expected_digests: deployment["expected_digests"].to_json().unwrap(),
        };
        let inverse = A::prepare_deactivation(
            &entry,
            &activation,
            "deactivate-test",
            &deployed,
            &guard,
            None,
        )
        .unwrap();
        A::publish(&entry, &inverse, &guard, None).unwrap();
        let before = tree(&source);
        assert!(
            before
                .keys()
                .any(|p| p.starts_with(".kpopper/history-commits/"))
        );
        let preview = ok(run(&source, &["history", "migrate"]));
        assert_eq!(preview["prior_history"]["record_id"], "record-test");
        assert_eq!(preview["prior_history"]["generation"], 2);
        let copy = root.join("copy");
        ok(run(
            &source,
            &["history", "migrate", "--to", copy.to_str().unwrap()],
        ));
        assert_eq!(tree(&source), before);
        let copied = status(&copy);
        assert_eq!(copied["authority"]["record_id"], "record-test");
        assert_eq!(copied["authority"]["generation"], 3);
        assert_eq!(copied["authority"]["profile"], "node-history/v1");
        assert_eq!(copied["subjects"]["d.gone"]["acceptance"], "retired");
        // Every earlier-generation file is archived exactly and none is active.
        let (_, archive) = retained(&copy);
        let copied_files = tree(&copy);
        let prior = before
            .keys()
            .filter(|p| {
                p.starts_with(".kpopper/history") || p.starts_with(".kpopper-history-migration/")
            })
            .count();
        assert!(prior >= 4, "{:?}", before.keys());
        for (name, raw) in &before {
            // Private local state and locks are not record members for any import.
            if name.starts_with(".kpopper/.history-local/") || name.ends_with(".lock") {
                assert!(!archive.files().contains_key(name), "{name}");
                continue;
            }
            assert_eq!(archive.files().get(name), Some(raw), "{name}");
            if name.starts_with(".kpopper-history-migration/")
                || name.starts_with(".kpopper/history-commits/")
                || name.ends_with(".yaml") && name.starts_with(".kpopper/history/")
            {
                assert!(copied_files.get(name) != Some(raw), "{name} became active");
            }
        }
        // The import semantics are those of the established generation-3 import.
        let mut same = options("import-generation");
        same.record_id = None;
        let legacy =
            M::Plan::prepare(Path::new("GROUNDING.yaml"), &source, same.clone(), None).unwrap();
        let exact = root.join("exact");
        I::Plan::prepare(Path::new("GROUNDING.yaml"), &source, same, None)
            .unwrap()
            .publish(&exact)
            .unwrap();
        let captured = Capture::read(&exact).unwrap();
        let (mapping, _) = retained(&exact);
        let imported = map(&mapping)["objects"].to_json().unwrap();
        let legacy_objects = legacy
            .files()
            .iter()
            .filter(|(p, _)| p.starts_with(".kpopper/history/"))
            .map(|(_, raw)| {
                let object = Y::decode_document(raw).unwrap();
                (text(&map(&object)["id"]).to_owned(), object)
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(imported.as_array().unwrap().len(), 9);
        for row in imported.as_array().unwrap() {
            let id = row["semantic_id"].as_str().unwrap();
            let expected = &legacy_objects[id];
            assert_eq!(
                &captured
                    .object(text(&map(expected)["subject"]), id)
                    .unwrap(),
                expected
            );
        }
    }
}
