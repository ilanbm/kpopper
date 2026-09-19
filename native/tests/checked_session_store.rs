use kpop_native::{
    checked_session_store::CheckedSessionStore, reasoning_context::CapturedAssessment,
    reasoning_snapshot::Snapshot, tokenizer::Encoding, value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::{
    fs,
    sync::{Arc, Barrier},
    time::Duration,
};

#[cfg(unix)]
use std::{
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    process::Command,
};

fn context() -> CapturedAssessment {
    let corpus: J = serde_json::from_str(include_str!("fixtures/reasoning-context.json")).unwrap();
    CapturedAssessment::from_data(&corpus["contexts"][0]["serialized"]).unwrap()
}
fn snapshot(c: &CapturedAssessment) -> Snapshot {
    c.snapshot().clone()
}

#[test]
fn reopens_without_evaluator_and_rejects_changed_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("GROUNDING.yaml");
    fs::write(&source, b"record").unwrap();
    let c = context();
    let s = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &s).unwrap();
    let reopened = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    assert_eq!(reopened.load(&revision, &s).unwrap().revision(), revision);
    let changed = Snapshot::from_data(
        &V::from_json(&json!({"meta":{"scope":"changed"}})).unwrap(),
        Default::default(),
    )
    .unwrap();
    assert!(reopened.load(&revision, &changed).is_err());
}

#[test]
fn identity_and_collision_are_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let c = context();
    let s = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &s).unwrap();
    let foreign = CheckedSessionStore::open(
        dir.path().join("state"),
        "other",
        &source,
        None,
        Encoding::O200kBase,
    );
    assert!(foreign.is_err());
    let path = store.context_path(&revision).unwrap();
    let mut payload: J = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    payload["project"] = json!("forged");
    fs::write(&path, serde_json::to_vec(&payload).unwrap()).unwrap();
    assert!(store.load(&revision, &s).is_err());
}

#[test]
fn concurrent_identical_create_is_allowed_and_complete() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let c = context();
    let s = snapshot(&c);
    let store = std::sync::Arc::new(
        CheckedSessionStore::open(
            dir.path().join("state"),
            "fixture",
            &source,
            None,
            Encoding::O200kBase,
        )
        .unwrap(),
    );
    let mut workers = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        let c = c.clone();
        let s = s.clone();
        workers.push(std::thread::spawn(move || store.save(&c, &s).unwrap()));
    }
    let revisions: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(revisions.iter().all(|r| r == &revisions[0]));
    assert_eq!(
        store.load(&revisions[0], &s).unwrap().revision(),
        revisions[0]
    );
}

#[test]
fn identity_schema_binds_resolved_input_and_profile_but_not_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let actual = dir.path().join("actual");
    fs::create_dir(&actual).unwrap();
    #[cfg(unix)]
    symlink(&actual, dir.path().join("linked")).unwrap();
    #[cfg(unix)]
    let source = dir.path().join("linked/../linked/missing.yaml");
    #[cfg(not(unix))]
    let source = actual.join("missing.yaml");
    let profile = V::from_json(&json!({"groups":{"priority":["p.input"]}})).unwrap();
    let c = context();
    let first = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        Some(profile.clone()),
        Encoding::O200kBase,
    )
    .unwrap();
    let second = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        actual.join("missing.yaml"),
        Some(profile),
        Encoding::Cl100kBase,
    )
    .unwrap();
    assert_eq!(first.revision(&c).unwrap(), second.revision(&c).unwrap());
    let revision = first.save(&c, &snapshot(&c)).unwrap();
    let project: J =
        serde_json::from_slice(&fs::read(dir.path().join("state/project.json")).unwrap()).unwrap();
    let resolved_input = actual.canonicalize().unwrap().join("missing.yaml");
    assert_eq!(
        project,
        json!({"project":"fixture", "input_path":resolved_input})
    );
    let payload: J =
        serde_json::from_slice(&fs::read(first.context_path(&revision).unwrap()).unwrap()).unwrap();
    assert_eq!(payload.as_object().unwrap().len(), 4);
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["project_identity"]["version"], 1);
    assert_eq!(payload["project_identity"]["project"], "fixture");
    assert_eq!(
        payload["project_identity"]["input_path"],
        resolved_input.to_string_lossy().as_ref()
    );
    assert!(payload["project_identity"]["navigation_profile_sha256"].is_string());

    let different_input = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        actual.join("other.yaml"),
        None,
        Encoding::O200kBase,
    );
    assert!(different_input.is_err());
}

