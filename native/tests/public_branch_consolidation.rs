use kpop_native::public_consolidation::{self, Options};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn commit(root: &Path, message: &str) -> String {
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=f@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
    git(root, &["rev-parse", "HEAD"])
}
fn image(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, at: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(at).unwrap() {
            let path = item.unwrap().path();
            if path.file_name().and_then(|v| v.to_str()) == Some(".git") {
                continue;
            }
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root).unwrap().into(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
fn ordinary_branch_preview_and_fold_keep_source_ref_and_write_only_destination() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 1\n    of: 2026-09-19\n",
    )
    .unwrap();
    let source = commit(&root, "source");
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 3\n    of: 2026-09-18\n",
    )
    .unwrap();
    commit(&root, "current");
    let before_head = git(&root, &["rev-parse", "HEAD"]);
    let options = Options {
        from_refs: vec![source.clone()],
        dry_run: true,
        ..Default::default()
    };
    let preview = public_consolidation::dispatch(&options, &root);
    assert_eq!(preview.code, 0, "{}", preview.stderr);
    assert!(preview.stdout.contains(&source), "{}", preview.stdout);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), before_head);
    let folded = public_consolidation::dispatch(
        &Options {
            dry_run: false,
            ..options
        },
        &root,
    );
    assert_eq!(folded.code, 0, "{}{}", folded.stdout, folded.stderr);
    assert!(folded.stdout.contains(&format!("folded {source}")));
    assert!(
        fs::read_to_string(root.join("GROUNDING.yaml"))
            .unwrap()
            .contains("v: 1")
    );
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), before_head);
    assert_eq!(
        git(&root, &["show", &format!("{source}:GROUNDING.yaml")]),
        "known:\n  p.value:\n    v: 1\n    of: 2026-09-19"
    );
}

#[test]
fn ordinary_branch_fold_refuses_nonfinite_source_without_changing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value: {v: .nan}\n",
    )
    .unwrap();
    let source = commit(&root, "nonfinite source");
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.value: {v: 3}\n").unwrap();
    commit(&root, "finite current");
    let before = image(&root);
    let output = public_consolidation::dispatch(
        &Options {
            from_refs: vec![source],
            ..Default::default()
        },
        &root,
    );
    assert_eq!(output.code, 1);
    assert_eq!(output.stdout, "");
    assert_eq!(
        output.stderr,
        "invalid_history_value: snapshot data contains a nonfinite value\n"
    );
    assert_eq!(image(&root), before);
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn ordinary_public_cli_matches_python_complete_output_and_files() {
    let source = tempfile::tempdir().unwrap();
    let repository = source.path().join("source");
    fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-q", "-b", "main"]);
    fs::write(
        repository.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 1\n    of: 2026-09-19\n",
    )
    .unwrap();
    let oid = commit(&repository, "source");
    fs::write(
        repository.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 3\n    of: 2026-09-18\n",
    )
    .unwrap();
    commit(&repository, "current");
    let native = source.path().join("native");
    let python = source.path().join("python");
    for target in [&native, &python] {
        let out = Command::new("git")
            .args(["clone", "-q", "--no-hardlinks"])
            .arg(&repository)
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    let args = [
        "consolidate",
        "--from",
        oid.as_str(),
        "--as-of",
        "2026-09-20",
    ];
    let actual = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .current_dir(&native)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("native-state"))
        .output()
        .unwrap();
    let oracle_python = std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python");
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let expected = Command::new(oracle_python)
        .arg(oracle_root.join("scripts/cli.py"))
        .current_dir(&python)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("python-state"))
        .output()
        .unwrap();
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout, "stdout");
    assert_eq!(actual.stderr, expected.stderr, "stderr");
    assert_eq!(image(&native), image(&python), "complete after image");
}
