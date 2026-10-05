use std::{fs, path::Path, process::Command};

fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(args)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .env("KPOPPER_PRIVATE_HOME", root.join("private"))
        .env("XDG_STATE_HOME", root.join("private-state"))
        .output()
        .unwrap()
}

fn record(bytes: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("GROUNDING.yaml"), bytes).unwrap();
    root
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
fn a_record_with_conflict_markers_names_the_consolidate_recovery() {
    let root = record(CONFLICTED);
    for args in COMMANDS {
        let out = cli(root.path(), args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.starts_with(
                "GROUNDING.yaml: the record is not valid YAML.\nwhile scanning a simple key\n  in \"GROUNDING.yaml\", line 2, column 1:\n    <<<<<<< HEAD\n"
            ),
            "{args:?}: {stderr}"
        );
        assert!(
            stderr.contains(
                "\nGROUNDING.yaml holds Git conflict markers. Keep this branch's side, then fold the other branch in:\n  git checkout --ours GROUNDING.yaml\n  kpop consolidate --from other --dry-run\n  kpop consolidate --from other\n"
            ),
            "{args:?}: {stderr}"
        );
    }
    assert_eq!(
        fs::read_to_string(root.path().join("GROUNDING.yaml")).unwrap(),
        CONFLICTED
    );
}

#[test]
fn a_marker_without_a_single_branch_name_leaves_the_name_to_fill_in() {
    for closing in [">>>>>>> 1a2b3c4 (Add the venue)", ">>>>>>>"] {
        let root = record(&format!(
            "known:\r\n<<<<<<< HEAD\r\n  a.b: {{v: 1}}\r\n=======\r\n  c.d: {{v: 2}}\r\n{closing}\r\n"
        ));
        let out = cli(root.path(), &["check"]);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{closing}: {stderr}");
        assert!(
            stderr.contains(
                "  git checkout --ours GROUNDING.yaml\n  kpop consolidate --from <branch> --dry-run\n  kpop consolidate --from <branch>\n"
            ),
            "{closing}: {stderr}"
        );
    }
}

#[test]
fn a_malformed_record_without_markers_names_no_recovery() {
    for bytes in [
        "known:\n  a.b: {v: 1\n",
        // A lone marker-like line is not a conflict.
        "known:\n=======\n  a.b: {v: 1}\n",
    ] {
        let root = record(bytes);
        for args in COMMANDS {
            let out = cli(root.path(), args);
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
