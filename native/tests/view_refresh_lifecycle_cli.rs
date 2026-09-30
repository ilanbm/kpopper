use kpop_native::value::TypedValue;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

struct Probe {
    root: tempfile::TempDir,
    session: String,
}

impl Probe {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let session = format!("refresh-life-{}", uuid::Uuid::new_v4());
        fs::write(root.path().join("GROUNDING.yaml"),
            "meta:\n  scope: Water allocation\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.opening: {v: 106}\n  p.inflow: {v: 42}\n  p.reserve: {v: 22}\n  p.deduction: {v: 9}\ncomputed:\n  r.usable: {rule: {op: sub, args: [{op: sub, args: [{op: add, args: [{ref: p.opening}, {ref: p.inflow}]}, {ref: p.reserve}]}, {ref: p.deduction}]}}\n").unwrap();
        fs::write(
            root.path().join("transcript.jsonl"),
            json!({"type":"session_meta","payload":{"id":session}}).to_string() + "\n",
        )
        .unwrap();
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        fs::create_dir_all(root.path().join("resources/reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.kpopper-runtime")),
            root.path()
                .join("resources/reasoning")
                .join(format!("{target}.zip")),
        )
        .unwrap();
        kpop_native::view_continuation::initialize(root.path(), &session, true).unwrap();
        Self { root, session }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
        command
            .args([
                "--workspace",
                self.root.path().to_str().unwrap(),
                "--frozen",
            ])
            .env(
                "KPOPPER_NATIVE_RESOURCES",
                self.root.path().join("resources"),
            )
            .env("KPOPPER_NATIVE_CACHE", self.root.path().join("cache"))
            .env(
                "KPOPPER_SESSION_CONFIG",
                self.root.path().join("preferences.json"),
            )
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env_remove("KPOPPER_SESSION_DISABLE")
            .env_remove("KPOPPER_CANONICAL_VIEW");
        command
    }
    fn session(&self, args: &[&str]) -> String {
        let result = self
            .command()
            .args([
                "session",
                "--no-settings",
                "--input",
                "GROUNDING.yaml",
                "--project",
                "refresh-life",
                "--state",
                "state",
                "--assessment-profile",
                "core/v1",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    }
    fn revision(&self) -> String {
        self.session(&["open", "--tokens", "16000"])
            .lines()
            .find_map(|line| line.strip_prefix("project=refresh-life revision="))
            .unwrap()
            .to_owned()
    }
    fn append(&self, line: &str) {
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(self.root.path().join("transcript.jsonl"))
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }
    fn context(&self, text: &str) {
        self.append(
            &json!({"type":"response_item","payload":{"type":"message","role":"developer",
            "content":[{"type":"input_text","text":text}]}})
            .to_string(),
        );
    }
    fn hook(&self, event: &str, turn: Option<&str>, extra: Value, disable: bool) -> String {
        let mut payload = json!({"session_id":self.session,"hook_event_name":event,
            "cwd":self.root.path(),"transcript_path":self.root.path().join("transcript.jsonl")});
        if let Some(turn) = turn {
            payload["turn_id"] = json!(turn);
        }
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut command = self.command();
        if disable {
            command.env("KPOPPER_SESSION_DISABLE", "1");
        }
        let mut child = command
            .args(["_hook", "continuation", "codex"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        if result.stdout.is_empty() {
            return String::new();
        }
        serde_json::from_slice::<Value>(&result.stdout).unwrap()["hookSpecificOutput"]["additionalContext"]
            .as_str().unwrap().to_owned()
    }
    fn read(&self) -> String {
        let revision = self.revision();
        let marker = self.session(&[
            "view",
            "--revision",
            &revision,
            "--tokens",
            "16000",
            "--max-view-bytes",
            "39000",
            "--view-format",
            "checked-text-tagged",
            "--expand",
            "group:/",
            "--context-session",
            &self.session,
        ]);
        assert!(marker.starts_with("KPOPPER_CONTEXT_QUEUED "), "{marker}");
        let frame = self.hook(
            "PostToolUse",
            Some("first"),
            json!({"tool_response":{"output":marker}}),
            false,
        );
        self.context(&frame);
        self.hook(
            "Stop",
            Some("first"),
            json!({"last_assistant_message":"p.deduction and r.usable"}),
            false,
        );
        frame
    }
    fn read_one(&self, id: &str, turn: &str) {
        let revision = self.revision();
        let marker = self.session(&["view", "--revision", &revision, "--tokens", "16000",
            "--max-view-bytes", "39000", "--view-format", "checked-text-tagged",
            "--id", id, "--context-session", &self.session]);
        assert!(marker.starts_with("KPOPPER_CONTEXT_QUEUED "), "{marker}");
        let frame = self.hook("PostToolUse", Some(turn), json!({"tool_response":{"output":marker}}), false);
        assert!(frame.contains("KPOPPER_CONTEXT_FRAME"), "{frame}");
        self.context(&frame);
        self.hook("Stop", Some(turn), json!({"last_assistant_message":id}), false);
    }
    fn session_start(&self) -> String {
        let mut child = self.command().args(["session-start", "--host", "codex"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(json!({"cwd":self.root.path(),"session_id":self.session})
            .to_string().as_bytes()).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        String::from_utf8(result.stdout).unwrap()
    }
    fn change(&self) {
        let path = self.root.path().join("GROUNDING.yaml");
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("p.deduction: {v: 9}", "p.deduction: {v: 19}"),
        )
        .unwrap();
    }
}

fn evidence_packet(text: &str) -> Value {
    let start = text.find("# Checked graph view").unwrap();
    let end = text[start..].find("KPOPPER_CONTEXT_END ").unwrap() + start;
    kpop_native::view_format::decode_checked_text(&text[start..end]).unwrap()
}

fn evidence_value(packet: &Value, id: &str) -> Value {
    let row = packet["nodes"].as_array().unwrap().iter()
        .find(|row| packet["dictionary"][row[0].as_str().unwrap()]["original"] == id).unwrap();
    TypedValue::from_tagged(&row[1]).unwrap().to_json().unwrap()
}

impl Drop for Probe {
    fn drop(&mut self) {
        let path = kpop_native::session_activity::temporary_directory()
            .join(format!("kpopper-view-{}", self.session));
        let _ = fs::remove_dir_all(path);
    }
}

#[test]
fn interrupt_keeps_source_obligation_and_refreshes_changed_evidence() {
    let p = Probe::new();
    p.read();
    p.hook("Interrupt", Some("first"), json!({}), false);
    p.change();
    let refreshed = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(refreshed.contains("KPOPPER_SOURCE_REFRESH"), "{refreshed}");
    assert!(refreshed.contains("KPOPPER_CONTEXT_FRAME"), "{refreshed}");
    assert_eq!(evidence_value(&evidence_packet(&refreshed), "p.deduction")["v"], 19);
}

#[test]
fn failed_session_start_preserves_prior_source_obligation() {
    let p = Probe::new();
    p.read();
    fs::write(p.root.path().join("GROUNDING.yaml"), "known: [unterminated").unwrap();
    p.session_start();
    let notice = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(notice.contains("KPOPPER_SOURCE_REFRESH"), "{notice}");
    assert!(notice.contains("\"status\":\"unavailable\""), "{notice}");
}

#[test]
fn missing_record_on_same_session_restart_keeps_refresh_active() {
    let p = Probe::new();
    p.read();
    let source = p.root.path().join("GROUNDING.yaml");
    let original = fs::read_to_string(&source).unwrap();
    fs::remove_file(&source).unwrap();
    p.session_start();
    let unavailable = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(unavailable.contains("\"status\":\"unavailable\""), "{unavailable}");
    fs::write(&source, original.replace("p.deduction: {v: 9}", "p.deduction: {v: 19}")).unwrap();
    let refreshed = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(refreshed.contains("KPOPPER_CONTEXT_FRAME"), "{refreshed}");
    assert_eq!(evidence_value(&evidence_packet(&refreshed), "p.deduction")["v"], 19);
}

#[test]
fn canonical_fallback_restart_keeps_prior_refresh_active() {
    let p = Probe::new();
    p.read();
    let source = p.root.path().join("GROUNDING.yaml");
    fs::write(&source, fs::read_to_string(&source).unwrap().replace("known:\n", "# reviewed\nknown:\n")).unwrap();
    let unavailable = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(unavailable.contains("\"status\":\"unavailable\""), "{unavailable}");
    p.session_start();
    p.change();
    let current = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(!current.is_empty(), "fallback restart must retain the earlier source warning");
}

#[test]
fn compaction_keeps_source_obligation_without_reusing_old_reference() {
    let p = Probe::new();
    let old = p.read();
    let old_ref = old.lines().find_map(|line| line.strip_prefix("KPOPPER_CONTEXT_FRAME "))
        .and_then(|line| serde_json::from_str::<Value>(line).ok()).unwrap()["revision_ref"]
        .as_str().unwrap().to_owned();
    p.hook("PreCompact", Some("compact"), json!({}), false);
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), &old_ref).is_err());
    p.change();
    let refreshed = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(refreshed.contains("KPOPPER_SOURCE_REFRESH"), "{refreshed}");
    assert!(refreshed.contains("KPOPPER_CONTEXT_FRAME"), "{refreshed}");
}

#[test]
fn malformed_or_unknown_transcript_keeps_source_obligation() {
    for barrier in [
        "{broken",
        r#"{"type":"future_context_rewrite","payload":{}}"#,
    ] {
        let p = Probe::new();
        p.read();
        p.append(barrier);
        p.hook(
            "Stop",
            Some("second"),
            json!({"last_assistant_message":"second"}),
            false,
        );
        p.change();
        let refreshed = p.hook("UserPromptSubmit", None, json!({}), false);
        assert!(refreshed.contains("KPOPPER_SOURCE_REFRESH"), "{refreshed}");
    }
}

#[test]
fn opt_out_suppresses_warning_after_receipt_loss() {
    let p = Probe::new();
    p.read();
    p.change();
    let unreceived = p.hook("UserPromptSubmit", Some("second"), json!({}), false);
    assert!(unreceived.contains("KPOPPER_CONTEXT_FRAME"), "{unreceived}");
    p.hook("Stop", Some("second"), json!({"last_assistant_message":"current frame missing"}), false);
    assert!(p.hook("UserPromptSubmit", None, json!({}), true).is_empty());
    fs::write(
        p.root.path().join("preferences.json"),
        json!({"schema":1,"enabled":false}).to_string(),
    )
    .unwrap();
    assert!(
        p.hook("UserPromptSubmit", None, json!({}), false)
            .is_empty()
    );
}

#[test]
fn legacy_readerless_warning_obeys_explicit_durable_opt_out() {
    let p = Probe::new();
    p.read();
    p.change();
    let unreceived = p.hook("UserPromptSubmit", Some("second"), json!({}), false);
    assert!(unreceived.contains("KPOPPER_CONTEXT_FRAME"), "{unreceived}");
    p.hook("Stop", Some("second"), json!({"last_assistant_message":"lost refresh"}), false);
    let state_path = kpop_native::session_activity::temporary_directory()
        .join(format!("kpopper-view-{}", p.session)).join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    assert!(state["refresh_error"].is_string());
    state.as_object_mut().unwrap().remove("freshness");
    state["reader"] = Value::Null;
    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let settings = p.root.path().join("preferences.json");
    fs::write(&settings, json!({"schema":1,"enabled":true,
        "native":env!("CARGO_BIN_EXE_kpop"),"tokens":16000}).to_string()).unwrap();
    let enabled = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(enabled.contains("\"status\":\"unavailable\""), "{enabled}");
    fs::write(&settings, "{broken").unwrap();
    let unreadable = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(unreadable.contains("\"status\":\"unavailable\""), "{unreadable}");
    fs::write(&settings, json!({"schema":1,"enabled":false}).to_string()).unwrap();
    assert!(p.hook("UserPromptSubmit", None, json!({}), false).is_empty());
}

#[test]
fn lost_refresh_is_retried_and_only_received_frame_recovers() {
    let p = Probe::new();
    p.read();
    p.change();
    let first = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(first.contains("KPOPPER_CONTEXT_FRAME"), "{first}");
    let retry = p.hook("UserPromptSubmit", Some("second"), json!({}), false);
    assert!(retry.contains("KPOPPER_CONTEXT_FRAME"), "{retry}");
    p.context(&retry);
    p.hook(
        "Stop",
        Some("second"),
        json!({"last_assistant_message":"current p.deduction"}),
        false,
    );
    assert!(
        p.hook("UserPromptSubmit", None, json!({}), false)
            .is_empty()
    );
}

#[test]
fn complete_managed_read_recovers_after_unavailable_source() {
    let p = Probe::new();
    p.read();
    let source = p.root.path().join("GROUNDING.yaml");
    let original = fs::read_to_string(&source).unwrap();
    fs::remove_file(&source).unwrap();
    let unavailable = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(unavailable.contains("\"status\":\"unavailable\""), "{unavailable}");
    fs::write(&source, original.replace("p.deduction: {v: 9}", "p.deduction: {v: 19}")).unwrap();
    let complete = p.read();
    assert_eq!(evidence_value(&evidence_packet(&complete), "p.deduction")["v"], 19);
    assert!(p.hook("UserPromptSubmit", None, json!({}), false).is_empty());
}

#[test]
fn exact_source_restore_clears_unavailable_without_reviving_old_reference() {
    let p = Probe::new();
    let old = p.read();
    let old_ref = old.lines().find_map(|line| line.strip_prefix("KPOPPER_CONTEXT_FRAME "))
        .and_then(|line| serde_json::from_str::<Value>(line).ok()).unwrap()["revision_ref"]
        .as_str().unwrap().to_owned();
    let source = p.root.path().join("GROUNDING.yaml");
    let original = fs::read(&source).unwrap();
    fs::remove_file(&source).unwrap();
    let unavailable = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(unavailable.contains("\"status\":\"unavailable\""), "{unavailable}");
    fs::write(&source, original).unwrap();
    assert!(p.hook("UserPromptSubmit", None, json!({}), false).is_empty());
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), &old_ref).is_err());
}

