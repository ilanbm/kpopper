use chrono::{Duration, Utc};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn run(work: &Path, home: &Path, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(work)
        .env("KPOPPER_PRIVATE_HOME", home.join("private"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("CODEX_THREAD_ID")
        .env_remove("KPOPPER_AGENT_SESSION")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let work = t.path().join("work");
    fs::create_dir(&work).unwrap();
    fs::write(
        work.join("PROVENANCE.yaml"),
        "meta:\n  name: Execution fixture\nknown:\n  facts.count:\n    v: 1\n",
    )
    .unwrap();
    run(
        &work,
        t.path(),
        &["followups", "setup", "--private", "--timezone", "UTC"],
    );
    run(
        &work,
        t.path(),
        &[
            "followups",
            "daily",
            "adoption",
            "authorized",
            "--evidence",
            "fixture://user-grant",
        ],
    );
    let bind = t.path().join("binding.json");
    fs::write(&bind,serde_json::to_vec(&json!({"host":"fixture","id":"bound-job","state":"active","evidence":"fixture://configuration-only"})).unwrap()).unwrap();
    run(
        &work,
        t.path(),
        &[
            "followups",
            "daily",
            "bind",
            "--file",
            bind.to_str().unwrap(),
        ],
    );
    let due = (Utc::now() + Duration::days(1))
        .date_naive()
        .and_hms_opt(9, 0, 0)
        .unwrap()
        .and_utc();
    let declaration = json!({"schema":"kpopper.maintenance-declaration/v1","kind":"clock","id":"clock-check","title":"Declared clock check","why":"Keep clock continuity explicit","how":"Read native time","scope":"Read only","related":["facts.count"],"cadence_days":1,"timezone":"UTC","check_time":"09:00","use_policy":"require_live","evidence_requirement":"trusted_clock","deadline":{"utc":due.to_rfc3339()}});
    let spec = t.path().join("clock-spec.json");
    fs::write(
        &spec,
        serde_json::to_vec(
            &kpop_native::maintenance_contract::compile(&declaration).unwrap()["spec"],
        )
        .unwrap(),
    )
    .unwrap();
    run(
        &work,
        t.path(),
        &["followups", "add", "--file", spec.to_str().unwrap()],
    );
    let phase = t.path().join("phase.json");
    fs::write(&phase,serde_json::to_vec(&json!({"host":"fixture","id":"bound-job","cadence_days":1,"anchor_at":due.to_rfc3339(),"timezone":"UTC","interval_semantics":"calendar_days","observed_at":Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true),"evidence":"fixture://normalized-phase"})).unwrap()).unwrap();
    run(
        &work,
        t.path(),
        &[
            "followups",
            "daily",
            "mode",
            "native",
            "--file",
            phase.to_str().unwrap(),
            "--authorization-evidence",
            "fixture://existing-owner-grant",
        ],
    );
    (t, work)
}
#[test]
fn manual_completion_keeps_host_unverified_and_retains_manual_authority() {
    let (t, w) = fixture();
    let started = run(
        &w,
        t.path(),
        &[
            "followups",
            "daily",
            "start",
            "--owner",
            "bound-job",
            "--manual-evidence",
            "fixture://current-user-grant",
        ],
    );
    run(
        &w,
        t.path(),
        &[
            "followups",
            "daily",
            "finish",
            "--token",
            started["claim"]["token"].as_str().unwrap(),
            "--evidence",
            "fixture://manual-completion",
        ],
    );
    let status = run(&w, t.path(), &["followups", "daily", "status"]);
    assert_eq!(
        status["adoption"]["configuration"],
        "configuration_unverified"
    );
    assert_eq!(
        status["maintenance_health"]["obligations"][0]["host_state"],
        "configuration_unverified"
    );
    assert_eq!(started["claim"]["execution_origin"], "manual");
    assert_eq!(
        started["claim"]["manual_authorization_reference"],
        "fixture://current-user-grant"
    );
}
#[test]
fn owner_text_and_unattested_completion_cannot_establish_host_execution() {
    let (t, w) = fixture();
    let started = run(
        &w,
        t.path(),
        &[
            "followups",
            "daily",
            "start",
            "--owner",
            "fixture/bound-job",
        ],
    );
    run(
        &w,
        t.path(),
        &[
            "followups",
            "daily",
            "finish",
            "--token",
            started["claim"]["token"].as_str().unwrap(),
            "--evidence",
            "fixture://ordinary-completion",
        ],
    );
    let status = run(&w, t.path(), &["followups", "daily", "status"]);
    assert_eq!(
        status["adoption"]["configuration"],
        "configuration_unverified"
    );
    assert_eq!(
        status["maintenance_health"]["obligations"][0]["host_state"],
        "configuration_unverified"
    );
    assert_eq!(started["claim"]["execution_origin"], "unattested");
    // Historical completion receipts predate execution-origin metadata.
    let ledger = run(&w, t.path(), &["followups", "status"])["ledger"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut data: Value = serde_json::from_slice(&fs::read(&ledger).unwrap()).unwrap();
    data["daily"]["receipts"][0]
        .as_object_mut()
        .unwrap()
        .remove("execution_origin");
    fs::write(&ledger, serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(
        run(&w, t.path(), &["followups", "daily", "status"])["adoption"]["configuration"],
        "configuration_unverified"
    );
}
#[test]
fn actual_current_matching_host_attestation_is_distinct_from_manual_completion() {
    let (t, w) = fixture();
    let now = Utc::now();
    let file = t.path().join("host-execution.json");
    let report = json!({"schema":"kpopper.host-execution/v1","trigger":"scheduled","host":"fixture","id":"bound-job","executed_at":now.to_rfc3339(),"observed_at":now.to_rfc3339(),"evidence":"fixture://actual-host-tool-run-readback"});
    fs::write(&file, serde_json::to_vec(&report).unwrap()).unwrap();
    let started = run(
        &w,
        t.path(),
        &[
            "followups",
            "daily",
            "start",
            "--owner",
            "unique-runtime-session",
            "--host-execution",
            file.to_str().unwrap(),
        ],
    );
    assert_eq!(
        started["claim"]["execution_origin"],
        "self_reported_scheduled"
    );
    assert_eq!(started["claim"]["host_execution"], report);
    run(
        &w,
        t.path(),
        &[
            "followups",
            "daily",
            "finish",
            "--token",
            started["claim"]["token"].as_str().unwrap(),
            "--evidence",
            "fixture://scheduled-native-completion",
        ],
    );
    let status = run(&w, t.path(), &["followups", "daily", "status"]);
    assert_eq!(
        status["adoption"]["configuration"],
        "scheduled_execution_reported"
    );
    assert_eq!(
        status["maintenance_health"]["obligations"][0]["host_state"],
        "scheduled_execution_reported"
    );
    // A host attestation is explicit evidence; stale or foreign reports cannot admit a lease.
    for (key, value) in [
        ("id", json!("foreign-job")),
        (
            "executed_at",
            json!((now - Duration::minutes(11)).to_rfc3339()),
        ),
        ("trigger", json!("manual")),
        (
            "observed_at",
            json!((now + Duration::minutes(1)).to_rfc3339()),
        ),
    ] {
        let mut invalid = report.clone();
        invalid[key] = value;
        fs::write(&file, serde_json::to_vec(&invalid).unwrap()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&w)
            .env("KPOPPER_PRIVATE_HOME", t.path().join("private"))
            .env("XDG_STATE_HOME", t.path().join("state"))
            .args([
                "followups",
                "daily",
                "start",
                "--owner",
                "another-session",
                "--host-execution",
                file.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "invalid {key} must refuse before already-completed shortcut"
        );
    }
}

#[test]
fn offset_timestamp_round_trips_preserve_native_nanosecond_precision() {
    use kpop_native::followup_triggers::{parse_time, stamp};
    for text in ["2026-10-10T09:00:00.123456789Z", "2026-10-10T11:00:00.1234567+02:00", "2026-10-10T09:00:00.000000001Z", "2026-10-10T09:00:00.123456Z"] {
        let exact = chrono::DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc);
        assert_eq!(parse_time(text, "UTC").unwrap(), exact);
        assert_eq!(parse_time(&stamp(exact), "UTC").unwrap(), exact);
    }
    for text in ["2026-10-10T09:00:00.1234567891Z", "2026-10-10T09:00:00.123456789", "2026-10-10T09:00:60.123456789Z"] {
        assert!(parse_time(text, "UTC").is_err());
    }
}

#[test]
fn daily_claim_expiry_respects_the_microsecond_store_clock_boundary() {
    use chrono::SubsecRound;
    use kpop_native::{followup_daily, followup_store::Store};
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-10T09:00:00.123456789Z").unwrap().with_timezone(&Utc);
    for offset in [-1, 0, 1] {
        let t = tempfile::tempdir().unwrap();
        let work = t.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::write(work.join("PROVENANCE.yaml"), "meta: {name: Claim precision}
known:
  facts.count: {v: 1}
").unwrap();
        let state = t.path().join("state");
        let store = Store::at_in_state(&work, &state, now).unwrap();
        store.setup(None, "UTC", None, true).unwrap();
        let started = followup_daily::start_with_mode(&store, "owner", |_| Ok(None), None).unwrap();
        let expires = now.trunc_subsecs(6) + Duration::minutes(30);
        assert_eq!(kpop_native::followup_triggers::parse_time(started["claim"]["expires_at"].as_str().unwrap(), "UTC").unwrap(), expires);
        let boundary = Store::at_in_state(&work, &state, expires + Duration::nanoseconds(offset)).unwrap();
        let recovered = followup_daily::recover(&boundary, "fixture://reconciled");
        assert_eq!(recovered.is_ok(), offset >= 0);
        if offset < 0 {
            assert!(followup_daily::finish(&boundary, started["claim"]["token"].as_str().unwrap(), "fixture://finished").is_ok());
        }
    }
}
