use base64::{Engine as _, engine::general_purpose::STANDARD};
use kpop_native::{
    history_authoring::{self, Options},
    history_node_archive::Archive,
    history_node_capture::Capture,
    history_node_migration::Plan,
    history_node_publication as P, history_node_writer as W,
    history_paths::Scheme,
    history_store::Store,
    history_yaml as Y,
    reasoning_runtime::{OperationalBounds, Runtime},
    value::TypedValue as V,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/history-authoring.json")).unwrap()
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
fn files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                let portable = relative
                    .components()
                    .map(|part| match part {
                        Component::Normal(name) => name.to_str().expect("fixture path is UTF-8"),
                        _ => panic!("unexpected inventory path: {}", relative.display()),
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(portable, fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
fn write_case(root: &Path, case: &Value) {
    for (name, raw) in case["files"].as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
}
fn write_encoded(root: &Path, files: &serde_json::Map<String, Value>) -> BTreeMap<String, Vec<u8>> {
    let mut exact = BTreeMap::new();
    for (name, encoded) in files {
        let bytes = STANDARD.decode(encoded.as_str().unwrap()).unwrap();
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &bytes).unwrap();
        exact.insert(name.clone(), bytes);
    }
    exact
}
fn map(value: &V) -> &BTreeMap<String, V> {
    let V::Map(map) = value else {
        panic!("expected map")
    };
    map
}
fn text(value: &V) -> &str {
    let V::Text(text) = value else {
        panic!("expected text")
    };
    text
}
fn list(value: &V) -> &[V] {
    let V::List(list) = value else {
        panic!("expected list")
    };
    list
}
fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    let support = root.parent().unwrap().join("node-migration-test-support");
    let reasoning = support.join("reasoning");
    fs::create_dir_all(&reasoning).unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        reasoning.join(format!("{target}.zip")),
    )
    .unwrap();
    let ordinary = support.join("ordinary").join(&target);
    fs::create_dir_all(&ordinary).unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .unwrap(),
            )
            .join(".cache/kpopper/lean")
            .join(&target)
            .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    for filename in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(program.join(filename), ordinary.join(filename)).unwrap();
    }
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("KPOPPER_NATIVE_RESOURCES", support)
        .env(
            "KPOPPER_NATIVE_CACHE",
            root.parent().unwrap().join("node-migration-test-cache"),
        )
        .args(args)
        .output()
        .unwrap()
}
fn migrated_case(
    index: usize,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    BTreeMap<String, Vec<u8>>,
) {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    write_case(source.path(), &fixture()["cases"][index]);
    let before = files(source.path());
    let plan = Plan::prepare(&source.path().join("GROUNDING.yaml")).unwrap();
    plan.publish(&destination.path().join("converted")).unwrap();
    assert_eq!(files(source.path()), before);
    (source, destination, before)
}

#[test]
fn converts_committed_set_and_add_with_original_semantic_ids() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let cases = fixture();
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    write_case(source.path(), &cases["cases"][0]);
    let store = Store::new(&source.path().join("GROUNDING.yaml")).unwrap();
    for (index, operation) in [(0, "legacy-set"), (7, "legacy-add")] {
        let action = V::from_tagged(&cases["cases"][index]["action"]).unwrap();
        let options = Options {
            operation: operation.into(),
            recorded_at: "2026-09-19T09:30:00+00:00".into(),
            recording_day: "2026-09-19".into(),
            by: V::Text("writer".into()),
            strict: true,
            paths: Scheme::Hashed,
            receipt_version: None,
        };
        let capture = store.capture().unwrap();
        let mutation =
            history_authoring::prepare(&store, &capture, &action, &options, Some(&runtime))
                .unwrap();
        history_authoring::commit(&store, &mutation, Some(&runtime), &mut |_| Ok(())).unwrap();
    }
    let before = files(source.path());
    let plan = Plan::prepare(&store.entry).unwrap();
    let root = destination.path().join("converted");
    plan.publish(&root).unwrap();
    let copied = Capture::read(&root).unwrap();
    assert_eq!(
        copied.object_count(),
        store.capture().unwrap().objects.len()
    );
    assert_eq!(files(source.path()), before);
    let detached = P::export(&root).unwrap().reconstruct().unwrap();
    assert_eq!(
        Capture::read(detached.path()).unwrap().object_count(),
        copied.object_count()
    );
}