#[test]
fn exact_restore_does_not_acknowledge_a_lost_changed_frame() {
    let p = Probe::new();
    p.read();
    let source = p.root.path().join("GROUNDING.yaml");
    let original = fs::read(&source).unwrap();
    p.change();
    let unreceived = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(unreceived.contains("KPOPPER_CONTEXT_FRAME"), "{unreceived}");
    fs::write(&source, original).unwrap();
    let notice = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(notice.contains("\"status\":\"unavailable\""), "{notice}");
    assert!(!notice.contains("KPOPPER_CONTEXT_FRAME"));
}

#[test]
fn partial_read_cannot_use_pre_warning_frames_to_clear_lost_refresh() {
    for late_receipt in [false, true] {
        let p = Probe::new();
        p.read();
        let source = p.root.path().join("GROUNDING.yaml");
        let original = fs::read(&source).unwrap();
        p.change();
        let changed = p.hook("UserPromptSubmit", Some("second"), json!({}), false);
        assert!(changed.contains("KPOPPER_CONTEXT_FRAME"), "{changed}");
        p.hook("Stop", Some("second"), json!({"last_assistant_message":"changed value"}), false);
        if late_receipt { p.context(&changed); }
        fs::write(&source, original).unwrap();
        let warning = p.hook("UserPromptSubmit", Some("third"), json!({}), false);
        assert!(warning.contains("\"status\":\"unavailable\""), "{warning}");
        p.read_one("p.opening", "third");
        let next = p.hook("UserPromptSubmit", Some("fourth"), json!({}), false);
        assert!(next.contains("\"status\":\"unavailable\""), "late_receipt={late_receipt}: {next}");
    }
}

