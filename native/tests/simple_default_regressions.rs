//! Regressions for ownership and selection invariants in automatic Simple mode.
#[path = "support/simple_runtime.rs"]
mod simple_runtime;

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    home: PathBuf,
    projects: PathBuf,
    resources: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let root = base.join("workspace");
        let home = base.join("home");
        let projects = base.join("projects");
        let resources = base.join("resources");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&projects).unwrap();
        simple_runtime::resources(&resources);
        let fixture = Self {
            _temp: temp,
            root,
            home,
            projects,
            resources,
        };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "Simple regression"]);
        fixture.git(&["config", "user.email", "simple@example.invalid"]);
        fixture
    }

    fn command_at(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("KPOPPER_PRIVATE_HOME", &self.home)
            .env("KPOPPER_PROJECTS_DIR", &self.projects)
            .env("KPOPPER_NATIVE_RESOURCES", &self.resources)
            .env("KPOPPER_NATIVE_CACHE", self.home.join("cache"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_REMOTE")
            .env_remove("KPOPPER_READ_MODE")
            .output()
            .unwrap()
    }

    fn command(&self, args: &[&str]) -> Output {
        self.command_at(&self.root, args)
    }

    fn git(&self, args: &[&str]) -> Output {
        let out = Command::new("git")
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .current_dir(&self.root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn add(&self, args: &[&str]) -> Output {
        let out = self.command(args);
        assert!(
            out.status.success(),
            "args={args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn policy(&self) -> PathBuf {
        self.root.join(".git/kpopper/project/project.json")
    }
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

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn copied_linked_worktree_cannot_steal_live_owner_and_original_remains_writable() {
    let f = Fixture::new();
    fs::write(f.root.join("README.md"), "fixture\n").unwrap();
    f.git(&["add", "README.md"]);
    f.git(&["commit", "-qm", "base"]);
    let main_first = f.add(&["add", "main.first", "v=1"]);
    assert!(main_first.status.success());
    let where_output = f.command(&["--json", "where"]);
    assert!(where_output.status.success(), "{}", text(&where_output));
    let envelope: Value = serde_json::from_slice(&where_output.stdout).unwrap();
    let original_record = PathBuf::from(envelope["output"].as_str().unwrap().trim());
    let linked = f.home.parent().unwrap().join("linked");
    let out = Command::new("git")
        .args(["worktree", "add", "-q", "-b", "writer"])
        .arg(&linked)
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let last_writer = f.command_at(&linked, &["add", "linked.last", "v=2"]);
    assert!(last_writer.status.success(), "{}", text(&last_writer));
    let before = fs::read(&original_record).unwrap();
    let remove = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&linked)
        .current_dir(&f.root)
        .output()
        .unwrap();
    assert!(
        remove.status.success(),
        "{}",
        String::from_utf8_lossy(&remove.stderr)
    );

    // This is an independent Git copy of the main checkout, made only after
    // the last writer's linked worktree has been removed.
    let copy = f.home.parent().unwrap().join("copied-main");
    copy_tree(&f.root, &copy);

    let refused = f.command_at(&copy, &["add", "copy.entry", "v=2"]);
    assert!(
        !refused.status.success(),
        "copied worktree unexpectedly wrote: {}",
        text(&refused)
    );
    assert!(
        text(&refused).contains("generated_binding_copy_requires_recovery"),
        "{}",
        text(&refused)
    );
    assert_eq!(fs::read(&original_record).unwrap(), before);
    assert!(copy.join(".git/kpopper/project/project.json").is_file());
    assert!(!copy.join("GROUNDING.yaml").exists());
    assert!(fs::read(&f.policy()).is_ok());

    let reset_copy = f.command_at(&copy, &["config", "--new-project"]);
    assert!(reset_copy.status.success(), "{}", text(&reset_copy));
    assert_eq!(fs::read(&original_record).unwrap(), before);
    let copy_write = f.command_at(&copy, &["add", "copy.separate", "v=5"]);
    assert!(copy_write.status.success(), "{}", text(&copy_write));
    assert_eq!(fs::read(&original_record).unwrap(), before);

    let original = f.add(&["add", "original.after", "v=3"]);
    assert!(original.status.success());
    let pulled = f.command(&["pull", "original.after"]);
    assert!(pulled.status.success(), "{}", text(&pulled));
    let final_record = fs::read(&original_record).unwrap();
    for id in ["main.first", "linked.last", "original.after"] {
        assert!(
            String::from_utf8_lossy(&final_record).contains(id),
            "original record lost {id}"
        );
    }
}

#[test]
fn competing_copies_of_absent_original_have_exactly_one_claim_winner() {
    let f = Fixture::new();
    let seed = f.add(&["add", "seed", "v=1"]);
    assert!(seed.status.success());
    let original = f.home.parent().unwrap().join("original");
    fs::rename(&f.root, &original).unwrap();
    let a = f.home.parent().unwrap().join("copy-a");
    let b = f.home.parent().unwrap().join("copy-b");
    copy_tree(&original, &a);
    copy_tree(&original, &b);
    let policy_a = a.join(".git/kpopper/project/project.json");
    let policy_b = b.join(".git/kpopper/project/project.json");
    let policy_a_before = fs::read(&policy_a).unwrap();
    let policy_b_before = fs::read(&policy_b).unwrap();
    let initial_config: Value = serde_json::from_slice(&policy_a_before).unwrap();
    let initial_record = PathBuf::from(initial_config["record"].as_str().unwrap());
    let record_before = fs::read(&initial_record).unwrap();
    fs::remove_dir_all(original).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let run = |path: PathBuf,
               id: &'static str,
               home: PathBuf,
               projects: PathBuf,
               resources: PathBuf,
               barrier: std::sync::Arc<std::sync::Barrier>| {
        barrier.wait();
        Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(["add", id, "v=2"])
            .current_dir(path)
            .env("HOME", &home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("KPOPPER_PRIVATE_HOME", &home)
            .env("KPOPPER_PROJECTS_DIR", &projects)
            .env("KPOPPER_NATIVE_RESOURCES", &resources)
            .env("KPOPPER_NATIVE_CACHE", home.join("cache"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_REMOTE")
            .env_remove("KPOPPER_READ_MODE")
            .output()
            .unwrap()
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let handles: Vec<_> = [(a.clone(), "copy.a"), (b.clone(), "copy.b")]
        .into_iter()
        .map(|(path, id)| {
            let tx = tx.clone();
            let h = f.home.clone();
            let p = f.projects.clone();
            let r = f.resources.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || tx.send((id, run(path, id, h, p, r, barrier))).unwrap())
        })
        .collect();
    drop(tx);
    for handle in handles {
        handle.join().unwrap();
    }
    let outputs: Vec<_> = rx.into_iter().collect();
    assert_eq!(
        outputs.iter().filter(|(_, o)| o.status.success()).count(),
        1,
        "{outputs:?}"
    );
    let (winner_id, _winner) = outputs.iter().find(|(_, o)| o.status.success()).unwrap();
    let (loser_id, loser) = outputs.iter().find(|(_, o)| !o.status.success()).unwrap();
    assert!(
        text(loser).contains("generated_binding_copy_requires_recovery"),
        "{}",
        text(loser)
    );
    let winner_root = if *winner_id == "copy.a" { &a } else { &b };
    let loser_root = if *loser_id == "copy.a" { &a } else { &b };
    let winner_policy = winner_root.join(".git/kpopper/project/project.json");
    let loser_policy = loser_root.join(".git/kpopper/project/project.json");
    let winner_config: Value = serde_json::from_slice(&fs::read(&winner_policy).unwrap()).unwrap();
    let identity = &winner_config["default_identity"];
    let receipt_path = initial_record.parent().unwrap().join(".kpopper/owner.json");
    let receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["token"], identity["token"]);
    assert_eq!(
        receipt["owner_anchor"],
        winner_root.join(".git").to_string_lossy().as_ref()
    );
    assert_eq!(
        receipt["owner_path"],
        winner_root.to_string_lossy().as_ref()
    );
    let winner_bytes = fs::read(&initial_record).unwrap();
    assert!(String::from_utf8_lossy(&winner_bytes).contains(winner_id));
    assert!(!String::from_utf8_lossy(&winner_bytes).contains(loser_id));
    assert_eq!(
        fs::read(&loser_policy).unwrap(),
        if *loser_id == "copy.a" {
            policy_a_before
        } else {
            policy_b_before
        }
    );
    assert_ne!(
        winner_bytes, record_before,
        "successful mutation left record unchanged"
    );
}

#[test]
fn legacy_history_branch_birth_has_local_closure_and_main_continues() {
    let f = Fixture::new();
    fs::write(f.root.join("README.md"), "fixture\n").unwrap();
    f.git(&["add", "README.md"]);
    f.git(&["commit", "-qm", "base"]);
    let base = String::from_utf8_lossy(&f.git(&["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_owned();
    // Create a valid local history-backed record, then leave only its committed
    // legacy files (no project policy or private baseline binary).
    let configured = f.command(&["config", "--mode", "advanced", "--record", "GROUNDING.yaml"]);
    assert!(configured.status.success(), "{}", text(&configured));
    f.add(&["add", "p.main", "v=1"]);
    fs::remove_file(f.policy()).unwrap();
    f.git(&["add", "-A"]);
    f.git(&["commit", "-qm", "main record"]);
    f.git(&["checkout", "-qb", "branch-before-record", &base]);
    let born = f.add(&["add", "p.branch", "v=2"]);
    assert!(born.status.success(), "{}", text(&born));
    assert!(f.root.join("GROUNDING.yaml").is_file());
    assert!(!f.policy().exists());
    f.git(&["add", "-A"]);
    f.git(&["commit", "-qm", "branch record closure"]);
    let branch_record = fs::read(f.root.join("GROUNDING.yaml")).unwrap();
    assert!(String::from_utf8_lossy(&branch_record).contains("p.branch"));
    f.git(&["checkout", "-q", "main"]);
    assert!(f.command(&["pull", "p.main"]).status.success());
    let after = f.add(&["add", "p.main_after", "v=3"]);
    assert!(after.status.success(), "{}", text(&after));
    assert!(!f.policy().exists());
    assert!(f.command(&["pull", "p.main_after"]).status.success());
}

#[cfg(unix)]
#[test]
fn legacy_history_birth_rejects_symlinked_empty_history_residue() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(f.root.join("README.md"), "fixture\n").unwrap();
    f.git(&["add", "README.md"]);
    f.git(&["commit", "-qm", "base"]);
    let base = String::from_utf8_lossy(&f.git(&["rev-parse", "HEAD"]).stdout)
        .trim()
        .to_owned();
    let configured = f.command(&["config", "--mode", "advanced", "--record", "GROUNDING.yaml"]);
    assert!(configured.status.success(), "{}", text(&configured));
    f.add(&["add", "p.main", "v=1"]);
    fs::remove_file(f.policy()).unwrap();
    f.git(&["add", "-A"]);
    f.git(&["commit", "-qm", "main record"]);
    f.git(&["checkout", "-qb", "residue-copy", &base]);
    let residue = f.root.join(".kpopper/history");
    let target = f.home.parent().unwrap().join("empty-history");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(residue.parent().unwrap()).unwrap();
    if let Ok(metadata) = fs::symlink_metadata(&residue) {
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            assert!(fs::read_dir(&residue).unwrap().next().is_none());
            fs::remove_dir(&residue).unwrap();
        } else {
            panic!(
                "expected empty checkout-residue directory at {}",
                residue.display()
            );
        }
    }
    symlink(&target, &residue).unwrap();
    let refused = f.command(&["add", "p.branch", "v=2"]);
    assert!(
        !refused.status.success(),
        "symlink residue was accepted: {}",
        text(&refused)
    );
    assert!(!f.policy().exists());
    assert!(!f.root.join("GROUNDING.yaml").exists());
    assert!(fs::read_dir(&target).unwrap().next().is_none());
}

#[test]
fn nested_legacy_records_win_implicit_and_explicit_selection_before_and_after_root_binding() {
    let f = Fixture::new();
    // A local authored record is copied into the repository as a legacy nested record.
    let author = f.home.parent().unwrap().join("author");
    fs::create_dir_all(&author).unwrap();
    let seed = f.command_at(
        &author,
        &["config", "--mode", "simple", "--record", "GROUNDING.yaml"],
    );
    assert!(seed.status.success(), "{}", text(&seed));
    fs::write(author.join("src.md"), "source\n").unwrap();
    let added = f.command_at(&author, &["add", "nested.seed", "v=1"]);
    assert!(added.status.success(), "{}", text(&added));
    let nested_dir = f.root.join("models/nested");
    fs::create_dir_all(nested_dir.parent().unwrap()).unwrap();
    copy_tree(&author, &nested_dir);
    for name in [
        ".kpopper/project.json",
        ".kpopper/project.lock",
        ".kpopper/mode-history",
        ".kpopper/.history-local",
    ] {
        let path = nested_dir.join(name);
        if path.is_file() {
            fs::remove_file(path).unwrap();
        }
    }
    f.git(&["add", "-A"]);
    f.git(&["commit", "-qm", "nested legacy record"]);
    let nested_record = nested_dir.join("GROUNDING.yaml");
    let implicit = f.command_at(&nested_dir, &["add", "nested.implicit", "v=2"]);
    assert!(implicit.status.success(), "{}", text(&implicit));
    let explicit = f.command_at(
        &f.root,
        &[
            "add",
            "nested.explicit",
            "v=3",
            nested_record.to_str().unwrap(),
        ],
    );
    assert!(explicit.status.success(), "{}", text(&explicit));
    assert!(!f.policy().exists());
    let root_bind = f.add(&["add", "root.entry", "v=1"]);
    assert!(root_bind.status.success(), "{}", text(&root_bind));
    let after_implicit = f.command_at(&nested_dir, &["add", "nested.after_implicit", "v=4"]);
    assert!(after_implicit.status.success(), "{}", text(&after_implicit));
    let after_explicit = f.command_at(
        &f.root,
        &[
            "add",
            "nested.after_explicit",
            "v=5",
            nested_record.to_str().unwrap(),
        ],
    );
    assert!(after_explicit.status.success(), "{}", text(&after_explicit));
    let body = fs::read_to_string(nested_record).unwrap();
    for id in [
        "nested.seed",
        "nested.implicit",
        "nested.explicit",
        "nested.after_implicit",
        "nested.after_explicit",
    ] {
        assert!(body.contains(id), "nested record lost {id}: {body}");
    }
}

#[test]
fn failed_central_allocation_does_not_pin_binding_and_retry_succeeds() {
    let f = Fixture::new();
    let blocked = f.home.parent().unwrap().join("read-only-projects");
    fs::create_dir_all(&blocked).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o555)).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(["add", "retry.entry", "v=1"])
            .current_dir(&f.root)
            .env("HOME", &f.home)
            .env("XDG_STATE_HOME", f.home.join("state"))
            .env("KPOPPER_PRIVATE_HOME", &f.home)
            .env("KPOPPER_PROJECTS_DIR", &blocked)
            .env("KPOPPER_NATIVE_RESOURCES", &f.resources)
            .env("KPOPPER_NATIVE_CACHE", f.home.join("cache"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_REMOTE")
            .env_remove("KPOPPER_READ_MODE")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "read-only allocation unexpectedly succeeded: {}",
            text(&output)
        );
        assert!(!f.policy().exists(), "failed allocation persisted policy");
        assert!(
            fs::read_dir(&blocked).unwrap().next().is_none(),
            "failed allocation left central data"
        );
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
        let retry = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .args(["add", "retry.entry", "v=1"])
            .current_dir(&f.root)
            .env("HOME", &f.home)
            .env("XDG_STATE_HOME", f.home.join("state"))
            .env("KPOPPER_PRIVATE_HOME", &f.home)
            .env("KPOPPER_PROJECTS_DIR", &blocked)
            .env("KPOPPER_NATIVE_RESOURCES", &f.resources)
            .env("KPOPPER_NATIVE_CACHE", f.home.join("cache"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_REMOTE")
            .env_remove("KPOPPER_READ_MODE")
            .output()
            .unwrap();
        assert!(retry.status.success(), "retry: {}", text(&retry));
        assert!(f.policy().is_file());
        let policy: Value = serde_json::from_slice(&fs::read(f.policy()).unwrap()).unwrap();
        assert!(Path::new(policy["record"].as_str().unwrap()).starts_with(&blocked));
        assert!(text(&f.command(&["pull", "retry.entry"])).contains("retry.entry"));
    }
}

#[test]
fn explicit_missing_target_refuses_without_redirect_or_binding() {
    let f = Fixture::new();
    let requested = f.home.parent().unwrap().join("model/GROUNDING.yaml");
    fs::create_dir_all(requested.parent().unwrap()).unwrap();
    let out = f.command(&[
        "add",
        "explicit.missing",
        "v=1",
        requested.to_str().unwrap(),
    ]);
    assert!(
        !out.status.success(),
        "missing explicit target unexpectedly succeeded: {}",
        text(&out)
    );
    assert!(!requested.exists());
    assert!(
        !requested
            .parent()
            .unwrap()
            .join(".kpopper/project.json")
            .exists()
    );
    assert!(!f.policy().exists());
    assert!(fs::read_dir(&f.projects).unwrap().next().is_none());
    assert!(!f.root.join("GROUNDING.yaml").exists());
}

fn ingest_event(f: &Fixture) -> PathBuf {
    let event = f.home.parent().unwrap().join("event.json");
    fs::write(&event, r#"{"event_id":"e1","source_quote":"nested.seed is now 2.","target":"nested.seed","value":2,"date":"2026-10-27","kind":"report"}"#).unwrap();
    event
}

#[test]
fn ingest_capture_on_nested_existing_record_keeps_root_unbound() {
    let f = Fixture::new();
    let nested = f.root.join("models/record");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join("GROUNDING.yaml"),
        "known:\n  nested.seed: {v: 1}\n",
    )
    .unwrap();
    f.git(&["add", "-A"]);
    f.git(&["commit", "-qm", "nested record"]);
    let before = fs::read(nested.join("GROUNDING.yaml")).unwrap();
    let event = ingest_event(&f);
    let event = event.to_str().unwrap();
    let state_dir = f.home.parent().unwrap().join("ingest-state");
    let out = f.command_at(
        &nested,
        &[
            "--json",
            "ingest",
            "capture",
            "--file",
            event,
            "--no-start",
            "--state-dir",
            state_dir.to_str().unwrap(),
        ],
    );
    assert!(out.status.success(), "{}", text(&out));
    let captured_events = fs::read_dir(state_dir.join("events"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<Value>(&fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(captured_events.len(), 1);
    assert_eq!(captured_events[0]["state"], "captured");
    assert_eq!(
        captured_events[0]["record"],
        nested.join("GROUNDING.yaml").to_string_lossy().as_ref()
    );
    assert_eq!(fs::read(nested.join("GROUNDING.yaml")).unwrap(), before);
    assert!(!f.policy().exists());
    assert!(fs::read_dir(&f.projects).unwrap().next().is_none());
}

#[test]
fn ingest_capture_on_fresh_git_project_refuses_without_binding_or_store_entry() {
    let f = Fixture::new();
    fs::write(f.root.join("README.md"), "fixture\n").unwrap();
    f.git(&["add", "README.md"]);
    f.git(&["commit", "-qm", "base"]);
    let event = ingest_event(&f);
    let state_dir = f.home.parent().unwrap().join("ingest-state");
    let out = f.command(&[
        "--json",
        "ingest",
        "capture",
        "--file",
        event.to_str().unwrap(),
        "--no-start",
        "--state-dir",
        state_dir.to_str().unwrap(),
    ]);
    assert!(
        !out.status.success(),
        "fresh ingest unexpectedly succeeded: {}",
        text(&out)
    );
    assert!(!f.policy().exists());
    assert!(fs::read_dir(&f.projects).unwrap().next().is_none());
    assert!(!f.root.join("GROUNDING.yaml").exists());
    assert!(
        !state_dir.exists(),
        "refused capture created ingestion state"
    );
}

#[test]
fn no_data_failed_binding_can_reset_but_unknown_state_is_preserved_and_refused() {
    for receipt_present in [true, false] {
        let f = Fixture::new();
        let failed = f.command(&["add", "d.invalid", "rests_on=[missing]"]);
        assert!(!failed.status.success(), "{}", text(&failed));
        let old_policy = fs::read(f.policy()).unwrap();
        let config: Value = serde_json::from_slice(&old_policy).unwrap();
        let old_record = PathBuf::from(config["record"].as_str().unwrap());
        assert!(!old_record.exists());
        let receipt = old_record.parent().unwrap().join(".kpopper/owner.json");
        let old_receipt = fs::read(&receipt).unwrap();
        if !receipt_present {
            fs::remove_file(&receipt).unwrap();
        }
        let unknown = old_record.parent().unwrap().join(".kpopper/unknown-data");
        fs::write(&unknown, b"preserve this unknown data").unwrap();
        let refused = f.command(&["config", "--new-project"]);
        assert!(!refused.status.success(), "{}", text(&refused));
        assert_eq!(fs::read(f.policy()).unwrap(), old_policy);
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve this unknown data");
        fs::remove_file(&unknown).unwrap();
        let reset = f.command(&["config", "--new-project"]);
        assert!(reset.status.success(), "{}", text(&reset));
        let next: Value = serde_json::from_slice(&fs::read(f.policy()).unwrap()).unwrap();
        assert_ne!(next["record"], config["record"]);
        assert!(!old_record.exists());
        if receipt_present {
            assert_eq!(fs::read(&receipt).unwrap(), old_receipt);
        } else {
            assert!(!receipt.exists());
        }
        f.add(&["add", "retry.valid", "v=1"]);
        assert!(f.command(&["pull", "retry.valid"]).status.success());
    }
}
