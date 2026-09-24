use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    private: PathBuf,
}
fn success(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        let private = temp.path().join("private");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&private).unwrap();
        let f = Self {
            _temp: temp,
            root,
            private,
        };
        f.git(&["init", "-q", "-b", "main"]);
        f.git(&["config", "user.name", "Fixture"]);
        f.git(&["config", "user.email", "fixture@example.test"]);
        fs::write(f.root.join("GROUNDING.yaml"), "known:\n  p.x: {v: 1}\n").unwrap();
        f.git(&["add", "GROUNDING.yaml"]);
        f.git(&["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"]);
        f.git(&[
            "remote",
            "add",
            "origin",
            "https://github.invalid/fixture/repo.git",
        ]);
        f
    }
    fn git(&self, args: &[&str]) -> String {
        success(
            Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap(),
        )
    }
    fn command_at(&self, root: &Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kpop"));
        c.current_dir(root)
            .arg("--workspace")
            .arg(root)
            .env("XDG_STATE_HOME", self.private.join("state"))
            .env("TMPDIR", &self.private)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("KPOPPER_READ_MODE");
        if self.private.join("bin").exists() {
            c.env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(self.private.join("bin"))
                        .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
                )
                .unwrap(),
            )
            .env("BOARD_CALLS", self.private.join("calls"));
        }
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        serde_json::from_str(&success(
            self.command_at(&self.root).args(args).output().unwrap(),
        ))
        .unwrap()
    }
    fn opener(&self) -> String {
        let mut c = self
            .command_at(&self.root)
            .args(["session-start", "--host", "codex"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin
            .take()
            .unwrap()
            .write_all(json!({"cwd":self.root}).to_string().as_bytes())
            .unwrap();
        success(c.wait_with_output().unwrap())
    }
    #[cfg(unix)]
    fn provider(&self, push: bool) {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.private.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let script = format!(
            r#"#!/bin/sh
[ "$1" = api ] && [ "$2" = --hostname ] && [ "$3" = github.invalid ] && [ "$5" = GET ] || exit 2
printf '%s\n' "$6" >> "$BOARD_CALLS"
case "$6" in
repos/fixture/repo) printf '%s\n' '{{"full_name":"fixture/repo","default_branch":"main","permissions":{{"push":{push}}}}}' ;;
repos/fixture/repo/git/ref/heads/main) printf '%s\n' '{{"ref":"refs/heads/main","object":{{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}}' ;;
*) exit 2 ;;
esac
"#
        );
        fs::write(bin.join("gh"), script).unwrap();
        fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn board_offer_is_read_only_and_local_choice_is_shared_by_worktrees() {
    let f = Fixture::new();
    let status = f.cli(&["board"]);
    assert_eq!(status["mode"], "advanced");
    assert_eq!(status["mode_offer"], true);
    assert_eq!(status["offer"], false);
    assert_eq!(status["advanced_experimental"], true);
    assert_eq!(
        status["remotes"][0]["repository"],
        "https://github.invalid/fixture/repo.git"
    );
    assert!(
        status["illustration"]
            .as_str()
            .unwrap()
            .ends_with("two-working-modes.png")
    );
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
    assert!(f.opener().contains("kpopper Board"));
    assert_eq!(f.cli(&["board", "local"])["choice"], "local");
    assert!(!f.opener().contains("KPOPPER_BOARD_OFFER"));
    let sibling = f._temp.path().join("sibling");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "sibling",
        sibling.to_str().unwrap(),
    ]);
    let same: Value = serde_json::from_str(&success(
        f.command_at(&sibling).arg("board").output().unwrap(),
    ))
    .unwrap();
    assert_eq!(same["choice"], "local");
    assert_eq!(same["offer"], false);
    assert!(
        f.git(&["for-each-ref", "--format=%(refname)", "refs/kpopper"])
            .is_empty()
    );
}

#[test]
fn shown_is_not_authority_and_suppresses_repeated_offers() {
    let f = Fixture::new();
    f.cli(&["board", "shown", "--mode"]);
    let status = f.cli(&["board"]);
    assert_eq!(status["mode_offer"], false);
    assert_eq!(status["offer"], false);
    assert_eq!(status["standing_permission"], false);
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
}

