use chrono::{Duration, TimeZone, Utc};
use kpop_native::followup_store::{Store, parse_input};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
    thread,
};

fn record(path: &Path, count: i64) {
    fs::write(
        path.join("PROVENANCE.yaml"),
        format!(
            "meta:\n  name: Followup fixture\n  updated: 2026-09-10\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown:\n  facts.count:\n    name: Count\n    v: {count}\n  facts.other:\n    name: Other count\n    v: 10\njudgments:\n  c.acceptable:\n    rests_on: [facts.count]\n    verdict: Count is acceptable.\n    wrong_if: facts.count < 0\n    seen: {{facts.count: 1}}\n"
        ),
    )
    .unwrap();
}

fn spec(when: Value) -> Value {
    json!({
        "id":"review","title":"Review the count","why":"Keep the decision current.",
        "how":"Inspect the count and its evidence.","scope":"Read the count and report findings.",
        "related":["facts.count"],"when":when,
    })
}

fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    chrono::DateTime<Utc>,
) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    record(&workspace, 1);
    let state = temp.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "Asia/Jerusalem", None, true).unwrap();
    (temp, workspace, state, now)
}

#[test]
fn maintenance_metadata_is_consumed_by_the_local_followup_add_path() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let added = store.add(proposal["spec"].clone()).unwrap();

    assert!(
        added["spec"]["maintenance"]["consent_digest"]
            .as_str()
            .is_some()
    );
    assert_eq!(
        store.show("source-refresh").unwrap()["spec"]["maintenance"]["cadence_days"],
        3
    );
    assert!(store.add(added["spec"].clone()).is_err());
}

fn source_declaration() -> Value {
    json!({
        "schema":"kpopper.maintenance-declaration/v1",
        "kind":"source",
        "id":"source-refresh",
        "title":"Refresh the report",
        "why":"Keep the source evidence current.",
        "how":"Inspect the selected report and retain its result.",
        "scope":"Inspect the report and report material changes.",
        "related":["facts.count"],
        "cadence_days":3,
        "timezone":"Europe/Prague", "check_time":"09:00", "use_policy":"allow_cached_until_expiry", "evidence_requirement":"host_attested",
        "first_due_at":"2026-09-10T12:00:00Z",
        "source":{
            "source_id":"quarterly-report",
            "locator":"https://example.test/report",
            "publisher":"Example publisher",
            "selection":"Latest issued report",
            "adapter":"host-read/v1",
            "evidence_format":"text", "tool_policy":"read-only selected host source tool", "allowed_roots":["https://example.test/"],
            "max_age_hours":72.0
        }
    })
}

#[test]
fn maintenance_compiler_proposes_local_inspection_without_copying_authority() {
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let proposed = &proposal["spec"];
    assert_eq!(proposal["unresolved"], json!(["authorization"]));
    assert_eq!(proposed["when"], json!({"at":"2026-09-10T12:00:00Z"}));
    assert_eq!(proposed["maintenance"]["cadence_days"], 3);
    assert_eq!(proposed["maintenance"]["max_age_hours"], 72.0);
    assert!(proposed["maintenance"]["consent_digest"].is_null());
    assert_ne!(
        proposed["maintenance"]["inspection_digest"],
        proposed["maintenance"]["policy_digest"]
    );
    assert!(
        kpop_native::followup_triggers::referenced_external(&proposed["when"])
            .unwrap()
            .is_empty()
    );

    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let added = store.add(proposed.clone()).unwrap();
    assert!(
        added["spec"]["maintenance"]["consent_digest"]
            .as_str()
            .is_some()
    );
    assert_ne!(
        added["spec"]["maintenance"]["consent_digest"],
        proposed["maintenance"]["inspection_digest"]
    );
    let mut changed_cadence = source_declaration();
    changed_cadence["cadence_days"] = json!(7);
    let changed = kpop_native::maintenance_contract::compile(&changed_cadence).unwrap();
    assert_eq!(
        changed["spec"]["maintenance"]["inspection_digest"],
        proposed["maintenance"]["inspection_digest"]
    );
    assert_ne!(
        changed["spec"]["maintenance"]["policy_digest"],
        proposed["maintenance"]["policy_digest"]
    );
    let mut copied_authority = source_declaration();
    copied_authority["consent_digest"] = json!(added["spec"]["maintenance"]["consent_digest"]);
    assert!(kpop_native::maintenance_contract::compile(&copied_authority).is_err());
}

