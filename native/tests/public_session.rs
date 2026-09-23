use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: tempfile::TempDir,
    private: tempfile::TempDir,
    resources: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        let resources = private.path().join("resources");
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        fs::create_dir_all(resources.join("reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.kpopper-runtime")),
            resources.join("reasoning").join(format!("{target}.zip")),
        )
        .unwrap();
        let ordinary = resources.join("ordinary").join(&target);
        fs::create_dir_all(&ordinary).unwrap();
        let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(
                    std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .unwrap(),
                )
                .join(".cache/kpopper/lean")
                .join(if cfg!(windows) {
                    "windows-amd64"
                } else {
                    &target
                })
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
            });
        for name in [
            "build.json",
            if cfg!(windows) {
                "epistemic-core.exe"
            } else {
                "epistemic-core"
            },
        ] {
            fs::copy(program.join(name), ordinary.join(name)).unwrap();
        }
        Self {
            root,
            private,
            resources,
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
        command
            .current_dir(self.root.path())
            .env("KPOPPER_AGENT_SESSION", "flow")
            .env_remove("CODEX_THREAD_ID")
            .env("TMPDIR", self.private.path())
            .env("KPOPPER_NATIVE_RESOURCES", &self.resources)
            .env("KPOPPER_NATIVE_CACHE", self.private.path().join("cache"))
            .env("XDG_STATE_HOME", self.private.path().join("state"));
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn hook(&self, command: &str, extra: Value) -> Output {
        self.hook_with(&[command], extra)
    }
    fn hook_with(&self, args: &[&str], extra: Value) -> Output {
        let mut payload = json!({"cwd":self.root.path(),"session_id":"flow"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut child = self
            .command()
            .args(args)
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
fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn prompt_context_is_once_per_finding_and_resume_preserves_the_original_mark() {
    let f = Fixture::new();
    success(f.hook("session-start", json!({})));
    let state = f.private.path().join("kpopper-base-flow");
    let baseline = fs::read(&state).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&baseline).unwrap()["ids"],
        json!([])
    );
    assert!(!f.root.path().join("GROUNDING.yaml").exists());
    success(f.run(&["add", "p.x", "v=1"]));
    success(f.hook("session-start", json!({"source":"resume"})));
    assert_eq!(fs::read(&state).unwrap(), baseline);
    let stopped = success(f.hook("session-stop", json!({})));
    assert!(stopped.stdout.is_empty() && stopped.stderr.is_empty());
    let child = success(f.hook("session-context", json!({"agent_id":"child"})));
    assert!(child.stdout.is_empty() && child.stderr.is_empty());
    let first = f.hook("session-context", json!({}));
    assert_eq!(
        first.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&first.stdout)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("p.x"));
    let repeat = success(f.hook("session-context", json!({"stop_hook_active":true})));
    assert!(repeat.stdout.is_empty());
    success(f.run(&["add", "p.y", "v=2"]));
    let next = f.hook("session-context", json!({"stop_hook_active":true}));
    assert_eq!(next.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&next.stdout).contains("p.y"));
    assert!(!String::from_utf8_lossy(&next.stdout).contains("p.x"));
    success(
        f.command()
            .env("KPOPPER_AGENT_SESSION", "other")
            .args(["add", "p.external", "v=3"])
            .output()
            .unwrap(),
    );
    assert!(
        success(f.hook("session-context", json!({})))
            .stderr
            .is_empty()
    );
}
#[test]
fn a_workspace_without_a_record_gets_no_prompt_context_until_its_first_write() {
    let f = Fixture::new();
    git(f.root.path(), &["init", "-q"]);
    fs::write(f.root.path().join("README.md"), "# empty\n").unwrap();
    git(f.root.path(), &["add", "README.md"]);
    git(f.root.path(), &["commit", "-qm", "README"]);
    success(f.hook(
        "session-start",
        json!({"hook_event_name":"SessionStart","source":"startup"}),
    ));
    assert!(f.private.path().join("kpopper-base-flow").is_file());
    for event in ["UserPromptSubmit", "PostToolUse"] {
        let quiet = success(f.hook_with(
            &["session-context", "--event", event, "--host", "claude"],
            json!({"hook_event_name":event,"prompt":"plan my week"}),
        ));
        assert!(
            quiet.stdout.is_empty() && quiet.stderr.is_empty(),
            "{event}: stdout={} stderr={}",
            String::from_utf8_lossy(&quiet.stdout),
            String::from_utf8_lossy(&quiet.stderr)
        );
    }
    assert!(!f.root.path().join("GROUNDING.yaml").exists());
    // The empty mark still assesses the record the session's first write creates.
    success(f.run(&["add", "p.x", "v=1"]));
    let first = success(f.hook_with(
        &[
            "session-context",
            "--event",
            "UserPromptSubmit",
            "--host",
            "claude",
        ],
        json!({"hook_event_name":"UserPromptSubmit","prompt":"plan my week"}),
    ));
    assert!(String::from_utf8_lossy(&first.stdout).contains("p.x"));
}
#[test]
fn a_record_that_can_no_longer_be_assessed_is_reported_as_unavailable() {
    let f = Fixture::new();
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        "known: {p.x: {v: 1}}\n",
    )
    .unwrap();
    success(f.hook("session-start", json!({})));
    assert!(f.private.path().join("kpopper-base-flow").is_file());
    fs::write(f.root.path().join("GROUNDING.yaml"), "known: [p.x\n").unwrap();
    for event in ["UserPromptSubmit", "PostToolUse"] {
        let prompt = success(f.hook_with(
            &["session-context", "--event", event, "--host", "claude"],
            json!({"hook_event_name":event,"prompt":"plan my week"}),
        ));
        assert!(
            prompt.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&prompt.stderr)
        );
        let context: Value = serde_json::from_slice(&prompt.stdout).unwrap();
        assert_eq!(context["hookSpecificOutput"]["hookEventName"], event);
        assert!(
            context["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap()
                .ends_with(
                    "The record assessment is unavailable. Run kpop check before relying on the record."
                ),
            "{context}"
        );
    }
}
#[test]
fn ordinary_mark_and_gate_ignore_layout_and_never_block_for_untouched_records() {
    let f = Fixture::new();
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        "known: {p.x: {v: 1}}\n",
    )
    .unwrap();
    fs::create_dir(f.root.path().join(".kpopper")).unwrap();
    fs::write(f.root.path().join(".kpopper/view.yaml"), "invalid: [yaml").unwrap();
    success(f.hook("session-start", json!({})));
    fs::write(
        f.private.path().join("kpopper-ground-flow.json"),
        "{\"turns\":8}",
    )
    .unwrap();
    let first = success(f.hook("session-stop", json!({})));
    assert!(first.stdout.is_empty());
    assert!(first.stderr.is_empty());
    assert!(success(f.hook("session-stop", json!({}))).stderr.is_empty());
}
#[test]
fn direct_gate_reports_new_failures_and_invalid_session_ids_create_no_mark() {
    let f = Fixture::new();
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        "known: {p.x: {v: 1}}\n",
    )
    .unwrap();
    let path = f.private.path().join("mark.json");
    success(f.run(&["mark", path.to_str().unwrap()]));
    fs::write(f.root.path().join("GROUNDING.yaml"), "schema: {deps: rests_on, predicate: wrong_if}\nknown: {p.x: {v: 1}}\njudgments: {d.bad: {rests_on: [absent], wrong_if: 'p.x > 2'}}\n").unwrap();
    let output = f.run(&["gate", path.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stdout).contains("FAIL d.bad:"));
    success(f.hook("session-start", json!({"session_id":"../invalid"})));
    assert!(!f.private.path().join("kpopper-base-../invalid").exists());
}

