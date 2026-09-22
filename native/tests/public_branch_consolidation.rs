use kpop_native::public_consolidation::{self, Options};
use serde_json::Value as J;
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
fn canonicalize_commit_objects(mut body: String) -> String {
    let Some(start) = body.find("objects:\n") else {
        return body;
    };
    let rows = start + "objects:\n".len();
    let Some(relative_end) = body[rows..].find("\noperation:") else {
        return body;
    };
    let end = rows + relative_end + 1;
    let mut blocks = body[rows..end]
        .split("- id: ")
        .skip(1)
        .map(|value| format!("- id: {value}"))
        .collect::<Vec<_>>();
    blocks.sort();
    body.replace_range(rows..end, &blocks.concat());
    body
}

#[test]
fn generated_object_row_normalization_is_order_independent() {
    let a = "objects:\n- id: $OBJECT-2\n  subject: p.b\n- id: kept\n  subject: p.a\n- id: $OBJECT-1\n  subject: p.c\noperation: $OP\n";
    let b = "objects:\n- id: $OBJECT-1\n  subject: p.c\n- id: $OBJECT-2\n  subject: p.b\n- id: kept\n  subject: p.a\noperation: $OP\n";
    assert_eq!(
        canonicalize_commit_objects(a.into()),
        canonicalize_commit_objects(b.into())
    );
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
fn ordinary_branch_fold_requires_a_committed_record_and_sidecar() {
    for dirty_sidecar in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(
            root.join("GROUNDING.yaml"),
            "known:\n  p.value: {v: 1, of: 2026-09-19}\n",
        )
        .unwrap();
        fs::create_dir_all(root.join(".kpopper")).unwrap();
        fs::write(root.join(".kpopper/replaced.yaml"), "{}\n").unwrap();
        let source = commit(&root, "source");
        fs::write(
            root.join("GROUNDING.yaml"),
            "known:\n  p.value: {v: 3, of: 2026-09-18}\n",
        )
        .unwrap();
        commit(&root, "current");
        let path = if dirty_sidecar {
            root.join(".kpopper/replaced.yaml")
        } else {
            root.join("GROUNDING.yaml")
        };
        fs::write(
            &path,
            if dirty_sidecar {
                "changed: true\n"
            } else {
                "known:\n  p.value: {v: 42, note: uncommitted}\n"
            },
        )
        .unwrap();
        let before = image(&root);
        let output = public_consolidation::dispatch(
            &Options {
                from_refs: vec![source],
                as_of: Some("2026-09-20".into()),
                ..Default::default()
            },
            &root,
        );
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("carries uncommitted changes"),
            "{}",
            output.stderr
        );
        assert_eq!(image(&root), before);
    }
}

#[test]
fn ordinary_branch_fold_accepts_ignored_replaced_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(root.join(".gitignore"), ".kpopper/\n").unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value: {v: 1, of: 2026-09-19}\n",
    )
    .unwrap();
    let source = commit(&root, "source");
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.value: {v: 3, of: 2026-09-18}\n",
    )
    .unwrap();
    commit(&root, "current");
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/replaced.yaml"), "{}\n").unwrap();
    let result = public_consolidation::dispatch(
        &Options {
            from_refs: vec![source],
            as_of: Some("2026-09-20".into()),
            ..Default::default()
        },
        &root,
    );
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    assert!(fs::read_to_string(root.join("GROUNDING.yaml")).unwrap().contains("v: 1"));
}

#[test]
fn ordinary_branch_fold_is_idempotent_and_accepts_multiple_refs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 0, of: 2026-09-17}\n  p.a: {v: 1, of: 2026-09-19}\n",
    )
    .unwrap();
    let one = commit(&root, "one");
    fs::write(root.join("GROUNDING.yaml"),"known:\n  p.base: {v: 0, of: 2026-09-17}\n  p.a: {v: 1, of: 2026-09-19}\n  p.b: {v: 2, of: 2026-09-19}\n").unwrap();
    let two = commit(&root, "two");
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 0, of: 2026-09-17}\n",
    )
    .unwrap();
    commit(&root, "current");
    let options = Options {
        from_refs: vec![one.clone(), two.clone()],
        as_of: Some("2026-09-20".into()),
        ..Default::default()
    };
    let output = public_consolidation::dispatch(&options, &root);
    assert_eq!(output.code, 0, "{}{}", output.stdout, output.stderr);
    let body = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(body.contains("p.a:"));
    assert!(body.contains("p.b:"));
    commit(&root, "folded");
    let before = image(&root);
    let again = public_consolidation::dispatch(&options, &root);
    assert_eq!(again.code, 0, "{}{}", again.stdout, again.stderr);
    assert!(again.stdout.contains("nothing to write"));
    assert_eq!(image(&root), before);
}

