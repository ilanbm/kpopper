use kpop_native::{
    checked_session_store::{CheckedSessionStore, ProposalRequest, proposal_id},
    reasoning_context::CapturedAssessment,
    reasoning_snapshot::Snapshot,
    tokenizer::Encoding,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
#[cfg(windows)]
use std::path::PathBuf;
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

fn request(kind: &str, text: &str, basis: &[&str], revisit: &str) -> ProposalRequest {
    ProposalRequest {
        kind: kind.into(),
        text: text.into(),
        basis: basis.iter().map(|item| (*item).into()).collect(),
        revisit: revisit.into(),
    }
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
fn retained_context_version_cannot_be_forged_with_a_private_number_object() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let context = context();
    let snapshot = snapshot(&context);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&context, &snapshot).unwrap();
    let path = store.context_path(&revision).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    let forged = raw.replacen(
        r#""version":1"#,
        r#""version":{"$serde_json::private::Number":"1"}"#,
        1,
    );
    assert_ne!(forged, raw);
    fs::write(path, forged).unwrap();
    assert!(store.load(&revision, &snapshot).is_err());
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
fn proposal_id_and_refusals_match_the_python_oracle_fixture() {
    let oracle: J = serde_json::from_str(include_str!("fixtures/session-proposals.json")).unwrap();
    let revision = oracle["base_revision"].as_str().unwrap();
    let inferred = request(
        "inferred",
        "Observed conclusion",
        &[
            "source:source.one",
            "external:https://example.test/evidence",
            "node:fact.one",
            "node:fact.one",
        ],
        "Changes if the source or node changes.",
    );
    assert_eq!(
        proposal_id("fixture", revision, &inferred).unwrap(),
        oracle["cases"][0]["ok"]["id"]
    );
    assert_eq!(
        proposal_id(
            "fixture",
            revision,
            &request("question", "What should be checked?", &[], ""),
        )
        .unwrap(),
        oracle["cases"][1]["ok"]["id"]
    );

    let refusals = [
        request("known", "x", &["node:fact.one"], "later"),
        request("assumed", "  ", &[], "later"),
        request("observed", "x", &[], "later"),
        request("assumed", "x", &[], ""),
        request("observed", "x", &["external:javascript:alert(1)"], "later"),
        request("observed", "x", &["external:https:"], "later"),
        request("question", "x", &["pending"], ""),
    ];
    let oracle_indexes = [2, 3, 4, 5, 8, 9, 10];
    for (input, index) in refusals.iter().zip(oracle_indexes) {
        assert_eq!(
            proposal_id("fixture", revision, input).unwrap_err().0,
            oracle["cases"][index]["refused"]
        );
    }
}

#[test]
fn proposal_listing_matches_python_oracle_values_including_old_revisions() {
    let oracle: J = serde_json::from_str(include_str!("fixtures/session-proposals.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    for (id, proposal) in oracle["proposals"].as_object().unwrap() {
        fs::write(
            dir.path().join("state").join(format!("proposal-{id}.json")),
            serde_json::to_vec(proposal).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(
        serde_json::to_value(store.proposals().unwrap()).unwrap(),
        oracle["proposals"]
    );
}

#[test]
fn proposal_is_idempotent_and_keeps_canonical_source_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("GROUNDING.yaml");
    let canonical = b"canonical record bytes";
    fs::write(&source, canonical).unwrap();
    let c = context();
    let fresh = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &fresh).unwrap();
    let input = request(
        "inferred",
        "Observed conclusion",
        &[
            "node:p.a",
            "external:https://example.test/evidence",
            "node:p.a",
        ],
        "Changes if the source or node changes.",
    );
    let first = store.propose(&revision, &fresh, &input).unwrap();
    let path = dir
        .path()
        .join("state")
        .join(format!("proposal-{}.json", first["id"].as_str().unwrap()));
    let first_bytes = fs::read(&path).unwrap();
    std::thread::sleep(Duration::from_millis(2));
    assert_eq!(store.propose(&revision, &fresh, &input).unwrap(), first);
    assert_eq!(fs::read(&path).unwrap(), first_bytes);
    assert_eq!(fs::read(&source).unwrap(), canonical);

    let listed = store.proposals().unwrap();
    let stored = &listed[first["id"].as_str().unwrap()];
    assert_eq!(
        stored["basis"],
        json!(["external:https://example.test/evidence", "node:p.a"])
    );
    assert_eq!(
        stored["claimed_by"],
        "session; not authenticated as a person"
    );
    assert_eq!(
        stored["unverified_external_basis"],
        json!(["external:https://example.test/evidence"])
    );
    assert!(chrono::DateTime::parse_from_rfc3339(stored["created_at"].as_str().unwrap()).is_ok());
}

#[test]
fn proposing_requires_fresh_session_and_valid_recorded_basis() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let c = context();
    let fresh = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &fresh).unwrap();
    for basis in ["node:missing", "source:missing"] {
        assert_eq!(
            store
                .propose(
                    &revision,
                    &fresh,
                    &request("inferred", "x", &[basis], "later"),
                )
                .unwrap_err()
                .0,
            "unlisted operation or identifier"
        );
    }
    let changed = Snapshot::from_data(
        &V::from_json(&json!({"meta":{"scope":"changed"}})).unwrap(),
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        store
            .propose(
                &revision,
                &changed,
                &request("question", "Still current?", &[], ""),
            )
            .unwrap_err()
            .0,
        "project record or history changed; reopen"
    );
    assert!(store.proposals().unwrap().is_empty());
}

