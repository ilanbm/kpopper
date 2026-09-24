//! Lifecycle findings reach the agent through the packaged shell hooks as context.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

const RECORD: &str =
    "sources:\n  s.file: {file: source.txt}\nknown:\n  p.old: {v: 1, from: s.file}\n";
const SCHEMA: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n";
const SID: &str = "gate-delivery";

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(fixture.work()).unwrap();
        fs::create_dir_all(fixture.private()).unwrap();
        fs::write(
            fixture.work().join("source.txt"),
            "A source in this project.\n",
        )
        .unwrap();
        fixture.save(RECORD);
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for name in ["session_gate.sh", "hook.sh", "native_runtime.sh"] {
            fs::create_dir_all(fixture.scripts()).unwrap();
            fs::copy(
                repo.join("scripts").join(name),
                fixture.scripts().join(name),
            )
            .unwrap();
        }
        let binary = fixture
            .scripts()
            .join("runtime")
            .join(kpop_native::reasoning_runtime::target_name().unwrap())
            .join("kpop");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        symlink(env!("CARGO_BIN_EXE_kpop"), binary).unwrap();
        // Establish the baseline through the same native route the installed opener uses.
        let opened = fixture.hook("hook.sh", &["session_start.py", "--host", "codex"]);
        successful(&opened);
        assert!(fixture.mark().is_file(), "{}", diagnostic(&opened));
        fixture
    }

    fn work(&self) -> PathBuf {
        self.root.path().join("project with spaces")
    }
    fn scripts(&self) -> PathBuf {
        self.root.path().join("plugin with spaces/scripts")
    }
    fn private(&self) -> PathBuf {
        self.root.path().join("tmp")
    }
    fn mark(&self) -> PathBuf {
        self.private().join(format!("kpopper-base-{SID}"))
    }
    fn save(&self, text: &str) {
        fs::write(self.work().join("GROUNDING.yaml"), text).unwrap();
    }
    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(self.work())
            .env("TMPDIR", self.private())
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("KPOPPER_NATIVE_CACHE", self.root.path().join("cache"))
            .env("KPOPPER_AGENT_SESSION", SID)
            .env("KPOPPER_READ_MODE", "frozen");
        for name in [
            "KPOPPER_RUNTIME",
            "KPOPPER_ROOT",
            "KPOPPER_NATIVE_RESOURCES",
            "KPOPPER_SESSION_CONFIG",
            "KPOPPER_SESSION_DISABLE",
            "CODEX_THREAD_ID",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
        ] {
            command.env_remove(name);
        }
        command
    }
    fn cli(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_kpop"))
            .args(args)
            .output()
            .unwrap()
    }
    fn hook(&self, script: &str, args: &[&str]) -> Output {
        let payload = json!({
            "session_id": SID,
            "cwd": self.work(),
            "hook_event_name": "UserPromptSubmit",
            "prompt": "Which version won?",
        });
        let mut child = self
            .command("sh")
            .arg(self.scripts().join(script))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Stop intentionally exits before reading its payload.
        if let Err(error) = child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
        {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        child.wait_with_output().unwrap()
    }
    fn gate(&self, args: &[&str]) -> Output {
        self.hook("session_gate.sh", args)
    }
    fn context(&self) -> Output {
        self.gate(&["--context", "UserPromptSubmit"])
    }
}

fn diagnostic(output: &Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn successful(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", diagnostic(output));
    assert!(output.stderr.is_empty(), "{}", diagnostic(output));
}
fn silent(output: &Output) {
    successful(output);
    assert!(output.stdout.is_empty(), "{}", diagnostic(output));
}
fn context(output: &Output, fragment: &str) -> String {
    successful(output);
    let reply: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", diagnostic(output)));
    assert_eq!(reply.as_object().unwrap().len(), 1, "{reply}");
    let body = &reply["hookSpecificOutput"];
    assert_eq!(body.as_object().unwrap().len(), 2, "{reply}");
    assert_eq!(body["hookEventName"], "UserPromptSubmit");
    let text = body["additionalContext"].as_str().unwrap();
    assert!(text.contains(fragment), "{text}");
    assert!(
        text.contains("Complete the user's current request"),
        "{text}"
    );
    text.to_owned()
}

#[test]
fn missing_intent_is_silent_at_stop_and_context_on_the_next_prompt() {
    let f = Fixture::new();
    successful(&f.cli(&["add", "p.new", "v=2", "from=s.file"]));
    for _ in 0..2 {
        silent(&f.gate(&["--host", "codex"]));
    }
    context(
        &f.gate(&["--host", "codex", "--context", "UserPromptSubmit"]),
        "p.new",
    );
    silent(&f.context());
}

#[test]
fn structural_failures_are_context_but_explicit_checks_still_fail() {
    let f = Fixture::new();
    f.save(&format!(
        "{RECORD}{SCHEMA}judgments:\n  d.bad: {{verdict: bad, rests_on: [p.missing], seen: {{}}}}\n"
    ));
    silent(&f.gate(&[]));
    context(&f.context(), "d.bad");
    let gate = f.cli(&["gate", f.mark().to_str().unwrap()]);
    assert_eq!(gate.status.code(), Some(2), "{}", diagnostic(&gate));
    let check = f.cli(&["check"]);
    assert!(!check.status.success(), "{}", diagnostic(&check));
    assert!(
        String::from_utf8_lossy(&check.stdout).contains("d.bad"),
        "{}",
        diagnostic(&check)
    );
}

#[test]
fn failed_assessment_cannot_block_or_replace_the_request() {
    let f = Fixture::new();
    f.save("not: [valid yaml");
    silent(&f.gate(&[]));
    context(&f.context(), "unavailable");
}

#[test]
fn resolved_findings_are_not_replayed_and_omitted_findings_are_not_consumed() {
    let f = Fixture::new();
    let mut record = format!("{RECORD}{SCHEMA}judgments:\n");
    for i in 0..10 {
        record.push_str(&format!(
            "  d.bad{i}: {{verdict: bad, rests_on: [p.missing{i}], seen: {{}}}}\n"
        ));
    }
    f.save(&record);
    let mut delivered = context(&f.context(), "d.bad0");
    // More findings than one context can hold must remain eligible for later prompts.
    assert!((0..10).any(|i| !delivered.contains(&format!("d.bad{i}"))));
    for _ in 0..5 {
        let next = f.context();
        if next.stdout.is_empty() {
            silent(&next);
        } else {
            delivered.push_str(&context(&next, "d.bad"));
        }
    }
    for i in 0..10 {
        assert!(delivered.contains(&format!("d.bad{i}")), "{delivered}");
    }
    silent(&f.context());
    f.save(RECORD);
    silent(&f.context());
}
