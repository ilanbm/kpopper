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

use chrono::{TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store};
use std::fs;

fn setup() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Adoption fixture\n  updated: 2026-10-06\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    admit(&workspace);
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &temp.path().join("state"), now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    (temp, store)
}

#[test]
fn acknowledgement_is_not_consent_and_adoption_states_remain_distinct() {
    let (_temp, store) = setup();
    let proposed = followup_daily::status_with_maintenance(&store).unwrap();
    assert_eq!(proposed["adoption"]["state"], "proposed");
    assert_eq!(proposed["adoption"]["authorized"], false);

    let shown = followup_daily::record_adoption(&store, "shown", None).unwrap();
    assert_eq!(shown["state"], "shown");
    assert_eq!(shown["authorized"], false);

    let declined = followup_daily::record_adoption(&store, "declined", None).unwrap();
    assert_eq!(declined["state"], "declined");
    assert_eq!(declined["authorized"], false);

    let snoozed =
        followup_daily::record_adoption(&store, "snoozed", Some("2026-10-08T12:00:00Z")).unwrap();
    assert_eq!(snoozed["state"], "snoozed");

    let authorized =
        followup_daily::record_adoption(&store, "authorized", Some("explicit user approval"))
            .unwrap();
    assert_eq!(authorized["state"], "authorized_uninstalled");
    assert_eq!(authorized["authorized"], true);
    let status = followup_daily::status_with_maintenance(&store).unwrap();
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
        followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["state"],
        "configuration_unverified"
    );
    assert_eq!(
        followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["configuration"],
        "configuration_unverified"
    );
    let run = followup_daily::start_attested(&store, "observed-session", serde_json::json!({"schema":"kpopper.host-execution/v1","trigger":"scheduled","host":"fixture","id":"schedule-1","executed_at":"2026-10-06T12:00:00Z","observed_at":"2026-10-06T12:00:00Z","evidence":"fixture://actual-host-runtime-readback"})).unwrap();
    followup_daily::finish(
        &store,
        run["claim"]["token"].as_str().unwrap(),
        "completed host-attested scheduled execution",
    )
    .unwrap();
    assert_eq!(
        followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["state"],
        "scheduled_execution_reported"
    );
    followup_daily::record_adoption(&store, "declined", None).unwrap();
    let declined_with_binding = followup_daily::status_with_maintenance(&store).unwrap();
    assert_eq!(declined_with_binding["adoption"]["state"], "declined");
    assert_eq!(
        declined_with_binding["adoption"]["configuration"],
        "scheduled_execution_reported"
    );
    followup_daily::bind(
        &store,
        serde_json::json!({"host":"fixture","id":"schedule-1","state":"paused","evidence":"host readback"}),
    )
    .unwrap();
    assert_eq!(
        followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["state"],
        "declined"
    );
    assert_eq!(
        followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["configuration"],
        "paused"
    );
}
