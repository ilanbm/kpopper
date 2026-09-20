use chrono::{Duration, TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store};
use serde_json::json;
use std::{
    fs,
    sync::{Arc, Barrier},
    thread,
};

fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Daily fixture\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    let state = temp.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    store.add(json!({"id":"review","title":"Review","why":"Keep current","how":"Read it","scope":"Read only","related":["facts.count"],"when":{"at":"2019-12-31"}})).unwrap();
    (temp, workspace, state)
}

#[test]
fn daily_lease_outputs_and_durable_receipt_are_complete() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let started = followup_daily::start(&store, "daily-owner").unwrap();
    assert_eq!(started["state"], "running");
    assert_eq!(
        started["limits"],
        json!({"followup_actions":3,"maintenance_actions":1})
    );
    let token = started["claim"]["token"].as_str().unwrap().to_owned();
    assert!(followup_daily::start(&store, "competing-owner").is_err());
    let renewed = followup_daily::renew(&store, &token).unwrap();
    assert_eq!(renewed["expires_at"], "2026-09-11T22:30:00Z");
    assert_eq!(
        followup_daily::finish(&store, &token, "No useful authorized work.").unwrap()["state"],
        "complete"
    );
    let data = store.load(true).unwrap().unwrap();
    assert!(data["daily"]["claim"].is_null());
    assert_eq!(
        data["daily"]["receipts"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["outcome"],
        "complete"
    );
}

#[test]
fn expired_daily_claim_requires_recovery_and_race_has_one_winner() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let first = Store::at_in_state(&workspace, &state, now).unwrap();
    let started = followup_daily::start(&first, "owner").unwrap();
    let token = started["claim"]["token"].as_str().unwrap().to_owned();
    let expired = Store::at_in_state(&workspace, &state, now + Duration::minutes(31)).unwrap();
    assert!(followup_daily::finish(&expired, &token, "too late").is_err());
    assert_eq!(
        followup_daily::recover(&expired, "Reconciled item effects.").unwrap()["state"],
        "recovered"
    );

    let barrier = Arc::new(Barrier::new(2));
    let wins = (0..2)
        .map(|i| {
            let workspace = workspace.clone();
            let state = state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let store =
                    Store::at_in_state(&workspace, &state, now + Duration::days(1)).unwrap();
                barrier.wait();
                followup_daily::start(&store, &format!("race-{i}")).is_ok()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        wins.into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count(),
        1
    );
}

fn host_report(now: chrono::DateTime<Utc>, schedules: serde_json::Value) -> serde_json::Value {
    json!({"host":"fake-host","observed_at":now.to_rfc3339(),"complete":true,"schedules":schedules,"evidence":"Independent host readback"})
}
fn reserve(store: &Store, owner: &str) -> serde_json::Value {
    followup_daily::install_begin(store, Some(owner), None, None, None, false, false, false)
        .unwrap()["installation"]
        .clone()
}
fn schedule(store: &Store, prompt: serde_json::Value) -> serde_json::Value {
    json!({"id":"schedule","state":"active","workspace_key":store.load(true).unwrap().unwrap()["workspace_key"],"cadence":"daily","time":"09:00","timezone":"UTC","prompt":prompt,"access_verified":true})
}

#[test]
fn full_prompt_pins_runtime_and_state_without_discarding_payload() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let plan = followup_daily::plan(&store, "08:15").unwrap();
    let prompt = plan["prompt"].as_str().unwrap();
    let payload: serde_json::Value =
        serde_json::from_str(prompt.rsplit_once('\n').unwrap().1).unwrap();
    let config = store.load(true).unwrap().unwrap()["config"].clone();
    assert_eq!(
        payload,
        json!({"workspace":config["workspace"],"record":config["record"],"ledger":store.path,"timezone":"UTC","runtime":{"command":[std::env::current_exe().unwrap()],"environment":{"XDG_STATE_HOME":state}}})
    );
    assert!(prompt.contains("\n{\"workspace\": "));
    let job = reserve(&store, "owner");
    assert_eq!(
        job["template_hash"],
        kpop_native::followup_store::digest(&job["prompt"]).unwrap()
    );
    assert_eq!(
        job["config"],
        kpop_native::followup_store::digest(&config).unwrap()
    );
}

