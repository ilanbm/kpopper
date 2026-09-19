use kpop_native::{
    checked_session_store::CheckedSessionStore, reasoning_context::CapturedAssessment,
    reasoning_snapshot::Snapshot, tokenizer::Encoding, value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::fs;

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
