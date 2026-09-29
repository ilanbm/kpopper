use kpop_native::value::TypedValue;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path, process::{Command, Stdio}};

struct Probe {
    root: tempfile::TempDir,
    session: String,
    assessment_profile: &'static str,
}

impl Probe {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let session = format!("refresh-{}", uuid::Uuid::new_v4());
        fs::write(root.path().join("GROUNDING.yaml"),
            "meta:\n  scope: Water allocation\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  p.opening: {v: 106}\n  p.inflow: {v: 42}\n  p.reserve: {v: 22}\n  p.deduction: {v: 9}\ncomputed:\n  r.usable: {rule: {op: sub, args: [{op: sub, args: [{op: add, args: [{ref: p.opening}, {ref: p.inflow}]}, {ref: p.reserve}]}, {ref: p.deduction}]}}\n").unwrap();
        fs::write(root.path().join("transcript.jsonl"),
            json!({"type":"session_meta","payload":{"id":session}}).to_string()+"\n").unwrap();
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        fs::create_dir_all(root.path().join("resources/reasoning")).unwrap();
        fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")), root.path().join("resources/reasoning").join(format!("{target}.zip"))).unwrap();
        kpop_native::view_continuation::initialize(root.path(), &session, true).unwrap();
        Self { root, session, assessment_profile: "core/v1" }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kpop"));
        c.args(["--workspace", self.root.path().to_str().unwrap(), "--frozen"])
            .env("KPOPPER_NATIVE_RESOURCES", self.root.path().join("resources"))
            .env("KPOPPER_NATIVE_CACHE", self.root.path().join("cache"))
            .env("KPOPPER_SESSION_CONFIG", self.root.path().join("preferences.json"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env_remove("KPOPPER_SESSION_DISABLE").env_remove("KPOPPER_CANONICAL_VIEW");
        c
    }
    fn session(&self, args: &[&str]) -> String {
        let out = self.command().args(["session", "--no-settings", "--input", "GROUNDING.yaml",
            "--project", "refresh-test", "--state", "state", "--assessment-profile", self.assessment_profile])
            .args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }
    fn revision(&self) -> String {
        self.session(&["open", "--tokens", "16000"]).lines()
            .find_map(|l| l.strip_prefix("project=refresh-test revision=")).unwrap().to_owned()
    }
    fn context(&self, text: &str) {
        let mut f = fs::OpenOptions::new().append(true).open(self.root.path().join("transcript.jsonl")).unwrap();
        writeln!(f, "{}", json!({"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":text}]}})).unwrap();
    }
    fn hook(&self, event: &str, turn: Option<&str>, extra: Value) -> String {
        let mut payload = json!({"session_id":self.session,"hook_event_name":event,
            "cwd":self.root.path(),"transcript_path":self.root.path().join("transcript.jsonl")});
        if let Some(turn) = turn { payload["turn_id"] = json!(turn); }
        payload.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let mut child = self.command().args(["_hook", "continuation", "codex"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
        if out.stdout.is_empty() { return String::new(); }
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_owned()
    }
    fn read(&self) -> String {
        let rev = self.revision();
        let marker = self.session(&["view", "--revision", &rev, "--tokens", "16000",
            "--max-view-bytes", "39000", "--view-format", "checked-text-tagged",
            "--expand", "group:/", "--context-session", &self.session]);
        assert!(marker.starts_with("KPOPPER_CONTEXT_QUEUED "), "{marker}");
        let frame = self.hook("PostToolUse", Some("first"), json!({"tool_response":{"output":marker}}));
        self.context(&frame);
        self.hook("Stop", Some("first"), json!({"last_assistant_message":"117 liters, from p.deduction and r.usable"}));
        rev
    }
    fn change(&self) {
        let path = self.root.path().join("GROUNDING.yaml");
        fs::write(&path, fs::read_to_string(&path).unwrap().replace("p.deduction: {v: 9}", "p.deduction: {v: 19}")).unwrap();
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        let path = kpop_native::session_activity::temporary_directory().join(format!("kpopper-view-{}", self.session));
        let _ = fs::remove_dir_all(path);
    }
}

fn packet(text: &str) -> Value {
    let start = text.find("# Checked graph view").unwrap_or_else(||
        panic!("complete checked graph must reach the next prompt: {}", text.chars().take(2048).collect::<String>()));
    let end = text[start..].find("KPOPPER_CONTEXT_END ").unwrap() + start;
    kpop_native::view_format::decode_checked_text(&text[start..end]).unwrap()
}
fn value(packet: &Value, id: &str) -> Value {
    let row = packet["nodes"].as_array().unwrap().iter()
        .find(|r| packet["dictionary"][r[0].as_str().unwrap()]["original"] == id).unwrap();
    TypedValue::from_tagged(&row[1]).unwrap().to_json().unwrap()
}

#[test]
fn next_prompt_delivers_updated_evidence_without_a_model_tool_call() {
    let p = Probe::new();
    let old = p.read();
    assert!(p.hook("UserPromptSubmit", None, json!({})).is_empty());
    p.change();
    let before = fs::read(p.root.path().join("GROUNDING.yaml")).unwrap();
    let fresh = p.hook("UserPromptSubmit", None, json!({}));
    let current = packet(&fresh);
    assert_ne!(current["revision"], old);
    assert_eq!(value(&current, "p.deduction")["v"], 19);
    let row = current["nodes"].as_array().unwrap().iter()
        .find(|r| current["dictionary"][r[0].as_str().unwrap()]["original"] == "r.usable").unwrap();
    let attributes = kpop_native::view_attributes::restore_attributes(&current, row).unwrap();
    assert_eq!(attributes["status"]["computation"]["value_text"], "107", "current computed value, not a substring in a hash");
    assert!(fresh.contains("KPOPPER_SOURCE_REFRESH"));
    assert_eq!(fs::read(p.root.path().join("GROUNDING.yaml")).unwrap(), before);
    p.context(&fresh);
    p.hook("Stop", Some("second"), json!({"last_assistant_message":"107 liters, from p.deduction and r.usable"}));
    assert!(p.hook("UserPromptSubmit", None, json!({})).is_empty());
}

#[test]
fn removed_record_invalidates_old_evidence_and_reports_unavailability() {
    let p = Probe::new();
    p.read();
    fs::remove_file(p.root.path().join("GROUNDING.yaml")).unwrap();
    let notice = p.hook("UserPromptSubmit", None, json!({}));
    assert!(notice.contains("KPOPPER_SOURCE_REFRESH"), "{notice}");
    assert!(notice.contains("\"status\":\"unavailable\""), "{notice}");
    assert!(!notice.contains("KPOPPER_CONTEXT_FRAME"));
    assert_eq!(p.hook("UserPromptSubmit", None, json!({})), notice);
}

#[test]
fn a_deleted_received_entry_is_named_and_never_copied_into_the_current_frame() {
    let p = Probe::new();
    p.read();
    let path = p.root.path().join("GROUNDING.yaml");
    fs::write(&path, fs::read_to_string(&path).unwrap().replace("  p.deduction: {v: 9}\n", "")).unwrap();
    let text = p.hook("UserPromptSubmit", None, json!({}));
    let notice = text.lines().find_map(|line| line.strip_prefix("KPOPPER_SOURCE_REFRESH ")).unwrap();
    let notice: Value = serde_json::from_str(notice).unwrap();
    assert_eq!(notice["removed_ids"], json!(["p.deduction"]));
    let current = packet(&text);
    assert!(!current["nodes"].as_array().unwrap().iter()
        .any(|row| current["dictionary"][row[0].as_str().unwrap()]["original"] == "p.deduction"));
}

#[test]
fn oversized_updated_body_is_never_cropped_or_presented_as_refreshed() {
    let p = Probe::new();
    p.read();
    let path = p.root.path().join("GROUNDING.yaml");
    fs::write(&path, fs::read_to_string(&path).unwrap().replace("p.deduction: {v: 9}",
        &format!("p.deduction: {{v: 19, note: '{}' }}", "complete source detail ".repeat(3000)))).unwrap();
    let notice = p.hook("UserPromptSubmit", None, json!({}));
    assert!(notice.contains("\"status\":\"unavailable\""), "{notice}");
    assert!(!notice.contains("KPOPPER_CONTEXT_FRAME"));
    assert!(notice.len() < 2048);
}

#[test]
fn unrelated_files_do_not_refresh_the_record() {
    let p = Probe::new();
    p.read();
    fs::write(p.root.path().join("unrelated.txt"), "something changed outside the captured source").unwrap();
    assert!(p.hook("UserPromptSubmit", None, json!({})).is_empty());
}

#[test]
fn revision_references_resolve_only_received_current_session_frames() {
    let p = Probe::new();
    let old = p.read();
    assert_eq!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), "view:1").unwrap(), old);
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), "view:999").is_err());
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), None, "view:1").is_err());
    let read = p.session(&["view", "--revision", "view:1", "--tokens", "16000", "--view-transport", "stdout", "--context-session", &p.session]);
    assert_eq!(serde_json::from_str::<Value>(&read).unwrap()["revision"], old);
    p.change();
    let fresh = p.hook("UserPromptSubmit", None, json!({}));
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), "view:1").is_err());
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), "view:2").is_err(), "issued is not received");
    let revision = packet(&fresh)["revision"].as_str().unwrap().to_owned();
    p.context(&fresh);
    assert_eq!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), "view:2").unwrap(), revision);
    p.hook("PreCompact", Some("compact"), json!({}));
    assert!(kpop_native::view_continuation::resolve_revision(p.root.path(), Some(&p.session), "view:2").is_err());
}

