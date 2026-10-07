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
    assert_eq!(status["adoption"]["reason"], "first_use_choice_unreadable");
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

fn reader_command(workspace: &Path, state: &Path, operation: &str, revision: Option<&str>, extra: &[&str]) -> Output {
    let mut args = vec!["--workspace", workspace.to_str().unwrap(), "session", operation, "--no-settings", "--input", "PROVENANCE.yaml", "--project", "maintenance-reader", "--state", "reader-state"];
    if let Some(revision) = revision {args.extend(["--revision", revision]);}
    args.extend_from_slice(extra);
    command(workspace, state, &args, None)
}

#[test]
fn constrained_view_allocates_continuity_and_exact_read_and_hook_view_have_no_bypass() {
    let (_temp, workspace, state, store) = source_fixture();
    let item = store.show("source-check").unwrap();
    store.record_maintenance_attempt(json!({"id":"source-check","policy_digest":item["spec"]["maintenance"]["policy_digest"],"source_ref":item["spec"]["maintenance"]["source_ref"],"reason":"provider unavailable","evidence":"fixture://failure"})).unwrap();
    let initial = session_packet(&workspace, &state, "facts.count", "json");
    let revision = initial["revision"].as_str().unwrap();
    let full_tokens = kpop_native::tokenizer::Encoding::O200kBase.count(&(serde_json::to_string(&initial).unwrap() + "\n"));
    let constrained = full_tokens.saturating_sub(1);
    let budget = constrained.to_string();
    let viewed = reader_command(&workspace, &state, "view", Some(revision), &["--id", "facts.count", "--tokens", &budget, "--view-format", "json"]);
    if viewed.status.success() {
        let view: Value = serde_json::from_slice(&viewed.stdout).unwrap();
        assert!(view["maintenance_continuity"]["obligations"].is_array());
        assert!(kpop_native::tokenizer::Encoding::O200kBase.count(&String::from_utf8(viewed.stdout).unwrap()) <= constrained);
    } else {
        // When mandatory evidence cannot fit, refuse explicitly. The advertised
        // exact reader below must still carry the same required continuity.
        assert!(String::from_utf8_lossy(&viewed.stderr).contains("token budget"));
    }
    let insufficient = reader_command(&workspace, &state, "view", Some(revision), &["--id", "facts.count", "--tokens", "64", "--view-format", "json"]);
    assert!(!insufficient.status.success());
    assert!(String::from_utf8_lossy(&insufficient.stderr).contains("token budget"));
    let read = reader_command(&workspace, &state, "read", Some(revision), &["--ref", "node:facts.count", "--tokens", "1200"]);
    assert!(read.status.success(), "{}", String::from_utf8_lossy(&read.stderr));
    let read: Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(read["value"]["body"]["v"], 1);
    assert_eq!(read["maintenance_continuity"]["obligations"][0]["failure"], "provider unavailable");
    let hook = reader_command(&workspace, &state, "hook-view", None, &["--tokens", "16000", "--view-format", "json"]);
    assert!(hook.status.success(), "{}", String::from_utf8_lossy(&hook.stderr));
    let text = String::from_utf8(hook.stdout).unwrap();
    assert!(text.contains("maintenance_continuity") && text.contains("provider unavailable"), "{text}");
}

#[test]
fn future_choice_schema_and_absent_read_only_state_are_preserved_on_reads() {
    let (_temp, workspace, state, store) = source_fixture();
    let project = state.join("kpopper/first-use/projects").join(kpop_native::onboarding::project_key(&workspace.canonicalize().unwrap()));
    fs::create_dir_all(&project).unwrap();
    let path = project.join("maintenance-choice.json");
    let future = json!({"schema":2,"choice":{"state":"future"}}).to_string();
    fs::write(&path, &future).unwrap();
    assert_eq!(followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["reason"], "first_use_choice_unsupported_schema");
    assert_eq!(fs::read_to_string(&path).unwrap(), future);
    assert_eq!(fs::read_dir(project).unwrap().count(), 1);
    let empty_state = workspace.join("empty-readonly-state");
    fs::create_dir(&empty_state).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&empty_state, fs::Permissions::from_mode(0o500)).unwrap();
    }
    let output = command(&workspace, &empty_state, &["_hook", "followups", "codex"], Some(&json!({"cwd":workspace,"session_id":"readonly-choice"})));
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("KPOPPER_MAINTENANCE_CHOICE"), "{text}");
    assert!(!text.contains("restore the required retained segments"), "{text}");
    assert_eq!(fs::read_dir(&empty_state).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn explicit_choice_recovers_unreadable_current_state_and_clears_unknown_metadata() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, workspace, state, store) = source_fixture();
    save_first_use_choice(&workspace, &state, json!({"state":"shown"}));
    let project = state.join("kpopper/first-use/projects").join(kpop_native::onboarding::project_key(&workspace.canonicalize().unwrap()));
    let path = project.join("maintenance-choice.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(followup_daily::status_with_maintenance(&store).unwrap()["adoption"]["reason"], "first_use_choice_unreadable");
    assert_eq!(followup_daily::record_adoption(&store, "declined", None).unwrap()["choice"], "declined");
    let current: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert!(current["choice"].get("choice").is_none());
    assert!(current["choice"].get("quarantined_path").is_none());
    assert!(fs::read_dir(project).unwrap().flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("maintenance-choice.retained-")));
}

