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

/// History/v1 records keep their established resolve path. First authorship is now
/// compact, so the legacy record is created explicitly by the retained import emitter.
fn history_merge(competing: bool, corrupt: bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let ordinary = temp.path().join("ordinary");
    fs::create_dir(&ordinary).unwrap();
    fs::write(
        ordinary.join("GROUNDING.yaml"),
        "meta: {reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}}\nknown:\n  p.base: {v: 0}\n",
    )
    .unwrap();
    kpop_native::history_migration::Plan::prepare(
        Path::new("GROUNDING.yaml"),
        &ordinary,
        kpop_native::history_migration::Options {
            operation: "import-fixture".into(),
            recorded_at: "2026-09-25T00:00:00+00:00".into(),
            record_id: Some("record-fixture".into()),
            read_mode: kpop_native::source_capture::ReadMode::Frozen,
            route: false,
            as_of: None,
        },
        None,
    )
    .unwrap()
    .publish(&root)
    .unwrap();
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
    commit(&root);
    let marker = fs::read_to_string(root.join(".kpopper/history.yaml")).unwrap();
    assert!(marker.contains("history/v1") && !marker.contains("node-history"), "{marker}");
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

/// A compact node-history record born by the first public add, then merged in Git.
struct Node {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    resources: std::path::PathBuf,
}
impl Node {
    fn kpop(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&self.root)
            .args(args)
            .env("KPOPPER_NATIVE_RESOURCES", &self.resources)
            .env("KPOPPER_NATIVE_CACHE", self.root.parent().unwrap().join("cache"))
            .env("XDG_STATE_HOME", self.root.parent().unwrap().join("state"))
            .env_remove("KPOPPER_READ_MODE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap()
    }
    fn succeeds(&self, args: &[&str]) -> String {
        let out = self.kpop(args);
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
    /// Working files and the exact index, for refusal checks.
    fn image(&self) -> (Vec<(String, Vec<u8>)>, Vec<u8>) {
        let mut files = vec![];
        let mut pending = vec![self.root.clone()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let name = path.strip_prefix(&self.root).unwrap().to_string_lossy().into_owned();
                if name == ".git" || name.starts_with(".kpopper/.history-local") || name.ends_with(".lock") {
                    continue;
                }
                if path.is_dir() {
                    pending.push(path);
                } else {
                    files.push((name, fs::read(&path).unwrap()));
                }
            }
        }
        files.sort();
        (files, fs::read(self.root.join(".git/index")).unwrap())
    }
    fn refused(&self, message: &str) {
        let before = self.image();
        let out = self.kpop(&["consolidate", "--resolve"]);
        assert!(!out.status.success(), "unexpected success");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(self.image(), before, "a refused resolution changed files or the index");
    }
}
fn node_merge(ours: &[&[&str]], theirs: &[&[&str]]) -> Node {
    node_merge_at(ours, theirs, "GROUNDING.yaml")
}
fn node_merge_at(ours: &[&[&str]], theirs: &[&[&str]], record: &str) -> Node {
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
    let node = Node { _temp: temp, root, resources };
    ok(&node.root, &["init", "-q", "-b", "main"]);
    if record != "GROUNDING.yaml" {
        let config = node.root.join(".git/kpopper/project/project.json");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::create_dir_all(node.root.join(record).parent().unwrap()).unwrap();
        fs::write(config, serde_json::to_vec(&serde_json::json!({
            "version":1, "mode":"simple", "generation":0, "record":record, "publication":null
        })).unwrap()).unwrap();
    }
    node.succeeds(&["add", "p.base", "v=0"]);
    assert!(fs::read_to_string(node.root.join(record).parent().unwrap().join(".kpopper/history.yaml")).unwrap().contains("node-history/v1"));
    commit(&node.root);
    ok(&node.root, &["switch", "-qc", "other"]);
    for args in theirs {
        node.succeeds(args);
    }
    commit(&node.root);
    ok(&node.root, &["switch", "-q", "main"]);
    for args in ours {
        node.succeeds(args);
    }
    commit(&node.root);
    assert!(!git(&node.root, &["merge", "--no-commit", "other"]).status.success());
    assert_eq!(ok(&node.root, &["diff", "--name-only", "--diff-filter=U"]), format!("{record}\n"));
    node
}
fn subjects(node: &Node) -> Vec<String> {
    let status: serde_json::Value =
        serde_json::from_str(&node.succeeds(&["history", "status"])).unwrap();
    let mut names = status["subjects"].as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn compact_nested_record_resolution_names_its_own_manifest() {
    let node = node_merge_at(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]], "nested/GROUNDING.yaml");
    let before = node.image();
    node.succeeds(&["consolidate", "--resolve", "--dry-run"]);
    assert_eq!(node.image(), before);
    let out = node.succeeds(&["consolidate", "--resolve"]);
    assert!(out.contains("nested/GROUNDING.yaml") && out.contains("nested/.kpopper/history-commits/resolve-"), "{out}");
    assert_eq!(node.image().1, before.1);
    assert_eq!(subjects(&node), ["p.a", "p.b", "p.base"]);
    node.succeeds(&["--frozen", "check"]);
}

