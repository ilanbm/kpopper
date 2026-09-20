#![cfg(unix)]
use kpop_native::{
    identity, ingestion_delivery as delivery, ingestion_orchestration as ingestion, ingestion_state,
};
use serde_json::{Value as J, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Command,
    thread,
    time::{Duration, Instant},
};

fn fixture(fired: bool) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("old.txt"), "old fixture\n").unwrap();
    let wrong_if = if fired {
        "facts.count == false"
    } else {
        "facts.count < 0"
    };
    let value = if fired { "true" } else { "3" };
    fs::write(temp.path().join("PROVENANCE.yaml"), format!("meta: {{name: Queue fixture, updated: 2026-09-07}}\nschema: {{deps: rests_on, snapshot: seen, predicate: wrong_if}}\nsources:\n  s.old: {{name: Old fixture, file: old.txt, read: 2026-09-07}}\nknown:\n  facts.count: {{name: Count, v: {value}, from: s.old, at: line 1, of: 2026-09-07}}\n  facts.other: {{name: Other, v: 10, from: s.old, at: line 1, of: 2026-09-07}}\njudgments:\n  c.acceptable: {{rests_on: [facts.count], verdict: The count remains acceptable., wrong_if: {wrong_if}, seen: {{facts.count: {value}}}}}\n")).unwrap();
    temp
}

fn envelope(id: &str, value: J) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "event_id":id, "session_id":"session-1", "source_quote":"There are four packages now.",
        "target":"facts.count", "value":value, "date":"2026-09-08", "kind":"report"
    }))
    .unwrap()
}
fn custom_envelope(id: &str, target: &str, value: J, date: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "event_id":id, "source_quote":format!("{target} is now {value}."),
        "target":target, "value":value, "date":date, "kind":"report"
    }))
    .unwrap()
}