#[test]
fn actual_uncovered_session_reads_deliver_conditional_decision_without_a_policy() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("project");
    let state = temp.path().join("state");
    fs::create_dir(&workspace).unwrap(); fs::create_dir(&state).unwrap();
    record(&workspace, &state);
    let before = fs::read(workspace.join("PROVENANCE.yaml")).unwrap();
    let packet = session_packet(&workspace, &state, "facts.count", "checked-text-tagged");
    let discovery = &packet["maintenance_discovery"];
    assert_eq!(discovery["subjects"], json!(["facts.count"]));
    assert_eq!(discovery["kind"], "unestablished");
    assert_eq!(discovery["applicability"], "agent_task_assessment_required");
    assert_eq!(discovery["record_refs"], json!(["node:facts.count#"]));
    assert_eq!(discovery["declaration"], "none"); assert_eq!(discovery["consent"], "none");
    assert!(discovery["response_obligation"]["matching_kind"]["pending_clock"].as_str().unwrap().contains("clock-only"));
    assert!(discovery["response_obligation"]["matching_kind"]["changing_named_source"].as_str().unwrap().contains("named-source"));
    assert!(discovery["response_obligation"]["ongoing_scope"].as_str().unwrap().contains("future_validity"));
    let revision = packet["revision"].as_str().unwrap();
    let read = reader_command(&workspace, &state, "read", Some(revision), &["--ref", "node:facts.count", "--tokens", "2000"]);
    assert!(read.status.success(), "{}", String::from_utf8_lossy(&read.stderr));
    let read: Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(read["maintenance_discovery"]["subjects"], json!(["facts.count"]));
    assert_eq!(fs::read(workspace.join("PROVENANCE.yaml")).unwrap(), before);
    assert!(!state.join("kpopper/followups").exists());
    let store = Store::at_in_state(&workspace, &state, Utc.with_ymd_and_hms(2026,10,6,12,0,0).unwrap()).unwrap();
    followup_daily::record_adoption(&store, "declined", None).unwrap();
    let suppressed = session_packet(&workspace, &state, "facts.count", "json");
    assert!(suppressed.get("maintenance_discovery").is_none());
}

#[test]
fn optional_discovery_never_blocks_small_reads_or_replaces_hook_evidence() {
    let temp = tempfile::tempdir().unwrap(); let workspace=temp.path().join("project"); let state=temp.path().join("state");
    fs::create_dir(&workspace).unwrap(); fs::create_dir(&state).unwrap(); record(&workspace,&state);
    let initial=session_packet(&workspace,&state,"facts.count","json"); let revision=initial["revision"].as_str().unwrap();
    for tokens in ["64","600"] {
        let out=reader_command(&workspace,&state,"read",Some(revision), &["--ref","facts.count","--tokens",tokens]);
        assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
        assert!(kpop_native::tokenizer::Encoding::O200kBase.count(&String::from_utf8(out.stdout).unwrap())<=tokens.parse::<usize>().unwrap());
    }
    let proposed=reader_command(&workspace,&state,"hook-view",None,&["--tokens","1200","--view-format","json"]);
    assert!(proposed.status.success(),"{}",String::from_utf8_lossy(&proposed.stderr));
    let graph=String::from_utf8(proposed.stdout).unwrap().lines().find_map(|line|line.strip_prefix("KPOPPER_CANONICAL_GRAPH_VIEW ").map(str::to_owned)).unwrap();
    let proposed:Value=serde_json::from_str(&graph).unwrap(); assert!(proposed["coverage"]["direct_count"].as_u64().unwrap()>0);
    let store=Store::at_in_state(&workspace,&state,Utc.with_ymd_and_hms(2026,10,6,12,0,0).unwrap()).unwrap();
    followup_daily::record_adoption(&store,"declined",None).unwrap();
    let declined=reader_command(&workspace,&state,"hook-view",None,&["--tokens","1200","--view-format","json"]);
    let graph=String::from_utf8(declined.stdout).unwrap().lines().find_map(|line|line.strip_prefix("KPOPPER_CANONICAL_GRAPH_VIEW ").map(str::to_owned)).unwrap();
    let declined:Value=serde_json::from_str(&graph).unwrap();
    assert_eq!(proposed["coverage"]["direct_count"],declined["coverage"]["direct_count"]);
}