#[test]
fn compact_resolution_ignores_project_configuration_above_tmpdir() {
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    let poison = tempfile::tempdir().unwrap();
    ok(poison.path(), &["init", "-q", "-b", "main"]);
    fs::create_dir_all(poison.path().join(".git/kpopper/project")).unwrap();
    fs::write(poison.path().join(".git/kpopper/project/project.json"), b"broken json").unwrap();
    let scratch = poison.path().join("tmp");
    fs::create_dir(&scratch).unwrap();
    let before = node.image();
    let out = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&node.root).args(["consolidate", "--resolve", "--dry-run"])
        .env("TMPDIR", &scratch).env("KPOPPER_NATIVE_RESOURCES", &node.resources)
        .env("KPOPPER_NATIVE_CACHE", node.root.parent().unwrap().join("cache"))
        .env_remove("KPOPPER_READ_MODE").env_remove("GIT_INDEX_FILE").output().unwrap();
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(node.image(), before);
}

#[test]
fn compact_branches_resolve_to_one_union_binding_without_staging() {
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    let (before, index) = node.image();
    let head = ok(&node.root, &["rev-parse", "HEAD"]);
    node.succeeds(&["consolidate", "--resolve", "--dry-run"]);
    assert_eq!(node.image(), (before.clone(), index.clone()));
    let out = node.succeeds(&["consolidate", "--resolve"]);
    let (after, index_after) = node.image();
    assert_eq!(index_after, index, "the index changed");
    assert_eq!(ok(&node.root, &["rev-parse", "HEAD"]), head);
    // Every earlier file is byte-identical except the view; one manifest is new.
    let old = before.into_iter().collect::<std::collections::BTreeMap<_, _>>();
    let new = after.into_iter().collect::<std::collections::BTreeMap<_, _>>();
    let added = new.keys().filter(|p| !old.contains_key(*p)).cloned().collect::<Vec<_>>();
    assert_eq!(added.len(), 1, "{added:?}");
    assert!(added[0].starts_with(".kpopper/history-commits/resolve-") && added[0].ends_with(".json"));
    for (path, raw) in &old {
        if path != "GROUNDING.yaml" {
            assert_eq!(&new[path], raw, "{path} changed");
        }
    }
    assert!(out.contains(&format!("{:?}", added[0])) && out.contains("\"GROUNDING.yaml\""), "{out}");
    assert_eq!(subjects(&node), ["p.a", "p.b", "p.base"]);
    node.succeeds(&["--frozen", "check"]);
    // Only the user stages and commits; afterwards ordinary writes continue.
    ok(&node.root, &["add", "--", "GROUNDING.yaml", &added[0]]);
    ok(&node.root, &["commit", "-qm", "merged"]);
    node.succeeds(&["add", "p.c", "v=3"]);
    assert_eq!(subjects(&node), ["p.a", "p.b", "p.base", "p.c"]);
}

#[test]
fn compact_competing_additions_are_refused_without_changes() {
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.a", "v=2"]]);
    node.refused("resolve_contested: p.a");
}

