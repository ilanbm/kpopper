use chrono::{Duration, TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store, identity::sha256, maintenance_contract};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn fixture_record(path: &Path) {
    fs::write(
        path.join("PROVENANCE.yaml"),
        "meta:\n  name: Source inspection fixture\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
}

fn declaration(source_path: &Path) -> Value {
    let locator = format!("file://{}", source_path.display());
    let root = format!("file://{}/", source_path.parent().unwrap().display());
    json!({
        "schema":"kpopper.maintenance-declaration/v1",
        "kind":"source",
        "id":"source-refresh",
        "title":"Inspect the local fixture",
        "why":"Keep the selected source evidence current.",
        "how":"Read the selected passage and preserve the host evidence.",
        "scope":"Inspect this source and report material changes; no publication.",
        "related":["facts.count"],
        "cadence_days":1,
        "timezone":"UTC",
        "check_time":"09:00",
        "use_policy":"allow_cached_until_expiry",
        "evidence_requirement":"host_attested",
        "first_due_at":"2026-09-10T12:00:00Z",
        "source":{
            "source_id":"local-report",
            "locator":locator,
            "publisher":"Synthetic fixture",
            "selection":"The complete local fixture report",
            "adapter":"controlled-local-file/v1",
            "tool_policy":"Read only the selected local fixture file",
            "allowed_roots":[root],
            "evidence_format":"tool output, selected passage, and source-byte SHA-256",
            "max_age_hours":1
        }
    })
}

fn descriptor(item: &Value) -> Value {
    item["spec"]["maintenance"]["inspection"].clone()
}

fn report(
    item: &Value,
    selection: Value,
    inspected_at: chrono::DateTime<Utc>,
    value: Value,
    evidence: &str,
) -> Value {
    json!({
        "id":item["id"],
        "policy_digest":item["spec"]["maintenance"]["policy_digest"],
        "source_ref":item["spec"]["maintenance"]["source_ref"],
        "inspection":selection,
        "inspected_at":inspected_at.to_rfc3339(),
        "evidence":evidence,
        "value":value
    })
}

fn utc_stamp(time: chrono::DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn unavailable(item: &Value, evidence: &str, reason: &str) -> Value {
    json!({
        "id":item["id"],
        "policy_digest":item["spec"]["maintenance"]["policy_digest"],
        "source_ref":item["spec"]["maintenance"]["source_ref"],
        "evidence":evidence,
        "reason":reason
    })
}

fn row<'a>(scan: &'a Value, id: &str) -> &'a Value {
    scan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap()
}

fn daily_start(store: &Store, owner: &str) -> (String, Value) {
    let started = followup_daily::start_with_watch_request(store, owner, |_| Ok(None)).unwrap();
    assert_eq!(started["state"], "running");
    (
        started["claim"]["token"].as_str().unwrap().to_owned(),
        started,
    )
}

