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
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
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
        let mut payload = json!({"cwd":self.root.path(),"session_id":"flow"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let mut child = self
            .command()
            .arg(command)
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
#[test]
fn first_write_stop_is_once_per_finding_and_resume_preserves_the_original_mark() {
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
    let first = f.hook("session-stop", json!({}));
    assert_eq!(
        first.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stderr).contains("p.x"));
    let repeat = success(f.hook("session-stop", json!({"stop_hook_active":true})));
    assert!(repeat.stderr.is_empty());
    success(f.run(&["add", "p.y", "v=2"]));
    let next = f.hook("session-stop", json!({"stop_hook_active":true}));
    assert_eq!(next.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&next.stderr).contains("p.y"));
    assert!(!String::from_utf8_lossy(&next.stderr).contains("p.x"));
    success(
        f.command()
            .env("KPOPPER_AGENT_SESSION", "other")
            .args(["add", "p.external", "v=3"])
            .output()
            .unwrap(),
    );
    assert!(success(f.hook("session-stop", json!({}))).stderr.is_empty());
}
#[test]
fn ordinary_mark_and_gate_ignore_layout_and_nudge_once() {
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
    let first = f.hook("session-stop", json!({}));
    assert_eq!(first.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&first.stderr).contains("8 prompts in"));
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