#[test]
fn maintenance_compiler_reports_missing_age_and_rejects_non_day_cadence() {
    let mut declaration = source_declaration();
    declaration["source"]
        .as_object_mut()
        .unwrap()
        .remove("max_age_hours");
    let missing_age = kpop_native::maintenance_contract::compile(&declaration).unwrap();
    assert!(missing_age["spec"].is_null());
    assert!(
        missing_age["unresolved"]
            .as_array()
            .unwrap()
            .contains(&json!("source.max_age_hours"))
    );

    declaration = source_declaration();
    declaration["cadence_days"] = json!(1.5);
    let invalid_cadence = kpop_native::maintenance_contract::compile(&declaration).unwrap();
    assert!(invalid_cadence["spec"].is_null());
    assert!(
        invalid_cadence["unresolved"]
            .as_array()
            .unwrap()
            .contains(&json!("cadence_days"))
    );
}

#[test]
fn maintenance_inspection_retains_success_after_a_failed_attempt() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let item = store.add(proposal["spec"].clone()).unwrap();
    let mut report = json!({"id":"source-refresh", "policy_digest":item["spec"]["maintenance"]["policy_digest"],
        "source_ref":item["spec"]["maintenance"]["source_ref"], "evidence":"fixture://inspection", "reason":"source unavailable"});
    store.record_maintenance_attempt(report.clone()).unwrap();
    report.as_object_mut().unwrap().remove("reason");
    report["inspected_at"] = json!("2026-09-10T11:59:00+00:00");
    report["value"] = json!({"revision":"accepted"});
    report["inspection"] = item["spec"]["maintenance"]["inspection"].clone();
    let result = store.inspect_maintenance(report).unwrap();
    assert_eq!(result["outcome"], "observed");
    let shown = store.show("source-refresh").unwrap();
    let attempt = shown["attempts"].as_array().unwrap().last().unwrap();
    assert_eq!(attempt["outcome"], "observed");
    assert_eq!(attempt["receipt_at"], "2026-09-10T12:00:00Z");
    assert!(attempt["observation_digest"].as_str().is_some());
}

#[test]
fn maintenance_add_acknowledgement_is_bound_to_its_workspace() {
    let (_a, workspace_a, state_a, now) = fixture();
    let (_b, workspace_b, state_b, _) = fixture();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let a = Store::at_in_state(&workspace_a, &state_a, now)
        .unwrap()
        .add(proposal["spec"].clone())
        .unwrap();
    let b = Store::at_in_state(&workspace_b, &state_b, now)
        .unwrap()
        .add(proposal["spec"].clone())
        .unwrap();
    assert_ne!(
        a["spec"]["maintenance"]["consent_digest"],
        b["spec"]["maintenance"]["consent_digest"]
    );
}

#[test]
fn maintenance_inspection_refuses_a_changed_task_until_explicit_refresh() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let item = store.add(proposal["spec"].clone()).unwrap();
    fs::write(
        item["task"].as_str().unwrap(),
        "Changed selected inspection scope",
    )
    .unwrap();
    let report = json!({"id":"source-refresh", "policy_digest":item["spec"]["maintenance"]["policy_digest"],
        "source_ref":item["spec"]["maintenance"]["source_ref"], "evidence":"fixture://changed-task",
        "inspected_at":"2026-09-10T11:59:00Z", "value":{"revision":"new"}});
    assert!(store.inspect_maintenance(report).is_err());
    assert!(
        store.load(true).unwrap().unwrap()["observations"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn maintenance_inspection_identity_and_use_inputs_are_explicit() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    assert_eq!(
        proposal["spec"]["maintenance"]["inspection"]["locator"],
        "https://example.test/report"
    );
    let mut tampered = proposal["spec"].clone();
    tampered["maintenance"]["inspection"]["selection"] = json!("Different report");
    assert!(store.add(tampered).is_err());
    let mut missing = source_declaration();
    missing.as_object_mut().unwrap().remove("use_policy");
    let unresolved = kpop_native::maintenance_contract::compile(&missing).unwrap();
    assert!(unresolved["spec"].is_null());
    assert!(
        unresolved["unresolved"]
            .as_array()
            .unwrap()
            .contains(&json!("use_policy"))
    );
}

#[test]
fn maintenance_refresh_preserves_compatible_evidence_and_requires_scope_reference() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let first = store.add(proposal["spec"].clone()).unwrap();
    let mut cadence = source_declaration();
    cadence["cadence_days"] = json!(7);
    let next = kpop_native::maintenance_contract::compile(&cadence).unwrap();
    let changed = store
        .refresh(
            "source-refresh",
            next["spec"].clone(),
            "fixture://selected-cadence",
        )
        .unwrap();
    assert_eq!(
        changed["spec"]["maintenance"]["source_ref"],
        first["spec"]["maintenance"]["source_ref"]
    );
    assert_ne!(
        changed["spec"]["maintenance"]["policy_digest"],
        first["spec"]["maintenance"]["policy_digest"]
    );
    cadence["source"]["locator"] = json!("https://another.example.test/report");
    let outside = kpop_native::maintenance_contract::compile(&cadence).unwrap();
    assert!(
        store
            .refresh(
                "source-refresh",
                outside["spec"].clone(),
                "fixture://source-reread"
            )
            .is_err()
    );
    let authorized = store
        .refresh_with_authority(
            "source-refresh",
            outside["spec"].clone(),
            "fixture://source-reread",
            Some("fixture://existing-user-grant"),
        )
        .unwrap();
    assert_ne!(
        authorized["spec"]["maintenance"]["source_ref"],
        first["spec"]["maintenance"]["source_ref"]
    );
    assert_eq!(
        authorized["attempts"].as_array().unwrap().last().unwrap()["authorization_reference"],
        "fixture://existing-user-grant"
    );
}