#[test]
fn profile_changes_revision_and_cannot_cross_load() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("missing.yaml");
    let c = context();
    let s = snapshot(&c);
    let plain = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let profiled = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        Some(V::from_json(&json!({"groups":{}})).unwrap()),
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = plain.save(&c, &s).unwrap();
    assert_ne!(revision, profiled.revision(&c).unwrap());
    assert!(profiled.load(&revision, &s).is_err());
}

#[test]
fn revalidates_root_identity_and_project_claim_on_every_operation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let source = dir.path().join("input");
    let c = context();
    let s = snapshot(&c);
    let store =
        CheckedSessionStore::open(&root, "fixture", &source, None, Encoding::O200kBase).unwrap();
    let revision = store.save(&c, &s).unwrap();
    fs::write(
        root.join("project.json"),
        br#"{"project":"foreign","input_path":"/foreign"}"#,
    )
    .unwrap();
    assert!(store.load(&revision, &s).is_err());
    assert!(store.save(&c, &s).is_err());

    fs::rename(&root, dir.path().join("old-state")).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("project.json"),
        serde_json::to_vec(&json!({"project":"fixture","input_path":source})).unwrap(),
    )
    .unwrap();
    assert!(store.context_path(&revision).is_err());
    assert!(store.load(&revision, &s).is_err());
}

#[cfg(unix)]
#[test]
fn existing_directory_permissions_are_preserved_and_owned_files_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("existing");
    fs::create_dir(&existing).unwrap();
    fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();
    CheckedSessionStore::open(
        &existing,
        "fixture",
        dir.path().join("input"),
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    assert_eq!(
        fs::metadata(&existing).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(existing.join("project.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let created = dir.path().join("created");
    CheckedSessionStore::open(
        &created,
        "fixture",
        dir.path().join("other-input"),
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    assert_eq!(
        fs::metadata(created).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[cfg(unix)]
#[test]
fn fifo_context_is_rejected_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    let c = context();
    let s = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &s).unwrap();
    let path = store.context_path(&revision).unwrap();
    fs::remove_file(&path).unwrap();
    let status = Command::new("mkfifo").arg(&path).status().unwrap();
    assert!(status.success());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(store.load(&revision, &s).is_err()).unwrap());
    assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
}

#[cfg(unix)]
#[test]
fn oversized_regular_context_is_rejected_by_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    let c = context();
    let s = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &s).unwrap();
    let path = store.context_path(&revision).unwrap();
    let file = fs::OpenOptions::new()
        .write(true)
        .custom_flags(0)
        .open(path)
        .unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();
    assert!(store.load(&revision, &s).is_err());
}

#[test]
fn concurrent_different_identity_claims_have_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for project in ["first", "second"] {
        let root = root.clone();
        let source = dir.path().join(format!("{project}.yaml"));
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            CheckedSessionStore::open(root, project, source, None, Encoding::O200kBase).is_ok()
        }));
    }
    barrier.wait();
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(outcomes.iter().filter(|value| **value).count(), 1);
}

#[cfg(unix)]
#[test]
#[ignore = "requires an explicitly configured immutable Python oracle and tokenizer"]
fn python_oracle_accepts_rust_revision_and_payload() {
    let python =
        std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("set explicit oracle Python");
    let baseline = std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("set immutable oracle root");
    let tokenizer =
        std::env::var_os("KPOP_SESSION_ORACLE_TOKENIZER").expect("set oracle tokenizer directory");
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("missing.yaml");
    let c = context();
    let profile = json!({"groups":{"priority":["p.input"]}});
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        Some(V::from_json(&profile).unwrap()),
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &snapshot(&c)).unwrap();
    let path = store.context_path(&revision).unwrap();
    let script = r#"
import json, pathlib, sys
from scripts.reasoning.context import CapturedAssessment
from scripts.session.model import digest
payload = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
profile = json.loads(sys.argv[4])
identity = {'version': 1, 'project': 'fixture', 'input_path': str(pathlib.Path(sys.argv[2]).resolve()),
            'navigation_profile_sha256': digest(profile)}
assert set(payload) == {'version', 'project_identity', 'revision', 'context'}
assert payload['version'] == 1
assert payload['project_identity'] == identity
assert payload['revision'] == sys.argv[3]
context = CapturedAssessment.from_data(payload['context'])
assert context.session_revision(identity) == sys.argv[3]
"#;
    let output = Command::new(python)
        .arg("-c")
        .arg(script)
        .arg(path)
        .arg(source)
        .arg(&revision)
        .arg(serde_json::to_string(&profile).unwrap())
        .env(
            "PYTHONPATH",
            std::env::join_paths([baseline, tokenizer]).unwrap(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Python oracle rejected Rust payload: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
