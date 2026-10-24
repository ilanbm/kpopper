use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    process::{Command, Output, Stdio},
};

struct Fixture {
    work: tempfile::TempDir,
    state: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            work: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
        };
        fs::write(
            f.work.path().join("GROUNDING.yaml"),
            "known:\n  p.seed: {v: 1}\n",
        )
        .unwrap();
        f
    }

    fn hook(&self, event: &str, extra: Value) -> Output {
        self.host_hook("codex", event, extra)
    }

    fn host_hook(&self, host: &str, event: &str, extra: Value) -> Output {
        let mut payload = json!({"cwd": self.work.path(), "session_id": "handoff-test",
                                 "turn_id": "turn-one", "hook_event_name": event});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kpop"));
        cmd.current_dir(self.work.path())
            .env("TMPDIR", self.state.path())
            .env("XDG_STATE_HOME", self.state.path().join("state"))
            .env_remove("CODEX_THREAD_ID")
            .env_remove("KPOPPER_AGENT_SESSION");
        if event == "Stop" {
            cmd.arg("session-stop");
        } else {
            cmd.args(["session-context", "--host", host, "--event", event]);
        }
        let mut child = cmd
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
        child.wait_with_output().unwrap()
    }
}

fn context(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    if out.stdout.is_empty() {
        return String::new();
    }
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    assert!(v.get("decision").is_none() && v.get("continue").is_none());
    v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn first_tool_delivers_handoff_with_no_git_or_record_change_and_stop_stays_silent() {
    let f = Fixture::new();
    let before = fs::read(f.work.path().join("GROUNDING.yaml")).unwrap();
    let text = context(f.hook(
        "PostToolUse",
        json!({"tool_response": "new source findings"}),
    ));
    assert!(text.contains("Before finishing"), "{text}");
    assert!(text.contains("read back"), "{text}");
    assert!(text.contains("run check"), "{text}");
    assert!(text.contains("every write and correction"), "{text}");
    assert!(text.contains("outside"), "{text}");
    assert!(text.contains("authority"), "{text}");
    assert!(text.contains("pending"), "{text}");
    assert!(context(f.hook("PostToolUse", json!({}))).is_empty());
    let stopped = f.hook("Stop", json!({}));
    assert!(stopped.status.success() && stopped.stdout.is_empty() && stopped.stderr.is_empty());
    assert_eq!(
        before,
        fs::read(f.work.path().join("GROUNDING.yaml")).unwrap()
    );
}

#[test]
fn canonical_grounding_reads_do_not_consume_the_task_handoff() {
    let f = Fixture::new();
    for command in [
        "/plugin/kpop --workspace /project session view --input /record/GROUNDING.yaml",
        "kpop session read --ref node:p.seed# --revision abc",
        "kpop --workspace=/project session view",
        r"C:\plugin\kpop.exe --workspace C:\project session view",
        r"& 'C:\Program Files\kpop.exe' --workspace=C:\project session view",
    ] {
        assert!(context(f.hook("PostToolUse", json!({
            "tool_name":"Bash", "tool_input":{"command":command},
            "tool_response":"# Checked graph view"
        }))).is_empty());
    }
    let text = context(f.hook("PostToolUse", json!({
        "tool_name":"Bash", "tool_input":{"command":"cat /source.txt"},
        "tool_response":"The new load limit is 731 kg."
    })));
    assert!(text.contains("record skill"));
    assert!(context(f.hook("PostToolUse", json!({}))).is_empty());
}

#[test]
fn a_compound_grounding_and_source_read_is_still_a_task_result() {
    for command in ["kpop session view && cat /source.txt", r"& 'C:\plugin\kpop.exe' session view; Get-Content source.txt"] {
        let f = Fixture::new();
        assert!(!context(f.hook("PostToolUse", json!({
            "tool_name":"Bash", "tool_input":{"command":command}
        }))).is_empty());
    }
}

#[test]
fn a_new_turn_is_reminded_even_after_a_partial_record_write() {
    let f = Fixture::new();
    assert!(!context(f.hook("PostToolUse", json!({}))).is_empty());
    fs::write(
        f.work.path().join("GROUNDING.yaml"),
        "known:\n  p.seed: {v: 2}\n",
    )
    .unwrap();
    let text = context(f.hook("PostToolUse", json!({"turn_id": "turn-two"})));
    assert!(text.contains("all material"), "{text}");
    assert!(!text.contains("record untouched"));
    assert!(context(f.hook("PostToolUse", json!({"turn_id": "turn-two"}))).is_empty());
}

#[test]
fn missing_records_and_child_agents_do_not_get_a_recording_task() {
    let f = Fixture::new();
    assert!(context(f.hook("PostToolUse", json!({"agent_id": "child"}))).is_empty());
    fs::remove_file(f.work.path().join("GROUNDING.yaml")).unwrap();
    assert!(context(f.hook("PostToolUse", json!({}))).is_empty());
    assert!(!f.work.path().join("GROUNDING.yaml").exists());
}

#[test]
fn prompt_marker_supports_hosts_without_a_turn_id_and_state_stays_bounded() {
    let f = Fixture::new();
    for _ in 1..=12 {
        assert!(
            f.host_hook("claude", "UserPromptSubmit", json!({"turn_id":null}))
                .status
                .success()
        );
        assert!(!context(f.hook("PostToolUse", json!({"turn_id": null}))).is_empty());
        assert!(context(f.hook("PostToolUse", json!({"turn_id": null}))).is_empty());
    }
    let home = f.state.path().join("kpopper-session-handoff-test");
    let notices: Vec<_> = fs::read_dir(home)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name().to_string_lossy().starts_with("handoff-")
                && e.file_name() != "handoff-context.json"
        })
        .collect();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].metadata().unwrap().len() < 512);
}

