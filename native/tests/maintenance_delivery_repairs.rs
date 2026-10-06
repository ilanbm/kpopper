use chrono::{TimeZone, Utc};
use kpop_native::{followup_daily, followup_store::Store};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;

fn record(workspace: &Path, state: &Path) {
    fs::write(
        workspace.join("PROVENANCE.yaml"),
        "meta:\n  name: Delivery fixture\n  updated: 2026-10-06\nknown:\n  facts.count:\n    name: Count\n    v: 1\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(workspace)
        .arg("check")
        .env("XDG_STATE_HOME", state)
        .env("TMPDIR", state)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn command(workspace: &Path, state: &Path, args: &[&str], input: Option<&Value>) -> Output {
    let mut request = Command::new(env!("CARGO_BIN_EXE_kpop"));
    request
        .args(args)
        .current_dir(workspace)
        .env("XDG_STATE_HOME", state)
        .env("TMPDIR", state)
        .env_remove("KPOPPER_RUNTIME")
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_SESSION_CONFIG")
        .env_remove("KPOPPER_SESSION_DISABLE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if workspace.join("resources").exists() {
        request.env("KPOPPER_NATIVE_RESOURCES", workspace.join("resources"))
            .env("KPOPPER_NATIVE_CACHE", state.join("cache"));
    }
    let mut child = request.spawn().unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.to_string().as_bytes())
            .unwrap();
    }
    child.wait_with_output().unwrap()
}

fn source_fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf, Store) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    let state = temp.path().join("state");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&state).unwrap();
    record(&workspace, &state);
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let store = Store::at_in_state(&workspace, &state, now).unwrap();
    store.setup(None, "UTC", None, true).unwrap();
    let declaration = json!({
        "schema":"kpopper.maintenance-declaration/v1",
        "kind":"source",
        "id":"source-check",
        "title":"Inspect the selected source",
        "why":"Keep the evidence current.",
        "how":"Read the selected source and retain its observed state.",
        "scope":"Inspect this source only; do not publish changes.",
        "related":["facts.count"],
        "cadence_days":1,
        "timezone":"UTC",
        "check_time":"09:00",
        "use_policy":"allow_cached_until_expiry",
        "evidence_requirement":"host_attested",
        "first_due_at":now.to_rfc3339(),
        "source":{
            "source_id":"fixture-source",
            "locator":"https://example.test/status",
            "publisher":"Example",
            "selection":"Current status",
            "adapter":"host-read/v1",
            "evidence_format":"selected text",
            "tool_policy":"existing authorized read tool",
            "allowed_roots":["https://example.test/"],
            "max_age_hours":24
        }
    });
    let proposal = kpop_native::maintenance_contract::compile(&declaration).unwrap();
    store.add(proposal["spec"].clone()).unwrap();
    (temp, workspace, state, store)
}

fn save_first_use_choice(workspace: &Path, state: &Path, choice: Value) {
    let project =
        state
            .join("kpopper/first-use/projects")
            .join(kpop_native::onboarding::project_key(
                &workspace.canonicalize().unwrap(),
            ));
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("maintenance-choice.json"),
        json!({"schema":1,"choice":choice}).to_string(),
    )
    .unwrap();
}

#[test]
fn plain_directory_gets_no_maintenance_hook_or_private_delivery_state() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("plain");
    let state = temp.path().join("state");
    fs::create_dir(&workspace).unwrap();
    fs::create_dir(&state).unwrap();
    let output = command(
        &workspace,
        &state,
        &["_hook", "followups", "codex"],
        Some(&json!({"cwd":workspace,"session_id":"plain-session"})),
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!state.join("kpopper/followups").exists());
}

#[test]
fn applicable_source_health_is_delivered_with_scope_assessment_guidance() {
    let (_temp, workspace, state, _store) = source_fixture();
    let output = command(
        &workspace,
        &state,
        &["_hook", "followups", "codex"],
        Some(&json!({"cwd":workspace,"session_id":"source-session"})),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("source-check"), "{context}");
    assert!(context.contains("propose the relevant scoped clock/source check"));
    assert!(context.contains("kpop followups assess --ids ACTUAL_SUBJECT_IDS"));
    assert!(context.contains("actual failure reason"));
    assert!(context.contains("observed_at/due_at"));
}

