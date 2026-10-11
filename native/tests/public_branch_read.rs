//! Immutable Python/native CLI parity for committed branch reads. Every case
//! compares complete streams, exit status, and repository bytes.
mod support;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        // Fixture commits must not leave maintenance changing bytes during a read.
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn commit(root: &Path, message: &str) -> String {
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
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
    fn visit(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for item in fs::read_dir(path).unwrap() {
            let path = item.unwrap().path();
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
fn fixture() -> (tempfile::TempDir, PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        concat!(
            "known:\n",
            "  p.value: {v: 1}\n",
            "  p.same: {v: 5}\n",
            "  p.nonfinite: {v: .nan}\n",
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/scenario.yaml"),
        "hypothesis: {claim: Branch scenario, folds: never}\nknown: {p.value: {v: 8}}\n",
    )
    .unwrap();
    let source = commit(&root, "source");
    git(&root, &["tag", "source-tag", &source]);
    git(&root, &["branch", "source-branch", &source]);
    fs::write(
        root.join("GROUNDING.yaml"),
        concat!(
            "known:\n",
            "  p.value: {v: 3}\n",
            "  p.same: {v: 5}\n",
            "  p.nonfinite: {v: .inf}\n",
        ),
    )
    .unwrap();
    fs::remove_file(root.join(".kpopper/hypotheses/scenario.yaml")).unwrap();
    commit(&root, "current");
    (temp, root, source)
}
fn core_fixture() -> (tempfile::TempDir, PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let body = |value| {
        format!(
            "meta: {{reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}}}\nknown: {{p.value: {{v: {value}}}}}\n"
        )
    };
    fs::write(root.join("GROUNDING.yaml"), body(1)).unwrap();
    let source = commit(&root, "source core");
    fs::write(root.join("GROUNDING.yaml"), body(3)).unwrap();
    commit(&root, "current core");
    (temp, root, source)
}
fn private_home(root: &Path) -> PathBuf {
    root.parent().unwrap().join("private")
}
/// One record with a branch that proposes a new value, and one project-scope contribution
/// captured locally beside it.
fn pending_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let record = |value| {
        format!(
            "sources:\n  s.base: {{name: Base}}\nknown:\n  a.one:\n    v: {value}\n    from: s.base\n"
        )
    };
    fs::write(root.join("GROUNDING.yaml"), record(1)).unwrap();
    commit(&root, "base");
    git(&root, &["checkout", "-q", "-b", "other"]);
    fs::write(root.join("GROUNDING.yaml"), record(2)).unwrap();
    commit(&root, "other");
    git(&root, &["checkout", "-q", "main"]);
    import(&root, "project");
    (temp, root)
}
fn import(root: &Path, shareability: &str) {
    let source = root.parent().unwrap().join("import.yaml");
    fs::write(
        &source,
        "sources:\n  s.vendor: {name: Vendor}\nknown:\n  fact.import:\n    v: 10\n    from: s.vendor\n    scope: {kind: external, environment: vendor}\n",
    )
    .unwrap();
    let output = run(
        root,
        &[
            "knowledge",
            "import",
            source.to_str().unwrap(),
            "--shareability",
            shareability,
            "--scope",
            "external",
            "--environment",
            "vendor",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn run(root: &Path, args: &[&str]) -> Output {
    invoke(Path::new(env!("CARGO_BIN_EXE_kpop")), None, root, args)
}
fn stdout(root: &Path, args: &[&str]) -> String {
    let output = run(root, args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
const PENDING: &str = "PENDING cf9b97d86bfd · captured locally · external: vendor · fact.import\n";
const DRAFTS: &str = "1 private drafts retained; inspect `kpop knowledge status`\n";
const HINT: &str = "\naffects <entry> shows what a change reaches\n";
fn invoke(program: &Path, oracle: Option<&Path>, root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(program);
    if let Some(oracle) = oracle {
        command.arg(oracle.join("scripts/cli.py"));
    }
    let resources = root.parent().unwrap().join("resources");
    if oracle.is_none() && resources.is_dir() {
        command.env("KPOPPER_NATIVE_RESOURCES", resources);
    }
    command
        .current_dir(root)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("KPOPPER_NO_CACHE", "1")
        .env_remove("KPOPPER_READ_MODE")
        .env("XDG_STATE_HOME", root.join("state"))
        .env("KPOPPER_PRIVATE_HOME", private_home(root))
        .env("KPOPPER_NATIVE_CACHE", root.parent().unwrap().join("cache"))
        .output()
        .unwrap()
}
fn compare(root: &Path, args: &[&str]) {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python"));
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let before = image(root);
    let actual = invoke(Path::new(env!("CARGO_BIN_EXE_kpop")), None, root, args);
    assert_eq!(
        image(root),
        before,
        "native branch read changed repository bytes"
    );
    let expected = invoke(&python, Some(&oracle), root, args);
    assert_eq!(
        image(root),
        before,
        "Python branch read changed repository bytes"
    );
    assert_eq!(
        actual.status.code(),
        expected.status.code(),
        "exit: {args:?}\nnative stdout: {}\nnative stderr: {}\npython stdout: {}\npython stderr: {}",
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&actual.stderr),
        String::from_utf8_lossy(&expected.stdout),
        String::from_utf8_lossy(&expected.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&actual.stdout),
        String::from_utf8_lossy(&expected.stdout),
        "stdout: {args:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&actual.stderr),
        String::from_utf8_lossy(&expected.stderr),
        "stderr: {args:?}"
    );
}

fn history_fixture() -> (tempfile::TempDir, PathBuf, String) {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python"));
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let script = r#"
from pathlib import Path
import sys
from scripts import history_migration as M, history_authoring as A, project_modes as G
base=Path(sys.argv[1]); legacy=base/'legacy'; legacy.mkdir(); entry=legacy/'GROUNDING.yaml'
entry.write_text('known:\n  p.value: {v: 1}\n')
repo=base/'repo'; M.prepare(entry,operation='import',recorded_at='2026-09-17',record_id='branch-record').publish(repo)
h=repo/'.kpopper/hypotheses'; h.mkdir(); (h/'scenario.yaml').write_text('hypothesis: {claim: Physical scenario, folds: never}\nknown: {p.value: {v: 8}}\n')
G.git(repo,'init','-b','main'); G.git(repo,'config','user.name','Fixture'); G.git(repo,'config','user.email','fixture@example.invalid')
G.git(repo,'add','.'); G.git(repo,'-c','commit.gpgsign=false','commit','-m','source'); source=G.git(repo,'rev-parse','HEAD').stdout.decode().strip()
G.git(repo,'tag','source-tag',source)
G.git(repo,'branch','source-branch',source)
A.commit(repo/'GROUNDING.yaml',A.prepare(repo/'GROUNDING.yaml',{'kind':'set','id':'p.value','value':3}),verify=lambda data:None)
(h/'scenario.yaml').unlink(); G.git(repo,'add','.'); G.git(repo,'-c','commit.gpgsign=false','commit','-m','current')
print(source)
"#;
    let output = Command::new(python)
        .args(["-c", script])
        .arg(&base)
        .env("PYTHONPATH", oracle)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        temp,
        base.join("repo"),
        String::from_utf8(output.stdout).unwrap().trim().into(),
    )
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn ordinary_branch_tag_commit_and_nonfinite_are_exact_and_read_only() {
    let (_temp, root, oid) = fixture();
    for reference in ["source-branch", "source-tag", oid.as_str()] {
        compare(
            &root,
            &[
                "pull",
                "p.value",
                "p.same",
                "p.nonfinite",
                "--from",
                reference,
            ],
        );
    }
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle and parent-owned CLI refusal exit mapping"]
fn missing_ref_cli_contract_is_exact() {
    let (_temp, root, _) = fixture();
    compare(&root, &["pull", "p.value", "--from", "missing-ref"]);
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn legacy_entry_rename_and_branch_hypothesis_qualification_are_exact() {
    let (_temp, root, oid) = fixture();
    git(&root, &["mv", "GROUNDING.yaml", "PROVENANCE.yaml"]);
    commit(&root, "legacy entry rename");
    compare(
        &root,
        &["pull", "p.value", "--from", &oid, "PROVENANCE.yaml"],
    );
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle and parent-owned CLI refusal exit mapping"]
fn core_profile_branch_projection_is_exact() {
    let (_temp, root, oid) = core_fixture();
    compare(&root, &["pull", "p.value", "--from", &oid]);
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn active_history_branch_report_is_exact_for_tag_commit_budget_and_hypotheses() {
    let (_temp, root, oid) = history_fixture();
    for reference in ["source-branch", "source-tag", oid.as_str()] {
        compare(
            &root,
            &[
                "pull", "p.value", "missing", "--budget", "1", "--from", reference,
            ],
        );
    }
}

#[test]
fn branch_pull_leads_with_the_records_pending_lines() {
    let (_temp, root) = pending_fixture();
    let before = image(&root);
    assert!(stdout(&root, &["pull", "a.one"]).starts_with(PENDING));
    assert_eq!(
        stdout(&root, &["pull", "--from", "other", "a.one"]),
        format!("{PENDING}a.one: 1 <- s.base\n    proposes 1 -> 2, from other\n{HINT}")
    );
    assert_eq!(image(&root), before);
}

fn without_private_home(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    let resources = root.parent().unwrap().join("resources");
    if resources.is_dir() {
        command.env("KPOPPER_NATIVE_RESOURCES", resources);
    }
    command
        .current_dir(root)
        .args(args)
        .env_remove("HOME")
        .env_remove("KPOPPER_PRIVATE_HOME")
        .env_remove("KPOPPER_READ_MODE")
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("KPOPPER_NO_CACHE", "1")
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("KPOPPER_NATIVE_CACHE", root.parent().unwrap().join("cache"))
        .output()
        .unwrap()
}

#[test]
fn invalid_branch_budgets_are_refused_before_private_drafts_are_read() {
    let (_temp, root, _) = fixture();
    let output = without_private_home(
        &root,
        &[
            "pull",
            "p.value",
            "--from",
            "source-branch",
            "--budget",
            "0",
        ],
    );
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("invalid_pull_budget"), "{error}");
    assert!(!error.contains("home_directory_unavailable"), "{error}");
}

#[test]
fn native_history_profile_refusal_precedes_private_draft_access() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    let resources = temp.path().join("resources/reasoning");
    fs::create_dir_all(&resources).unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join(format!("{target}.kpopper-runtime")),
    )
    .unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    let added = run(&root, &["add", "p.value", "v=1"]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(root.join(".kpopper/history.yaml").is_file());
    commit(&root, "source");
    git(&root, &["branch", "source-branch"]);
    assert!(run(&root, &["set", "p.value", "2"]).status.success());
    commit(&root, "current");
    let before = image(&root);
    let output = without_private_home(&root, &["pull", "p.value", "--from", "source-branch"]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("unsupported_capability: use core/v1 consumer"),
        "{error}"
    );
    assert!(!error.contains("home_directory_unavailable"), "{error}");
    assert!(output.stdout.is_empty());
    assert_eq!(image(&root), before);
}

#[test]
fn branch_pull_counts_the_pending_lines_toward_the_budget() {
    let (_temp, root) = pending_fixture();
    // The pending line takes one of the budget's lines in both reads, so the same record
    // lines survive the cut.
    assert_eq!(
        stdout(&root, &["pull", "a.one", "--budget", "2"]),
        format!("{PENDING}a.one: 1 <- s.base\n{HINT}")
    );
    assert_eq!(
        stdout(
            &root,
            &["pull", "--from", "other", "a.one", "--budget", "2"]
        ),
        format!("{PENDING}a.one: 1 <- s.base\n... 1 more lines - raise the budget\n{HINT}")
    );
    assert_eq!(
        stdout(&root, &["pull", "a.one", "--budget", "1"]),
        format!("{PENDING}... 1 more lines - raise the budget\n{HINT}")
    );
    assert_eq!(
        stdout(
            &root,
            &["pull", "--from", "other", "a.one", "--budget", "1"]
        ),
        format!("{PENDING}... 2 more lines - raise the budget\n{HINT}")
    );
}

#[test]
fn branch_pull_names_private_drafts_only_in_a_live_read() {
    let (_temp, root) = pending_fixture();
    import(&root, "private");
    let proposal = "a.one: 1 <- s.base\n    proposes 1 -> 2, from other\n";
    assert!(stdout(&root, &["pull", "a.one"]).starts_with(&format!("{PENDING}{DRAFTS}")));
    assert_eq!(
        stdout(&root, &["pull", "--from", "other", "a.one"]),
        format!("{PENDING}{DRAFTS}{proposal}{HINT}")
    );
    let frozen = stdout(&root, &["--frozen", "pull", "--from", "other", "a.one"]);
    assert!(!frozen.contains("private drafts"), "{frozen}");
    assert!(!stdout(&root, &["--frozen", "pull", "a.one"]).contains("private drafts"));
}

#[test]
fn branch_pull_preserves_conflict_and_unverified_target_warnings() {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    for (name, warning) in [
        ("two", "CONFLICT api.limit:"),
        (
            "target_core",
            "TARGET UNVERIFIED: unsupported_capability: use core/v1 consumer",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let case = fixtures
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap();
        support::pending::fixture(&root, case);
        commit(&root, "branch snapshot");
        let plain = stdout(&root, &["pull", "local.one"]);
        assert!(plain.contains(warning), "{name}: {plain}");
        let before = image(&root);
        let branch = stdout(&root, &["pull", "local.one", "--from", "HEAD"]);
        assert!(
            branch.find(warning).unwrap() < branch.find("local.one:").unwrap(),
            "{name}: {branch}"
        );
        let prefix = plain.split("local.one:").next().unwrap();
        assert!(branch.starts_with(prefix), "{name}: {branch}");
        let limited = stdout(
            &root,
            &["pull", "local.one", "--from", "HEAD", "--budget", "1"],
        );
        assert!(
            limited.starts_with(plain.lines().next().unwrap()),
            "{name}: {limited}"
        );
        assert!(!limited.contains("local.one:"), "{name}: {limited}");
        assert_eq!(image(&root), before, "{name}: branch read wrote");
    }
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn branch_pull_pending_lines_and_budget_cut_are_exact() {
    let (_temp, root) = pending_fixture();
    compare(&root, &["pull", "--from", "other", "a.one"]);
    compare(
        &root,
        &["pull", "--from", "other", "a.one", "--budget", "2"],
    );
    import(&root, "private");
    compare(&root, &["pull", "--from", "other", "a.one"]);
}