#[test]
fn compact_stream_created_on_one_side_is_kept_exactly() {
    let node = node_merge(
        &[&["add", "p.a", "v=1"]],
        &[&["set", "p.base", "5", "--why", "remeasured"]],
    );
    let streams = ok(&node.root, &["ls-files", "--stage", ".kpopper/history"]);
    assert!(!streams.is_empty());
    node.succeeds(&["consolidate", "--resolve"]);
    assert_eq!(ok(&node.root, &["ls-files", "--stage", ".kpopper/history"]), streams);
    assert_eq!(subjects(&node), ["p.a", "p.base"]);
    node.succeeds(&["--frozen", "check"]);
}

#[test]
fn compact_corrupt_or_foreign_history_refuses_before_replacing_the_record() {
    // A staged history file that neither merge commit holds.
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    let manifest = ok(&node.root, &["diff", "--cached", "--name-only", "--diff-filter=A", "HEAD"])
        .lines()
        .find(|p| p.starts_with(".kpopper/history-commits/"))
        .unwrap()
        .to_owned();
    let path = node.root.join(&manifest);
    let mut raw = fs::read(&path).unwrap();
    raw.insert(raw.len() - 1, b' ');
    fs::write(&path, &raw).unwrap();
    ok(&node.root, &["add", "--", &manifest]);
    node.refused("resolve_history_staged_mismatch");
    // An unstaged change to staged history.
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    let marker = node.root.join(".kpopper/history.yaml");
    let mut raw = fs::read(&marker).unwrap();
    raw.extend_from_slice(b"# local\n");
    fs::write(&marker, raw).unwrap();
    node.refused("resolve_history_worktree_changed");
    // An untracked history file that is not this resolution.
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    fs::write(
        node.root.join(".kpopper/history-commits/resolve-foreign.json"),
        b"{}\n",
    )
    .unwrap();
    node.refused("resolve_history_untracked");
}

#[test]
fn compact_hand_edits_are_preserved_for_an_explicit_decision() {
    // The merged file differs from Git's original conflict output.
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    let entry = node.root.join("GROUNDING.yaml");
    let mut raw = fs::read(&entry).unwrap();
    raw.extend_from_slice(b"# my manual resolution\n");
    fs::write(&entry, raw).unwrap();
    node.refused("resolve_record_edited");
    // A committed side whose view was edited by hand, including its lazy originals.
    let temp = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    ok(&temp.root, &["merge", "--abort"]);
    ok(&temp.root, &["switch", "-q", "other"]);
    let entry = temp.root.join("GROUNDING.yaml");
    let mut document =
        kpop_native::history_yaml::decode_document(&fs::read(&entry).unwrap()).unwrap();
    let kpop_native::value::TypedValue::Map(fields) = &mut document else { panic!() };
    let kpop_native::value::TypedValue::Map(known) = fields.get_mut("known").unwrap() else { panic!() };
    known.insert(
        "p.b".into(),
        kpop_native::value::TypedValue::from_json(&serde_json::json!({"v": 7})).unwrap(),
    );
    fs::write(&entry, kpop_native::history_yaml::encode_document(&document).unwrap()).unwrap();
    commit(&temp.root);
    ok(&temp.root, &["switch", "-q", "main"]);
    assert!(!git(&temp.root, &["merge", "--no-commit", "other"]).status.success());
    temp.refused("resolve_history_side_invalid: MERGE_HEAD");
}

#[test]
fn compact_interrupted_resolution_rolls_forward_to_the_same_union() {
    let node = node_merge(&[&["add", "p.a", "v=1"]], &[&["add", "p.b", "v=2"]]);
    let entry = node.root.join("GROUNDING.yaml");
    let conflicted = fs::read(&entry).unwrap();
    node.succeeds(&["consolidate", "--resolve"]);
    let (resolved, index) = node.image();
    // The manifest was written first; the view is still Git's conflict output.
    fs::write(&entry, &conflicted).unwrap();
    node.succeeds(&["consolidate", "--resolve"]);
    assert_eq!(node.image(), (resolved, index));
}