#[test]
fn concurrent_proposal_creation_has_one_complete_value() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let c = context();
    let fresh = snapshot(&c);
    let store = Arc::new(
        CheckedSessionStore::open(
            dir.path().join("state"),
            "fixture",
            &source,
            None,
            Encoding::O200kBase,
        )
        .unwrap(),
    );
    let revision = store.save(&c, &fresh).unwrap();
    let input = request("question", "What next?", &[], "");
    let mut workers = Vec::new();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    for _ in 0..8 {
        let store = store.clone();
        let revision = revision.clone();
        let fresh = fresh.clone();
        let input = input.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            store.propose(&revision, &fresh, &input).unwrap()
        }));
    }
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert!(results.iter().all(|result| result == &results[0]));
    assert_eq!(store.proposals().unwrap().len(), 1);
}

#[cfg(windows)]
#[test]
fn proposal_reader_waits_for_transient_publisher_delete_handle() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let c = context();
    let fresh = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &fresh).unwrap();
    let input = request("question", "What next?", &[], "");
    let expected = store.propose(&revision, &fresh, &input).unwrap();
    let path = dir.path().join("state").join(format!(
        "proposal-{}.json",
        expected["id"].as_str().unwrap()
    ));
    // Model the DELETE handle held while MoveFileEx publishes a name. It
    // permits readers, but a reader denying delete sharing must wait for it.
    let publisher = fs::OpenOptions::new()
        .access_mode(0x0001_0000) // DELETE
        .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
        .open(&path)
        .unwrap();
    let closer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(80));
        drop(publisher);
    });
    let result = store.propose(&revision, &fresh, &input);
    closer.join().unwrap();
    assert_eq!(result.unwrap(), expected);
    assert_eq!(store.proposals().unwrap().len(), 1);
}

#[cfg(windows)]
#[test]
fn proposal_reader_still_refuses_a_persistent_delete_handle() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let publisher = fs::OpenOptions::new()
        .access_mode(0x0001_0000)
        .share_mode(0x0000_0001 | 0x0000_0002 | 0x0000_0004)
        .open(dir.path().join("state/project.json"))
        .unwrap();
    let started = std::time::Instant::now();
    let error = store.proposals().unwrap_err();
    assert!(error.0.contains("retained context"), "{}", error.0);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    drop(publisher);
    assert!(store.proposals().unwrap().is_empty());
}

#[test]
fn proposal_listing_refuses_forged_identity_project_status_and_filename() {
    for field in ["id", "project", "status", "filename"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input");
        fs::write(&source, b"x").unwrap();
        let c = context();
        let fresh = snapshot(&c);
        let store = CheckedSessionStore::open(
            dir.path().join("state"),
            "fixture",
            &source,
            None,
            Encoding::O200kBase,
        )
        .unwrap();
        let revision = store.save(&c, &fresh).unwrap();
        let result = store
            .propose(
                &revision,
                &fresh,
                &request("question", "What next?", &[], ""),
            )
            .unwrap();
        let path = dir
            .path()
            .join("state")
            .join(format!("proposal-{}.json", result["id"].as_str().unwrap()));
        if field == "filename" {
            fs::rename(
                &path,
                dir.path().join("state/proposal-0000000000000000000000000000000000000000000000000000000000000000.json"),
            )
            .unwrap();
        } else {
            let mut proposal: J = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            proposal[field] = json!("forged");
            fs::write(&path, serde_json::to_vec(&proposal).unwrap()).unwrap();
        }
        assert_eq!(
            store.proposals().unwrap_err().0,
            "proposal identity mismatch"
        );
    }
}

