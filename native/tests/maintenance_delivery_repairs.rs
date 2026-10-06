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
    let mut child = Command::new(env!("CARGO_BIN_EXE_kpop"))
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
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
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
    assert!(context.contains("Maintenance notice delivery state is unavailable"));
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
