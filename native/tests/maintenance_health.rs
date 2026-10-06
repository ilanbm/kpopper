fn admit(workspace: &std::path::Path) {
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(workspace)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture failed native admission: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

use chrono::{Duration, TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn record(path: &Path) {
    fs::write(
        path.join("PROVENANCE.yaml"),
        "meta:\n  name: Maintenance fixture\n  updated: 2026-10-06\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    admit(path);
}

fn declaration() -> Value {
    json!({
        "schema":"kpopper.maintenance-declaration/v1",
        "kind":"source",
        "id":"source-check",
        "title":"Inspect the selected source",
        "why":"Keep the evidence current.",
        "how":"Read the selected source and retain its observed state.",
        "scope":"Inspect this source only; do not publish changes.",
        "related":["facts.count"],
        "cadence_days":1,
        "timezone":"UTC",
        "check_time":"09:00",
        "use_policy":"allow_cached_until_expiry",
        "evidence_requirement":"host_attested",
        "first_due_at":"2026-10-06T12:00:00Z",
        "source":{
            "source_id":"fixture-source",
            "locator":"https://example.test/status",
            "publisher":"Example",
            "selection":"Current status",
            "adapter":"host-read/v1",
            "evidence_format":"selected text",
            "tool_policy":"existing authorized read tool",
            "allowed_roots":["https://example.test/"],
            "max_age_hours":24
        }
    })
}

#[test]
fn source_health_changes_at_expiry_without_mutating_the_ledger() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    record(&workspace);
    let state = temp.path().join("state");
    let checked_at = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, checked_at).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&declaration()).unwrap();
    store.add(proposal["spec"].clone()).unwrap();
    let source_ref = proposal["spec"]["maintenance"]["source_ref"]
        .as_str()
        .unwrap();
    store
        .inspect_maintenance(json!({
            "id":"source-check", "policy_digest":proposal["spec"]["maintenance"]["policy_digest"],
            "source_ref":source_ref, "inspection":proposal["spec"]["maintenance"]["inspection"],
            "value":"available",
            "inspected_at":checked_at.to_rfc3339(),
            "evidence":"host supplied selected status evidence"
        }))
        .unwrap();

    let fresh = followup_daily::status(&store).unwrap();
    let before = fs::read(&store.path).unwrap();
    let graph_before = fs::read(workspace.join("PROVENANCE.yaml")).unwrap();
    let expired_store =
        Store::at_in_state(&workspace, &state, checked_at + Duration::hours(24)).unwrap();
    let expired = followup_daily::status(&expired_store).unwrap();

    assert_eq!(
        fresh["maintenance_health"]["obligations"][0]["id"],
        "source-check"
    );
    assert_eq!(
        fresh["maintenance_health"]["obligations"][0]["source_state"],
        "within_age_window"
    );
    assert_eq!(
        expired["maintenance_health"]["obligations"][0]["source_state"],
        "stale"
    );
    assert_eq!(
        expired["maintenance_health"]["obligations"][0]["host_state"],
        "missing"
    );
    assert_eq!(expired["maintenance_health"]["degraded"], true);
    assert_ne!(
        fresh["maintenance_health"]["fingerprint"],
        expired["maintenance_health"]["fingerprint"]
    );
    assert_eq!(before, fs::read(&store.path).unwrap());
    assert_eq!(
        graph_before,
        fs::read(workspace.join("PROVENANCE.yaml")).unwrap()
    );
}

#[test]
fn periodic_check_rearms_from_admitted_local_day_and_health_changes_at_next_at() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    record(&workspace);
    let state = temp.path().join("state");
    let checked_at = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, checked_at).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&declaration()).unwrap();
    store.add(proposal["spec"].clone()).unwrap();
    let row = store.scan(20).unwrap()["items"][0].clone();
    let claim = store
        .claim(
            "source-check",
            row["occurrence"].as_str().unwrap(),
            "tester",
            None,
        )
        .unwrap();
    let token = claim["claim"]["token"].as_str().unwrap();
    assert!(
        store
            .finish(
                "source-check",
                token,
                "done",
                "not a periodic completion",
                None
            )
            .is_err()
    );
    assert!(
        store
            .finish(
                "source-check",
                token,
                "checked",
                "successful local check",
                Some("2026-10-07T12:00:00Z"),
            )
            .is_err()
    );
    store
        .finish(
            "source-check",
            token,
            "checked",
            "successful local check",
            Some("2026-10-07T09:00:00Z"),
        )
        .unwrap();

    let before = followup_daily::status(&store).unwrap();
    let ledger = fs::read(&store.path).unwrap();
    let after_store = Store::at_in_state(
        &workspace,
        &state,
        Utc.with_ymd_and_hms(2026, 10, 7, 9, 0, 0).unwrap(),
    )
    .unwrap();
    let after = followup_daily::status(&after_store).unwrap();

    assert_eq!(
        before["maintenance_health"]["obligations"][0]["check_state"],
        "scheduled"
    );
    assert_eq!(
        after["maintenance_health"]["obligations"][0]["check_state"],
        "due"
    );
    assert_ne!(
        before["maintenance_health"]["fingerprint"],
        after["maintenance_health"]["fingerprint"]
    );
    assert_eq!(ledger, fs::read(&store.path).unwrap());
}

#[test]
fn repeated_source_failures_keep_success_then_park_for_user() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    record(&workspace);
    let state = temp.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&declaration()).unwrap();
    let item = store.add(proposal["spec"].clone()).unwrap();
    let source_ref = item["spec"]["maintenance"]["source_ref"].as_str().unwrap();
    store
        .inspect_maintenance(json!({
            "id":"source-check", "policy_digest":item["spec"]["maintenance"]["policy_digest"], "source_ref":source_ref,
            "inspection":item["spec"]["maintenance"]["inspection"],
            "value":"last-success",
            "inspected_at":now.to_rfc3339(),
            "evidence":"prior successful source observation"
        }))
        .unwrap();
    for attempt_number in 0..3 {
        let result = store
            .record_maintenance_attempt(json!({
                "id":"source-check",
                "policy_digest":item["spec"]["maintenance"]["policy_digest"],
                "source_ref":source_ref,
                "evidence":format!("unavailable attempt {attempt_number}"),
                "reason":"authorized local source tool unavailable"
            }))
            .unwrap();
        if attempt_number < 2 {
            assert_eq!(result["attempt"]["retry_at"], "2026-10-07T09:00:00Z");
        } else {
            assert_eq!(result["attempt"]["retry_at"], Value::Null);
        }
    }

    let stored = store.show("source-check").unwrap();
    let health = followup_daily::status(&store).unwrap()["maintenance_health"].clone();
    assert_eq!(stored["state"], "needs_user");
    assert_eq!(
        stored["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["outcome"] == "unavailable")
            .count(),
        3
    );
    assert_eq!(
        health["obligations"][0]["source_state"],
        "within_age_window"
    );
    assert_eq!(health["obligations"][0]["check_state"], "needs_user");
    assert_eq!(health["obligations"][0]["failure_state"], "failed");
    assert_eq!(
        store.load(true).unwrap().unwrap()["observations"][source_ref]["value"],
        "last-success"
    );
}