#[test]
fn reinitialization_expires_old_epoch_reference() {
    let p = Probe::new();
    let old = p.read();
    let old_ref = old
        .lines()
        .find_map(|line| line.strip_prefix("KPOPPER_CONTEXT_FRAME "))
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .unwrap()["revision_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), &old_ref)
            .is_ok()
    );
    kpop_native::view_continuation::initialize(p.root.path(), &p.session, true).unwrap();
    assert!(
        kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), &old_ref)
            .is_err()
    );
    let current = p.read();
    let new_ref = current
        .lines()
        .find_map(|line| line.strip_prefix("KPOPPER_CONTEXT_FRAME "))
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .unwrap()["revision_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(new_ref, old_ref);
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), &new_ref).is_ok());
    assert!(
        kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), "view:1")
            .is_err()
    );
}

#[test]
fn persisted_pre_upgrade_reader_survives_same_root_restart() {
    let p = Probe::new();
    p.read();
    let state_path = kpop_native::session_activity::temporary_directory()
        .join(format!("kpopper-view-{}", p.session)).join("state.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
    state.as_object_mut().unwrap().remove("freshness");
    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    kpop_native::view_continuation::initialize(p.root.path(), &p.session, true).unwrap();
    p.change();
    let refreshed = p.hook("UserPromptSubmit", None, json!({}), false);
    assert!(refreshed.contains("KPOPPER_SOURCE_REFRESH"), "{refreshed}");
    assert_eq!(evidence_value(&evidence_packet(&refreshed), "p.deduction")["v"], 19);
}