#[test]
fn uncertain_installation_preserves_token_until_evidenced_reconciliation() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let job = reserve(&store, "owner");
    let token = job["token"].as_str().unwrap();
    assert_eq!(
        followup_daily::install_inspect(&store, token, host_report(now, json!([]))).unwrap()["action"],
        "create"
    );
    followup_daily::install_fail(&store, token, "Lost reply", false).unwrap();
    assert!(followup_daily::install_fail(&store, token, "No effects assumed", true).is_err());
    let other =
        followup_daily::install_begin(&store, Some("other"), None, None, None, false, false, false)
            .unwrap();
    assert_eq!(other["state"], "needs_reconciliation");
    assert_eq!(other["installation"]["token"], token);
    assert_eq!(
        followup_daily::install_inspect(&store, token, host_report(now, json!([]))).unwrap()["state"],
        "needs_reconciliation"
    );
    let mut report = host_report(now, json!([]));
    report["no_pending_request"] = json!(false);
    assert!(followup_daily::install_reconcile(&store, token, report.clone()).is_err());
    report["no_pending_request"] = json!(true);
    assert_eq!(
        followup_daily::install_reconcile(&store, token, report).unwrap()["state"],
        "reconciled"
    );
    let fresh = reserve(&store, "other");
    assert_ne!(fresh["token"], token);
    assert_eq!(
        store.load(true).unwrap().unwrap()["daily"]["installation_history"][0]["state"],
        "reconciled"
    );
}

#[test]
fn full_host_receipt_is_retained_and_exact_replay_is_required() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let job = reserve(&store, "owner");
    let token = job["token"].as_str().unwrap();
    followup_daily::install_inspect(&store, token, host_report(now, json!([]))).unwrap();
    let mut report = json!({"host":"fake-host","observed_at":now.to_rfc3339(),"schedule":schedule(&store,job["prompt"].clone()),"evidence":"Readback evidence 0123456789abcdef0123456789abcdef"});
    let finished = followup_daily::install_finish(&store, token, report.clone()).unwrap();
    assert_eq!(finished["state"], "installed");
    assert_eq!(finished["first_run_verified"], false);
    assert_eq!(finished["binding"]["managed_prompt"], true);
    assert_eq!(
        finished["binding"]["prompt_hash"],
        kpop_native::followup_store::digest(&job["prompt"]).unwrap()
    );
    assert_eq!(
        store.load(true).unwrap().unwrap()["daily"]["installation"]["receipt"],
        report
    );
    assert_eq!(
        followup_daily::install_finish(&store, token, report.clone()).unwrap()["state"],
        "already_recorded"
    );
    let before = fs::read(&store.path).unwrap();
    report["evidence"] = json!("Different result");
    assert!(followup_daily::install_finish(&store, token, report).is_err());
    assert_eq!(before, fs::read(&store.path).unwrap());
}

#[test]
fn embedded_conflicting_marker_cannot_hide_behind_a_valid_first_line() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let job = reserve(&store, "owner");
    let token = job["token"].as_str().unwrap();
    let candidate = schedule(
        &store,
        json!(format!(
            "{}\nUser prefix KPOPPER_DAILY_WORKSPACE={}",
            job["prompt"].as_str().unwrap(),
            "f".repeat(64)
        )),
    );
    assert!(
        followup_daily::install_inspect(&store, token, host_report(now, json!([candidate])))
            .is_err()
    );
}

#[test]
fn daily_item_claim_must_be_reconciled_before_expired_review() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let started = followup_daily::start(&store, "owner").unwrap();
    let token = started["claim"]["token"].as_str().unwrap();
    store
        .claim(
            "review",
            started["packet"]["items"][0]["occurrence"]
                .as_str()
                .unwrap(),
            "item-owner",
            Some(token),
        )
        .unwrap();
    let expired = Store::at_in_state(&workspace, &state, now + Duration::minutes(30)).unwrap();
    assert_eq!(
        followup_daily::status(&expired).unwrap()["run"],
        "interrupted"
    );
    assert!(followup_daily::renew(&expired, token).is_err());
    assert!(followup_daily::recover(&expired, "Inspected").is_err());
    expired
        .recover("review", "Inspected the item effects")
        .unwrap();
    followup_daily::recover(&expired, "Inspected all effects").unwrap();
    assert_eq!(
        expired.load(true).unwrap().unwrap()["daily"]["receipts"][0]["token"],
        token
    );
}

