use kpop_native::public_consolidation::{self, Options};
use std::{fs, path::Path, process::Command};

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
