//! A record left with Git conflict markers names the checked way back.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const RECOVERY: &str = "GROUNDING.yaml holds Git conflict markers. During a stopped git merge (not a rebase or cherry-pick), resolve it with:\n  kpop consolidate --resolve --dry-run\n  kpop consolidate --resolve\nThen review the record, git add it and finish the merge.\n";

fn kpop(root: &Path, resources: Option<&Path>, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .args(args)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .env_remove("GIT_INDEX_FILE")
        .env("KPOPPER_PRIVATE_HOME", root.with_extension("private"))
        .env("KPOPPER_NATIVE_CACHE", root.with_extension("cache"))
        .env("XDG_STATE_HOME", root.with_extension("state"));
    if let Some(resources) = resources {
        command.env("KPOPPER_NATIVE_RESOURCES", resources);
    }
    command.output().unwrap()
}

fn git(root: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .output()
        .unwrap()
}

fn ok(output: Output, what: &str) -> String {
    assert!(
        output.status.success(),
        "{what}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn commit(root: &Path) {
    ok(git(root, &["add", "."]), "git add");
    ok(git(root, &["commit", "-qm", "fixture"]), "git commit");
}

/// A record file written by hand into an empty directory.
fn record(bytes: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("repo");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("GROUNDING.yaml"), bytes).unwrap();
    (temp, root)
}

const CONFLICTED: &str =
    "known:\n<<<<<<< HEAD\n  a.b: {v: 1}\n=======\n  c.d: {v: 2}\n>>>>>>> other\n";

const COMMANDS: [&[&str]; 4] = [
    &["check"],
    &["pull", "a.b"],
    &["open"],
    &["add", "x.y", "v=1"],
];

#[test]
fn a_record_with_conflict_markers_names_the_resolution_after_the_yaml_account() {
    let (_temp, root) = record(CONFLICTED);
    for args in COMMANDS {
        let out = kpop(&root, None, args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.starts_with(
                "GROUNDING.yaml: the record is not valid YAML.\nwhile scanning a simple key\n  in \"GROUNDING.yaml\", line 2, column 1:\n    <<<<<<< HEAD\n"
            ),
            "{args:?}: {stderr}"
        );
        assert!(
            stderr.contains(&format!("\n{RECOVERY}")),
            "{args:?}: {stderr}"
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        CONFLICTED
    );
}

#[test]
fn branch_labels_never_reach_the_suggested_commands() {
    for closing in [
        ">>>>>>> topic;echo${IFS}UNEXPECTED",
        ">>>>>>> topic$(id)",
        ">>>>>>> 1a2b3c4 (Add the venue)",
        ">>>>>>>",
    ] {
        let (_temp, root) = record(&format!(
            "known:\r\n<<<<<<< HEAD\r\n  a.b: {{v: 1}}\r\n=======\r\n  c.d: {{v: 2}}\r\n{closing}\r\n"
        ));
        let out = kpop(&root, None, &["check"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{closing}: {stderr}");
        assert!(stderr.ends_with(RECOVERY), "{closing}: {stderr}");
        let suggested = &stderr[stderr.len() - RECOVERY.len()..];
        assert!(!suggested.contains("topic"), "{closing}: {stderr}");
    }
}

#[test]
fn a_malformed_record_without_a_complete_conflict_names_no_recovery() {
    for bytes in [
        "known:\n  a.b: {v: 1\n",
        // A lone divider is not a conflict.
        "known:\n=======\n  a.b: {v: 1}\n",
        // Markers out of order are not a conflict.
        "known:\n>>>>>>> other\n=======\n<<<<<<< HEAD\n  a.b: {v: 1\n",
        // Markers quoted inside a block scalar are indented, so they are not markers.
        "known:\n  a.b: |\n    <<<<<<< HEAD\n    =======\n    >>>>>>> other\n  c.d: {v: 1\n",
    ] {
        let (_temp, root) = record(bytes);
        for args in COMMANDS {
            let out = kpop(&root, None, args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
            assert!(
                stderr.starts_with("GROUNDING.yaml: the record is not valid YAML.\n"),
                "{args:?}: {stderr}"
            );
            assert!(!stderr.contains("conflict"), "{args:?}: {stderr}");
            assert!(!stderr.contains("consolidate"), "{args:?}: {stderr}");
        }
    }
}

/// A history-backed record born by the first public add, written on two branches and
/// merged in Git until the merge stops on the record.
fn history_merge() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("repo");
    fs::create_dir(&root).unwrap();
    let resources = temp.path().join("resources");
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources
            .join("reasoning")
            .join(format!("{target}.kpopper-runtime")),
    )
    .unwrap();
    let add = |args: &[&str]| ok(kpop(&root, Some(&resources), args), "kpop add");
    ok(git(&root, &["init", "-q", "-b", "main"]), "git init");
    add(&["add", "p.base", "v=0"]);
    assert!(root.join(".kpopper/history.yaml").is_file());
    commit(&root);
    ok(git(&root, &["switch", "-qc", "other"]), "git switch");
    add(&["add", "p.theirs", "v=2"]);
    commit(&root);
    ok(git(&root, &["switch", "-q", "main"]), "git switch");
    add(&["add", "p.ours", "v=1"]);
    commit(&root);
    assert!(
        !git(&root, &["merge", "--no-commit", "other"])
            .status
            .success()
    );
    let record = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(record.contains("\n=======\n"), "{record}");
    (temp, root, resources)
}

#[test]
fn a_merged_history_backed_record_names_the_resolution_that_then_keeps_both_sides() {
    let (_temp, root, resources) = history_merge();
    let conflicted = fs::read(root.join("GROUNDING.yaml")).unwrap();
    for (args, code) in [
        (&["check"][..], 2),
        (&["open"], 2),
        (&["pull", "p.base"], 2),
        (&["add", "p.more", "v=3"], 1),
    ] {
        let out = kpop(&root, Some(&resources), args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(
            stderr.contains(&format!("invalid_history_yaml\n{RECOVERY}")),
            "{args:?}: {stderr}"
        );
    }
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), conflicted);

    ok(
        kpop(
            &root,
            Some(&resources),
            &["consolidate", "--resolve", "--dry-run"],
        ),
        "resolve preview",
    );
    ok(
        kpop(&root, Some(&resources), &["consolidate", "--resolve"]),
        "resolve",
    );
    ok(git(&root, &["add", "GROUNDING.yaml"]), "git add");
    ok(git(&root, &["commit", "-qm", "merge"]), "git commit");
    ok(kpop(&root, Some(&resources), &["check"]), "check");
    for subject in ["p.base", "p.ours", "p.theirs"] {
        ok(kpop(&root, Some(&resources), &["pull", subject]), subject);
    }
}
