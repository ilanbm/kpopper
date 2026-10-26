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
    let output = Command::new("git")
        .args(args)
        .arg(root)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn run(root: &Path, home: &Path, projects: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .env("HOME", home)
        .env("KPOPPER_PROJECTS_DIR", projects)
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
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("README.md"), "fixture source text\n").unwrap();

    let first = {
        let root = root.clone();
        let home = home.clone();
        let projects = projects.clone();
        std::thread::spawn(move || run(&root, &home, &projects, &["add", "source.one", "file=README.md"]))
    };
    let second = {
        let root = root.clone();
        let home = home.clone();
        let projects = projects.clone();
        std::thread::spawn(move || run(&root, &home, &projects, &["add", "source.two", "file=README.md"]))
    };
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));

    let config_path = root.join(".git/kpopper/project/project.json");
    let config: Value = serde_json::from_slice(&fs::read(&config_path).unwrap()).unwrap();
    let record = PathBuf::from(config["record"].as_str().unwrap());
    assert!(record.starts_with(&projects));
    assert!(!root.join("GROUNDING.yaml").exists());
    assert!(record.is_file());
    let central_entries: Vec<_> = fs::read_dir(&projects).unwrap().map(|entry| entry.unwrap()).collect();
    assert_eq!(central_entries.len(), 1);
    assert!(central_entries[0].path().join("GROUNDING.yaml").is_file());

    for id in ["source.one", "source.two"] {
        let output = run(&root, &home, &projects, &["pull", id]);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}