#[test]
fn maintenance_wrong_selection_is_unavailable_and_preserves_success() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let item = store.add(proposal["spec"].clone()).unwrap();
    let mut report = json!({"id":"source-refresh", "policy_digest":item["spec"]["maintenance"]["policy_digest"],
        "source_ref":item["spec"]["maintenance"]["source_ref"], "inspection":item["spec"]["maintenance"]["inspection"],
        "evidence":"fixture://selected-source", "inspected_at":"2026-09-10T11:59:00Z", "value":{"revision":"accepted"}});
    store.inspect_maintenance(report.clone()).unwrap();
    let before = store.load(true).unwrap().unwrap()["observations"].clone();
    report["inspection"]["locator"] = json!("https://wrong.example.test/report");
    report["value"] = json!({"revision":"wrong-source"});
    let result = store.inspect_maintenance(report.clone()).unwrap();
    assert_eq!(result["outcome"], "unavailable");
    assert_eq!(store.load(true).unwrap().unwrap()["observations"], before);
    report["inspection"] = item["spec"]["maintenance"]["inspection"].clone();
    report["inspected_at"] = json!("2026-09-10T12:01:00Z");
    let future = store.inspect_maintenance(report).unwrap();
    assert_eq!(future["outcome"], "unavailable");
    assert!(
        future["attempt"]["reason"]
            .as_str()
            .unwrap()
            .contains("future")
    );
    assert_eq!(store.load(true).unwrap().unwrap()["observations"], before);
}

#[test]
fn maintenance_inspection_refusal_records_failure_without_replacing_success() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let proposal = kpop_native::maintenance_contract::compile(&source_declaration()).unwrap();
    let item = store.add(proposal["spec"].clone()).unwrap();
    let metadata = &item["spec"]["maintenance"];
    let source_ref = metadata["source_ref"].as_str().unwrap();
    let policy_digest = metadata["policy_digest"].as_str().unwrap();
    store
        .observe(json!({
            "ref":source_ref,
            "value":{"revision":"accepted"},
            "observed_at":"2026-09-10T12:00:00Z",
            "evidence":"evidence://accepted"
        }))
        .unwrap();

    let result = store
        .inspect_maintenance(json!({
            "id":"source-refresh",
            "policy_digest":policy_digest,
            "source_ref":source_ref,
            "inspection":metadata["inspection"],
            "inspected_at":"2026-09-10T11:59:00Z",
            "evidence":"evidence://older-copy",
            "value":{"revision":"older"}
        }))
        .unwrap();

    assert_eq!(result["outcome"], "unavailable");
    assert_eq!(
        store.load(true).unwrap().unwrap()["observations"][source_ref]["value"]["revision"],
        "accepted"
    );
    let attempts = store.show("source-refresh").unwrap()["attempts"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(attempts.last().unwrap()["outcome"], "unavailable");
    assert_eq!(
        attempts.last().unwrap()["receipt_at"],
        "2026-09-10T12:00:00Z"
    );
    assert!(
        attempts.last().unwrap()["reason"]
            .as_str()
            .unwrap()
            .contains("older")
    );
}

