//! Black-box contract tests for the simple-defaults CLI seam.
//!
//! Every invocation gets a private HOME and central root so a failed experiment
//! cannot touch the operator's state.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    projects: PathBuf,
}

impl Fixture {
    fn new(git: bool, record: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        let home = temp.path().join("home");
        let projects = temp.path().join("projects");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&projects).unwrap();
        if record {
            fs::write(root.join("GROUNDING.yaml"), "known: {}\n").unwrap();
        }
        if git {
            assert!(
                Command::new("git")
                    .args(["init", "-q", "-b", "main"])
                    .current_dir(&root)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        Self {
            _temp: temp,
            root,
            home,
            projects,
        }
    }

    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
        command
            .current_dir(cwd)
            .arg("--workspace")
            .arg(cwd)
            .env("HOME", &self.home)
            .env("KPOPPER_PROJECTS_DIR", &self.projects)
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID");
        command
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd).args(args).output().unwrap()
    }

    fn json(&self, cwd: &Path, args: &[&str]) -> Value {
        let output = self.run(cwd, args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn experimental_advanced_uses_the_validated_configuration_transition() {
    let fixture = Fixture::new(true, true);
    let output = fixture.json(&fixture.root, &["experimental", "advanced", "--json"]);
    assert_eq!(output["project"]["mode"], "advanced");
    assert!(
        fixture
            .root
            .join(output["project"]["record"].as_str().unwrap())
            .exists()
    );
}

#[test]
fn fresh_git_advanced_check_does_not_bind_and_actual_transition_uses_root_record() {
    let fixture = Fixture::new(true, false);
    let checked = fixture.run(
        &fixture.root,
        &["experimental", "advanced", "--check", "--json"],
    );
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    assert!(fs::read_dir(&fixture.projects).unwrap().next().is_none());
    let output = fixture.json(&fixture.root, &["experimental", "advanced", "--json"]);
    assert_eq!(output["project"]["mode"], "advanced");
    assert_eq!(output["project"]["record"], "GROUNDING.yaml");
    assert!(!fixture.root.join("GROUNDING.yaml").exists());
}

#[test]
fn explicit_local_simple_binding_stays_portable_after_relocation() {
    let fixture = Fixture::new(false, false);
    let configured = fixture.run(
        &fixture.root,
        &[
            "config",
            "--mode",
            "simple",
            "--record",
            "GROUNDING.yaml",
            "--json",
        ],
    );
    assert!(
        configured.status.success(),
        "{}",
        String::from_utf8_lossy(&configured.stderr)
    );
    let configured_policy: Value = serde_json::from_slice(&configured.stdout).unwrap();
    assert_eq!(configured_policy["project"]["mode_origin"], "configured");
    let added = fixture.run(&fixture.root, &["add", "p.local", "v=1"]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let local_record = fixture.root.join("GROUNDING.yaml");
    assert!(local_record.is_file());
    assert!(fs::read_dir(&fixture.projects).unwrap().next().is_none());

    let relocated = fixture._temp.path().join("relocated");
    copy_portable(&fixture.root, &relocated);
    fs::rename(
        &fixture.root,
        fixture._temp.path().join("author-unavailable"),
    )
    .unwrap();
    let read = fixture.run(&relocated, &["open"]);
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    let checked = fixture.run(&relocated, &["check"]);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
}

#[test]
fn config_mode_advanced_remains_a_compatibility_alias_with_notice() {
    let fixture = Fixture::new(true, true);
    let output = fixture.run(&fixture.root, &["config", "--mode", "advanced", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let notice = String::from_utf8_lossy(&output.stderr);
    assert!(
        notice.contains("Experimental"),
        "missing Experimental notice: {notice}"
    );
}

#[test]
fn advanced_requires_a_git_in_tree_workspace() {
    let fixture = Fixture::new(false, false);
    let output = fixture.run(&fixture.root, &["experimental", "advanced", "--json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Git"));
}

#[test]
fn check_is_read_only_and_does_not_bind_a_fresh_workspace() {
    let fixture = Fixture::new(false, false);
    let output = fixture.run(&fixture.root, &["config", "--check", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.root.join(".kpopper/project.json").exists());
    assert!(fs::read_dir(&fixture.projects).unwrap().next().is_none());
}

#[test]
fn blocked_transition_preserves_generated_binding_and_data() {
    let fixture = Fixture::new(true, false);
    let added = fixture.run(&fixture.root, &["add", "p.generated", "v=1"]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let record = fixture.json(&fixture.root, &["--json", "where"])["output"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    let before = fs::read(&record).unwrap();
    let output = fixture.run(&fixture.root, &["config", "--mode", "advanced", "--json"]);
    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
    assert_eq!(fs::read(record).unwrap(), before);
}

#[test]
fn rebind_repairs_a_move_but_new_project_recovers_a_copy_without_touching_original() {
    let fixture = Fixture::new(false, false);
    let added = fixture.run(&fixture.root, &["add", "p.original", "v=1"]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let original_record = fixture.json(&fixture.root, &["--json", "where"])["output"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    let original_record = PathBuf::from(original_record);
    let original_bytes = fs::read(&original_record).unwrap();
    let moved = fixture._temp.path().join("moved");
    fs::rename(&fixture.root, &moved).unwrap();

    let repair = fixture.run(&moved, &["config", "--rebind", "--json"]);
    assert!(
        repair.status.success(),
        "{}",
        String::from_utf8_lossy(&repair.stderr)
    );
    assert!(String::from_utf8_lossy(&repair.stdout).contains("rebind"));

    let copy = fixture._temp.path().join("copy");
    copy_tree(&moved, &copy);
    let recovery = fixture.run(&copy, &["config", "--new-project", "--json"]);
    assert!(
        recovery.status.success(),
        "{}",
        String::from_utf8_lossy(&recovery.stderr)
    );
    assert_eq!(fs::read(&original_record).unwrap(), original_bytes);
}

#[test]
fn recovery_check_conflict_is_rejected_before_mutation() {
    let fixture = Fixture::new(false, false);
    let added = fixture.run(&fixture.root, &["add", "p.original", "v=1"]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let record = fixture.json(&fixture.root, &["--json", "where"])["output"]
        .as_str()
        .unwrap()
        .trim()
        .to_owned();
    let before = fs::read(&record).unwrap();
    let refused = fixture.run(&fixture.root, &["config", "--rebind", "--check", "--json"]);
    assert!(!refused.status.success());
    assert_eq!(fs::read(record).unwrap(), before);
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn copy_portable(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            "project.json" | "project.lock" | "mode-history" | ".history-local"
        ) {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_portable(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