#[test]
fn watch_requests_are_injected_and_fail_without_blocking_daily_claim() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let started = followup_daily::start_with_watch_request(&store, "owner", |workspace| {
        Ok(Some(
            json!({"worktrees":[{"state":"pending","workspace":workspace}]}),
        ))
    })
    .unwrap();
    assert_eq!(
        started["packet"]["watch"]["worktrees"][0]["state"],
        "pending"
    );
    followup_daily::finish(&store, started["claim"]["token"].as_str().unwrap(), "Done").unwrap();
    let tomorrow = Store::at_in_state(&workspace, &state, now + Duration::days(1)).unwrap();
    let failed = followup_daily::start_with_watch_request(&tomorrow, "owner", |_| {
        Err(kpop_native::Error("x".repeat(600)))
    })
    .unwrap();
    assert_eq!(failed["state"], "running");
    assert_eq!(failed["packet"]["watch"]["state"], "unavailable");
    assert_eq!(
        failed["packet"]["watch"]["reason"].as_str().unwrap().len(),
        400
    );
}

#[test]
fn one_owner_reserves_one_installation_under_a_race() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let runs = (0..2)
        .map(|_| {
            let workspace = workspace.clone();
            let state = state.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let store = Store::at_in_state(&workspace, &state, now).unwrap();
                barrier.wait();
                reserve(&store, "same-owner")["token"].clone()
            })
        })
        .collect::<Vec<_>>();
    let tokens = runs
        .into_iter()
        .map(|run| run.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(tokens[0], tokens[1]);
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    assert!(
        store.load(true).unwrap().unwrap()["daily"]
            .get("installation_history")
            .is_none()
    );
}

#[test]
fn daily_action_budget_counts_released_claims_and_refuses_a_fourth() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    for id in ["second", "third", "fourth"] {
        store.add(json!({"id":id,"title":"Review","why":"Current","how":"Read","scope":"Read only","related":["facts.count"],"when":{"at":"2019-12-31"}})).unwrap();
    }
    let started = followup_daily::start(&store, "owner").unwrap();
    let token = started["claim"]["token"].as_str().unwrap();
    let items = started["packet"]["items"].as_array().unwrap();
    for row in &items[..3] {
        let id = row["id"].as_str().unwrap();
        let claim = store
            .claim(
                id,
                row["occurrence"].as_str().unwrap(),
                "worker",
                Some(token),
            )
            .unwrap();
        store
            .finish(
                id,
                claim["claim"]["token"].as_str().unwrap(),
                "released",
                "No effects",
                None,
            )
            .unwrap();
    }
    let last = &items[3];
    assert!(
        store
            .claim(
                last["id"].as_str().unwrap(),
                last["occurrence"].as_str().unwrap(),
                "worker",
                Some(token)
            )
            .is_err()
    );
    assert_eq!(
        store.load(true).unwrap().unwrap()["daily"]["claim"]["actions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn cli_routes_complete_host_and_daily_protocol_and_status() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    let state = temp.path().join("state");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: CLI daily\nknown: {}\n",
    )
    .unwrap();
    let call = |args: &[&str], input: Option<serde_json::Value>, success: bool| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
            .arg("--workspace")
            .arg(&workspace)
            .arg("followups")
            .args(args)
            .env("XDG_STATE_HOME", &state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(serde_json::to_string(&input).unwrap().as_bytes())
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert_eq!(
            out.status.success(),
            success,
            "args={args:?} stdout={} stderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if success {
            serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
        } else {
            json!(String::from_utf8_lossy(&out.stderr))
        }
    };
    call(&["daily", "install", "--unchanged"], None, false);
    call(&["daily", "install", "--inspect", "-"], None, false);
    call(&["setup", "--private", "--timezone", "UTC"], None, true);
    let plan = call(&["daily", "plan", "--time", "10:30"], None, true);
    let payload: serde_json::Value = serde_json::from_str(
        plan["prompt"]
            .as_str()
            .unwrap()
            .rsplit_once('\n')
            .unwrap()
            .1,
    )
    .unwrap();
    assert_eq!(
        payload["runtime"]["command"],
        json!([env!("CARGO_BIN_EXE_kpop-native")])
    );
    assert_eq!(
        payload["runtime"]["environment"]["XDG_STATE_HOME"],
        json!(state)
    );
    let install = call(
        &[
            "daily",
            "install",
            "--owner",
            "fake-host",
            "--time",
            "10:30",
        ],
        None,
        true,
    );
    let token = install["installation"]["token"].as_str().unwrap();
    let observed = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let report = json!({"host":"fake-host","observed_at":observed,"complete":true,"schedules":[],"evidence":"Readback"});
    let apply = call(
        &["daily", "install", "--token", token, "--inspect", "-"],
        Some(report),
        true,
    );
    let schedule = json!({"id":"fake-schedule","state":"active","workspace_key":install["workspace_key"],"cadence":"daily","time":"10:30","timezone":"UTC","prompt":apply["prompt"],"access_verified":true});
    let receipt = json!({"host":"fake-host","observed_at":observed,"schedule":schedule,"evidence":"Independent readback"});
    assert_eq!(
        call(
            &["daily", "install", "--token", token, "--result", "-"],
            Some(receipt.clone()),
            true
        )["state"],
        "installed"
    );
    assert_eq!(
        call(
            &["daily", "install", "--token", token, "--result", "-"],
            Some(receipt),
            true
        )["state"],
        "already_recorded"
    );
    assert_eq!(
        call(&["status"], None, true)["daily"]["state"],
        "active_reported"
    );
    let started = call(&["daily", "start", "--owner", "session"], None, true);
    let token = started["claim"]["token"].as_str().unwrap();
    call(&["daily", "renew", "--token", token], None, true);
    call(&["daily", "recover", "--evidence", "Live"], None, false);
    call(
        &[
            "daily",
            "finish",
            "--token",
            token,
            "--evidence",
            "No useful work",
        ],
        None,
        true,
    );
    assert_eq!(
        call(
            &[
                "daily",
                "finish",
                "--token",
                token,
                "--evidence",
                "No useful work"
            ],
            None,
            true
        )["state"],
        "already_completed"
    );
    assert_eq!(call(&["daily", "status"], None, true)["run"], "idle");
    let invalid = call(
        &[
            "daily", "install", "--token", "x", "--check", "--fail", "No",
        ],
        None,
        false,
    );
    assert!(invalid.as_str().unwrap().contains("cannot be check-only"));
}

