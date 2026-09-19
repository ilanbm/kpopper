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
fn core_record_baseline_retains_a_captured_reading_marker() {
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
    assert_eq!(row(&store.scan(20).unwrap())["state"], "waiting");
    fs::write(
        workspace.join("GROUNDING.yaml"),
        "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nknown: {p.x: {v: 2}}\n",
    )
    .unwrap();
    assert_eq!(row(&store.scan(20).unwrap())["state"], "ready");
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
    let binary = env!("CARGO_BIN_EXE_kpop-native");
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
