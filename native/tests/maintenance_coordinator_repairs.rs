use chrono::{Duration, TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store, maintenance_contract};
use serde_json::json;
use std::fs;
fn fixture(now: chrono::DateTime<Utc>) -> (tempfile::TempDir, Store) {
    let t = tempfile::tempdir().unwrap();
    let w = t.path().join("work");
    fs::create_dir(&w).unwrap();
    fs::write(
        w.join("GROUNDING.yaml"),
        "meta:\n  name: Coordinator repair\nknown:\n  facts.count:\n    v: 1\n",
    )
    .unwrap();
    let store = Store::at_in_state(&w, &t.path().join("state"), now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    (t, store)
}
fn clock(store: &Store) {
    let d = json!({"schema":"kpopper.maintenance-declaration/v1","kind":"clock","id":"clock-check","title":"Declared clock","why":"Keep clock continuity","how":"Read native time","scope":"Read only","related":["facts.count"],"cadence_days":7,"timezone":"UTC","check_time":"09:00","use_policy":"require_live","evidence_requirement":"trusted_clock","deadline":{"utc":"2026-10-05T09:00:00Z"}});
    store
        .add(maintenance_contract::compile(&d).unwrap()["spec"].clone())
        .unwrap();
}
fn binding(store: &Store) {
    followup_daily::bind(store,json!({"host":"fixture","id":"job","state":"active","evidence":"fixture://actual-owner-readback"})).unwrap();
}
fn native(store: &Store, observed: chrono::DateTime<Utc>) {
    followup_daily::maintenance_mode(store,"native","fixture://native-authorization",false,Some(json!({"host":"fixture","id":"job","cadence_days":7,"anchor_at":"2026-10-05T09:00:00Z","timezone":"UTC","evidence":"fixture://phase-readback","observed_at":observed.to_rfc3339(),"interval_semantics":"calendar_days"}))).unwrap();
}
#[test]
fn stale_native_phase_and_ordinary_items_do_not_block_received_catchup() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 10, 0, 0).unwrap();
    let (_t, store) = fixture(now);
    clock(&store);
    binding(&store);
    native(&store, now - Duration::hours(25));
    assert_eq!(
        followup_daily::status(&store).unwrap()["wake"]["state"],
        "unknown"
    );
    let run = followup_daily::start(&store, "actual-received-wake")
        .expect("Unknown phase must remain explicit without preventing due work");
    assert_eq!(run["claim"]["execution_origin"], "unattested");
    followup_daily::finish(
        &store,
        run["claim"]["token"].as_str().unwrap(),
        "fixture://local-work-completed",
    )
    .unwrap();
    native(&store, now);
    store.add(json!({"id":"future-task","title":"Future ordinary work","why":"Future work","how":"Read","scope":"Read only","related":["facts.count"],"when":{"at":"2027-01-01T00:00:00Z"}})).unwrap();
    assert_eq!(
        followup_daily::status(&store).unwrap()["wake"]["state"],
        "unknown"
    );
}
#[test]
fn routine_shown_choice_keeps_zero_maintenance_ledger_v1() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 10, 0, 0).unwrap();
    let (_t, store) = fixture(now);
    assert_eq!(store.load(true).unwrap().unwrap()["version"], 1);
    followup_daily::record_adoption(&store, "shown", None).unwrap();
    assert_eq!(store.load(true).unwrap().unwrap()["version"], 1);
    assert_eq!(
        followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["choice"],
        "shown"
    );
}
#[test]
fn current_report_is_self_reported_and_survives_same_owner_readback() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 10, 0, 0).unwrap();
    let (t, store) = fixture(now);
    binding(&store);
    let later = Store::at_in_state(
        t.path().join("work").as_path(),
        &t.path().join("state"),
        now + Duration::hours(25),
    )
    .unwrap();
    let executed = now + Duration::hours(25);
    let run=followup_daily::start_attested(&later,"actual-runtime",json!({"schema":"kpopper.host-execution/v1","trigger":"scheduled","host":"fixture","id":"job","executed_at":executed.to_rfc3339(),"observed_at":executed.to_rfc3339(),"evidence":"fixture://host-reported-current-invocation"})).unwrap();
    followup_daily::finish(
        &later,
        run["claim"]["token"].as_str().unwrap(),
        "fixture://native-completion",
    )
    .unwrap();
    assert_eq!(
        followup_daily::status_with_maintenance(&later).unwrap()["adoption"]["configuration"],
        "scheduled_execution_reported"
    );
    binding(&later);
    assert_eq!(
        followup_daily::status_with_maintenance(&later).unwrap()["adoption"]["configuration"],
        "scheduled_execution_reported"
    );
    let paused = followup_daily::bind(
        &later,
        json!({"host":"fixture","id":"job","state":"paused","evidence":"fixture://pause-readback"}),
    )
    .unwrap();
    assert_eq!(paused["state"], "paused");
    binding(&later);
    assert_eq!(
        followup_daily::status_with_maintenance(&later).unwrap()["adoption"]["configuration"],
        "configuration_unverified"
    );
}
#[test]
fn authorized_closed_obligation_is_informational_and_not_degraded() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 10, 0, 0).unwrap();
    let (_t, store) = fixture(now);
    clock(&store);
    store
        .resolve_with_authority(
            "clock-check",
            "cancelled",
            "fixture://cancelled-scope",
            Some("fixture://user-cancel-grant"),
        )
        .unwrap();
    assert_eq!(
        followup_daily::status(&store).unwrap()["maintenance_health"]["degraded"],
        false
    );
    assert_eq!(store.scan(20).unwrap()["notification"], false);
}

#[test]
fn actual_checked_clock_uses_semantic_timestamp_identity_after_rearm() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 10, 0, 0).unwrap();
    let (_t, store) = fixture(now);
    clock(&store);
    let daily = followup_daily::start(&store, "clock-reader").unwrap();
    let daily_token = daily["claim"]["token"].as_str().unwrap();
    let row = store.scan(20).unwrap()["items"][0].clone();
    let claim = store
        .claim(
            "clock-check",
            row["occurrence"].as_str().unwrap(),
            "clock-reader",
            Some(daily_token),
        )
        .unwrap();
    store
        .finish(
            "clock-check",
            claim["claim"]["token"].as_str().unwrap(),
            "checked",
            "fixture://actual-native-clock",
            Some("2026-10-13T09:00:00+00:00"),
        )
        .unwrap();
    followup_daily::finish(&store, daily_token, "fixture://clock-daily-completion").unwrap();
    assert_eq!(
        store.assess_use(&["facts.count".into()], &[]).unwrap()["status"],
        "adequate"
    );
}