#[test]
fn copies_real_authoring_history_with_original_bytes_and_source_free_replay() {
    let (_source, destination, before) = migrated_case(0);
    let root = destination.path().join("converted");
    let capture = Capture::read(&root).unwrap();
    assert!(capture.object_count() > 0);
    assert!(
        !root.join(".kpopper/history").exists(),
        "unchanged singleton should be lazy in the view"
    );
    let export = P::export(&root).unwrap();
    let detached = export.reconstruct().unwrap();
    let reread = Capture::read(detached.path()).unwrap();
    assert_eq!(reread.object_count(), capture.object_count());
    let archive = fs::read_dir(root.join("evidence/legacy"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let archive = Archive::decode(&fs::read(archive).unwrap()).unwrap();
    for (path, bytes) in before {
        assert_eq!(archive.files().get(&path), Some(&bytes), "{path}");
    }
}

#[test]
fn destination_exists_and_source_change_refuse_publication() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    write_case(source.path(), &fixture()["cases"][0]);
    let plan = Plan::prepare(&source.path().join("GROUNDING.yaml")).unwrap();
    assert!(plan.publish(destination.path()).is_err());
    let target = destination.path().join("copy");
    fs::write(source.path().join("GROUNDING.yaml"), "changed").unwrap();
    assert!(plan.publish(&target).is_err());
    assert!(!target.exists());
}

#[test]
fn tampered_archive_refuses_source_free_capture() {
    let (_source, destination, _) = migrated_case(0);
    let root = destination.path().join("converted");
    let archive = fs::read_dir(root.join("evidence/legacy"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut bytes = fs::read(&archive).unwrap();
    bytes[0] ^= 1;
    fs::write(archive, bytes).unwrap();
    assert!(Capture::read(&root).is_err());
}

#[test]
fn cancelled_generation_stays_archived_while_active_history_is_replayed() {
    let cases: Value = serde_json::from_str(include_str!("fixtures/history-branch.json")).unwrap();
    let case = &cases[2];
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let exact = write_encoded(source.path(), case["files"].as_object().unwrap());
    let store = Store::new(&source.path().join("GROUNDING.yaml")).unwrap();
    let original = store.capture().unwrap();
    let inactive = &original.inactive_generations["3"];
    let (archived_id, archived_object) = inactive.objects.iter().next().unwrap();
    let archived_subject = &map(archived_object)["subject"];
    let V::Text(archived_subject) = archived_subject else {
        panic!()
    };
    assert!(!original.objects.contains_key(archived_id));
    let plan = Plan::prepare(&store.entry).unwrap();
    let archive_raw = plan
        .files()
        .iter()
        .find(|(path, _)| path.starts_with("evidence/legacy/"))
        .unwrap()
        .1;
    assert_eq!(Archive::decode(archive_raw).unwrap().files(), &exact);
    let root = destination.path().join("converted");
    plan.publish(&root).unwrap();
    assert_eq!(files(source.path()), exact);
    let copied = Capture::read(&root).unwrap();
    assert_eq!(
        copied.object(archived_subject, archived_id).unwrap(),
        *archived_object
    );
    for (id, object) in &original.objects {
        let V::Text(subject) = &map(object)["subject"] else {
            panic!()
        };
        assert_eq!(copied.object(subject, id).unwrap(), *object);
    }
    let detached = P::export(&root).unwrap().reconstruct().unwrap();
    let replayed = Capture::read(detached.path()).unwrap();
    assert_eq!(
        replayed.object(archived_subject, archived_id).unwrap(),
        *archived_object
    );
}

#[test]
fn adopted_legacy_inventory_reuses_original_object_and_receipts() {
    let cases: Value =
        serde_json::from_str(include_str!("fixtures/history-branch-adoption.json")).unwrap();
    let case = &cases[3];
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let normalized = case["target"].as_object().unwrap();
    let mut mapped = serde_json::Map::new();
    for (path, encoded) in normalized {
        let actual = match path.as_str() {
            "entry.yaml" => "GROUNDING.yaml".to_owned(),
            "authority.yaml" => ".kpopper/history.yaml".to_owned(),
            _ if path.starts_with("commits/") => format!(".kpopper/history-commits/{}", &path[8..]),
            _ if path.starts_with("objects/") => format!(".kpopper/history/{}", &path[8..]),
            _ => panic!("unexpected bundle path {path}"),
        };
        mapped.insert(actual, encoded.clone());
    }
    let exact = write_encoded(source.path(), &mapped);
    let store = Store::new(&source.path().join("GROUNDING.yaml")).unwrap();
    let original = store.capture().unwrap();
    assert_eq!(original.commits.len(), 2);
    assert_eq!(original.objects.len(), 2);
    let plan = Plan::prepare(&store.entry).unwrap();
    let root = destination.path().join("converted");
    plan.publish(&root).unwrap();
    assert_eq!(files(source.path()), exact);
    let copied = Capture::read(&root).unwrap();
    assert_eq!(copied.object_count(), original.objects.len());
    let snapshot = P::capture_snapshot(&root).unwrap();
    assert_eq!(
        snapshot.transactions["cycle-0"].parents,
        vec!["import".to_owned()]
    );
    let imported = snapshot
        .versions
        .values()
        .flat_map(|v| v.values())
        .find(|v| v.operation() == "import")
        .unwrap();
    let adopted = snapshot
        .versions
        .values()
        .flat_map(|v| v.values())
        .find(|v| v.operation() == "cycle-0")
        .unwrap();
    assert!(adopted.parents().contains(&imported.id().to_owned()));
    for (operation, raw) in &original.commits {
        let commit = Y::decode_document(raw).unwrap();
        assert_eq!(
            W::receipt(&snapshot, operation).unwrap(),
            map(&commit)["receipt"]
        );
    }
    for (id, object) in &original.objects {
        let V::Text(subject) = &map(object)["subject"] else {
            panic!()
        };
        assert_eq!(copied.object(subject, id).unwrap(), *object);
    }
}

#[test]
fn public_node_history_copy_preserves_source_and_opens_as_native_history() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    write_case(source.path(), &fixture()["cases"][0]);
    let before = files(source.path());
    let target = destination.path().join("copy");
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(source.path())
        .args([
            "history",
            "migrate",
            "--node-history",
            "--to",
            target.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(files(source.path()), before);
    assert_eq!(Capture::read(&target).unwrap().object_count(), 1);
    assert!(P::export(&target).is_ok());
}

#[test]
fn history_import_replaced_sidecar_is_verified_then_redundant_member_is_dropped() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ordinary");
    let v1 = temp.path().join("history-v1");
    let node = temp.path().join("node-copy");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("GROUNDING.yaml"),
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.runs: {v: 0, of: 2026-09-01}\njudgments:\n  d.done:\n    verdict: not demonstrated\n    because: no brief\n    rests_on: [p.runs]\n    seen: {p.runs: 0}\n    wrong_if: p.runs > 0\n",
    )
    .unwrap();
    let changed_reading = cli(&source, &["set", "p.runs", "1", "--as-of", "2026-09-02"]);
    assert!(
        changed_reading.status.success(),
        "{}{}",
        String::from_utf8_lossy(&changed_reading.stdout),
        String::from_utf8_lossy(&changed_reading.stderr)
    );
    let changed_judgment = cli(
        &source,
        &[
            "add",
            "d.done",
            "verdict=done",
            "because=one brief",
            "rests_on=[p.runs]",
            "wrong_if=p.runs < 1",
        ],
    );
    assert!(
        changed_judgment.status.success(),
        "{}{}",
        String::from_utf8_lossy(&changed_judgment.stdout),
        String::from_utf8_lossy(&changed_judgment.stderr)
    );
    let generated_replaced = fs::read(source.join(".kpopper/replaced.yaml")).unwrap();
    let generated_document = Y::decode_source_value(&generated_replaced).unwrap().typed();
    let generated_versions = list(&map(&generated_document)["d.done"]);
    let generated_entry = map(&generated_versions[0]);
    assert_eq!(text(&generated_entry["because"]), "no brief");
    let generated_day = generated_entry["day"].to_json().unwrap();
    let generated_ended = generated_entry["ended"].to_json().unwrap();
    assert!(
        !generated_entry.contains_key("dropped"),
        "the CLI fixture already has dropped; update this test fixture deliberately"
    );
    // Retain the real authoring output and enrich that same archived entry with
    // a dropped annotation, so the chain covers CLI-authored and archive data.
    let mut enriched_replaced = generated_replaced;
    if !enriched_replaced.ends_with(b"\n") {
        enriched_replaced.push(b'\n');
    }
    enriched_replaced.extend_from_slice(
        b"  dropped: {p.old: retired earlier}\n",
    );
    fs::write(source.join(".kpopper/replaced.yaml"), &enriched_replaced).unwrap();
    let replaced = fs::read(source.join(".kpopper/replaced.yaml")).unwrap();
    let migrated = cli(
        &source,
        &["history", "migrate", "--to", v1.to_str().unwrap()],
    );
    assert!(
        migrated.status.success(),
        "{}{}",
        String::from_utf8_lossy(&migrated.stdout),
        String::from_utf8_lossy(&migrated.stderr)
    );
    let v1_before = files(&v1);
    let entry_bytes = v1_before.get("GROUNDING.yaml").unwrap().clone();
    let sidecar_bytes = v1_before.get(".kpopper/replaced.yaml").unwrap().clone();
    assert_eq!(
        sidecar_bytes, replaced,
        "history/v1 changed the replaced source bytes"
    );
    assert_eq!(
        sidecar_bytes, replaced,
        "history/v1 changed the replaced source bytes"
    );
    let v1_document = Y::decode_document(&entry_bytes).unwrap();
    let meta = map(&v1_document)["meta"].clone();
    let import = map(&meta)["history_import"].clone();
    let original_members = list(&map(&import)["members"]);
    let replaced_member = original_members
        .iter()
        .find(|member| {
            map(member)
                .get("role")
                .is_some_and(|role| text(role) == "replaced")
        })
        .expect("the v1 import carries replaced.yaml");
    let replaced_member = map(replaced_member);
    let replaced_hash = text(&replaced_member["sha256"]);
    let retained_prefix = format!(
        "{}/",
        kpop_native::history_transaction::Layout::for_entry("GROUNDING.yaml")
            .unwrap()
            .retained
    );
    let retained_member = original_members
        .iter()
        .map(map)
        .find(|fields| {
            fields
                .get("role")
                .is_some_and(|role| text(role) == "retained_original")
                && text(&fields["path"]).starts_with(&retained_prefix)
                && text(&fields["sha256"]) == replaced_hash
        })
        .expect("the identical replaced bytes already have a retained original");
    let retained_path = text(&retained_member["path"]);
    assert_eq!(v1_before.get(retained_path).unwrap(), &sidecar_bytes);

    let retained_file = v1.join(retained_path);
    fs::remove_file(&retained_file).unwrap();
    let missing_retained = Plan::prepare(&v1.join("GROUNDING.yaml")).err().unwrap();
    assert_eq!(
        missing_retained.0,
        "node_migration_replaced_member_unretained"
    );
    fs::write(&retained_file, &sidecar_bytes).unwrap();
    fs::write(&retained_file, b"mismatched retained bytes").unwrap();
    let mismatched_retained = Plan::prepare(&v1.join("GROUNDING.yaml")).err().unwrap();
    assert_eq!(
        mismatched_retained.0,
        "node_migration_replaced_member_unretained"
    );
    fs::write(&retained_file, &sidecar_bytes).unwrap();

    let plan = Plan::prepare(&v1.join("GROUNDING.yaml")).unwrap();
    assert_eq!(
        files(&v1),
        v1_before,
        "preparing the copy mutated its source"
    );
    let copied_document = Y::decode_document(plan.files().get("GROUNDING.yaml").unwrap()).unwrap();
    let copied_meta = map(&copied_document)["meta"].clone();
    let copied_import = map(&copied_meta)["history_import"].clone();
    let copied_members = list(&map(&copied_import)["members"]);
    assert!(copied_members.iter().all(|member| {
        map(member)
            .get("role")
            .is_none_or(|role| text(role) != "replaced")
    }));
    for member in copied_members {
        let member = map(member);
        let path = text(&member["path"]);
        let expected = text(&member["sha256"]);
        let raw = plan
            .files()
            .get(path)
            .unwrap_or_else(|| panic!("missing evidence {path}"));
        assert_eq!(kpop_native::identity::sha256(raw), expected);
    }
    assert_eq!(
        plan.files().get(retained_path).unwrap(),
        &sidecar_bytes,
        "the retained original stays byte-identical"
    );
    let archive = plan
        .files()
        .iter()
        .find(|(path, _)| path.starts_with("evidence/legacy/") && path.ends_with(".zip"))
        .unwrap()
        .1;
    let archived = Archive::decode(archive).unwrap();
    assert_eq!(
        archived.files().get(".kpopper/replaced.yaml"),
        Some(&sidecar_bytes)
    );
    assert_eq!(archived.files().get("GROUNDING.yaml"), Some(&entry_bytes));
    plan.publish(&node).unwrap();
    assert_eq!(files(&v1), v1_before);
    let node_before_reads = files(&node);
    assert!(Capture::read(&node).is_ok());
    assert_eq!(
        files(&node),
        node_before_reads,
        "node reads changed the copied tree"
    );
    let open_before_review = cli(&node, &["open"]);
    assert!(
        open_before_review.status.success(),
        "{}",
        String::from_utf8_lossy(&open_before_review.stderr)
    );
    assert!(
        String::from_utf8_lossy(&open_before_review.stdout).contains("review_provenance_missing"),
        "{}",
        String::from_utf8_lossy(&open_before_review.stdout)
    );

    for args in [
        vec!["open".into()],
        vec!["check".into()],
        vec!["pull".into(), "d.done".into(), "--history".into()],
        vec![
            "--json".into(),
            "pull".into(),
            "d.done".into(),
            "--history".into(),
        ],
        vec!["history".into(), "status".into()],
    ] {
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = cli(&node, &args);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let chained_history = cli(&node, &["pull", "d.done", "--history"]);
    assert!(chained_history.status.success());
    let chained_history_text = String::from_utf8_lossy(&chained_history.stdout);
    assert!(
        chained_history_text.contains("LEGACY ARCHIVE EVIDENCE"),
        "chained copy omitted separately labeled archive evidence: {chained_history_text}"
    );
    assert!(
        chained_history_text.contains("no brief"),
        "chained copy omitted the old reason: {chained_history_text}"
    );
    let chained_json = cli(
        &node,
        &["--json", "pull", "d.done", "--history"],
    );
    assert!(chained_json.status.success());
    let wrapper: serde_json::Value = serde_json::from_slice(&chained_json.stdout).unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(wrapper["output"].as_str().unwrap()).unwrap();
    let archive_rows = payload["historical_section"]["legacy_archive_evidence"]
        .as_array()
        .unwrap();
    assert_eq!(archive_rows.len(), 1, "{archive_rows:?}");
    let archived = &archive_rows[0];
    assert_eq!(archived["source"], "verified_legacy_archive");
    assert_eq!(archived["archive_member"], ".kpopper/replaced.yaml");
    assert_eq!(archived["entry"]["because"], "no brief");
    assert_eq!(archived["entry"]["day"], generated_day);
    assert_eq!(archived["entry"]["ended"], generated_ended);
    assert_eq!(archived["entry"]["dropped"]["p.old"], "retired earlier");
    assert!(archived.get("id").is_none(), "archive evidence gained a semantic id: {archived}");
    for args in [
        vec!["set", "p.runs", "3", "--why", "another observed run"],
        vec!["review", "d.done", "--by", "reviewer"],
    ] {
        let output = cli(&node, &args);
        assert!(
            output.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let open_after_review = cli(&node, &["open"]);
    assert!(
        open_after_review.status.success(),
        "{}",
        String::from_utf8_lossy(&open_after_review.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&open_after_review.stdout).contains("review_provenance_missing"),
        "{}",
        String::from_utf8_lossy(&open_after_review.stdout)
    );

    let sibling = P::export(&node).unwrap().reconstruct().unwrap();
    let branch_action = V::from_json(&serde_json::json!({
        "kind": "add",
        "id": "p.branch",
        "body": {"v": 7}
    }))
    .unwrap();
    let branch_options = Options {
        operation: "copy-branch".into(),
        recorded_at: "2026-09-03T10:00:00+00:00".into(),
        recording_day: "2026-09-03".into(),
        by: V::Text("branch-writer".into()),
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    };
    let branch_write = W::prepare(sibling.path(), &branch_action, &branch_options, None).unwrap();
    W::publish(sibling.path(), &branch_write, None, |_| Ok(())).unwrap();
    let branch_bundle = P::export(sibling.path()).unwrap();
    let branch_merge =
        kpop_native::history_node_branch::prepare(&node, &[branch_bundle], "copy-branch-merge")
            .unwrap();
    W::publish(&node, &branch_merge, None, |_| Ok(())).unwrap();
    assert!(Capture::read(&node).is_ok());
}

#[test]
fn unbound_replaced_sidecar_in_a_legacy_copy_stays_raw_and_is_not_displayed_as_verified() {
    let temp = tempfile::tempdir().unwrap();
    let ordinary = temp.path().join("ordinary");
    let v1 = temp.path().join("history-v1");
    let node = temp.path().join("node-copy");
    fs::create_dir(&ordinary).unwrap();
    fs::write(
        ordinary.join("GROUNDING.yaml"),
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown: {p.runs: {v: 0, of: 2026-09-01}}\njudgments:\n  d.done: {verdict: not demonstrated, rests_on: [p.runs], seen: {p.runs: 0}, wrong_if: 'p.runs > 0'}\n",
    )
    .unwrap();
    let migrated = cli(
        &ordinary,
        &["history", "migrate", "--to", v1.to_str().unwrap()],
    );
    assert!(
        migrated.status.success(),
        "{}{}",
        String::from_utf8_lossy(&migrated.stdout),
        String::from_utf8_lossy(&migrated.stderr)
    );

    // The import never witnessed this file. A later malformed sidecar is preserved
    // inside the immutable archive, but cannot become verified display evidence.
    let unbound = b"d.done: [\n";
    fs::write(v1.join(".kpopper/replaced.yaml"), unbound).unwrap();
    let plan = Plan::prepare(&v1.join("GROUNDING.yaml")).unwrap();
    let archive = plan
        .files()
        .iter()
        .find(|(path, _)| path.starts_with("evidence/legacy/") && path.ends_with(".zip"))
        .unwrap()
        .1;
    let archived = Archive::decode(archive).unwrap();
    assert_eq!(archived.files().get(".kpopper/replaced.yaml"), Some(&unbound.to_vec()));
    plan.publish(&node).unwrap();

    let pulled = cli(
        &node,
        &["--json", "--frozen", "pull", "d.done", "--history"],
    );
    assert!(
        pulled.status.success(),
        "{}{}",
        String::from_utf8_lossy(&pulled.stdout),
        String::from_utf8_lossy(&pulled.stderr)
    );
    let wrapper: serde_json::Value = serde_json::from_slice(&pulled.stdout).unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(wrapper["output"].as_str().unwrap()).unwrap();
    assert_eq!(
        payload["historical_section"]["legacy_archive_evidence"],
        serde_json::json!([])
    );
}