#[test]
fn bare_and_group_exact_refs_preserve_required_health_without_invented_subjects() {
    let (_temp,workspace,state,store)=source_fixture(); let item=store.show("source-check").unwrap();
    store.record_maintenance_attempt(json!({"id":"source-check","policy_digest":item["spec"]["maintenance"]["policy_digest"],"source_ref":item["spec"]["maintenance"]["source_ref"],"reason":"provider unavailable","evidence":"fixture://failure"})).unwrap();
    let initial=session_packet(&workspace,&state,"facts.count","json"); let revision=initial["revision"].as_str().unwrap();
    for reference in ["facts.count","node:facts.count","conditions:/","/"] {
        let out=reader_command(&workspace,&state,"read",Some(revision),&["--ref",reference,"--tokens","4000"]);
        assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
        let continuity = if reference == "/" {
            let text=String::from_utf8(out.stdout).unwrap(); assert!(text.starts_with("revision="));
            let row=text.lines().find_map(|line|line.strip_prefix("KPOPPER_SCOPED_CONTINUITY ")).unwrap();
            serde_json::from_str::<Value>(row).unwrap()
        } else {serde_json::from_slice::<Value>(&out.stdout).unwrap()["maintenance_continuity"].clone()};
        assert_eq!(continuity["subjects"],json!(["facts.count"]));
        assert_eq!(continuity["obligations"][0]["failure"],"provider unavailable");
    }
    let out=reader_command(&workspace,&state,"read",Some(revision),&["--ref","source:record","--tokens","4000"]);
    assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
    let packet:Value=serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(packet["maintenance_continuity"]["state"],"unknown");
    assert_eq!(packet["maintenance_continuity"]["reason"],"body_bearing_reference_scope_unresolved");
}

#[test]
fn ledgerless_documented_shown_ack_and_state_precedence_suppress_discovery() {
    let temp=tempfile::tempdir().unwrap(); let workspace=temp.path().join("project"); let state=temp.path().join("state");
    fs::create_dir(&workspace).unwrap(); fs::create_dir(&state).unwrap(); record(&workspace,&state);
    let first=session_packet(&workspace,&state,"facts.count","json"); assert!(first.get("maintenance_discovery").is_some());
    let ack=command(&workspace,&state,&["_agent","shown","followups"],None); assert!(ack.status.success(),"{}",String::from_utf8_lossy(&ack.stderr));
    assert!(!state.join("kpopper/followups").exists());
    assert!(session_packet(&workspace,&state,"facts.count","json").get("maintenance_discovery").is_none());
    save_first_use_choice(&workspace,&state,json!({"state":"declined","choice":"proposed"}));
    assert!(session_packet(&workspace,&state,"facts.count","json").get("maintenance_discovery").is_none());
}

#[test]
fn unresolved_refs_without_active_maintenance_are_quiet_at_small_budgets() {
    let temp=tempfile::tempdir().unwrap(); let workspace=temp.path().join("project"); let state=temp.path().join("state");
    fs::create_dir(&workspace).unwrap(); fs::create_dir(&state).unwrap(); record(&workspace,&state);
    let initial=session_packet(&workspace,&state,"facts.count","json"); let revision=initial["revision"].as_str().unwrap();
    for reference in ["source:record","source:record#/sha256","pending","native"] {
        let out=reader_command(&workspace,&state,"read",Some(revision),&["--ref",reference,"--tokens","100"]);
        assert!(out.status.success(),"{reference}: {}",String::from_utf8_lossy(&out.stderr));
        let packet:Value=serde_json::from_slice(&out.stdout).unwrap();
        assert!(packet.get("maintenance_continuity").is_none(),"{reference}: {packet}");
    }
}

#[test]
fn pre_record_shown_ack_does_not_suppress_a_later_record() {
    let temp=tempfile::tempdir().unwrap(); let workspace=temp.path().join("project"); let state=temp.path().join("state");
    fs::create_dir(&workspace).unwrap(); fs::create_dir(&state).unwrap();
    let ack=command(&workspace,&state,&["_agent","shown","followups"],None); assert!(ack.status.success());
    record(&workspace,&state);
    let packet=session_packet(&workspace,&state,"facts.count","json");
    assert!(packet.get("maintenance_discovery").is_some());
}

#[test]
fn compact_discovery_retains_scope_and_inference_guards_and_recovery_hint() {
    let temp=tempfile::tempdir().unwrap(); let workspace=temp.path().join("project"); let state=temp.path().join("state");
    fs::create_dir(&workspace).unwrap(); fs::create_dir(&state).unwrap(); record(&workspace,&state);
    let initial=session_packet(&workspace,&state,"facts.count","json"); let revision=initial["revision"].as_str().unwrap();
    let out=reader_command(&workspace,&state,"read",Some(revision),&["--ref","facts.count","--tokens","600"]); assert!(out.status.success());
    let packet:Value=serde_json::from_slice(&out.stdout).unwrap(); let discovery=&packet["maintenance_discovery"];
    assert_eq!(discovery["subjects"],json!(["facts.count"]));
    if discovery["state"]=="advisory_decision_pointer" {
        let obligation=discovery["obligation"].as_str().unwrap();
        assert!(obligation.contains("names or source prose") && obligation.contains("unrelated"));
    }
    assert!(discovery["retrieval_hint"].as_str().is_some_and(|hint|hint.contains("--tokens")));
}