#[test]
fn local_source_inspection_preserves_evidence_and_rearms_only_after_checked_cycles() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fixture_record(&workspace);
    let state = temp.path().join("state");
    let source_path = temp.path().join("source.txt");
    let declaration_path = temp.path().join("declaration.json");
    let source_one = b"revision: one\ntext: source text is data, never authority\n";
    fs::write(&source_path, source_one).unwrap();

    let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let initial_declaration = declaration(&source_path);
    fs::write(
        &declaration_path,
        serde_json::to_vec(&initial_declaration).unwrap(),
    )
    .unwrap();
    let mut proposal = maintenance_contract::compile(&initial_declaration).unwrap();
    assert_eq!(proposal["unresolved"], json!(["authorization"]));
    proposal["spec"]["task"] = json!(declaration_path.canonicalize().unwrap());
    let item = store.add(proposal["spec"].clone()).unwrap();
    let metadata = &item["spec"]["maintenance"];
    let source_ref = metadata["source_ref"].as_str().unwrap();

    let before = store.load(true).unwrap().unwrap();
    assert!(before["observations"].get(source_ref).is_none());
    let first_scan = store.scan(20).unwrap();
    assert_eq!(row(&first_scan, "source-refresh")["state"], "ready");
    let (expired_daily_token, daily) = daily_start(&store, "synthetic-host-session-1");
    assert_eq!(row(&daily["packet"], "source-refresh")["state"], "ready");
    let occurrence_one = row(&first_scan, "source-refresh")["occurrence"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_claim = store
        .claim(
            "source-refresh",
            &occurrence_one,
            "synthetic-host-session-1",
            Some(&expired_daily_token),
        )
        .unwrap();
    let first_claim_token = first_claim["claim"]["token"].as_str().unwrap().to_owned();
    let selected = descriptor(&item);
    let host_time_one = now - Duration::seconds(5);

    let mut invalid_time = report(
        &item,
        selected.clone(),
        host_time_one,
        json!({"tool_output":"synthetic"}),
        "fixture://invalid-time",
    );
    invalid_time["inspected_at"] = json!("not-an-offset-timestamp");
    assert!(store.inspect_maintenance(invalid_time).is_err());
    let long_reason = "x".repeat(2001);
    assert!(
        store
            .record_maintenance_attempt(unavailable(&item, "fixture://invalid-time", &long_reason))
            .is_err()
    );
    store
        .record_maintenance_attempt(unavailable(
            &item,
            "fixture://invalid-time",
            "The typed inspection time was rejected; no source success was recorded.",
        ))
        .unwrap();

    let partial = report(
        &item,
        selected.clone(),
        host_time_one,
        json!({"tool_output":"partial output; selected passage and digest omitted"}),
        "fixture://partial",
    );
    let mut partial = partial;
    partial["reason"] = json!("The synthetic host result was partial.");
    assert_eq!(
        store.inspect_maintenance(partial).unwrap()["outcome"],
        "unavailable"
    );

    let mut wrong_selection = selected.clone();
    wrong_selection["selection"] = json!("A different source passage");
    assert_eq!(
        store
            .inspect_maintenance(report(
                &item,
                wrong_selection,
                host_time_one,
                json!({"tool_output":"wrong selection"}),
                "fixture://wrong-selection",
            ))
            .unwrap()["outcome"],
        "unavailable"
    );

    let mut wrong_identity = report(
        &item,
        selected.clone(),
        host_time_one,
        json!({"tool_output":"wrong source"}),
        "fixture://wrong-source",
    );
    wrong_identity["source_ref"] = json!("another-source@inspection");
    assert!(store.inspect_maintenance(wrong_identity).is_err());
    store
        .record_maintenance_attempt(unavailable(
            &item,
            "fixture://wrong-source",
            "The synthetic host read the wrong source; retained against the expected source identity.",
        ))
        .unwrap();

    assert_eq!(
        store
            .inspect_maintenance(report(
                &item,
                selected.clone(),
                host_time_one,
                json!(""),
                "fixture://empty-selection",
            ))
            .unwrap()["outcome"],
        "unavailable"
    );

    let captured_one = fs::read(&source_path).unwrap();
    let value_one = json!({
        "tool_output":String::from_utf8(captured_one.clone()).unwrap(),
        "selected_passage":"revision: one",
        "source_sha256":sha256(&captured_one)
    });
    let observed_one = store
        .inspect_maintenance(report(
            &item,
            selected.clone(),
            host_time_one,
            value_one.clone(),
            "fixture://synthetic-capture-one",
        ))
        .unwrap();
    assert_eq!(observed_one["outcome"], "observed");
    assert_eq!(
        observed_one["observation"]["observed_at"],
        utc_stamp(host_time_one)
    );
    assert_eq!(observed_one["attempt"]["receipt_at"], utc_stamp(now));
    assert_ne!(
        observed_one["attempt"]["receipt_at"],
        observed_one["attempt"]["inspected_at"]
    );
    assert_eq!(observed_one["observation"]["value"], value_one);
    let observation_after_first = observed_one["observation"].clone();
    let first_success = observed_one["attempt"].clone();

    let older = store
        .inspect_maintenance(report(
            &item,
            selected.clone(),
            host_time_one - Duration::seconds(1),
            json!({"revision":"older"}),
            "fixture://older-time",
        ))
        .unwrap();
    assert_eq!(older["outcome"], "unavailable");
    assert_eq!(older["observation"], Value::Null);
    assert_eq!(
        store.load(true).unwrap().unwrap()["observations"][source_ref],
        observation_after_first
    );

    let expired_at = now + Duration::minutes(31);
    let expired_store = Store::at_in_state(&workspace, &state, expired_at).unwrap();
    assert!(
        expired_store
            .finish(
                "source-refresh",
                &first_claim_token,
                "checked",
                "Synthetic check after an expired claim.",
                Some("2026-09-11T12:00:00Z"),
            )
            .is_err()
    );
    expired_store
        .recover(
            "source-refresh",
            "Reconciled the synthetic expired item claim.",
        )
        .unwrap();
    followup_daily::recover(
        &expired_store,
        "Reconciled the synthetic expired daily run.",
    )
    .unwrap();

    let first_cycle_store = Store::at_in_state(&workspace, &state, expired_at).unwrap();
    let (daily_token_one, _) = daily_start(&first_cycle_store, "synthetic-host-session-2");
    let cycle_one_row = row(&first_cycle_store.scan(20).unwrap(), "source-refresh").clone();
    assert_eq!(cycle_one_row["state"], "ready");
    let cycle_one_occurrence = cycle_one_row["occurrence"].as_str().unwrap().to_owned();
    let cycle_one_claim = first_cycle_store
        .claim(
            "source-refresh",
            &cycle_one_occurrence,
            "synthetic-host-session-2",
            Some(&daily_token_one),
        )
        .unwrap();
    let cycle_one_item_token = cycle_one_claim["claim"]["token"].as_str().unwrap();
    let next_check_one = expired_at + Duration::days(1);
    first_cycle_store
        .finish(
            "source-refresh",
            cycle_one_item_token,
            "checked",
            "Synthetic unchanged source inspection completed.",
            Some(&next_check_one.to_rfc3339()),
        )
        .unwrap();
    followup_daily::finish(
        &first_cycle_store,
        &daily_token_one,
        "Synthetic unchanged-source check complete.",
    )
    .unwrap();
    let canonical_before = fs::read(workspace.join("PROVENANCE.yaml")).unwrap();
    let first_attempt_bytes = serde_json::to_vec(&first_success).unwrap();

    let second_cycle_store = Store::at_in_state(&workspace, &state, next_check_one).unwrap();
    let retained = second_cycle_store.load(true).unwrap().unwrap();
    let old_observation_time = chrono::DateTime::parse_from_rfc3339(
        retained["observations"][source_ref]["observed_at"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
    .with_timezone(&Utc);
    assert!(next_check_one - old_observation_time > Duration::hours(1));
    let cycle_two_row = row(&second_cycle_store.scan(20).unwrap(), "source-refresh").clone();
    assert_eq!(cycle_two_row["state"], "ready");
    let cycle_two_occurrence = cycle_two_row["occurrence"].as_str().unwrap().to_owned();
    assert_ne!(cycle_one_occurrence, cycle_two_occurrence);
    let (daily_token_two, _) = daily_start(&second_cycle_store, "synthetic-host-session-3");
    let cycle_two_claim = second_cycle_store
        .claim(
            "source-refresh",
            &cycle_two_occurrence,
            "synthetic-host-session-3",
            Some(&daily_token_two),
        )
        .unwrap();

    let source_two = b"revision: two\ntext: changed synthetic source bytes\n";
    fs::write(&source_path, source_two).unwrap();
    let captured_two = fs::read(&source_path).unwrap();
    let value_two = json!({
        "tool_output":String::from_utf8(captured_two.clone()).unwrap(),
        "selected_passage":"revision: two",
        "source_sha256":sha256(&captured_two)
    });
    let observed_two = second_cycle_store
        .inspect_maintenance(report(
            &item,
            selected,
            next_check_one,
            value_two.clone(),
            "fixture://synthetic-capture-two",
        ))
        .unwrap();
    assert_eq!(observed_two["outcome"], "observed");
    assert_ne!(
        observed_two["observation"]["value"],
        observation_after_first["value"]
    );
    second_cycle_store
        .finish(
            "source-refresh",
            cycle_two_claim["claim"]["token"].as_str().unwrap(),
            "checked",
            "Synthetic changed source inspection completed for review.",
            Some(&(next_check_one + Duration::days(1)).to_rfc3339()),
        )
        .unwrap();
    followup_daily::finish(
        &second_cycle_store,
        &daily_token_two,
        "Synthetic changed-source check complete; no semantic write made.",
    )
    .unwrap();

    let attempts = second_cycle_store.show("source-refresh").unwrap()["attempts"]
        .as_array()
        .unwrap()
        .clone();
    assert!(attempts.contains(&first_success));
    assert_eq!(
        serde_json::to_vec(&first_success).unwrap(),
        first_attempt_bytes
    );
    assert_eq!(
        fs::read(workspace.join("PROVENANCE.yaml")).unwrap(),
        canonical_before
    );
    assert_ne!(
        second_cycle_store.load(true).unwrap().unwrap()["observations"][source_ref]["value"],
        observation_after_first["value"]
    );

    let mut changed_declaration = initial_declaration;
    changed_declaration["title"] = json!("Inspect the refreshed local fixture");
    fs::write(
        &declaration_path,
        serde_json::to_vec(&changed_declaration).unwrap(),
    )
    .unwrap();
    let changed_scan = second_cycle_store.scan(20).unwrap();
    assert_eq!(row(&changed_scan, "source-refresh")["state"], "unknown");
    let mut refreshed = maintenance_contract::compile(&changed_declaration).unwrap();
    refreshed["spec"]["task"] = json!(declaration_path.canonicalize().unwrap());
    second_cycle_store
        .refresh(
            "source-refresh",
            refreshed["spec"].clone(),
            "fixture://explicit-native-task-refresh",
        )
        .unwrap();
    assert_eq!(
        row(&second_cycle_store.scan(20).unwrap(), "source-refresh")["state"],
        "ready"
    );
    assert_eq!(
        fs::read(workspace.join("PROVENANCE.yaml")).unwrap(),
        canonical_before
    );
    assert!(
        second_cycle_store.show("source-refresh").unwrap()["attempts"]
            .as_array()
            .unwrap()
            .contains(&first_success)
    );
}