#[test]
fn legacy_followup_specs_without_maintenance_metadata_still_roundtrip() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let original = spec(json!({"external":{"ref":"legacy-source","equals":"current"}}));
    let added = store.add(original.clone()).unwrap();
    assert_eq!(added["spec"], original);
    assert!(
        store.show("review").unwrap()["spec"]
            .get("maintenance")
            .is_none()
    );
    assert_eq!(store.load(true).unwrap().unwrap()["version"], 1);
}

#[test]
fn maintenance_compile_and_attempt_are_reachable_through_the_followups_cli() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    let state = temp.path().join("state");
    fs::create_dir(&workspace).unwrap();
    record(&workspace, 1);
    let binary = env!("CARGO_BIN_EXE_kpop");
    let invoke = |arguments: &[&str]| {
        Command::new(binary)
            .env("HOME", temp.path())
            .env("XDG_STATE_HOME", &state)
            .args(["--workspace", workspace.to_str().unwrap(), "followups"])
            .args(arguments)
            .output()
            .unwrap()
    };
    let declaration = temp.path().join("declaration.json");
    fs::write(
        &declaration,
        serde_json::to_vec(&source_declaration()).unwrap(),
    )
    .unwrap();
    let compiled = invoke(&["compile", "--file", declaration.to_str().unwrap()]);
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let compiled: Value = serde_json::from_slice(&compiled.stdout).unwrap();
    assert!(compiled["spec"]["maintenance"]["consent_digest"].is_null());
    assert_eq!(
        compiled["spec"]["task"],
        json!(declaration.canonicalize().unwrap())
    );

    let now = Utc::now();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let item = store.add(compiled["spec"].clone()).unwrap();
    let attempt = temp.path().join("attempt.json");
    fs::write(
        &attempt,
        serde_json::to_vec(&json!({
            "id":"source-refresh",
            "policy_digest":item["spec"]["maintenance"]["policy_digest"],
            "source_ref":item["spec"]["maintenance"]["source_ref"],
            "evidence":"evidence://unavailable",
            "reason":"The local inspection tool could not read the source."
        }))
        .unwrap(),
    )
    .unwrap();
    let result = invoke(&["attempt", "--file", attempt.to_str().unwrap()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["outcome"],
        "unavailable"
    );
    assert_eq!(
        store.show("source-refresh").unwrap()["attempts"][0]["outcome"],
        "unavailable"
    );
    // Even a whitespace-only authorized declaration edit requires the native
    // refresh receipt, without requesting the same source permission again.
    let mut bytes = fs::read(&declaration).unwrap();
    bytes.push(b' ');
    fs::write(&declaration, bytes).unwrap();
    assert!(
        !invoke(&["attempt", "--file", attempt.to_str().unwrap()])
            .status
            .success()
    );
    let refreshed = invoke(&["compile", "--file", declaration.to_str().unwrap()]);
    let refreshed: Value = serde_json::from_slice(&refreshed.stdout).unwrap();
    store
        .refresh(
            "source-refresh",
            refreshed["spec"].clone(),
            "fixture://authorized-innocuous-reread",
        )
        .unwrap();
    assert!(
        invoke(&["attempt", "--file", attempt.to_str().unwrap()])
            .status
            .success()
    );
}

fn row(scan: &Value) -> &Value {
    &scan["items"][0]
}

fn python_oracle() -> Value {
    serde_json::from_str(include_str!("fixtures/followups-oracle.json")).unwrap()
}