#[test]
fn capture_is_private_durable_and_idempotent() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("private-state");
    let first = ingestion::capture(
        &envelope("stable", json!(4)),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let second = ingestion::capture(
        &envelope("stable", json!(4)),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    assert_eq!(first["event_id"], second["event_id"]);
    assert_eq!(first["state"], "captured");
    let stored = ingestion::status(
        first["event_id"].as_str(),
        Some(&record),
        Some(&state),
        temp.path(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        fs::read_to_string(stored["source_file"].as_str().unwrap()).unwrap(),
        "There are four packages now."
    );
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(stored["source_file"].as_str().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let before = fs::read(&record).unwrap();
    assert!(
        ingestion::capture(
            &envelope("stable", json!(5)),
            Some(&record),
            Some(&state),
            temp.path(),
            false
        )
        .unwrap_err()
        .0
        .contains("reused")
    );
    assert_eq!(fs::read(&record).unwrap(), before);
}

#[test]
fn permanent_write_failure_becomes_terminal_without_automatic_retries() {
    let temp = fixture(false);
    let state = tempfile::tempdir().unwrap();
    let record = temp.path().join("PROVENANCE.yaml");
    let before = fs::read(&record).unwrap();
    let captured = ingestion::capture(
        &envelope("readonly", json!(4)), Some(&record), Some(state.path()), temp.path(), false,
    ).unwrap();
    let permissions = fs::metadata(temp.path()).unwrap().permissions();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let processed = ingestion::process(Some(&record), Some(state.path()), temp.path(), None, 32);
    fs::set_permissions(temp.path(), permissions).unwrap();
    let processed = processed.unwrap();
    assert_eq!(processed.len(), 1);
    assert_eq!(processed[0]["state"], "needs_primary", "{}", processed[0]);
    assert_eq!(fs::read(&record).unwrap(), before);
    assert!(ingestion::process(Some(&record), Some(state.path()), temp.path(), None, 32)
        .unwrap().is_empty());
    let stored = ingestion::status(captured["event_id"].as_str(), Some(&record),
        Some(state.path()), temp.path()).unwrap().unwrap();
    assert_eq!(stored["state"], "needs_primary");
    let event: J = serde_json::from_slice(&fs::read(state.path().join("events")
        .join(format!("{}.json", captured["event_id"].as_str().unwrap()))).unwrap()).unwrap();
    assert_eq!(event["attempts"], 1);
}

#[test]
fn process_uses_the_single_writer_and_is_idempotent() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let captured = ingestion::capture(
        &envelope("apply", json!(4)),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let id = captured["event_id"].as_str().unwrap();
    let results =
        ingestion::process(Some(&record), Some(&state), temp.path(), Some(id), 32).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["state"], "applied", "{}", results[0]);
    assert!(fs::read_to_string(&record).unwrap().contains("v: 4"));
    assert!(
        ingestion::process(Some(&record), Some(&state), temp.path(), Some(id), 32)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ingestion::status(Some(id), Some(&record), Some(&state), temp.path())
            .unwrap()
            .unwrap(),
        results[0]
    );
    assert_eq!(
        fs::read_to_string(results[0]["source_file"].as_str().unwrap()).unwrap(),
        "There are four packages now."
    );
}

#[test]
fn tampered_capture_and_private_override_fail_closed() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let captured = ingestion::capture(
        &envelope("tamper", json!(4)),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let id = captured["event_id"].as_str().unwrap();
    let before = fs::read(&record).unwrap();
    let stored = ingestion::status(Some(id), Some(&record), Some(&state), temp.path())
        .unwrap()
        .unwrap();
    fs::write(stored["source_file"].as_str().unwrap(), "substituted").unwrap();
    let result =
        ingestion::process(Some(&record), Some(&state), temp.path(), Some(id), 32).unwrap();
    assert_eq!(result[0]["state"], "needs_primary");
    assert_eq!(fs::read(&record).unwrap(), before);

    let private = serde_json::to_vec(&json!({
        "event_id":"private", "source_quote":"private source", "target":"facts.count",
        "value":4, "date":"2026-09-08", "privacy":"private", "kind":"report"
    }))
    .unwrap();
    let captured =
        ingestion::capture(&private, Some(&record), Some(&state), temp.path(), false).unwrap();
    let id = captured["event_id"].as_str().unwrap();
    let result =
        ingestion::process(Some(&record), Some(&state), temp.path(), Some(id), 32).unwrap();
    assert_eq!(result[0]["state"], "needs_primary");
    assert_eq!(fs::read(&record).unwrap(), before);
}

#[test]
fn generated_event_identity_is_bound_through_processing() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let raw = serde_json::to_vec(&json!({
        "source_quote":"There are four packages now.", "target":"facts.count",
        "value":4, "date":"2026-09-08", "kind":"report"
    }))
    .unwrap();
    let captured =
        ingestion::capture(&raw, Some(&record), Some(&state), temp.path(), false).unwrap();
    let id = captured["event_id"].as_str().unwrap();
    assert_eq!(id.len(), 32);
    let receipt =
        &ingestion::process(Some(&record), Some(&state), temp.path(), Some(id), 32).unwrap()[0];
    assert_eq!(receipt["event_id"], id);
    assert_eq!(receipt["source"], format!("s.ingest_{id}"), "{receipt}");
    assert_eq!(
        receipt["envelope_sha256"],
        identity::sha256(&fs::read(state.join("envelopes").join(format!("{id}.json"))).unwrap())
    );
}

#[test]
fn independent_targets_do_not_stale_each_other() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let first = ingestion::capture(
        &custom_envelope("first", "facts.count", json!(4), "2026-09-08"),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let second = ingestion::capture(
        &custom_envelope("second", "facts.other", json!(11), "2026-09-08"),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    for event in [&first, &second] {
        let id = event["event_id"].as_str().unwrap();
        let result =
            ingestion::process(Some(&record), Some(&state), temp.path(), Some(id), 32).unwrap();
        assert_eq!(result[0]["state"], "applied", "{}", result[0]);
    }
    let text = fs::read_to_string(record).unwrap();
    assert!(text.contains("v: 4"));
    assert!(text.contains("v: 11"));
}

#[test]
fn later_arriving_older_report_never_supersedes() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let newer = ingestion::capture(
        &custom_envelope("newer", "facts.count", json!(5), "2026-09-10"),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let older = ingestion::capture(
        &custom_envelope("older", "facts.count", json!(4), "2026-09-09"),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let first = ingestion::process(
        Some(&record),
        Some(&state),
        temp.path(),
        newer["event_id"].as_str(),
        32,
    )
    .unwrap();
    let second = ingestion::process(
        Some(&record),
        Some(&state),
        temp.path(),
        older["event_id"].as_str(),
        32,
    )
    .unwrap();
    assert_eq!(first[0]["state"], "applied");
    assert_eq!(second[0]["state"], "needs_primary");
    assert!(fs::read_to_string(record).unwrap().contains("v: 5"));
}

#[test]
fn detached_worker_finishes_and_releases_its_lease() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let report = temp.path().join("report.json");
    fs::write(&report, envelope("detached", json!(4))).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["ingest", "capture", "--file"])
        .arg(&report)
        .arg("--record")
        .arg(&record)
        .arg("--state-dir")
        .arg(&state)
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let event: J = serde_json::from_slice(&output.stdout).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let receipt = loop {
        let value = ingestion::status(
            event["event_id"].as_str(),
            Some(&record),
            Some(&state),
            temp.path(),
        )
        .unwrap()
        .unwrap();
        if matches!(
            value["state"].as_str(),
            Some("applied" | "needs_primary" | "error")
        ) && !state.join("worker.lease").exists()
        {
            break value;
        }
        assert!(Instant::now() < deadline, "worker did not finish: {value}");
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(receipt["state"], "applied", "{receipt}");
    assert!(fs::read_to_string(record).unwrap().contains("v: 4"));
}

#[test]
fn repaired_judgment_stops_pending_delivery_but_keeps_history() {
    let temp = fixture(true);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let captured = ingestion::capture(
        &envelope("contradiction", json!(false)),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    ingestion::process(
        Some(&record),
        Some(&state),
        temp.path(),
        captured["event_id"].as_str(),
        32,
    )
    .unwrap();
    assert_eq!(
        ingestion::pending(Some(&record), Some(&state), temp.path(), false)
            .unwrap()
            .len(),
        1
    );
    let raw = fs::read_to_string(&record).unwrap();
    fs::write(&record, raw.replacen("v: false", "v: true", 1)).unwrap();
    let pending = ingestion::pending(Some(&record), Some(&state), temp.path(), false).unwrap();
    assert!(
        pending.is_empty(),
        "record={} pending={pending:?}",
        fs::read_to_string(&record).unwrap()
    );
    assert_eq!(fs::read_dir(state.join("signals")).unwrap().count(), 1);
}

#[test]
fn delivery_claim_completion_and_epoch_suppression_are_bounded() {
    let temp = fixture(true);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let captured = delivery::capture(
        &envelope("delivery", json!(false)),
        "trusted-thread",
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    assert_eq!(captured["delivery_job"]["dispatch_required"], true);
    let event = captured["event_id"].as_str().unwrap();
    ingestion::process(Some(&record), Some(&state), temp.path(), Some(event), 32).unwrap();
    let attention = delivery::wait(
        captured["delivery_job"]["id"].as_str().unwrap(),
        Some(&record),
        Some(&state),
        temp.path(),
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(attention["status"], "attention", "{attention}");
    assert!(
        attention["message"]
            .as_str()
            .unwrap()
            .starts_with("KPOPPER_ATTENTION\n")
    );
    let completed = delivery::complete(
        attention["id"].as_str().unwrap(),
        attention["claim_token"].as_str().unwrap(),
        "sent",
        Some(&record),
        Some(&state),
        temp.path(),
    )
    .unwrap();
    assert_eq!(completed["status"], "sent");
    let notices = ingestion::pending(Some(&record), Some(&state), temp.path(), false).unwrap();
    let layout = ingestion_state::Layout::create(&record, Some(&state)).unwrap();
    assert!(
        delivery::hook_visible(
            &layout,
            "trusted-thread",
            notices.clone(),
            completed["epoch"].as_str().unwrap()
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        delivery::hook_visible(
            &layout,
            "trusted-thread",
            notices,
            "00000000000000000000000000000000"
        )
        .unwrap()
        .len(),
        1
    );
    let retried = delivery::capture(
        &envelope("delivery", json!(false)),
        "trusted-thread",
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    assert_eq!(retried["delivery_job"]["dispatch_required"], false);
}

#[test]
fn unsafe_and_foreign_state_directories_are_refused_without_mutation() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let occupied = temp.path().join("occupied");
    fs::create_dir(&occupied).unwrap();
    fs::set_permissions(&occupied, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(occupied.join("keep.txt"), "keep").unwrap();
    let error = ingestion::capture(
        &envelope("occupied", json!(4)),
        Some(&record),
        Some(&occupied),
        temp.path(),
        false,
    )
    .unwrap_err();
    assert!(error.0.contains("not owned"));
    assert_eq!(
        fs::metadata(&occupied).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::read_to_string(occupied.join("keep.txt")).unwrap(),
        "keep"
    );
    assert_eq!(fs::read_dir(&occupied).unwrap().count(), 1);

    let foreign = temp.path().join("foreign");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("record.json"), "{\"record\":\"/tmp/other\"}\n").unwrap();
    let before = fs::read(foreign.join("record.json")).unwrap();
    let error = ingestion::capture(
        &envelope("foreign", json!(4)),
        Some(&record),
        Some(&foreign),
        temp.path(),
        false,
    )
    .unwrap_err();
    assert!(error.0.contains("another provenance record"));
    assert_eq!(fs::read(foreign.join("record.json")).unwrap(), before);
}

#[test]
fn processing_respects_capture_order_and_maximum() {
    let temp = fixture(false);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let first = ingestion::capture(
        &custom_envelope("order-first", "facts.count", json!(4), "2026-09-08"),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let second = ingestion::capture(
        &custom_envelope("order-second", "facts.other", json!(11), "2026-09-08"),
        Some(&record),
        Some(&state),
        temp.path(),
        false,
    )
    .unwrap();
    let result = ingestion::process(Some(&record), Some(&state), temp.path(), None, 1).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["event_id"], first["event_id"]);
    assert_eq!(
        ingestion::status(
            second["event_id"].as_str(),
            Some(&record),
            Some(&state),
            temp.path()
        )
        .unwrap()
        .unwrap()["state"],
        "captured"
    );
}

#[test]
fn delivery_cli_reserves_waits_and_completes_one_trusted_recipient() {
    let temp = fixture(true);
    let record = temp.path().join("PROVENANCE.yaml");
    let state = temp.path().join("state");
    let report = temp.path().join("report.json");
    fs::write(&report, envelope("delivery-cli", json!(false))).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .args(args)
            .current_dir(temp.path())
            .env("CODEX_SESSION_ID", "trusted-cli-thread")
            .output()
            .unwrap()
    };
    let captured = run(&[
        "ingest",
        "capture",
        "--file",
        report.to_str().unwrap(),
        "--record",
        record.to_str().unwrap(),
        "--state-dir",
        state.to_str().unwrap(),
        "--no-start",
        "--notify-task",
        "trusted-cli-thread",
    ]);
    assert!(
        captured.status.success(),
        "{}",
        String::from_utf8_lossy(&captured.stdout)
    );
    let captured: J = serde_json::from_slice(&captured.stdout).unwrap();
    assert_eq!(captured["delivery_job"]["dispatch_required"], true);
    let processed = run(&[
        "ingest",
        "process",
        "--record",
        record.to_str().unwrap(),
        "--state-dir",
        state.to_str().unwrap(),
        "--event-id",
        captured["event_id"].as_str().unwrap(),
    ]);
    assert!(
        processed.status.success(),
        "{}",
        String::from_utf8_lossy(&processed.stdout)
    );
    let waited = run(&[
        "ingest",
        "wait-delivery",
        captured["delivery_job"]["id"].as_str().unwrap(),
        "--record",
        record.to_str().unwrap(),
        "--state-dir",
        state.to_str().unwrap(),
        "--timeout",
        "0",
    ]);
    assert!(
        waited.status.success(),
        "{}",
        String::from_utf8_lossy(&waited.stdout)
    );
    let waited: J = serde_json::from_slice(&waited.stdout).unwrap();
    assert_eq!(waited["status"], "attention", "{waited}");
    let completed = run(&[
        "ingest",
        "complete-delivery",
        waited["id"].as_str().unwrap(),
        "--claim-token",
        waited["claim_token"].as_str().unwrap(),
        "--outcome",
        "sent",
        "--record",
        record.to_str().unwrap(),
        "--state-dir",
        state.to_str().unwrap(),
    ]);
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stdout)
    );
    let completed: J = serde_json::from_slice(&completed.stdout).unwrap();
    assert_eq!(completed["status"], "sent");
}