#[test]
fn proposing_refuses_a_tampered_same_id_collision() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let c = context();
    let fresh = snapshot(&c);
    let store = CheckedSessionStore::open(
        dir.path().join("state"),
        "fixture",
        &source,
        None,
        Encoding::O200kBase,
    )
    .unwrap();
    let revision = store.save(&c, &fresh).unwrap();
    let input = request("question", "What next?", &[], "");
    let result = store.propose(&revision, &fresh, &input).unwrap();
    let path = dir
        .path()
        .join("state")
        .join(format!("proposal-{}.json", result["id"].as_str().unwrap()));
    let mut proposal: J = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    proposal["text"] = json!("tampered");
    fs::write(path, serde_json::to_vec(&proposal).unwrap()).unwrap();
    assert_eq!(
        store.propose(&revision, &fresh, &input).unwrap_err().0,
        "proposal collision"
    );
}

#[cfg(unix)]
#[test]
fn proposal_listing_refuses_symlink_fifo_and_oversized_members_without_blocking() {
    for kind in ["symlink", "fifo", "oversized"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input");
        fs::write(&source, b"x").unwrap();
        let store = CheckedSessionStore::open(
            dir.path().join("state"),
            "fixture",
            &source,
            None,
            Encoding::O200kBase,
        )
        .unwrap();
        let member = dir.path().join(
            "state/proposal-0000000000000000000000000000000000000000000000000000000000000000.json",
        );
        match kind {
            "symlink" => {
                let outside = dir.path().join("outside");
                fs::write(&outside, b"{}").unwrap();
                symlink(outside, &member).unwrap();
            }
            "fifo" => assert!(
                Command::new("mkfifo")
                    .arg(&member)
                    .status()
                    .unwrap()
                    .success()
            ),
            "oversized" => {
                let file = fs::File::create(&member).unwrap();
                file.set_len(64 * 1024 * 1024 + 1).unwrap();
            }
            _ => unreachable!(),
        }
        let message = store.proposals().unwrap_err().0;
        assert!(
            message.contains("retained context") || message.contains("proposal identity"),
            "{kind}: {message}"
        );
    }
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

    #[cfg(unix)]
    {
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
    #[cfg(windows)]
    {
        assert!(fs::rename(&root, dir.path().join("old-state")).is_err());
        assert!(store.context_path(&revision).is_err());
    }
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

#[cfg(windows)]
#[test]
fn windows_store_anchors_reads_and_refuses_network_and_reparse_roots() {
    use std::process::Command;

    assert!(
        CheckedSessionStore::open(
            r"\\example.invalid\share\state",
            "fixture",
            r"C:\missing.yaml",
            None,
            Encoding::O200kBase,
        )
        .is_err()
    );

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("StateRoot");
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    let c = context();
    let s = snapshot(&c);
    let store =
        CheckedSessionStore::open(&root, "fixture", &source, None, Encoding::O200kBase).unwrap();
    let revision = store.save(&c, &s).unwrap();

    let case_alias = PathBuf::from(root.to_string_lossy().to_lowercase());
    let reopened =
        CheckedSessionStore::open(case_alias, "fixture", &source, None, Encoding::O200kBase)
            .unwrap();
    assert_eq!(reopened.load(&revision, &s).unwrap().revision(), revision);

    let junction = dir.path().join("state-junction");
    let linked = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    assert!(
        CheckedSessionStore::open(&junction, "fixture", &source, None, Encoding::O200kBase,)
            .is_err()
    );

    let context_path = store.context_path(&revision).unwrap();
    fs::remove_file(&context_path).unwrap();
    fs::create_dir(&context_path).unwrap();
    assert!(store.load(&revision, &s).is_err());
}

#[cfg(windows)]
#[test]
fn windows_store_never_reads_project_identity_from_process_cwd() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let source = dir.path().join("input");
    fs::write(&source, b"x").unwrap();
    CheckedSessionStore::open(&root, "fixture", &source, None, Encoding::O200kBase).unwrap();
    let unrelated = dir.path().join("unrelated");
    fs::create_dir(&unrelated).unwrap();
    fs::write(
        unrelated.join("project.json"),
        br#"{"project":"foreign","input_path":"C:\\foreign"}"#,
    )
    .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "windows_store_cwd_child", "--nocapture"])
        .current_dir(&unrelated)
        .env("KPOP_CHECKED_STORE_ROOT", &root)
        .env("KPOP_CHECKED_STORE_SOURCE", &source)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(windows)]
#[test]
fn windows_store_cwd_child() {
    let Some(root) = std::env::var_os("KPOP_CHECKED_STORE_ROOT") else {
        return;
    };
    let source = std::env::var_os("KPOP_CHECKED_STORE_SOURCE").unwrap();
    CheckedSessionStore::open(root, "fixture", source, None, Encoding::O200kBase).unwrap();
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