fn oracle_stdout(index: usize) -> Value {
    serde_json::from_str(
        python_oracle()["commands"][index]["stdout"]
            .as_str()
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn claim_renew_release_expiry_recovery_and_resolution_are_durable() {
    let (_temp, workspace, state, start) = fixture();
    let store = Store::at_in_state(&workspace, &state, start).unwrap();
    store.add(spec(json!({"at":"2026-09-10"}))).unwrap();
    let initial = store.scan(20).unwrap();
    assert_eq!(row(&initial)["state"], "ready");
    let occurrence = row(&initial)["occurrence"].as_str().unwrap();
    let first = store.claim("review", occurrence, "first", None).unwrap();
    let token = first["claim"]["token"].as_str().unwrap().to_owned();
    assert!(store.claim("review", occurrence, "second", None).is_err());

    let later = Store::at_in_state(&workspace, &state, start + Duration::minutes(20)).unwrap();
    assert_eq!(
        later.renew("review", &token).unwrap()["expires_at"],
        "2026-09-10T12:50:00Z"
    );
    later
        .finish("review", &token, "released", "No work started.", None)
        .unwrap();
    let scan = later.scan(20).unwrap();
    let second = later
        .claim(
            "review",
            row(&scan)["occurrence"].as_str().unwrap(),
            "second",
            None,
        )
        .unwrap();
    let second_token = second["claim"]["token"].as_str().unwrap().to_owned();

    let expired = Store::at_in_state(&workspace, &state, start + Duration::minutes(50)).unwrap();
    assert_eq!(row(&expired.scan(20).unwrap())["state"], "interrupted");
    assert!(
        expired
            .finish("review", &second_token, "done", "Too late.", None)
            .is_err()
    );
    expired
        .recover("review", "Verified no external effect.")
        .unwrap();
    expired
        .resolve("review", "done", "Owner completed it.")
        .unwrap();
    assert_eq!(
        expired
            .resolve("review", "done", "Owner completed it.")
            .unwrap()["already_recorded"],
        true
    );
    let saved = expired.show("review").unwrap();
    assert_eq!(saved["state"], "done");
    assert_eq!(saved["attempts"].as_array().unwrap().len(), 3);
    let outcomes = saved["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|attempt| attempt["outcome"].clone())
        .collect::<Vec<_>>();
    let expected = python_oracle()["ledger"]["items"]["review"]["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|attempt| attempt["outcome"].clone())
        .collect::<Vec<_>>();
    assert_eq!(outcomes, expected);
}

#[test]
fn changed_source_invalidates_occurrence_and_parks_a_stale_finish() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let added = store.add(spec(json!({"changed":"facts.count"}))).unwrap();
    assert_eq!(
        added["task_fingerprint"],
        oracle_stdout(1)["task_fingerprint"]
    );
    assert_eq!(row(&store.scan(20).unwrap())["state"], "waiting");
    record(&workspace, 2);
    let ready = store.scan(20).unwrap();
    assert_eq!(row(&ready)["state"], "ready");
    assert_eq!(
        row(&ready)["reasons"],
        oracle_stdout(6)["items"][0]["reasons"]
    );
    assert_eq!(
        row(&ready)["occurrence"],
        oracle_stdout(6)["items"][0]["occurrence"]
    );
    let claim = store
        .claim(
            "review",
            row(&ready)["occurrence"].as_str().unwrap(),
            "worker",
            None,
        )
        .unwrap();
    record(&workspace, 3);
    let result = store
        .finish(
            "review",
            claim["claim"]["token"].as_str().unwrap(),
            "done",
            "Used the earlier reading.",
            None,
        )
        .unwrap();
    assert_eq!(result["state"], "needs_user");
    assert_eq!(result["inputs_changed"], true);
    assert_eq!(store.show("review").unwrap()["baseline"]["facts.count"], 1);
}

#[test]
fn date_trigger_uses_the_configured_iana_timezone() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.add(spec(json!({"at":"2026-09-11"}))).unwrap();
    let scan = store.scan(20).unwrap();
    assert_eq!(row(&scan)["state"], "waiting");
    assert_eq!(row(&scan)["wake_hint"], "2026-09-10T21:00:00Z");
}

#[test]
fn a_core_record_without_a_runtime_retains_unavailable_evidence() {
    if std::env::var_os("KPOP_TEST_NO_RUNTIME_CHILD").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "a_core_record_without_a_runtime_retains_unavailable_evidence",
                "--nocapture",
            ])
            .env("KPOP_TEST_NO_RUNTIME_CHILD", "1")
            .env_remove("KPOPPER_NATIVE_RESOURCES")
            .env_remove("KPOPPER_NATIVE_CACHE")
            .env_remove("KPOP_TEST_ORDINARY_PROGRAM")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("GROUNDING.yaml"),
        "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nknown: {p.x: {v: 1}}\n",
    )
    .unwrap();
    let state = temp.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let mut followup = spec(json!({"changed":"p.x"}));
    followup["related"] = json!(["p.x"]);
    let added = store.add(followup).unwrap();
    assert_eq!(added["core_baseline"], json!(["p.x"]));
    assert_eq!(added["baseline"]["p.x"]["core"]["version"], 1);
    assert_eq!(added["baseline"]["p.x"]["core"]["available"], false);
    assert_eq!(row(&store.scan(20).unwrap())["state"], "unknown");
    fs::write(
        workspace.join("GROUNDING.yaml"),
        "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nknown: {p.x: {v: 2}}\n",
    )
    .unwrap();
    assert_eq!(row(&store.scan(20).unwrap())["state"], "unknown");
}

