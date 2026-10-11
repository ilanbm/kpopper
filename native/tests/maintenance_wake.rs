use chrono::{Duration, TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store};
use serde_json::json;
use std::fs;
fn fixture(
    now: chrono::DateTime<Utc>,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    Store,
) {
    fixture_with(now, 7, "09:00", "2026-10-05T09:00:00Z")
}
fn fixture_with(
    now: chrono::DateTime<Utc>,
    cadence: u64,
    check_time: &str,
    due: &str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    Store,
) {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::write(work.join("PROVENANCE.yaml"),"meta:\n  name: Wake fixture\nknown:\n  deadline.value:\n    name: Selected deadline\n    v: 1\n").unwrap();
    let state = temp.path().join("state");
    let store = Store::at_in_state(&work, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let d = json!({"schema":"kpopper.maintenance-declaration/v1","kind":"clock","id":"clock-check","title":"Clock check","why":"Check declared time","how":"Read native clock","scope":"Read the declared clock; no publication","related":["deadline.value"],"cadence_days":cadence,"timezone":"UTC","check_time":check_time,"use_policy":"require_live","evidence_requirement":"trusted_clock","deadline":{"utc":due}});
    store
        .add(kpop_native::maintenance_contract::compile(&d).unwrap()["spec"].clone())
        .unwrap();
    (temp, work, state, store)
}
#[test]
fn local_pause_is_truthful_without_fabricating_host_readback_and_manual_is_explicit() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 9, 0, 0).unwrap();
    let (_t, _w, _s, store) = fixture(now);
    let result =
        followup_daily::maintenance_mode(&store, "paused", "fixture://user-pause", false, None)
            .unwrap();
    assert_eq!(result["host_mutation"], "none");
    assert_eq!(result["wake"]["state"], "paused_locally");
    let data = store.load(true).unwrap().unwrap();
    assert!(data["daily"]["binding"].is_null());
    assert_eq!(data["version"], 2);
    assert!(followup_daily::start(&store, "background").is_err());
    followup_daily::maintenance_mode(&store, "manual", "fixture://user-manual", false, None)
        .unwrap();
    assert!(followup_daily::start(&store, "background").is_err());
    assert!(
        followup_daily::start_manual(&store, "current-user", "fixture://current-user-task").is_ok()
    );
}
#[test]
fn late_tuesday_catchup_exposes_fixed_monday_phase_incompatibility() {
    let monday = Utc.with_ymd_and_hms(2026, 10, 5, 9, 0, 0).unwrap();
    let (_t, work, state, store) = fixture(monday);
    followup_daily::bind(&store,json!({"host":"fixture","id":"native-7","state":"active","evidence":"fixture://host-readback"})).unwrap();
    let descriptor = |time: chrono::DateTime<Utc>| json!({"host":"fixture","id":"native-7","cadence_days":7,"anchor_at":monday.to_rfc3339(),"timezone":"UTC","evidence":"fixture://actual-normalized-readback","observed_at":time.to_rfc3339(),"interval_semantics":"calendar_days"});
    followup_daily::maintenance_mode(
        &store,
        "native",
        "fixture://user-native-7",
        false,
        Some(descriptor(monday)),
    )
    .unwrap();
    assert_eq!(
        followup_daily::status(&store).unwrap()["wake"]["state"],
        "compatible"
    );
    let tuesday = monday + Duration::days(1);
    let late = Store::at_in_state(&work, &state, tuesday).unwrap();
    followup_daily::maintenance_mode(
        &late,
        "native",
        "fixture://standing-user-native-7",
        false,
        Some(descriptor(tuesday)),
    )
    .unwrap();
    let daily = followup_daily::start(&late, "catchup-host").unwrap();
    let token = daily["claim"]["token"].as_str().unwrap();
    let row = late.scan(20).unwrap()["items"][0].clone();
    let claimed = late
        .claim(
            "clock-check",
            row["occurrence"].as_str().unwrap(),
            "catchup-host",
            Some(token),
        )
        .unwrap();
    let next = (tuesday + Duration::days(7)).to_rfc3339();
    late.finish(
        "clock-check",
        claimed["claim"]["token"].as_str().unwrap(),
        "checked",
        "fixture://current-clock",
        Some(&next),
    )
    .unwrap();
    followup_daily::finish(&late, token, "fixture://catchup-complete").unwrap();
    let status = followup_daily::status(&late).unwrap();
    assert_eq!(status["wake"]["state"], "incompatible");
    assert_eq!(status["maintenance_health"]["degraded"], true);
    assert_eq!(
        late.load(true).unwrap().unwrap()["daily"]["binding"]["id"],
        "native-7"
    );
    assert!(
        followup_daily::maintenance_mode(
            &late,
            "daily_fallback",
            "fixture://user-fallback",
            false,
            None
        )
        .is_err()
    );
    followup_daily::maintenance_mode(
        &late,
        "daily_fallback",
        "fixture://user-fallback-cost",
        true,
        None,
    )
    .unwrap();
    // Selection records intent; no host schedule has been changed and incompatibility stays visible.
    assert_ne!(
        followup_daily::status(&late).unwrap()["wake"]["state"],
        "compatible"
    );
}

