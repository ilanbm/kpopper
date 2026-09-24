use base64::{engine::general_purpose::STANDARD, Engine as _};
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
use std::{collections::BTreeMap, fs, path::Path, process::Command};

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
                out.insert(
                    path.strip_prefix(root).unwrap().to_str().unwrap().into(),
                    fs::read(path).unwrap(),
                );
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