#[test]
fn claude_prompt_marker_advances_even_when_the_record_cannot_be_parsed() {
    let f = Fixture::new();
    for turn in 0..3 {
        if turn == 1 {
            fs::write(
                f.work.path().join("GROUNDING.yaml"),
                "known: [invalid yaml\n",
            )
            .unwrap();
        }
        assert!(
            f.host_hook("claude", "UserPromptSubmit", json!({"turn_id":null}))
                .status
                .success()
        );
        assert!(!context(f.host_hook("claude", "PostToolUse", json!({"turn_id":null}))).is_empty());
        assert!(context(f.host_hook("claude", "PostToolUse", json!({"turn_id":null}))).is_empty());
    }
    let invalid = Fixture::new();
    fs::write(
        invalid.work.path().join("GROUNDING.yaml"),
        "known: [invalid yaml\n",
    )
    .unwrap();
    assert!(
        invalid
            .host_hook("claude", "UserPromptSubmit", json!({"turn_id":null}))
            .status
            .success()
    );
    assert!(
        !context(invalid.host_hook("claude", "PostToolUse", json!({"turn_id":null}))).is_empty()
    );
}

#[test]
fn compaction_rearms_once_without_changing_the_record_or_starting_a_turn() {
    for (host, event, turn) in [
        ("claude", "SessionStart", Value::Null),
        ("codex", "PostCompact", json!("turn-one")),
    ] {
        let f = Fixture::new();
        let before = fs::read(f.work.path().join("GROUNDING.yaml")).unwrap();
        assert!(
            f.host_hook(host, "UserPromptSubmit", json!({"turn_id":turn}))
                .status
                .success()
        );
        assert!(!context(f.host_hook(host, "PostToolUse", json!({"turn_id":turn}))).is_empty());
        let reset = f.host_hook(host, event, json!({"turn_id":turn,"source":"compact"}));
        assert!(reset.status.success() && reset.stdout.is_empty() && reset.stderr.is_empty());
        assert!(!context(f.host_hook(host, "PostToolUse", json!({"turn_id":turn}))).is_empty());
        assert!(context(f.host_hook(host, "PostToolUse", json!({"turn_id":turn}))).is_empty());
        assert_eq!(
            before,
            fs::read(f.work.path().join("GROUNDING.yaml")).unwrap()
        );
    }
}