#[test]
fn an_unreadable_record_keeps_the_session_context_and_leaves_no_baseline() {
    let f = Fixture::new();
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        "known:\n  local.one: {v: 1}\njudgments:\n  d.use_limit:\n    verdict: Batch requests at the vendor limit\n    rests_on: [api.limit]\n    seen: {api.limit: 10}\n    wrong_if: api.limit > 20\n",
    )
    .unwrap();
    let opened = success(f.hook("session-start", json!({})));
    let stdout = String::from_utf8(opened.stdout).unwrap();
    let stderr = String::from_utf8(opened.stderr).unwrap();
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 3, "stdout={stdout} stderr={stderr}");
    assert_eq!(
        lines[0],
        format!(
            "The knowledge record could not be opened. Read it before relying on it: {}",
            f.root
                .path()
                .canonicalize()
                .unwrap()
                .join("GROUNDING.yaml")
                .display()
        )
    );
    let context: Value =
        serde_json::from_str(lines[1].strip_prefix("KPOPPER_AGENT_CONTEXT ").unwrap()).unwrap();
    assert_eq!(context["environment"]["KPOPPER_AGENT_SESSION"], "flow");
    assert!(lines[2].starts_with("Pass this session environment"));
    // The reader's diagnostic appears once; the baseline it prevented adds nothing.
    let diagnostic = stderr.lines().next().unwrap();
    assert_eq!(stderr.matches(diagnostic).count(), 1, "{stderr}");
    assert!(
        !stderr.contains("not opened") && !stderr.contains("baseline"),
        "{stderr}"
    );
    // An empty baseline would report the record's existing failures as this
    // session's once it reads again; without one, prompt diagnostics stay silent.
    assert!(!f.private.path().join("kpopper-base-flow").exists());
    let prompt = success(f.hook("session-context", json!({})));
    assert!(prompt.stdout.is_empty() && prompt.stderr.is_empty());
}

