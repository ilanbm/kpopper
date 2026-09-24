//! Exercise automatic invocation through the real CLI; Git transport is local
//! and the GitHub executable is a fixture. No credentials or network are used.
#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
    bin: PathBuf,
    git: String,
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
        let remote = temp.path().join("remote.git");
        let bin = temp.path().join("bin");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&bin).unwrap();
        let git = success(
            Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap(),
        )
        .trim()
        .to_owned();
        let f = Self {
            _temp: temp,
            root,
            remote,
            bin,
            git,
        };
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.test"],
            vec!["config", "commit.gpgsign", "false"],
            vec!["config", "core.hooksPath", "/dev/null"],
        ] {
            f.git(&args);
        }
        fs::write(f.root.join("GROUNDING.yaml"), "known:\n  p.base: {v: 1}\n").unwrap();
        fs::write(f.root.join("app.txt"), "target code\n").unwrap();
        f.git(&["add", "GROUNDING.yaml", "app.txt"]);
        f.git(&["commit", "-qm", "fixture"]);
        f.git(&["init", "-q", "--bare", f.remote.to_str().unwrap()]);
        f.git(&["push", f.remote.to_str().unwrap(), "main"]);
        f.git(&[
            "remote",
            "add",
            "team",
            "https://github.invalid/fixture/repo.git",
        ]);
        // Only substitute the exact synthetic URL. Configuration/scope checks
        // retain that URL; the underlying Git is forbidden network protocols.
        f.script(
            "git",
            r#"#!/bin/sh
for argument do
  shift
  if [ "$argument" = https://github.invalid/fixture/repo.git ]; then
    set -- "$@" "$KPOP_TEST_REMOTE"
  else
    set -- "$@" "$argument"
  fi
done
exec "$KPOP_TEST_GIT" "$@"
"#,
        );
        f.script("gh", r#"#!/bin/sh
[ "$1" = api ] && [ "$2" = --hostname ] && [ "$3" = github.invalid ] || exit 2
printf '%s\n' "$5" >> "$KPOP_TEST_CALLS"
case "$5" in
GET)
  case "$6" in
  repos/fixture/repo) printf '%s\n' '{"full_name":"fixture/repo","default_branch":"main","permissions":{"push":true}}' ;;
  repos/fixture/repo/git/ref/heads/main)
    target=$("$KPOP_TEST_GIT" -C "$KPOP_TEST_REMOTE" rev-parse refs/heads/main) || exit
    printf '{"ref":"refs/heads/main","object":{"sha":"%s"}}\n' "$target"
    ;;
  *) printf '[]\n' ;;
  esac ;;
POST)
  cat > "$KPOP_TEST_BODY"
  head=$("$KPOP_TEST_GIT" -C "$KPOP_TEST_REMOTE" rev-parse refs/heads/pending_grounding) || exit
  printf '{"number":1,"state":"open","html_url":"fixture://pr/1","head":{"sha":"%s"},"body":""}\n' "$head"
  ;;