#[test]
fn unidentified_tool_results_do_not_fall_back_to_full_record_assessment() {
    let f = Fixture::new();
    fs::write(f.state.path().join("kpopper-base-handoff-test"), "{}").unwrap();
    fs::write(
        f.work.path().join("GROUNDING.yaml"),
        "known: [invalid yaml\n",
    )
    .unwrap();
    assert!(context(f.host_hook("claude", "PostToolUse", json!({"turn_id":null}))).is_empty());
}

#[test]
fn late_tool_result_from_an_old_turn_cannot_consume_the_current_notice() {
    let f = Fixture::new();
    assert!(
        f.hook("UserPromptSubmit", json!({"turn_id":"turn-two"}))
            .status
            .success()
    );
    assert!(context(f.hook("PostToolUse", json!({"turn_id":"turn-one"}))).is_empty());
    assert!(!context(f.hook("PostToolUse", json!({"turn_id":"turn-two"}))).is_empty());
    assert!(context(f.hook("PostToolUse", json!({"turn_id":"turn-two"}))).is_empty());
}

#[test]
fn recreated_prompt_state_does_not_collide_with_a_previous_notice() {
    let f = Fixture::new();
    assert!(
        f.host_hook("claude", "UserPromptSubmit", json!({"turn_id":null}))
            .status
            .success()
    );
    assert!(!context(f.host_hook("claude", "PostToolUse", json!({"turn_id":null}))).is_empty());
    fs::remove_file(
        f.state
            .path()
            .join("kpopper-session-handoff-test/handoff-context.json"),
    )
    .unwrap();
    assert!(context(f.host_hook("claude", "PostToolUse", json!({"turn_id":null}))).is_empty());
    assert!(
        f.host_hook("claude", "UserPromptSubmit", json!({"turn_id":null}))
            .status
            .success()
    );
    assert!(!context(f.host_hook("claude", "PostToolUse", json!({"turn_id":null}))).is_empty());
}

#[test]
fn concurrent_tools_prepare_only_one_notice() {
    let f = Fixture::new();
    let outputs = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| f.hook("PostToolUse", json!({}))))
            .collect();
        handles
            .into_iter()
            .map(|h| context(h.join().unwrap()))
            .collect::<Vec<_>>()
    });
    assert_eq!(outputs.iter().filter(|text| !text.is_empty()).count(), 1);
}

#[test]
fn external_canonical_record_is_not_confused_with_the_workspace() {
    let f = Fixture::new();
    let external = tempfile::tempdir().unwrap();
    let record = external.path().join("GROUNDING.yaml");
    fs::rename(f.work.path().join("GROUNDING.yaml"), &record).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(f.work.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        f.work.path().join(".git/kpopper-record"),
        format!("{}\n", record.display()),
    )
    .unwrap();
    let before = fs::read(&record).unwrap();
    assert!(!context(f.hook("PostToolUse", json!({}))).is_empty());
    assert!(!f.work.path().join("GROUNDING.yaml").exists());
    assert_eq!(before, fs::read(record).unwrap());
}

#[test]
fn each_record_has_one_notice_and_returning_to_it_does_not_replay() {
    let first = Fixture::new();
    let second = Fixture::new();
    assert!(!context(first.hook("PostToolUse", json!({}))).is_empty());
    assert!(!context(first.hook("PostToolUse", json!({"cwd": second.work.path()}))).is_empty());
    assert!(context(first.hook("PostToolUse", json!({}))).is_empty());
    assert!(context(first.hook("PostToolUse", json!({"cwd": second.work.path()}))).is_empty());
}

#[cfg(unix)]
#[test]
fn unavailable_private_state_does_not_block_and_a_later_tool_can_retry() {
    let f = Fixture::new();
    let link = f.state.path().join("kpopper-session-handoff-test");
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    assert!(context(f.hook("PostToolUse", json!({}))).is_empty());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    fs::remove_file(link).unwrap();
    assert!(!context(f.hook("PostToolUse", json!({}))).is_empty());
}
