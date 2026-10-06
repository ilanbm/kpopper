use chrono::{DateTime, TimeZone, Utc};
use kpop_native::followup_store::Store;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn record(path: &Path) {
    fs::write(
        path.join("PROVENANCE.yaml"),
        "meta:\n  name: Maintenance repair fixture\n  updated: 2026-10-06\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(path)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture admission failed: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn fixture(now: DateTime<Utc>, timezone: &str) -> (tempfile::TempDir, Store, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    record(&workspace);
    let state = temp.path().join("state");
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, timezone, None, true).unwrap();
    (temp, store, workspace)
}

fn ordinary(id: &str, at: &str) -> Value {
    json!({
        "id":id,
        "title":id,
        "why":"Keep this synthetic work current.",
        "how":"Inspect the recorded fixture value.",
        "scope":"Read the selected fixture value only.",
        "related":["facts.count"],
        "when":{"at":at}
    })
}

fn clock_declaration(
    id: &str,
    timezone: &str,
    check_time: &str,
    cadence_days: u64,
    due: &str,
) -> Value {
    json!({
        "schema":"kpopper.maintenance-declaration/v1",
        "kind":"clock",
        "id":id,
        "title":"Declared synthetic clock check",
        "why":"Keep local check cadence explicit.",
        "how":"Read the synthetic clock fixture.",
        "scope":"Read only; do not publish changes.",
        "related":["facts.count"],
        "cadence_days":cadence_days,
        "timezone":timezone,
        "check_time":check_time,
        "use_policy":"require_live",
        "evidence_requirement":"trusted_clock",
        "deadline":{"utc":due}
    })
}

fn checked_clock_at(now: DateTime<Utc>, timezone: &str, expected_next: &str) {
    let (_temp, store, _) = fixture(now.clone(), timezone);
    let declaration = clock_declaration(
        "clock-check",
        timezone,
        "02:30",
        1,
        &kpop_native::followup_triggers::stamp(now - chrono::Duration::hours(1)),
    );
    let spec = kpop_native::maintenance_contract::compile(&declaration).unwrap()["spec"].clone();
    store.add(spec).unwrap();
    let row = store.scan(20).unwrap()["items"][0].clone();
    assert_eq!(row["state"], "ready");
    let claim = store
        .claim(
            "clock-check",
            row["occurrence"].as_str().unwrap(),
            "synthetic-checker",
            None,
        )
        .unwrap();
    store
        .finish(
            "clock-check",
            claim["claim"]["token"].as_str().unwrap(),
            "checked",
            "fixture://synthetic-local-check",
            Some(expected_next),
        )
        .unwrap();
    assert_eq!(store.show("clock-check").unwrap()["next_at"], expected_next);
}

#[test]
fn truncated_waiting_queue_reports_omission_without_notification_or_degradation() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let (_temp, store, _) = fixture(now, "UTC");
    for index in 0..4 {
        store
            .add(ordinary(
                &format!("waiting-{index}"),
                "2026-10-08T12:00:00Z",
            ))
            .unwrap();
    }

    let report = store.scan(3).unwrap();

    assert_eq!(report["omitted"], 1);
    assert_eq!(report["notification"], false);
    assert!(report.get("degraded").is_none());
}

#[test]
fn graph_failure_remains_unknown_and_degraded() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let (_temp, store, workspace) = fixture(now, "UTC");
    store
        .add(ordinary("graph-dependent", "2026-10-06T11:00:00Z"))
        .unwrap();
    fs::write(workspace.join("PROVENANCE.yaml"), "meta: [invalid\n").unwrap();

    let report = store.scan(3).unwrap();

    assert!(report["graph_error"].is_string());
    assert_eq!(report["counts"]["unknown"], 1);
    assert_eq!(report["degraded"], true);
    assert_eq!(report["notification"], true);
}

#[test]
fn maintenance_health_degradation_remains_visible() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let (_temp, store, _) = fixture(now, "UTC");
    let declaration = clock_declaration(
        "clock-obligation",
        "UTC",
        "09:00",
        1,
        "2026-10-08T09:00:00Z",
    );
    store
        .add(kpop_native::maintenance_contract::compile(&declaration).unwrap()["spec"].clone())
        .unwrap();

    let report = store.scan(3).unwrap();

    assert_eq!(report["maintenance_health"]["degraded"], true);
    assert_eq!(report["degraded"], true);
    assert_eq!(report["notification"], true);
}

#[test]
fn intervention_items_precede_ready_rows_in_a_truncated_scan() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let (temp, store, workspace) = fixture(now.clone(), "UTC");
    store
        .add(ordinary("needs-user", "2026-10-06T11:00:00Z"))
        .unwrap();
    let initial = store.scan(20).unwrap()["items"][0].clone();
    let claim = store
        .claim(
            "needs-user",
            initial["occurrence"].as_str().unwrap(),
            "synthetic-checker",
            None,
        )
        .unwrap();
    let path = workspace.join("PROVENANCE.yaml");
    let changed = fs::read_to_string(&path).unwrap().replace("v: 1", "v: 2");
    fs::write(path, changed).unwrap();
    store
        .finish(
            "needs-user",
            claim["claim"]["token"].as_str().unwrap(),
            "done",
            "fixture://stale-input",
            None,
        )
        .unwrap();
    store
        .add(ordinary("interrupted", "2026-10-06T11:00:00Z"))
        .unwrap();
    let row = store.scan(20).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "interrupted")
        .unwrap()
        .clone();
    store
        .claim(
            "interrupted",
            row["occurrence"].as_str().unwrap(),
            "synthetic-checker",
            None,
        )
        .unwrap();
    store
        .add(ordinary("unknown", "2026-10-06T11:00:00Z"))
        .unwrap();
    let task = store.show("unknown").unwrap()["task"]
        .as_str()
        .unwrap()
        .to_owned();
    fs::remove_file(task).unwrap();
    for id in ["ready-a", "ready-b", "ready-c"] {
        store.add(ordinary(id, "2026-10-06T11:00:00Z")).unwrap();
    }

    let later = Store::at_in_state(
        &workspace,
        &temp.path().join("state"),
        now + chrono::Duration::minutes(31),
    )
    .unwrap();
    let report = later.scan(3).unwrap();

    assert_eq!(report["items"][0]["state"], "interrupted");
    assert_eq!(report["items"][1]["state"], "unknown");
    assert_eq!(report["items"][2]["state"], "needs_user");
    assert_eq!(report["items"][2]["id"], "needs-user");
    assert_eq!(report["counts"]["ready"], 3);
    assert_eq!(report["omitted"], 3);
}

