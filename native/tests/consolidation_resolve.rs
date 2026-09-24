//! Real Git conflicts: the resolver must preserve both knowledge and the user's merge state.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

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
fn ok(root: &Path, args: &[&str]) -> String {
    let result = git(root, args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn commit(root: &Path) {
    ok(root, &["add", "."]);
    ok(root, &["commit", "-qm", "fixture"]);
}
fn conflict(base: &str, ours: &str, theirs: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    ok(root, &["init", "-q", "-b", "main"]);
    fs::write(root.join("GROUNDING.yaml"), base).unwrap();
    commit(root);
    ok(root, &["switch", "-qc", "other"]);
    fs::write(root.join("GROUNDING.yaml"), theirs).unwrap();
    commit(root);
    ok(root, &["switch", "-q", "main"]);
    fs::write(root.join("GROUNDING.yaml"), ours).unwrap();
    commit(root);
    assert!(
        !git(root, &["merge", "--no-commit", "other"])
            .status
            .success()
    );
    temp
}
fn cli(root: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(["consolidate", "--resolve"])
        .args(extra)
        .env_remove("KPOPPER_READ_MODE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap()
}
const BASE: &str = "meta:\n  name: Demo\nknown:\n  p.base: {v: 0}\n";

#[test]
fn independent_additions_preview_then_resolve_without_staging_or_committing() {
    let temp = conflict(
        BASE,
        &(BASE.to_owned() + "  p.a: {v: 1}\n"),
        &(BASE.to_owned() + "  p.b: {v: 2}\n"),
    );
    let root = temp.path();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let head = ok(root, &["rev-parse", "HEAD"]);
    let preview = cli(root, &["--dry-run"]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    let result = cli(root, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let after = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(after.contains("p.a:") && after.contains("p.b:") && !after.contains("<<<<<<<"));
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(ok(root, &["rev-parse", "HEAD"]), head);
    assert!(String::from_utf8_lossy(&result.stdout).contains("git add"));
}

#[test]
fn competing_edits_are_refused_without_changing_files_or_index() {
    let temp = conflict(
        BASE,
        &BASE.replace("v: 0", "v: 1"),
        &BASE.replace("v: 0", "v: 2"),
    );
    let root = temp.path();
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let result = cli(root, &[]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("p.base"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

fn additive() -> tempfile::TempDir {
    conflict(
        BASE,
        &(BASE.to_owned() + "  p.a: {v: 1}\n"),
        &(BASE.to_owned() + "  p.b: {v: 2}\n"),
    )
}
fn refused(root: &Path, args: &[&str], message: &str) {
    let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let output = cli(root, args);
    assert!(!output.status.success(), "unexpected success");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
}

#[test]
fn comments_unicode_and_crlf_survive() {
    let base = "# Project שלום\r\nknown:\r\n  p.base: {v: 0} # retained\r\n";
    let ours = base.to_owned() + "  p.a: {v: 1} # first\r\n";
    let theirs = base.to_owned() + "  p.b: {v: 2} # second\r\n";
    let temp = conflict(base, &ours, &theirs);
    let output = cli(temp.path(), &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap();
    assert_eq!(result, ours + "  p.b: {v: 2} # second\r\n");
}

#[test]
fn preserves_a_partial_manual_resolution_instead_of_overwriting_it() {
    let temp = additive();
    let path = temp.path().join("GROUNDING.yaml");
    fs::write(
        &path,
        fs::read_to_string(&path)
            .unwrap()
            .replace("Demo", "My manual edit"),
    )
    .unwrap();
    refused(temp.path(), &[], "resolve_record_edited");
}

#[test]
fn rejects_other_unmerged_paths() {
    let temp = additive();
    let stages = ok(temp.path(), &["ls-files", "-u", "GROUNDING.yaml"]);
    let oid = stages
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let mut child = Command::new("git")
        .current_dir(temp.path())
        .args(["update-index", "--index-info"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    writeln!(
        child.stdin.take().unwrap(),
        "100644 {oid} 2\tcode.rs\n100644 {oid} 3\tcode.rs"
    )
    .unwrap();
    assert!(child.wait().unwrap().success());
    refused(
        temp.path(),
        &[],
        "resolve_other_conflicts: resolve and stage these paths first: code.rs",
    );
}

#[test]
fn candidate_with_a_dangling_dependency_is_not_written() {
    let ours = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n".to_owned()
        + BASE
        + "  p.a: {v: 1}\njudgments:\n  d.bad:\n    verdict: broken\n    rests_on: [p.missing]\n    seen: {p.missing: 1}\n    wrong_if: p.missing > 2\n";
    let temp = conflict(BASE, &ours, &(BASE.to_owned() + "  p.b: {v: 2}\n"));
    refused(temp.path(), &[], "resolve_check_failed");
}

#[test]
fn conflicted_hypotheses_block_the_candidate() {
    let temp = additive();
    let dir = temp.path().join(".kpopper/hypotheses");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("one.yaml"), "known:\n  p.rival: {v: 10}\n").unwrap();
    fs::write(dir.join("two.yaml"), "known:\n  p.rival: {v: 11}\n").unwrap();
    ok(temp.path(), &["add", ".kpopper/hypotheses"]);
    let before = fs::read(temp.path().join("GROUNDING.yaml")).unwrap();
    let output = cli(temp.path(), &[]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("p.rival"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read(temp.path().join("GROUNDING.yaml")).unwrap(),
        before
    );
}

#[test]
fn delete_edit_of_one_entry_is_a_real_choice() {
    let base = "known:\n  p.base: {v: 0}\n  p.keep: {v: 5}\n";
    let temp = conflict(
        base,
        &base.replace("  p.base: {v: 0}\n", ""),
        &base.replace("v: 0", "v: 1"),
    );
    refused(temp.path(), &[], "resolve_contested: p.base");
}

#[test]
fn rejects_claim_selection_flags_and_respects_an_existing_index_lock() {
    let temp = additive();
    refused(temp.path(), &["--take", "p.a"], "--resolve accepts only");
    refused(
        temp.path(),
        &["--as", "s.fake"],
        "--as names the session source",
    );
    fs::write(temp.path().join(".git/index.lock"), "another Git writer").unwrap();
    refused(temp.path(), &[], "resolve_index_locked");
    assert_eq!(
        fs::read(temp.path().join(".git/index.lock")).unwrap(),
        b"another Git writer"
    );
}

#[test]
fn requires_a_real_git_merge_and_leaves_an_ordinary_record_alone() {
    let temp = tempfile::tempdir().unwrap();
    ok(temp.path(), &["init", "-q", "-b", "main"]);
    fs::write(temp.path().join("GROUNDING.yaml"), BASE).unwrap();
    commit(temp.path());
    refused(temp.path(), &[], "resolve_no_merge");
}

fn history_merge(competing: bool, corrupt: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
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
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&root)
            .args(args)
            .env("KPOPPER_NATIVE_RESOURCES", &resources)
            .env("KPOPPER_NATIVE_CACHE", temp.path().join("cache"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env_remove("KPOPPER_READ_MODE")
            .output()
            .unwrap()
    };
    let succeeds = |args: &[&str]| {
        let out = run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };
    ok(&root, &["init", "-q", "-b", "main"]);
    succeeds(&["add", "p.base", "v=0"]);
    commit(&root);
    ok(&root, &["switch", "-qc", "other"]);
    succeeds(&["add", if competing { "p.a" } else { "p.b" }, "v=2"]);
    commit(&root);
    ok(&root, &["switch", "-q", "main"]);
    succeeds(&["add", "p.a", "v=1"]);
    commit(&root);
    assert!(
        !git(&root, &["merge", "--no-commit", "other"])
            .status
            .success()
    );
    if corrupt {
        let path = ok(&root, &["ls-files", ".kpopper/history"])
            .lines()
            .next()
            .unwrap()
            .to_owned();
        let mut raw = fs::read(root.join(&path)).unwrap();
        raw.extend_from_slice(b"# changed immutable bytes\n");
        fs::write(root.join(&path), raw).unwrap();
        ok(&root, &["add", "--", &path]);
    }
    if competing || corrupt {
        let before = fs::read(root.join("GROUNDING.yaml")).unwrap();
        let index = fs::read(root.join(".git/index")).unwrap();
        let output = run(&["consolidate", "--resolve"]);
        assert!(!output.status.success(), "unexpected history acceptance");
        if competing {
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("resolve_contested: p.a"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        return;
    }
    let history = ok(&root, &["ls-files", "--stage", ".kpopper"]);
    let files: Vec<_> = ok(&root, &["ls-files", ".kpopper"])
        .lines()
        .map(|p| (p.to_owned(), fs::read(root.join(p)).unwrap()))
        .collect();
    succeeds(&["consolidate", "--resolve", "--dry-run"]);
    succeeds(&["consolidate", "--resolve"]);
    let after = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(after.contains("p.a:") && after.contains("p.b:"), "{after}");
    succeeds(&["--frozen", "check"]);
    assert_eq!(ok(&root, &["ls-files", "--stage", ".kpopper"]), history);
    for (path, raw) in files {
        assert_eq!(fs::read(root.join(path)).unwrap(), raw);
    }
}

#[test]
fn history_backed_branches_rebuild_from_preserved_immutable_objects() {
    history_merge(false, false);
}
#[test]
fn competing_history_versions_are_not_silently_dropped_by_rendering() {
    history_merge(true, false);
}
#[test]
fn corrupted_history_refuses_before_replacing_the_record() {
    history_merge(false, true);
}