#[test]
fn ordinary_branch_fold_keeps_local_hypotheses_in_the_selected_pool() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.branch: {v: 2, of: 2026-09-19}\n",
    )
    .unwrap();
    let source = commit(&root, "source");
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  p.base: {v: 1, of: 2026-09-18}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    let local = root.join(".kpopper/hypotheses/local.yaml");
    fs::write(
        &local,
        "hypothesis: {claim: local}\nknown:\n  p.local: {v: 7, of: 2026-09-19}\n",
    )
    .unwrap();
    commit(&root, "current");
    let output = public_consolidation::dispatch(
        &Options {
            from_refs: vec![source.clone()],
            as_of: Some("2026-09-20".into()),
            ..Default::default()
        },
        &root,
    );
    assert_eq!(output.code, 0, "{}{}", output.stdout, output.stderr);
    assert!(
        output.stdout.contains(&format!("folded {source}, local")),
        "{}",
        output.stdout
    );
    let body = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(body.contains("p.branch:"));
    assert!(body.contains("p.local:"));
    assert!(!local.exists());
}

#[test]
fn explicitly_selected_private_branch_hypothesis_retains_source_metadata_and_refuses() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta: {privacy: private}\nknown: {p.base: {v: 1}}\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/scenario.yaml"),
        "hypothesis: {claim: private branch choice}\nknown: {p.secret: {v: 7}}\n",
    )
    .unwrap();
    let source = commit(&root, "private source");
    fs::write(root.join("GROUNDING.yaml"), "known: {p.base: {v: 1}}\n").unwrap();
    fs::remove_dir_all(root.join(".kpopper")).unwrap();
    commit(&root, "public current");
    let before = image(&root);
    let private_temp = tempfile::tempdir().unwrap();
    let private = private_temp.path().to_path_buf();
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&root)
        .args([
            "consolidate",
            &format!("{source}:scenario"),
            "--from",
            &source,
            "--as-of",
            "2026-09-20",
        ])
        .env("KPOPPER_PRIVATE_HOME", &private)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("private draft retained"),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(image(&root), before);
    let drafts = image(&private);
    let bodies = drafts
        .iter()
        .filter(|(path, _)| path.extension().is_some_and(|v| v == "json"))
        .map(|(_, body)| body)
        .collect::<Vec<_>>();
    assert_eq!(bodies.len(), 1);
    let body = String::from_utf8_lossy(bodies[0]);
    assert!(body.contains("source_record_metadata"), "{body}");
    assert!(body.contains("privacy"), "{body}");
    assert!(body.contains("private"), "{body}");
    if let (Some(python), Some(oracle)) = (
        std::env::var_os("KPOP_SESSION_ORACLE_PYTHON"),
        std::env::var_os("KPOP_SESSION_ORACLE_ROOT"),
    ) {
        let python_private = tempfile::tempdir().unwrap();
        let expected = Command::new(python)
            .arg(PathBuf::from(oracle).join("scripts/cli.py"))
            .current_dir(&root)
            .args([
                "consolidate",
                &format!("{source}:scenario"),
                "--from",
                &source,
                "--as-of",
                "2026-09-20",
            ])
            .env("KPOPPER_PRIVATE_HOME", python_private.path())
            .env("KPOPPER_SESSION_DISABLE", "1")
            .output()
            .unwrap();
        assert_eq!(expected.status.code(), output.status.code());
        assert_eq!(expected.stdout, output.stdout);
        let python_files = image(python_private.path());
        let python_body = python_files
            .values()
            .find(|raw| String::from_utf8_lossy(raw).contains("source_record_metadata"))
            .unwrap();
        let normalize = |raw: &[u8]| {
            regex::Regex::new(r"(?:draft-)?[0-9a-f]{32,64}")
                .unwrap()
                .replace_all(&String::from_utf8_lossy(raw), "$ID")
                .into_owned()
        };
        assert_eq!(normalize(bodies[0]), normalize(python_body));
    }
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
        "known:\n  p.value:\n    v: 1\n    of: 2026-09-19\n  p.extra:\n    v: 2\n    of: 2026-09-19\n",
    )
    .unwrap();
    let second = commit(&repository, "second source");
    fs::write(
        repository.join("GROUNDING.yaml"),
        "known:\n  p.value:\n    v: 3\n    of: 2026-09-18\n",
    )
    .unwrap();
    fs::create_dir_all(repository.join(".kpopper")).unwrap();
    fs::write(repository.join(".kpopper/replaced.yaml"), "{}\n").unwrap();
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
    let oracle_python = std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python");
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let args = [
        "consolidate",
        "--from",
        oid.as_str(),
        "--from",
        second.as_str(),
        "--as-of",
        "2026-09-20",
    ];
    for relative in ["GROUNDING.yaml", ".kpopper/replaced.yaml"] {
        let np = native.join(relative);
        let pp = python.join(relative);
        let nb = fs::read(&np).unwrap();
        let pb = fs::read(&pp).unwrap();
        let dirty = if relative == "GROUNDING.yaml" {
            b"known: {p.value: {v: 42}}\n".as_slice()
        } else {
            b"changed: true\n".as_slice()
        };
        fs::write(&np, dirty).unwrap();
        fs::write(&pp, dirty).unwrap();
        let ni = image(&native);
        let pi = image(&python);
        let actual = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&native)
            .args(args)
            .env("KPOPPER_SESSION_DISABLE", "1")
            .env("XDG_STATE_HOME", source.path().join("native-state"))
            .output()
            .unwrap();
        let expected = Command::new(&oracle_python)
            .arg(oracle_root.join("scripts/cli.py"))
            .current_dir(&python)
            .args(args)
            .env("KPOPPER_SESSION_DISABLE", "1")
            .env("XDG_STATE_HOME", source.path().join("python-state"))
            .output()
            .unwrap();
        assert_eq!(
            (actual.status.code(), actual.stdout, actual.stderr),
            (expected.status.code(), expected.stdout, expected.stderr),
            "dirty {relative}"
        );
        assert_eq!(image(&native), ni);
        assert_eq!(image(&python), pi);
        fs::write(np, nb).unwrap();
        fs::write(pp, pb).unwrap();
    }
    let actual = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&native)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("native-state"))
        .output()
        .unwrap();
    let expected = Command::new(&oracle_python)
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
    for root in [&native, &python] {
        let committed = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["add", "-A"])
            .status()
            .unwrap();
        assert!(committed.success());
        let committed = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=f@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "folded",
            ])
            .env("GIT_AUTHOR_DATE", "2026-09-20T12:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-09-20T12:00:00Z")
            .status()
            .unwrap();
        assert!(committed.success());
    }
    let again = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&native)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("native-state"))
        .output()
        .unwrap();
    let expected_again = Command::new(&oracle_python)
        .arg(oracle_root.join("scripts/cli.py"))
        .current_dir(&python)
        .args(args)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .env("XDG_STATE_HOME", source.path().join("python-state"))
        .output()
        .unwrap();
    assert_eq!(
        (again.status.code(), again.stdout, again.stderr),
        (
            expected_again.status.code(),
            expected_again.stdout,
            expected_again.stderr
        )
    );
    assert_eq!(image(&native), image(&python));
}

