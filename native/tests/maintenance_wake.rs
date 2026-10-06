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
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::write(work.join("PROVENANCE.yaml"),"meta:\n  name: Wake fixture\nknown:\n  deadline.value:\n    name: Selected deadline\n    v: 1\n").unwrap();
    let state = temp.path().join("state");
    let store = Store::at_in_state(&work, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let d = json!({"schema":"kpopper.maintenance-declaration/v1","kind":"clock","id":"clock-check","title":"Clock check","why":"Check declared time","how":"Read native clock","scope":"Read the declared clock; no publication","related":["deadline.value"],"cadence_days":7,"timezone":"UTC","check_time":"09:00","use_policy":"require_live","evidence_requirement":"trusted_clock","deadline":{"utc":"2026-10-05T09:00:00Z"}});
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