*) exit 2 ;;
esac
"#);
        f
    }
    fn script(&self, name: &str, text: &str) {
        let p = self.bin.join(name);
        fs::write(&p, text).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fn git(&self, args: &[&str]) -> String {
        success(
            Command::new(&self.git)
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_ALLOW_PROTOCOL", "file")
                .output()
                .unwrap(),
        )
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_kpop"));
        let path = std::env::join_paths(
            std::iter::once(self.bin.clone())
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        c.current_dir(&self.root)
            .args(["--workspace"])
            .arg(&self.root)
            .env("PATH", path)
            .env("KPOP_TEST_GIT", &self.git)
            .env("KPOP_TEST_REMOTE", &self.remote)
            .env("KPOP_TEST_CALLS", self.bin.join("calls"))
            .env("KPOP_TEST_BODY", self.bin.join("body.json"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("XDG_STATE_HOME", self._temp.path().join("state"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("KPOPPER_READ_MODE")
            .env_remove("KPOPPER_NATIVE_RESOURCES");
        c
    }
    fn cli(&self, args: &[&str]) -> Value {
        serde_json::from_str(&success(self.command().args(args).output().unwrap())).unwrap()
    }
    fn grant(&self) {
        self.cli(&[
            "pending",
            "configure",
            "--remote",
            "team",
            "--target",
            "main",
            "--grant",
        ]);
    }
    fn capture(&self) -> Value {
        self.cli(&[
            "add",
            "vendor.limit",
            "v=10",
            "from=Approved fixture bulletin",
            "--scope",
            "external",
            "--environment",
            "fixture vendor",
            "--shareability",
            "project",
            "--event-id",
            "fixture-event",
        ])
    }
    fn start(&self, frozen: bool) {
        let mut c = self.command();
        if frozen {
            c.env("KPOPPER_READ_MODE", "frozen");
        }
        let mut child = c
            .args(["session-start", "--host", "codex"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(json!({"cwd": self.root}).to_string().as_bytes())
            .unwrap();
        success(child.wait_with_output().unwrap());
    }
    fn proposed(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let output = self.command().args(["pending", "status"]).output().unwrap();
            if output.status.success() {
                let status: Value = serde_json::from_slice(&output.stdout).unwrap();
                if status["pr"] == "1"
                    && status["states"]
                        .as_object()
                        .unwrap()
                        .values()
                        .any(|v| v == "proposed")
                {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "publisher did not finish: {status}"
                );
            } else {
                let error = String::from_utf8_lossy(&output.stderr);
                // The reader refuses a mixed snapshot while the child advances
                // publication state; retry that transient observation only.
                assert!(
                    error.contains("snapshot_changed") && Instant::now() < deadline,
                    "{error}"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let document = self.git(&[
            "--git-dir",
            self.remote.to_str().unwrap(),
            "show",
            "pending_grounding:GROUNDING.yaml",
        ]);
        assert!(document.contains("vendor.limit"));
        assert_eq!(
            self.git(&[
                "--git-dir",
                self.remote.to_str().unwrap(),
                "show",
                "pending_grounding:app.txt"
            ]),
            "target code\n"
        );
        let body: Value =
            serde_json::from_slice(&fs::read(self.bin.join("body.json")).unwrap()).unwrap();
        assert_eq!(body["base"], "main");
        assert_eq!(body["head"], "pending_grounding");
        assert_eq!(
            fs::read_to_string(self.bin.join("calls"))
                .unwrap()
                .lines()
                .filter(|s| *s == "POST")
                .count(),
            1
        );
    }
}

#[test]
fn granted_capture_starts_the_real_publisher_without_changing_feature_files() {
    let f = Fixture::new();
    f.grant();
    let before = fs::read(f.root.join("GROUNDING.yaml")).unwrap();
    fs::write(f.root.join("app.txt"), "unmerged feature code\n").unwrap();
    let receipt = f.capture();
    assert_eq!(receipt["publication_attempt"]["started"], true, "{receipt}");
    f.proposed();
    assert_eq!(fs::read(f.root.join("GROUNDING.yaml")).unwrap(), before);
    assert_eq!(
        fs::read_to_string(f.root.join("app.txt")).unwrap(),
        "unmerged feature code\n"
    );
}

#[test]
fn board_onboarding_connects_once_and_the_first_finding_opens_the_knowledge_pr() {
    let f = Fixture::new();
    assert_eq!(f.cli(&["board"])["mode_offer"], true);
    f.cli(&[
        "config",
        "--mode",
        "advanced",
        "--expected-generation",
        "0",
        "--json",
    ]);
    assert_eq!(f.cli(&["board"])["offer"], true);
    let inspected = f.cli(&["board", "inspect", "--remote", "team"]);
    assert_eq!(inspected["destination"]["target"], "main");
    let connected = f.cli(&[
        "board",
        "connect",
        "--remote",
        "team",
        "--repository",
        "https://github.invalid/fixture/repo.git",
        "--target",
        "main",
        "--generation",
        "1",
        "--grant",
    ]);
    assert_eq!(connected["connection_verified"], true);
    assert_eq!(connected["publication_verified"], false);
    assert_eq!(connected["publication_attempt"]["started"], false);
    assert_eq!(f.capture()["publication_attempt"]["started"], true);
    f.proposed();
    let body: Value = serde_json::from_slice(&fs::read(f.bin.join("body.json")).unwrap()).unwrap();
    assert_eq!(body["title"], "kpopper Board: shared findings");
    assert_eq!(f.cli(&["board"])["offer"], false);
}

#[test]
fn live_session_retries_after_grant_while_frozen_and_ungranted_sessions_stay_local() {
    let f = Fixture::new();
    assert_eq!(f.capture()["publication_attempt"]["started"], false);
    f.start(false);
    assert!(!f.bin.join("calls").exists());
    f.grant();
    f.start(true);
    assert!(!f.bin.join("calls").exists());
    f.start(false);
    f.proposed();
}

#[test]
fn paused_capture_is_durable_without_starting_a_publisher() {
    let f = Fixture::new();
    f.grant();
    f.cli(&["pending", "pause"]);
    assert_eq!(f.cli(&["board"])["paused"], true);
    let receipt = f.capture();
    assert_eq!(receipt["state"], "captured");
    assert_eq!(receipt["publication_attempt"]["started"], false);
    f.start(false);
    assert!(!f.bin.join("calls").exists());
}

#[test]
fn capture_replay_and_session_opening_respect_backoff_and_revocation() {
    let f = Fixture::new();
    f.grant();
    f.cli(&["pending", "pause"]);
    let captured = f.capture();
    let path = f.root.join(".git/kpopper/project/publication.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (failures, retry_at) in [(0, 4_000_000_000u64), (5, 0)] {
        state["paused"] = json!(false);
        state["failures"] = json!(failures);
        state["retry_at"] = json!(retry_at);
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
        let replay = f.capture();
        assert_eq!(replay["revision"], captured["revision"]);
        assert_eq!(replay["replay"], true);
        assert_eq!(
            replay["publication_attempt"]["reason"],
            "paused or bounded backoff"
        );
        f.start(false);
        assert!(!f.bin.join("calls").exists());
    }
    f.cli(&["pending", "configure", "--revoke"]);
    assert_eq!(
        f.capture()["publication_attempt"]["reason"],
        "no standing publication permission"
    );
    f.start(false);
    assert!(!f.bin.join("calls").exists());
}