#[test]
fn plan_and_scan_validate_before_loading_and_invalid_state_root_is_checked() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Validation\nknown: {}\n",
    )
    .unwrap();
    let state = temp.path().join("state");
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let mut store = Store::at_in_state(&workspace, &state, now).unwrap();
    assert_eq!(
        store.scan(0).unwrap_err().to_string(),
        "limit must be between 1 and 100"
    );
    assert_eq!(
        followup_daily::plan(&store, "24:00")
            .unwrap_err()
            .to_string(),
        "Daily time must be HH:MM"
    );
    store.setup(None, "UTC", None, true).unwrap();
    store.root = "/".into();
    assert_eq!(
        followup_daily::plan(&store, "09:00")
            .unwrap_err()
            .to_string(),
        "invalid followups state path"
    );
}

#[cfg(unix)]
#[test]
fn installation_recreates_empty_private_destination_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    fs::create_dir(&workspace).unwrap();
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Private\nknown: {}\n",
    )
    .unwrap();
    let state = temp.path().join("state");
    let store = Store::at_in_state(
        &workspace,
        &state,
        Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap(),
    )
    .unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    if store.root.join("items").exists() {
        fs::remove_dir(store.root.join("items")).unwrap();
    }
    reserve(&store, "owner");
    assert_eq!(
        fs::metadata(store.root.join("items"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn daily_owner_validation_uses_raw_unicode_character_limits_and_python_whitespace() {
    let (_temp, workspace, state) = fixture();
    let now = Utc.with_ymd_and_hms(2026, 9, 11, 22, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    let before = fs::read(&store.path).unwrap();
    assert!(followup_daily::start(&store, &format!("{}x", " ".repeat(12_000))).is_err());
    assert!(followup_daily::start(&store, "\u{1c}\u{1f}").is_err());
    assert_eq!(before, fs::read(&store.path).unwrap());
    let owner = "日".repeat(12_000);
    let started = followup_daily::start(&store, &owner).unwrap();
    assert_eq!(started["claim"]["owner"], owner);
}