#[test]
#[ignore = "requires immutable Python 1.8 oracle"]
fn history_public_cli_preview_and_adoption_match_complete_outputs_and_images() {
    let oracle_python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle Python"));
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().canonicalize().unwrap();
    let setup = r#"
from pathlib import Path
import json,sys
from scripts import history_migration as M, history_authoring as A, history_store as H, project_modes as G
b=Path(sys.argv[1]); l=b/'legacy'; l.mkdir(); e=l/'GROUNDING.yaml'; e.write_text('known:\n  p.value: {v: 1}\n')
r=b/'source'; M.prepare(e,operation='import',recorded_at='2026-09-17',record_id='branch-record').publish(r)
(r/'.kpopper/view.yaml').write_text('sections: []\n')
G.git(r,'init','-b','main'); G.git(r,'config','user.name','Fixture'); G.git(r,'config','user.email','fixture@example.invalid'); G.git(r,'add','.'); G.git(r,'-c','commit.gpgsign=false','commit','-m','source')
source=G.git(r,'rev-parse','HEAD').stdout.decode().strip(); head=H.Store(r/'GROUNDING.yaml').state()['subjects']['p.value']['head']
A.commit(r/'GROUNDING.yaml',A.prepare(r/'GROUNDING.yaml',{'kind':'set','id':'p.value','value':3}),verify=lambda d:None)
G.git(r,'add','.'); G.git(r,'-c','commit.gpgsign=false','commit','-m','current')
print(json.dumps({'source':source,'head':head}))
"#;
    let setup = Command::new(&oracle_python)
        .args(["-c", setup])
        .arg(&base)
        .env("PYTHONPATH", &oracle_root)
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "{}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let ids: J = serde_json::from_slice(&setup.stdout).unwrap();
    let source = ids["source"].as_str().unwrap();
    let head = ids["head"].as_str().unwrap();
    let native = base.join("native");
    let python = base.join("python");
    for target in [&native, &python] {
        let out = Command::new("git")
            .args(["clone", "-q", "--no-hardlinks"])
            .arg(base.join("source"))
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    let call = |program: &Path, oracle: bool, root: &Path, args: &[&str]| {
        let mut c = Command::new(program);
        if oracle {
            c.arg(oracle_root.join("scripts/cli.py"));
        }
        c.current_dir(root)
            .args(args)
            .env("KPOPPER_SESSION_DISABLE", "1")
            .env(
                "XDG_STATE_HOME",
                base.join(if oracle {
                    "python-state"
                } else {
                    "native-state"
                }),
            )
            .output()
            .unwrap()
    };
    let preview_args = ["consolidate", "--dry-run", "--from", source];
    let a = call(
        Path::new(env!("CARGO_BIN_EXE_kpop")),
        false,
        &native,
        &preview_args,
    );
    let p = call(&oracle_python, true, &python, &preview_args);
    assert_eq!(
        (a.status.code(), &a.stdout, &a.stderr),
        (p.status.code(), &p.stdout, &p.stderr)
    );
    assert_eq!(image(&native), image(&python));
    let baseline = image(&native);
    let hex = regex::Regex::new(r"[0-9a-f]{64}").unwrap();
    let baseline_ids = baseline
        .values()
        .flat_map(|raw| {
            let text = String::from_utf8_lossy(raw);
            hex.find_iter(&text)
                .map(|m| m.as_str().to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::BTreeSet<_>>();
    let choose = format!("p.value={head}");
    let args = [
        "consolidate",
        "--from",
        source,
        "--by",
        "reviewer",
        "--choose",
        &choose,
    ];
    for relative in ["GROUNDING.yaml", ".kpopper/view.yaml"] {
        let np = native.join(relative);
        let pp = python.join(relative);
        let nb = fs::read(&np).unwrap();
        let pb = fs::read(&pp).unwrap();
        let dirty = if relative == "GROUNDING.yaml" {
            b"known: {p.value: {v: 42}}\n".as_slice()
        } else {
            b"sections: [{title: dirty}]\n".as_slice()
        };
        fs::write(&np, dirty).unwrap();
        fs::write(&pp, dirty).unwrap();
        let before_n = image(&native);
        let before_p = image(&python);
        let na = call(
            Path::new(env!("CARGO_BIN_EXE_kpop")),
            false,
            &native,
            &args,
        );
        let py = call(&oracle_python, true, &python, &args);
        assert_eq!(
            (na.status.code(), na.stdout, na.stderr),
            (py.status.code(), py.stdout, py.stderr),
            "dirty {relative}"
        );
        assert_eq!(image(&native), before_n);
        assert_eq!(image(&python), before_p);
        fs::write(np, nb).unwrap();
        fs::write(pp, pb).unwrap();
    }
    let mut too_many = vec!["consolidate".to_owned(), "--dry-run".into()];
    for _ in 0..17 {
        too_many.extend(["--from".into(), source.into()]);
    }
    let borrowed = too_many.iter().map(String::as_str).collect::<Vec<_>>();
    let na = call(
        Path::new(env!("CARGO_BIN_EXE_kpop")),
        false,
        &native,
        &borrowed,
    );
    let py = call(&oracle_python, true, &python, &borrowed);
    assert_eq!(
        (na.status.code(), na.stdout, na.stderr),
        (py.status.code(), py.stdout, py.stderr)
    );
    let a = call(
        Path::new(env!("CARGO_BIN_EXE_kpop")),
        false,
        &native,
        &args,
    );
    let p = call(&oracle_python, true, &python, &args);
    assert_eq!(
        a.status.code(),
        p.status.code(),
        "{}{}",
        String::from_utf8_lossy(&a.stdout),
        String::from_utf8_lossy(&a.stderr)
    );
    fn operation(raw: &[u8]) -> String {
        String::from_utf8_lossy(raw)
            .lines()
            .find_map(|l| l.trim().strip_prefix("operation: "))
            .unwrap()
            .to_owned()
    }
    let ao = operation(&a.stdout);
    let po = operation(&p.stdout);
    let normalize = |raw: &[u8], op: &str| String::from_utf8_lossy(raw).replace(op, "$OP");
    assert_eq!(normalize(&a.stdout, &ao), normalize(&p.stdout, &po));
    assert_eq!(a.stderr, p.stderr);
    let native_raw = image(&native);
    let python_raw = image(&python);
    let object_id =
        |mine: &BTreeMap<PathBuf, Vec<u8>>, other: &BTreeMap<PathBuf, Vec<u8>>, op: &str| {
            mine.keys()
                .filter(|p| !other.contains_key(*p) && !p.to_string_lossy().contains(op))
                .filter_map(|p| p.file_stem().and_then(|v| v.to_str()))
                .find(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
                .unwrap()
                .to_owned()
        };
    let aid = object_id(&native_raw, &python_raw, &ao);
    let pid = object_id(&python_raw, &native_raw, &po);
    let normalize_image = |files: BTreeMap<PathBuf, Vec<u8>>, op: &str, id: &str| {
        let timestamp = regex::Regex::new(r"20[0-9]{2}-[0-9]{2}-[0-9]{2}T[0-9:.+\-Z]+").unwrap();
        let digest = regex::Regex::new(r"(committed_set_digest: )[0-9a-f]{64}").unwrap();
        files
            .into_iter()
            .map(|(path, raw)| {
                let body = normalize(&raw, op).replace(id, "$OBJECT");
                let body = timestamp.replace_all(&body, "$TIME");
                let body = digest.replace_all(&body, "${1}$SET");
                let body = hex.replace_all(&body, |caps: &regex::Captures<'_>| {
                    if baseline_ids.contains(&caps[0]) {
                        caps[0].to_owned()
                    } else {
                        "$GENERATED".into()
                    }
                });
                let body = canonicalize_commit_objects(body.into_owned());
                (
                    PathBuf::from(
                        path.to_string_lossy()
                            .replace(op, "$OP")
                            .replace(id, "$OBJECT"),
                    ),
                    body.into_bytes(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let ai = normalize_image(native_raw, &ao, &aid);
    let pi = normalize_image(python_raw, &po, &pid);
    assert_eq!(ai.keys().collect::<Vec<_>>(), pi.keys().collect::<Vec<_>>());
    for (path, left) in &ai {
        let right = &pi[path];
        assert_eq!(
            left,
            right,
            "image differs: {}\nnative: {}\npython: {}",
            path.display(),
            String::from_utf8_lossy(left),
            String::from_utf8_lossy(right)
        );
    }
}