#[test]
fn periodic_cadence_uses_claim_local_day_across_midnight() {
    let timezone = "Europe/Prague";
    let admitted = Utc.with_ymd_and_hms(2026, 10, 6, 21, 55, 0).unwrap();
    let finished = Utc.with_ymd_and_hms(2026, 10, 6, 22, 10, 0).unwrap();
    let (temp, store, _) = fixture(admitted, timezone);
    let declaration =
        clock_declaration("clock-check", timezone, "09:00", 1, "2026-10-06T07:00:00Z");
    store
        .add(kpop_native::maintenance_contract::compile(&declaration).unwrap()["spec"].clone())
        .unwrap();
    let row = store.scan(20).unwrap()["items"][0].clone();
    let claim = store
        .claim(
            "clock-check",
            row["occurrence"].as_str().unwrap(),
            "synthetic-checker",
            None,
        )
        .unwrap();
    assert_eq!(claim["claim"]["started_at"], "2026-10-06T21:55:00Z");
    let state = temp.path().join("state");
    let workspace = temp.path().join("project");
    let finish_store = Store::at_in_state(&workspace, &state, finished).unwrap();

    finish_store
        .finish(
            "clock-check",
            claim["claim"]["token"].as_str().unwrap(),
            "checked",
            "fixture://cross-midnight-check",
            Some("2026-10-07T07:00:00Z"),
        )
        .unwrap();

    assert_eq!(
        finish_store.show("clock-check").unwrap()["next_at"],
        "2026-10-07T07:00:00Z"
    );
}

#[test]
fn recurring_check_resolves_prague_dst_overlap_and_gap() {
    checked_clock_at(
        Utc.with_ymd_and_hms(2026, 10, 24, 10, 0, 0).unwrap(),
        "Europe/Prague",
        "2026-10-25T00:30:00Z",
    );
    checked_clock_at(
        Utc.with_ymd_and_hms(2026, 3, 28, 11, 0, 0).unwrap(),
        "Europe/Prague",
        "2026-03-29T01:00:00Z",
    );
}

#[test]
fn manual_current_use_release_preserves_periodic_cadence() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let (_temp, store, _) = fixture(now, "UTC");
    let declaration = json!({
        "schema":"kpopper.maintenance-declaration/v1",
        "kind":"source",
        "id":"source-check",
        "title":"Selected synthetic source check",
        "why":"Keep selected fixture evidence current.",
        "how":"Read the synthetic selected value.",
        "scope":"Inspect only; do not publish changes.",
        "related":["facts.count"],
        "cadence_days":7,
        "timezone":"UTC",
        "check_time":"09:00",
        "use_policy":"require_live",
        "evidence_requirement":"host_attested",
        "first_due_at":"2026-10-06T09:00:00Z",
        "source":{
            "source_id":"fixture-source",
            "locator":"https://example.test/synthetic-status",
            "publisher":"Synthetic fixture",
            "selection":"Selected synthetic status",
            "adapter":"fixture-reader/v1",
            "evidence_format":"selected text",
            "tool_policy":"existing authorized read tool",
            "allowed_roots":["https://example.test/"],
            "max_age_hours":24
        }
    });
    let spec = kpop_native::maintenance_contract::compile(&declaration).unwrap()["spec"].clone();
    store.add(spec).unwrap();
    let row = store.scan(20).unwrap()["items"][0].clone();
    let claim = store
        .claim(
            "source-check",
            row["occurrence"].as_str().unwrap(),
            "synthetic-checker",
            None,
        )
        .unwrap();
    store
        .inspect_maintenance(json!({
            "id":"source-check",
            "policy_digest":claim["spec"]["maintenance"]["policy_digest"],
            "source_ref":claim["spec"]["maintenance"]["source_ref"],
            "inspection":claim["spec"]["maintenance"]["inspection"],
            "inspected_at":claim["claim"]["started_at"],
            "value":"synthetic current value",
            "evidence":"fixture://selected-current-reading"
        }))
        .unwrap();
    store
        .finish(
            "source-check",
            claim["claim"]["token"].as_str().unwrap(),
            "checked",
            "fixture://periodic-check",
            Some("2026-10-13T09:00:00Z"),
        )
        .unwrap();
    let periodic_next = store.show("source-check").unwrap()["next_at"].clone();
    let waiting = store.scan(20).unwrap()["items"][0].clone();
    let manual = store
        .claim_for_use(
            "source-check",
            waiting["occurrence"].as_str().unwrap(),
            "synthetic-user",
            None,
            Some("fixture://current-use-authorization"),
        )
        .unwrap();
    store
        .finish(
            "source-check",
            manual["claim"]["token"].as_str().unwrap(),
            "released",
            "fixture://manual-use-complete",
            None,
        )
        .unwrap();

    assert_eq!(
        store.show("source-check").unwrap()["next_at"],
        periodic_next
    );
}