fn bind_daily(store: &Store, time: Option<&str>, zone: Option<&str>) {
    followup_daily::bind(store,json!({"host":"fixture","id":"daily-owner","state":"active","evidence":"fixture://host-readback"})).unwrap();
    let mut data = store.load(true).unwrap().unwrap();
    data["daily"]["binding"]["cadence"] = json!("daily");
    data["daily"]["binding"]["time"] = json!(time);
    data["daily"]["binding"]["timezone"] = json!(zone);
    fs::write(&store.path, serde_json::to_vec(&data).unwrap()).unwrap();
}
#[test]
fn daily_before_recurring_check_time_cannot_claim_calendar_continuity() {
    let tuesday = Utc.with_ymd_and_hms(2026, 10, 6, 9, 0, 0).unwrap();
    let (_temp, work, state, store) = fixture_with(tuesday, 1, "12:00", "2026-10-05T12:00:00Z");
    bind_daily(&store, Some("09:00"), Some("UTC"));
    assert_eq!(
        followup_daily::status(&store).unwrap()["wake"]["state"],
        "incompatible"
    );
    // Catching up an overdue first check does not repair the next recurring phase.
    let daily =
        followup_daily::start_manual(&store, "manual-catchup", "fixture://user-manual").unwrap();
    let token = daily["claim"]["token"].as_str().unwrap();
    let row = store.scan(20).unwrap()["items"][0].clone();
    let claimed = store
        .claim(
            "clock-check",
            row["occurrence"].as_str().unwrap(),
            "manual-catchup",
            Some(token),
        )
        .unwrap();
    store
        .finish(
            "clock-check",
            claimed["claim"]["token"].as_str().unwrap(),
            "checked",
            "fixture://current-clock",
            Some("2026-10-07T12:00:00Z"),
        )
        .unwrap();
    followup_daily::finish(&store, token, "fixture://catchup-complete").unwrap();
    let wednesday = Store::at_in_state(&work, &state, tuesday + Duration::days(1)).unwrap();
    assert_eq!(
        wednesday.scan(20).unwrap()["items"][0]["state"],
        "waiting"
    );
    assert_eq!(
        followup_daily::status(&wednesday).unwrap()["wake"]["state"],
        "incompatible"
    );
    assert_eq!(
        wednesday.load(true).unwrap().unwrap()["daily"]["binding"]["time"],
        "09:00"
    );
}
#[test]
fn daily_phase_proof_uses_recurring_time_without_rewriting_initial_due_or_ordinary_work() {
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 9, 0, 0).unwrap();
    for (time, zone, expected) in [
        (Some("12:00"), Some("UTC"), "compatible"),
        (Some("15:00"), Some("UTC"), "compatible"),
        (None, Some("UTC"), "unknown"),
        (Some("15:00"), Some("Europe/Prague"), "unknown"),
    ] {
        // Initial due can be later than the recurring time: it is caught up once.
        let (_temp, _work, _state, store) = fixture_with(now, 1, "12:00", "2026-10-06T18:00:00Z");
        bind_daily(&store, time, zone);
        assert_eq!(
            followup_daily::status(&store).unwrap()["wake"]["state"],
            expected
        );
        assert_eq!(
            store.load(true).unwrap().unwrap()["items"]["clock-check"]["spec"]["maintenance"]["due_at"],
            "2026-10-06T18:00:00Z"
        );
    }
    let (_temp, _work, _state, store) = fixture(now);
    let mut data = store.load(true).unwrap().unwrap();
    data["items"] = json!({});
    fs::write(&store.path, serde_json::to_vec(&data).unwrap()).unwrap();
    bind_daily(&store, None, None);
    assert_eq!(
        followup_daily::status(&store).unwrap()["wake"]["state"],
        "compatible"
    );
}
