#[path = "support/simple_runtime.rs"]
mod simple_runtime;

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use tempfile::TempDir;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_kpop")
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git").args(args).arg(root).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run(root: &Path, home: &Path, projects: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .env("HOME", home)
        .env("KPOPPER_PROJECTS_DIR", projects)
        .env("KPOPPER_NATIVE_RESOURCES", home.join("resources"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("KPOPPER_NATIVE_CACHE", home.join("cache"))
        .output()
        .unwrap()
}

#[test]
fn concurrent_first_writers_share_one_binding_and_retain_both_entries() {
    let temp = TempDir::new().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    let home = base.join("home");
    let projects = base.join("projects");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&home).unwrap();
    simple_runtime::resources(&home.join("resources"));
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("README.md"), "fixture source text\n").unwrap();

    let first = {
        let root = root.clone();
        let home = home.clone();
        let projects = projects.clone();
        std::thread::spawn(move || {
            run(
                &root,
                &home,
                &projects,
                &["add", "source.one", "file=README.md"],
            )
        })
    };
    let second = {
        let root = root.clone();
        let home = home.clone();
        let projects = projects.clone();
        std::thread::spawn(move || {
            run(
                &root,
                &home,
                &projects,
                &["add", "source.two", "file=README.md"],
            )
        })
    };
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    let config_path = root.join(".git/kpopper/project/project.json");
    let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    let record = PathBuf::from(config["record"].as_str().unwrap());
    assert!(record.starts_with(&projects));
    assert!(!root.join("GROUNDING.yaml").exists());
    assert!(record.is_file());
    let central_entries: Vec<_> = fs::read_dir(&projects)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect();
    assert_eq!(central_entries.len(), 1);
    assert!(central_entries[0].path().join("GROUNDING.yaml").is_file());

    for id in ["source.one", "source.two"] {
        let output = run(&root, &home, &projects, &["pull", id]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn first_binding_reports_an_existing_same_name_without_adopting_it() {
    let temp = TempDir::new().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    let home = base.join("home");
    let projects = base.join("projects");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&home).unwrap();
    simple_runtime::resources(&home.join("resources"));
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("README.md"), "fixture source\n").unwrap();
    let previous = projects.join("repo-00000000-0000-4000-8000-000000000001");
    fs::create_dir_all(&previous).unwrap();
    let prior_bytes = b"known: {prior: {v: 1}}\n";
    fs::write(previous.join("GROUNDING.yaml"), prior_bytes).unwrap();

    let preview = run(&root, &home, &projects, &["config", "--json"]);
    assert!(preview.status.success());
    assert!(preview.stderr.is_empty());
    let added = run(
        &root,
        &home,
        &projects,
        &["add", "source.new", "file=README.md"],
    );
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(String::from_utf8_lossy(&added.stderr).contains(previous.to_str().unwrap()));
    assert_eq!(
        fs::read(previous.join("GROUNDING.yaml")).unwrap(),
        prior_bytes
    );
    let policy = run(&root, &home, &projects, &["config", "--json"]);
    let policy: Value = serde_json::from_slice(&policy.stdout).unwrap();
    assert_ne!(
        Path::new(policy["record"].as_str().unwrap()),
        previous.join("GROUNDING.yaml")
    );
}

#[test]
fn private_home_default_obeys_remote_guard_and_keeps_existing_bindings_usable() {
    let temp = TempDir::new().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let root = base.join("repo");
    let home = base.join("home");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&home).unwrap();
    simple_runtime::resources(&home.join("resources"));
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("README.md"), "fixture source\n").unwrap();
    let command = |args: &[&str], remote: &str| {
        Command::new(binary())
            .args(args)
            .current_dir(&root)
            .env("HOME", &home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("KPOPPER_NATIVE_RESOURCES", home.join("resources"))
            .env("KPOPPER_NATIVE_CACHE", home.join("cache"))
            .env("CLAUDE_CODE_REMOTE", remote)
            .env_remove("KPOPPER_PROJECTS_DIR")
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .output()
            .unwrap()
    };
    let store = home.join(".kpopper/projects");
    let preview = command(&["config", "--json"], "false");
    assert!(preview.status.success());
    let preview: Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert!(Path::new(preview["record"].as_str().unwrap()).starts_with(&store));
    assert!(!store.exists());

    let refused = command(&["add", "source.first", "file=README.md"], "true");
    assert!(!refused.status.success());
    assert!(!root.join(".git/kpopper/project/project.json").exists());
    assert!(!store.exists());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("KPOPPER_PROJECTS_DIR"));

    let added = command(&["add", "source.first", "file=README.md"], "false");
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let policy = command(&["config", "--json"], "false");
    let policy: Value = serde_json::from_slice(&policy.stdout).unwrap();
    let record = Path::new(policy["record"].as_str().unwrap());
    assert!(record.starts_with(&store));
    assert!(record.is_file());
    let existing = command(&["add", "source.next", "file=README.md"], "true");
    assert!(
        existing.status.success(),
        "{}",
        String::from_utf8_lossy(&existing.stderr)
    );
}
