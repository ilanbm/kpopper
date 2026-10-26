//! Black-box contract tests for the simple-default onboarding surface.
//!
//! These deliberately exercise only temporary fixtures and give every process its
//! own HOME, state directory, and central projects root.

use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    state: PathBuf,
    projects: PathBuf,
}

impl Fixture {
    fn new(git: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let home = temp.path().join("home");
        let state = temp.path().join("state");
        let projects = temp.path().join("projects");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&projects).unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            home,
            state,
            projects,
        };
        if git {
            fixture.git(&["init", "-q", "-b", "main"]);
            fixture.git(&["config", "user.name", "Fixture"]);
            fixture.git(&["config", "user.email", "fixture@example.test"]);
        }
        fixture
    }

    fn git(&self, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
        command
            .args(args)
            .current_dir(&self.root)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", &self.state)
            .env("KPOPPER_PROJECTS_DIR", &self.projects)
            .env("TMPDIR", self._temp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("KPOPPER_READ_MODE");
        command
    }

    fn cli(&self, args: &[&str]) -> Value {
        let output = self.command(args).arg("--json").output().unwrap();
        assert!(
            output.status.success(),
            "args={args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn session_start(&self) -> Output {
        let mut child = self
            .command(&["session-start", "--host", "codex"])
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
                json!({"cwd": self.root, "session_id": "simple-default-test"})
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn tree(root: &Path) -> Vec<PathBuf> {
    fn walk(path: &Path, out: &mut Vec<PathBuf>) {
        if !path.exists() {
            return;
        }
        out.push(path.to_path_buf());
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries {
                walk(&entry.unwrap().path(), out);
            }
        }
    }
    let mut result = Vec::new();
    walk(root, &mut result);
    result.sort();
    result
}

#[test]
fn fresh_git_and_non_git_projects_have_silent_simple_onboarding() {
    for git in [true, false] {
        let fixture = Fixture::new(git);
        let status = fixture.cli(&["board"]);
        assert_eq!(status["mode"], "simple", "git={git}: {status}");
        assert_eq!(status["mode_offer"], false, "git={git}: {status}");
        assert_eq!(status["offer"], false, "git={git}: {status}");
        assert_eq!(status["standing_permission"], false, "git={git}: {status}");
        assert!(
            status["simple_record_suggestion"]
                .as_str()
                .is_none_or(|path| !path.ends_with("-knowledge/GROUNDING.yaml")),
            "stale sibling suggestion: {status}"
        );

        let opened = fixture.session_start();
        assert!(opened.status.success(), "{}", text(&opened));
        let output = text(&opened);
        assert!(!output.contains("KPOPPER_MODE_OFFER"), "{output}");
        assert!(
            !output.contains("Simple (recommended) or Advanced"),
            "{output}"
        );
        assert!(
            !output.contains("obtain a concrete Simple location or Advanced choice"),
            "{output}"
        );
    }
}

#[test]
fn existing_advanced_record_keeps_board_safe_without_a_mode_prompt_or_grant() {
    let fixture = Fixture::new(true);
    fs::write(
        fixture.root.join("GROUNDING.yaml"),
        "known:\n  fixture.value: {v: 1}\n",
    )
    .unwrap();
    let status = fixture.cli(&["board"]);
    assert_eq!(status["mode"], "advanced");
    assert_eq!(status["mode_offer"], false);
    assert_eq!(status["standing_permission"], false);
    assert!(status["publication"]["standing_permission"].is_null());
    let opened = fixture.session_start();
    assert!(opened.status.success(), "{}", text(&opened));
    assert!(!text(&opened).contains("KPOPPER_MODE_OFFER"));
}

#[test]
fn previews_and_guidance_disabled_do_not_bind_a_fresh_project() {
    let fixture = Fixture::new(true);
    let before_projects = tree(&fixture.projects);
    let checked = fixture
        .command(&["config", "--check", "--json"])
        .output()
        .unwrap();
    assert!(checked.status.success(), "{}", text(&checked));
    assert_eq!(tree(&fixture.projects), before_projects);
    assert!(!fixture
        .root
        .join(".git/kpopper/project/project.json")
        .exists());

    let disabled = fixture.cli(&["config", "--guidance", "off"]);
    assert_eq!(disabled["guidance"], false);
    let status = fixture.cli(&["board"]);
    assert_eq!(status["mode_offer"], false);
    assert_eq!(status["offer"], false);
    assert_eq!(status["standing_permission"], false);
    assert_eq!(tree(&fixture.projects), before_projects);
    assert!(!fixture
        .root
        .join(".git/kpopper/project/project.json")
        .exists());
}