#[test]
fn declined_offer_keeps_truthful_continuity_assurance() {
    let (_temp, workspace, state, store) = source_fixture();
    followup_daily::record_adoption(&store, "declined", None).unwrap();
    let output = command(
        &workspace,
        &state,
        &["_hook", "followups", "codex"],
        Some(&json!({"cwd":workspace,"session_id":"declined-session"})),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("\"choice\":\"declined\""));
    assert!(context.contains("\"promotion_allowed\":false"));
    assert!(context.contains("assess the actual requested scope"));
    assert!(context.contains("failed, paused, missing, overdue, stale, or unknown evidence"));
    assert!(!context.contains("propose the relevant scoped clock/source check"));
}

#[test]
fn snoozed_offer_keeps_its_until_without_suppressing_continuity() {
    let (_temp, workspace, state, _store) = source_fixture();
    save_first_use_choice(
        &workspace,
        &state,
        json!({
            "state":"snoozed",
            "choice":"snoozed",
            "until":"2099-12-31T23:59:59Z",
            "evidence_ref":"fixture://saved-choice"
        }),
    );
    let output = command(
        &workspace,
        &state,
        &["_hook", "followups", "codex"],
        Some(&json!({"cwd":workspace,"session_id":"snoozed-session"})),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("\"choice\":\"snoozed\""));
    assert!(context.contains("2099-12-31T23:59:59Z"));
    assert!(context.contains("fixture://saved-choice"));
    assert!(context.contains("\"promotion_allowed\":false"));
    assert!(context.contains("assess the actual requested scope"));
    assert!(!context.contains("propose the relevant scoped clock/source check"));
}

#[test]
fn corrupt_delivery_receipt_does_not_drop_the_applicable_notice() {
    let (_temp, workspace, state, store) = source_fixture();
    fs::write(store.root.join("delivery.yaml"), "not: [valid yaml").unwrap();
    let output = command(
        &workspace,
        &state,
        &["_hook", "followups", "codex"],
        Some(&json!({"cwd":workspace,"session_id":"delivery-error-session"})),
    );
    assert!(output.status.success());
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = reply["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(context.contains("source-check"));
    assert!(fs::read_dir(&store.root).unwrap().flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("delivery.corrupt-")));
    let repeat = command(&workspace, &state, &["_hook", "followups", "codex"],
        Some(&json!({"cwd":workspace,"session_id":"delivery-error-session"})));
    assert!(repeat.status.success());
    assert!(repeat.stdout.is_empty(), "Recovered delivery state must deduplicate");
}

#[test]
fn corrupt_ledger_does_not_replace_start_context_with_an_error() {
    let (_temp, workspace, state, store) = source_fixture();
    fs::write(&store.path, "{\"version\":3}").unwrap();
    let output = command(
        &workspace,
        &state,
        &["session-start", "--host", "codex"],
        Some(&json!({
            "cwd":workspace,
            "session_id":"corrupt-ledger-session",
            "hook_event_name":"SessionStart"
        })),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let context = String::from_utf8_lossy(&output.stdout);
    assert!(context.contains("KPOPPER_START"), "{context}");
    assert!(context.contains("agent_command"), "{context}");
    assert!(
        context.contains("Maintenance continuity health is unavailable from local state"),
        "{context}"
    );
}

#[test]
fn corrupt_choice_is_scoped_unknown_and_can_be_declined() {
    let (_temp, workspace, state, store) = source_fixture();
    save_first_use_choice(&workspace, &state, json!({"state":"shown"}));
    let project = state.join("kpopper/first-use/projects").join(kpop_native::onboarding::project_key(&workspace.canonicalize().unwrap()));
    fs::write(project.join("maintenance-choice.json"), "{").unwrap();
    let status = followup_daily::status_with_maintenance(&store).unwrap();
    assert_eq!(status["adoption"]["choice"], "unknown");
    assert!(status["adoption"]["reason"].as_str().unwrap().contains("first-use"));
    assert!(store.scan(3).is_ok());
    assert_eq!(followup_daily::record_adoption(&store, "declined", None).unwrap()["choice"], "declined");
    assert!(fs::read_dir(project).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("maintenance-choice.corrupt-")));
}

#[test]
fn malformed_snooze_is_unknown_without_breaking_status() {
    let (_temp, workspace, state, store) = source_fixture();
    save_first_use_choice(&workspace, &state, json!({"state":"snoozed"}));
    assert_eq!(followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["choice"], "unknown");
    assert_eq!(followup_daily::record_adoption(&store, "shown", None).unwrap()["choice"], "shown");
}