#[test]
fn an_unacknowledged_refresh_cannot_silently_become_current() {
    let p = Probe::new();
    p.read();
    p.change();
    let fresh = p.hook("UserPromptSubmit", None, json!({}));
    assert!(fresh.contains("KPOPPER_CONTEXT_FRAME"));
    // Simulate lost hook context: no retained developer frame and no completed turn.
    let notice = p.hook("UserPromptSubmit", None, json!({}));
    assert!(notice.contains("\"status\":\"unavailable\""), "{notice}");
    assert!(!notice.contains("KPOPPER_CONTEXT_FRAME"));
}

#[test]
fn changed_scope_requires_explicit_reopening() {
    let p = Probe::new();
    p.read();
    let path = p.root.path().join("GROUNDING.yaml");
    fs::write(&path, fs::read_to_string(&path).unwrap().replace("Water allocation", "Another project scope")).unwrap();
    let notice = p.hook("UserPromptSubmit", None, json!({}));
    assert!(notice.contains("\"status\":\"unavailable\""), "{notice}");
    assert!(notice.contains("scope changed"), "{notice}");
    assert!(!notice.contains("KPOPPER_CONTEXT_FRAME"));
}

#[test]
fn ordinary_records_with_non_string_keys_keep_their_reader_contract() {
    let mut p = Probe::new();
    p.assessment_profile = "checked-reader/v1";
    let path = p.root.path().join("GROUNDING.yaml");
    let record = fs::read_to_string(&path).unwrap()
        .replace("  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\n", "")
        .replace("known:\n", "known:\n  p.map:\n    v:\n      true: yes\n      1: one\n");
    fs::write(&path, record).unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM").map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
            .join(".cache/kpopper/lean").join(&target).join(env!("KPOP_ORDINARY_SOURCE_SHA256")));
    let destination = p.root.path().join("resources/ordinary").join(target);
    fs::create_dir_all(&destination).unwrap();
    for name in ["build.json", if cfg!(windows) { "epistemic-core.exe" } else { "epistemic-core" }] {
        fs::copy(ordinary.join(name), destination.join(name)).unwrap();
    }
    p.read();
    assert!(p.hook("UserPromptSubmit", None, json!({})).is_empty());
    p.change();
    let text = p.hook("UserPromptSubmit", None, json!({}));
    assert_eq!(value(&packet(&text), "p.deduction")["v"], 19);
}