#[test]
fn same_observation_refresh_does_not_invalidate_an_active_occurrence() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.observe(json!({"ref":"release","value":"ready","observed_at":"2026-09-10T12:00:00Z","evidence":"API read"})).unwrap();
    store
        .add(spec(json!({"external":{"ref":"release","equals":"ready"}})))
        .unwrap();
    let ready = store.scan(20).unwrap();
    let claim = store
        .claim(
            "review",
            row(&ready)["occurrence"].as_str().unwrap(),
            "worker",
            None,
        )
        .unwrap();
    let later = Store::at_in_state(&workspace, &state, now + Duration::minutes(1)).unwrap();
    later.observe(json!({"ref":"release","value":"ready","observed_at":"2026-09-10T12:01:00Z","evidence":"Fresh API read"})).unwrap();
    let result = later
        .finish(
            "review",
            claim["claim"]["token"].as_str().unwrap(),
            "done",
            "Completed against the same state.",
            None,
        )
        .unwrap();
    assert_eq!(result["state"], "done");
    assert_eq!(result["inputs_changed"], false);
}

#[test]
fn locking_allows_only_one_concurrent_claim() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.add(spec(json!({"at":"2026-09-10"}))).unwrap();
    let occurrence = row(&store.scan(20).unwrap())["occurrence"]
        .as_str()
        .unwrap()
        .to_owned();
    let barrier = Arc::new(Barrier::new(3));
    let handles = ["a", "b"]
        .into_iter()
        .map(|owner| {
            let workspace = workspace.clone();
            let state = state.clone();
            let occurrence = occurrence.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let store = Store::at_in_state(&workspace, &state, now).unwrap();
                barrier.wait();
                store.claim("review", &occurrence, owner, None).is_ok()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    assert_eq!(
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|won| *won)
            .count(),
        1
    );
}

#[test]
fn restore_preserves_corruption_and_parks_uncertain_work() {
    let (_temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.add(spec(json!({"at":"2026-09-10"}))).unwrap();
    let backup = store.root.join("snapshot.yaml");
    fs::copy(&store.path, &backup).unwrap();
    fs::write(&store.path, b"broken: [").unwrap();
    let result = store
        .restore(&backup, "Inspected backup; effects require reconciliation.")
        .unwrap();
    let quarantine = result["quarantine"].as_str().unwrap();
    assert_eq!(fs::read(quarantine).unwrap(), b"broken: [");
    assert_eq!(store.show("review").unwrap()["state"], "needs_user");
}

#[test]
fn raw_json_rejects_duplicate_keys_and_excessive_depth() {
    assert!(parse_input(br#"{"id":"a","id":"b"}"#).is_err());
    let nested = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    assert!(parse_input(nested.as_bytes()).is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_ledger_is_refused_without_touching_its_target() {
    use std::os::unix::fs::symlink;
    let (temp, workspace, state, now) = fixture();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let outside = temp.path().join("outside.json");
    fs::write(&outside, b"preserve me").unwrap();
    fs::remove_file(&store.path).unwrap();
    symlink(&outside, &store.path).unwrap();
    assert!(store.status().is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"preserve me");
}

#[test]
fn real_cli_uses_disposable_state_and_keeps_status_record_independent() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    let state = temp.path().join("state");
    fs::create_dir(&workspace).unwrap();
    record(&workspace, 1);
    let binary = env!("CARGO_BIN_EXE_kpop");
    let invoke = |arguments: &[&str]| {
        Command::new(binary)
            .env("HOME", temp.path())
            .env("XDG_STATE_HOME", &state)
            .args(["--workspace", workspace.to_str().unwrap(), "followups"])
            .args(arguments)
            .output()
            .unwrap()
    };
    let setup = invoke(&["setup", "--timezone", "Asia/Jerusalem", "--private"]);
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let specification = temp.path().join("spec.json");
    fs::write(
        &specification,
        serde_json::to_vec(&spec(json!({"at":"2020-01-01"}))).unwrap(),
    )
    .unwrap();
    let add = invoke(&["add", "--file", specification.to_str().unwrap()]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let scan = invoke(&["scan"]);
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let scan: Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert_eq!(scan["items"][0]["state"], "ready");
    fs::remove_file(workspace.join("PROVENANCE.yaml")).unwrap();
    let status = invoke(&["status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&status.stdout).unwrap()["configured"],
        true
    );
    assert!(!invoke(&["daily", "install"]).status.success());
}