#[test]
fn fresh_simple_selection_pins_one_external_home_without_creating_an_empty_record() {
    let f = Fixture::new();
    fs::remove_file(f.root.join("GROUNDING.yaml")).unwrap();
    // Test a genuinely new repository, with no committed record either.
    f.git(&["rm", "--cached", "GROUNDING.yaml"]);
    f.git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "-qm",
        "remove fixture record",
    ]);
    let destination = f.private.join("GROUNDING.yaml");
    let result = f.cli(&[
        "config",
        "--mode",
        "simple",
        "--record",
        destination.to_str().unwrap(),
        "--expected-generation",
        "0",
        "--json",
    ]);
    assert_eq!(result["project"]["mode"], "simple");
    assert!(!destination.exists());
    let sibling = f._temp.path().join("sibling");
    f.git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "sibling",
        sibling.to_str().unwrap(),
    ]);
    let second: Value = serde_json::from_str(&success(
        f.command_at(&sibling)
            .args(["config", "--json"])
            .output()
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(second["record"], result["record"]);
    assert_eq!(f.cli(&["board"])["mode_offer"], false);
    assert_eq!(f.cli(&["board"])["offer"], false);
    // First real finding creates the external record, visible from both worktrees.
    let resources = f.private.join("resources");
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    success(
        f.command_at(&sibling)
            .env("KPOPPER_NATIVE_RESOURCES", &resources)
            .env("KPOPPER_NATIVE_CACHE", f.private.join("cache"))
            .args([
                "add",
                "vendor.limit",
                "v=10",
                "from=Approved fixture source",
            ])
            .output()
            .unwrap(),
    );
    assert!(destination.is_file());
    assert!(!f.root.join("GROUNDING.yaml").exists());
    assert!(!sibling.join("GROUNDING.yaml").exists());
    let read = success(
        f.command_at(&f.root)
            .env("KPOPPER_NATIVE_RESOURCES", &resources)
            .env("KPOPPER_NATIVE_CACHE", f.private.join("cache"))
            .args(["pull", "vendor.limit"])
            .output()
            .unwrap(),
    );
    assert!(read.contains("10"));
}

#[test]
fn mode_choice_does_not_discard_existing_records_or_enable_board_publication() {
    let f = Fixture::new();
    let before = fs::read(f.root.join("GROUNDING.yaml")).unwrap();
    let missing = f.private.join("GROUNDING.yaml");
    let rejected = f
        .command_at(&f.root)
        .args([
            "config",
            "--mode",
            "simple",
            "--record",
            missing.to_str().unwrap(),
            "--expected-generation",
            "0",
        ])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert_eq!(fs::read(f.root.join("GROUNDING.yaml")).unwrap(), before);
    assert!(!missing.exists());
    f.cli(&[
        "config",
        "--mode",
        "advanced",
        "--expected-generation",
        "0",
        "--json",
    ]);
    let status = f.cli(&["board"]);
    assert_eq!(status["mode_offer"], false);
    assert_eq!(status["offer"], true);
    assert_eq!(status["standing_permission"], false);
}

#[test]
fn empty_simple_setup_refuses_missing_parents_and_orphan_sidecars() {
    let f = Fixture::new();
    fs::remove_file(f.root.join("GROUNDING.yaml")).unwrap();
    let unavailable = f.private.join("missing/GROUNDING.yaml");
    let output = f
        .command_at(&f.root)
        .args([
            "config",
            "--mode",
            "simple",
            "--record",
            unavailable.to_str().unwrap(),
            "--expected-generation",
            "0",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
    let target = f.private.join("GROUNDING.yaml");
    fs::create_dir(f.private.join(".kpopper")).unwrap();
    fs::write(
        f.private.join(".kpopper/preserved.txt"),
        "existing evidence",
    )
    .unwrap();
    let output = f
        .command_at(&f.root)
        .args([
            "config",
            "--mode",
            "simple",
            "--record",
            target.to_str().unwrap(),
            "--expected-generation",
            "0",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!target.exists());
    assert_eq!(
        fs::read_to_string(f.private.join(".kpopper/preserved.txt")).unwrap(),
        "existing evidence"
    );
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
}

#[cfg(unix)]
#[test]
fn connect_requires_exact_grant_verifies_destination_and_reads_configuration_back() {
    let f = Fixture::new();
    f.provider(true);
    let inspected = f.cli(&["board", "inspect", "--remote", "origin"]);
    assert_eq!(inspected["destination"]["target"], "main");
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
    let args = [
        "board",
        "connect",
        "--remote",
        "origin",
        "--repository",
        "https://github.invalid/fixture/repo.git",
        "--target",
        "main",
        "--generation",
        "0",
    ];
    assert!(
        !f.command_at(&f.root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
    let connected: Value = serde_json::from_str(&success(
        f.command_at(&f.root)
            .args(args)
            .arg("--grant")
            .output()
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(connected["choice"], "shared");
    assert_eq!(connected["standing_permission"], true);
    assert_eq!(connected["connection_verified"], true);
    assert_eq!(connected["publication_verified"], false);
    assert_eq!(connected["offer"], false);
    assert_eq!(f.cli(&["board", "local"])["standing_permission"], false);
}

#[cfg(unix)]
#[test]
fn changed_destination_and_unavailable_permissions_do_not_save_a_grant() {
    let f = Fixture::new();
    f.provider(false);
    let args = [
        "board",
        "connect",
        "--remote",
        "origin",
        "--repository",
        "https://github.invalid/fixture/repo.git",
        "--target",
        "main",
        "--generation",
        "0",
        "--grant",
    ];
    let failed = f.command_at(&f.root).args(args).output().unwrap();
    assert!(!failed.status.success());
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
    f.git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.invalid/other/repo.git",
    ]);
    let before = fs::read(f.private.join("calls")).unwrap();
    assert!(
        !f.command_at(&f.root)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(f.private.join("calls")).unwrap(), before);
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
}

#[test]
fn simple_and_guidance_disabled_projects_are_not_offered_a_mode_change() {
    let f = Fixture::new();
    f.cli(&["config", "--guidance", "off", "--json"]);
    assert_eq!(f.cli(&["board"])["offer"], false);
    let external = f.private.join("GROUNDING.yaml");
    fs::rename(f.root.join("GROUNDING.yaml"), &external).unwrap();
    fs::write(
        f.root.join(".git/kpopper-record"),
        external.to_str().unwrap(),
    )
    .unwrap();
    let status = f.cli(&["board"]);
    assert_eq!(status["mode"], "simple");
    assert_eq!(status["offer"], false);
    assert!(!f.root.join(".git/kpopper/project/project.json").exists());
}

#[test]
fn discovery_does_not_echo_credentials_embedded_in_remote_urls() {
    let f = Fixture::new();
    f.git(&[
        "remote",
        "set-url",
        "origin",
        "https://fixture-secret@github.invalid/fixture/repo.git",
    ]);
    let state = f.cli(&["board"]);
    assert!(!state.to_string().contains("fixture-secret"));
    assert!(state["remotes"][0]["unavailable"].is_string());
}

#[cfg(unix)]
#[test]
fn a_later_local_choice_invalidates_an_earlier_connection_offer() {
    let f = Fixture::new();
    f.provider(true);
    let inspected = f.cli(&["board", "inspect", "--remote", "origin"]);
    assert_eq!(inspected["generation"], 0);
    let local = f.cli(&["board", "local"]);
    assert_eq!(local["generation"], 1);
    assert_eq!(f.cli(&["board", "local"])["generation"], 1);
    let before = fs::read(f.private.join("calls")).unwrap();
    let result = f
        .command_at(&f.root)
        .args([
            "board",
            "connect",
            "--remote",
            "origin",
            "--repository",
            "https://github.invalid/fixture/repo.git",
            "--target",
            "main",
            "--generation",
            "0",
            "--grant",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(fs::read(f.private.join("calls")).unwrap(), before);
    assert_eq!(f.cli(&["board"])["choice"], "local");
}