#[test]
fn refresh_does_not_adopt_a_profile_created_after_the_original_read() {
    let p = Probe::new();
    p.read();
    fs::create_dir_all(p.root.path().join(".kpopper")).unwrap();
    fs::write(p.root.path().join(".kpopper/session.json"),
        json!({"scope":"Unrequested replacement scope","groups":{"new":["p.deduction"]}}).to_string()).unwrap();
    assert!(p.hook("UserPromptSubmit", None, json!({})).is_empty());
    p.change();
    let current = packet(&p.hook("UserPromptSubmit", None, json!({})));
    assert_ne!(current["scope"], "Unrequested replacement scope");
    assert_eq!(value(&current, "p.deduction")["v"], 19);
}

#[test]
fn disabling_session_preferences_stops_automatic_refresh_in_an_existing_session() {
    let p = Probe::new();
    p.read();
    p.change();
    let settings = p.root.path().join("preferences.json");
    fs::write(&settings, json!({"schema":1,"enabled":false}).to_string()).unwrap();
    assert!(p.hook("UserPromptSubmit", None, json!({})).is_empty());
    fs::write(&settings, json!({"schema":1,"enabled":true,"native":env!("CARGO_BIN_EXE_kpop"),"tokens":16000}).to_string()).unwrap();
    let current = packet(&p.hook("UserPromptSubmit", None, json!({})));
    assert_eq!(value(&current, "p.deduction")["v"], 19);
}

#[test]
fn large_updates_keep_unread_evidence_folded_and_current_support_complete() {
    let p = Probe::new();
    p.read();
    p.change();
    let path = p.root.path().join("GROUNDING.yaml");
    let mut extra = String::new();
    for index in 0..900 {
        extra.push_str(&format!("  background.item{index}: {{v: '{}' }}\n", "unread evidence ".repeat(32)));
    }
    fs::write(&path, fs::read_to_string(&path).unwrap().replace("computed:\n", &(extra + "computed:\n"))).unwrap();
    let text = p.hook("UserPromptSubmit", None, json!({}));
    let refreshed = packet(&text);
    assert_eq!(refreshed["coverage"]["count"], 905);
    assert_eq!(refreshed["nodes"].as_array().unwrap().len(), 5);
    assert_eq!(value(&refreshed, "p.deduction")["v"], 19);
    assert!(!refreshed["groups"].as_array().unwrap().is_empty());
    assert!(text.len() <= 39000);
}

#[test]
fn refresh_reads_a_new_dependency_of_previously_received_evidence() {
    let p = Probe::new();
    p.read();
    let path = p.root.path().join("GROUNDING.yaml");
    let text = fs::read_to_string(&path).unwrap().replace("known:\n", "known:\n  p.extra: {v: 5}\n")
        .replace("{ref: p.deduction}", "{ref: p.extra}");
    fs::write(&path, text).unwrap();
    let text = p.hook("UserPromptSubmit", None, json!({}));
    assert_eq!(value(&packet(&text), "p.extra")["v"], 5);
}
