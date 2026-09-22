//! Immutable Python/native CLI parity for committed branch reads. Every case
//! compares complete streams, exit status, and repository bytes.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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
fn invoke(program: &Path, oracle: Option<&Path>, root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(program);
    if let Some(oracle) = oracle {
        command.arg(oracle.join("scripts/cli.py"));
    }
    command
        .current_dir(root)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("KPOPPER_NO_CACHE", "1")
        .env("XDG_STATE_HOME", root.join("state"))
        .output()
        .unwrap()
}
fn compare(root: &Path, args: &[&str]) {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python"));
    let oracle = PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let before = image(root);
    let actual = invoke(
        Path::new(env!("CARGO_BIN_EXE_kpop")),
        None,
        root,
        args,
    );
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