#[test]
fn a_baseline_that_cannot_be_saved_leaves_the_opening_unchanged() {
    let f = Fixture::new();
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        "known: {p.x: {v: 1}}\n",
    )
    .unwrap();
    let baseline = f.private.path().join("kpopper-base-flow");
    fs::create_dir(&baseline).unwrap();
    let blocked = success(f.hook("session-start", json!({})));
    let stderr = String::from_utf8(blocked.stderr).unwrap();
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("kpop: session baseline was not saved: "))
            .count(),
        1,
        "{stderr}"
    );
    assert!(baseline.is_dir());
    fs::remove_dir(&baseline).unwrap();
    let saved = success(f.hook("session-start", json!({})));
    assert!(saved.stderr.is_empty());
    assert!(baseline.is_file());
    assert_eq!(
        String::from_utf8(blocked.stdout).unwrap(),
        String::from_utf8(saved.stdout).unwrap()
    );
}

#[test]
fn core_only_resources_support_public_reads_history_and_sessions() {
    let f = Fixture::new();
    fs::remove_dir_all(f.resources.join("ordinary")).unwrap();
    success(f.hook("session-start", json!({})));
    success(f.run(&["add", "p.x", "v=1"]));
    for args in [
        &["open"][..],
        &["check"],
        &["pull", "p.x"],
        &["affects", "p.x"],
        &["assess", "p.x"],
        &["page", "--verify"],
    ] {
        success(f.run(args));
    }
    success(f.hook("session-start", json!({})));
    assert!(success(f.hook("session-stop", json!({}))).stderr.is_empty());
    let legacy = f.private.path().join("legacy");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("GROUNDING.yaml"), "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nknown: {p.x: {v: 1}}\n").unwrap();
    success(
        f.command()
            .current_dir(legacy)
            .args(["history", "migrate"])
            .output()
            .unwrap(),
    );
    // An unrelated, corrupt ordinary program must not poison core commands.
    let ordinary = f
        .resources
        .join("ordinary")
        .join(kpop_native::reasoning_runtime::target_name().unwrap());
    fs::create_dir_all(&ordinary).unwrap();
    fs::write(ordinary.join("build.json"), "invalid").unwrap();
    success(f.run(&["check"]));
}

fn session_start(f: &Fixture, host: &str) -> Output {
    let mut child = f
        .command()
        .args(["session-start", "--host", host])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            json!({"cwd":f.root.path(),"session_id":"flow"})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    child.wait_with_output().unwrap()
}

/// The hook's opening is `open` in the 2000-character slot, with the next moves named as
/// its host invokes them; a core/v1 record, which has no next moves to name, still opens
/// under a hook that names its host, while a person's `--host` on it is refused.
#[test]
fn session_start_fills_the_slot_for_its_host_and_opens_a_core_record() {
    let f = Fixture::new();
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        include_str!("fixtures/opener/slot.yaml"),
    )
    .unwrap();
    for (host, next) in [
        ("claude", "next: /kpopper:ground <entry|prefix>"),
        ("codex", "next: $ground <entry|prefix>"),
    ] {
        let opening = String::from_utf8(success(f.run(&["open", "--host", host])).stdout).unwrap();
        assert!(
            opening.contains(&format!("  ... 14 more - raise --chars\n\n{next}")),
            "{opening}"
        );
        let started = String::from_utf8(success(session_start(&f, host)).stdout).unwrap();
        assert!(started.starts_with(&opening), "{started}");
        assert!(
            started[opening.len()..].starts_with("KPOPPER_AGENT_CONTEXT "),
            "{started}"
        );
    }
    fs::write(
        f.root.path().join("GROUNDING.yaml"),
        include_str!("fixtures/core-page/GROUNDING.yaml"),
    )
    .unwrap();
    let started = String::from_utf8(success(session_start(&f, "claude")).stdout).unwrap();
    assert!(
        started.starts_with("Core page fixture\ncore/v1 snapshot "),
        "{started}"
    );
    let refused = f.run(&["open", "--host", "claude"]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("core_profile_option_unsupported: --host"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}
