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
        "meta:\n  name: Fairness fixture\n  updated: 2026-10-06\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    admit(path);
}

fn ordinary(id: &str) -> Value {
    json!({
        "id":id,
        "title":id,
        "why":"Keep this work current.",
        "how":"Inspect the recorded value.",
        "scope":"Read the recorded value and report; do not change it.",
        "related":["facts.count"],
        "when":{"at":"2026-10-06T12:00:00Z"}
    })
}

fn maintenance(id: &str, kind: &str) -> Value {
    let mut declaration = json!({
        "schema":"kpopper.maintenance-declaration/v1",
        "kind":kind,
        "id":id,
        "title":id,
        "why":"Maintain a declared obligation.",
        "how":"Perform only the selected local check.",
        "scope":"Read the selected input only; do not publish changes.",
        "related":["facts.count"],
        "cadence_days":1,
        "timezone":"UTC",
        "check_time":"09:00",
        "use_policy":if kind == "clock" {"require_live"} else {"allow_cached_until_expiry"},
        "evidence_requirement":if kind == "clock" {"trusted_clock"} else {"host_attested"}
    });
    if kind == "clock" {
        declaration["deadline"] = json!({"utc":"2026-10-06T12:00:00Z"});
    } else {
        declaration["first_due_at"] = json!("2026-10-06T12:00:00Z");
        declaration["source"] = json!({
            "source_id":"fixture-source",
            "locator":"https://example.test/status",
            "publisher":"Example",
            "selection":"Current status",
            "adapter":"host-read/v1",
            "evidence_format":"selected text",
            "tool_policy":"existing authorized read tool",
            "allowed_roots":["https://example.test/"],
            "max_age_hours":24
        });
    }
    kpop_native::maintenance_contract::compile(&declaration).unwrap()["spec"].clone()
}

fn claim_and_check(store: &Store, id: &str, token: &str, next_at: &str) {
    let row = store.scan(20).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap()
        .clone();
    let claimed = store
        .claim(
            id,
            row["occurrence"].as_str().unwrap(),
            "tester",
            Some(token),
        )
        .unwrap();
    store
        .finish(
            id,
            claimed["claim"]["token"].as_str().unwrap(),
            "checked",
            "admitted local check",
            Some(next_at),
        )
        .unwrap();
}

#[test]
fn never_served_and_oldest_served_tasks_share_three_daily_slots() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    record(&workspace);
    let state = temp.path().join("state");
    let first_day = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, first_day).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    store.add(ordinary("served-old")).unwrap();

    let first = followup_daily::start(&store, "first-session").unwrap();
    let first_token = first["claim"]["token"].as_str().unwrap();
    claim_and_check(&store, "served-old", first_token, "2026-10-10T12:00:00Z");
    followup_daily::finish(&store, first_token, "one admitted check").unwrap();

    let second_day = first_day + Duration::days(1);
    let second_store = Store::at_in_state(&workspace, &state, second_day).unwrap();
    second_store.add(ordinary("served-new")).unwrap();
    let second = followup_daily::start(&second_store, "second-session").unwrap();
    let second_token = second["claim"]["token"].as_str().unwrap();
    claim_and_check(
        &second_store,
        "served-new",
        second_token,
        "2026-10-08T12:00:00Z",
    );
    followup_daily::finish(&second_store, second_token, "one admitted check").unwrap();
    second_store
        .add(maintenance("clock-never", "clock"))
        .unwrap();
    second_store
        .add(maintenance("source-never", "source"))
        .unwrap();

    let return_day = Store::at_in_state(&workspace, &state, first_day + Duration::days(5)).unwrap();
    let limited = return_day.scan(1).unwrap();
    assert_eq!(limited["omitted"], 3);
    assert_eq!(limited["degraded"], true);
    let run = followup_daily::start(&return_day, "third-session").unwrap();
    let token = run["claim"]["token"].as_str().unwrap();
    let packet = run["packet"].clone();
    let ready = packet["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["state"] == "ready")
        .map(|row| row["id"].as_str().unwrap())
        .take(3)
        .collect::<Vec<_>>();

    assert_eq!(ready, vec!["clock-never", "source-never", "served-old"]);
    let recurring_clock = packet["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "served-old")
        .unwrap();
    assert_eq!(recurring_clock["maintenance_advisory"]["kind"], "clock");
    assert_eq!(packet["omitted"], 0);
    assert_eq!(packet["maintenance_omitted"], 0);

    let items = packet["items"].as_array().unwrap();
    assert!(
        return_day
            .claim(
                items[1]["id"].as_str().unwrap(),
                items[1]["occurrence"].as_str().unwrap(),
                "tester",
                Some(token),
            )
            .is_err()
    );
    for item in items.iter().take(3) {
        return_day
            .claim(
                item["id"].as_str().unwrap(),
                item["occurrence"].as_str().unwrap(),
                "tester",
                Some(token),
            )
            .unwrap();
    }
    let fourth = return_day.scan(20).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "served-new")
        .unwrap()
        .clone();
    assert!(
        return_day
            .claim(
                "served-new",
                fourth["occurrence"].as_str().unwrap(),
                "tester",
                Some(token),
            )
            .is_err()
    );
}

#[test]
fn no_policy_discovery_is_sourced_and_skips_one_off_or_historical_work() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    record(&workspace);
    let state = temp.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();

    let one_off = store.add(ordinary("one-off")).unwrap();
    let one_off_row = store.scan(20).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "one-off")
        .unwrap()
        .clone();
    let one_off_claim = store
        .claim(
            "one-off",
            one_off_row["occurrence"].as_str().unwrap(),
            "tester",
            None,
        )
        .unwrap();
    store
        .finish(
            "one-off",
            one_off_claim["claim"]["token"].as_str().unwrap(),
            "done",
            "one-off task completed",
            None,
        )
        .unwrap();

    let mut source = ordinary("recurring-source");
    source["when"] = json!({"external":{"ref":"explicit-feed","equals":"open"}});
    store
        .observe(json!({
            "ref":"explicit-feed",
            "value":"open",
            "observed_at":now.to_rfc3339(),
            "evidence":"existing source observation"
        }))
        .unwrap();
    store.add(source).unwrap();
    let source_row = store.scan(20).unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "recurring-source")
        .unwrap()
        .clone();
    let source_claim = store
        .claim(
            "recurring-source",
            source_row["occurrence"].as_str().unwrap(),
            "tester",
            None,
        )
        .unwrap();
    store
        .finish(
            "recurring-source",
            source_claim["claim"]["token"].as_str().unwrap(),
            "checked",
            "recurring source check",
            Some("2026-10-07T09:00:00Z"),
        )
        .unwrap();

    let report = store.scan(20).unwrap();
    let source_row = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "recurring-source")
        .unwrap();
    let historical_row = report["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "one-off")
        .unwrap();

    assert_eq!(source_row["maintenance_advisory"]["kind"], "source");
    assert_eq!(
        source_row["maintenance_advisory"]["source_refs"],
        json!(["explicit-feed"])
    );
    assert!(
        source_row["maintenance_advisory"]["unresolved"]
            .as_array()
            .unwrap()
            .contains(&json!("authorization"))
    );
    assert!(historical_row.get("maintenance_advisory").is_none());
    assert_eq!(store.load(true).unwrap().unwrap()["version"], 1);
    assert_eq!(one_off["spec"]["maintenance"], Value::Null);
}
