use chrono::{TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store};
use std::fs;

fn setup() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Adoption fixture\n  updated: 2026-10-06\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &temp.path().join("state"), now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    (temp, store)
}

#[test]
fn acknowledgement_is_not_consent_and_adoption_states_remain_distinct() {
    let (_temp, store) = setup();
    let proposed = followup_daily::status(&store).unwrap();
    assert_eq!(proposed["adoption"]["state"], "proposed");
    assert_eq!(proposed["adoption"]["authorized"], false);

    let shown = followup_daily::record_adoption(&store, "shown", None).unwrap();
    assert_eq!(shown["state"], "shown");
    assert_eq!(shown["authorized"], false);

    let declined = followup_daily::record_adoption(&store, "declined", None).unwrap();
    assert_eq!(declined["state"], "declined");
    assert_eq!(declined["authorized"], false);

    let snoozed = followup_daily::record_adoption(
        &store,
        "snoozed",
        Some("2026-10-08T12:00:00Z"),
    )
    .unwrap();
    assert_eq!(snoozed["state"], "snoozed");

    let authorized = followup_daily::record_adoption(&store, "authorized", Some("explicit user approval"))
        .unwrap();
    assert_eq!(authorized["state"], "authorized_uninstalled");
    assert_eq!(authorized["authorized"], true);
    let status = followup_daily::status(&store).unwrap();
    assert_eq!(status["state"], "proposed");
    assert_eq!(status["adoption"]["state"], "authorized_uninstalled");
    assert_eq!(status["adoption"]["acknowledged"], true);
    let acknowledged_again = followup_daily::record_adoption(&store, "shown", None).unwrap();
    assert_eq!(acknowledged_again["state"], "authorized_uninstalled");
    assert_eq!(acknowledged_again["authorized"], true);

    followup_daily::bind(
        &store,
        serde_json::json!({"host":"fixture","id":"schedule-1","state":"active","evidence":"host readback"}),
    )
    .unwrap();
    assert_eq!(
        followup_daily::status(&store).unwrap()["adoption"]["state"],
        "configuration_unverified"
    );
    assert_eq!(
        followup_daily::status(&store).unwrap()["adoption"]["configuration"],
        "configuration_unverified"
    );
    let run = followup_daily::start(&store, "observed-session").unwrap();
    followup_daily::finish(
        &store,
        run["claim"]["token"].as_str().unwrap(),
        "observed local daily execution",
    )
    .unwrap();
    assert_eq!(
        followup_daily::status(&store).unwrap()["adoption"]["state"],
        "active_observed"
    );
    followup_daily::record_adoption(&store, "declined", None).unwrap();
    let declined_with_binding = followup_daily::status(&store).unwrap();
    assert_eq!(declined_with_binding["adoption"]["state"], "declined");
    assert_eq!(declined_with_binding["adoption"]["configuration"], "active_observed");
    followup_daily::bind(
        &store,
        serde_json::json!({"host":"fixture","id":"schedule-1","state":"paused","evidence":"host readback"}),
    )
    .unwrap();
    assert_eq!(
        followup_daily::status(&store).unwrap()["adoption"]["state"],
        "declined"
    );
    assert_eq!(followup_daily::status(&store).unwrap()["adoption"]["configuration"], "paused");
}