#[test]
fn closed_policy_stays_quiet_in_hook_payload() {
    let (_temp, workspace, state, store) = source_fixture();
    store.resolve_with_authority("source-check", "cancelled", "fixture://retired", Some("fixture://user-retired")).unwrap();
    let output = command(&workspace, &state, &["_hook", "followups", "codex"], Some(&json!({"cwd":workspace,"session_id":"retired-session"})));
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("KPOPPER_FOLLOWUPS"), "{text}");
    assert!(!text.contains("source-check"), "{text}");
    let assessment = store.assess_use(&["facts.count".into()], &[]).unwrap();
    assert!(!assessment.to_string().contains("maintenance_closed"));
}

fn session_packet(workspace: &Path, state: &Path, subject: &str, format: &str) -> Value {
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let resources = workspace.join("resources");
    if !resources.exists() {
        fs::create_dir_all(resources.join("reasoning")).unwrap();
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reasoning/native").join(format!("{target}.kpopper-runtime")), resources.join("reasoning").join(format!("{target}.zip"))).unwrap();
        let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM").map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cache/kpopper/lean").join(&target).join(env!("KPOP_ORDINARY_SOURCE_SHA256")));
        fs::create_dir_all(resources.join("ordinary").join(&target)).unwrap();
        for name in ["build.json", "epistemic-core"] { fs::copy(ordinary.join(name), resources.join("ordinary").join(&target).join(name)).unwrap(); }
    }
    let base = ["--workspace", workspace.to_str().unwrap(), "session", "open", "--no-settings", "--input", "PROVENANCE.yaml", "--project", "maintenance-reader", "--state", "reader-state", "--tokens", "16000"];
    let opened = command(workspace, state, &base, None);
    assert!(opened.status.success(), "{}", String::from_utf8_lossy(&opened.stderr));
    let open_text = String::from_utf8(opened.stdout).unwrap();
    let revision = open_text.lines().find_map(|line| line.strip_prefix("project=maintenance-reader revision=")).unwrap();
    let viewed = command(workspace, state, &["--workspace", workspace.to_str().unwrap(), "session", "view", "--no-settings", "--input", "PROVENANCE.yaml", "--project", "maintenance-reader", "--state", "reader-state", "--revision", revision, "--id", subject, "--tokens", "16000", "--view-format", format], None);
    assert!(viewed.status.success(), "{}", String::from_utf8_lossy(&viewed.stderr));
    let text = String::from_utf8(viewed.stdout).unwrap();
    if format == "json" { serde_json::from_str(&text).unwrap() }
    else { kpop_native::view_format::decode_checked_text(&text).unwrap() }
}

#[test]
fn actual_selected_session_view_delivers_failed_check_and_subject_identity_in_all_encodings() {
    let (_temp, workspace, state, store) = source_fixture();
    let item = store.show("source-check").unwrap();
    store.record_maintenance_attempt(json!({"id":"source-check","policy_digest":item["spec"]["maintenance"]["policy_digest"],"source_ref":item["spec"]["maintenance"]["source_ref"],"reason":"provider refused selected read","evidence":"fixture://failure"})).unwrap();
    for format in ["json", "checked-text-tagged"] {
        let packet = session_packet(&workspace, &state, "facts.count", format);
        let continuity = &packet["maintenance_continuity"];
        assert_eq!(continuity["subjects"], json!(["facts.count"]));
        assert_eq!(continuity["record_identity_match"], true);
        assert_eq!(continuity["obligations"][0]["failure"], "provider refused selected read");
        assert_eq!(continuity["obligations"][0]["source_id"], "fixture-source");
        assert_eq!(continuity["obligations"][0]["related"], json!(["facts.count"]));
        assert!(continuity["obligations"][0]["due_at"].is_string());
        assert!(continuity["disclosure"].as_str().unwrap().contains("recorded values"));
    }
    fs::write(workspace.join("PROVENANCE.yaml"), "meta:\n  name: Delivery fixture\nknown:\n  facts.count: {v: 1}\n  facts.other: {v: 2}\n").unwrap();
    let unrelated = session_packet(&workspace, &state, "facts.other", "json");
    assert!(unrelated.get("maintenance_continuity").is_none());
    store.resolve_with_authority("source-check", "cancelled", "fixture://retired", Some("fixture://user-retired")).unwrap();
    let closed = session_packet(&workspace, &state, "facts.count", "json");
    assert!(closed.get("maintenance_continuity").is_none());
}
