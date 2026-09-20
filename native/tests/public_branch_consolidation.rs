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
        Path::new(env!("CARGO_BIN_EXE_kpop-native")),
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
    let a = call(
        Path::new(env!("CARGO_BIN_EXE_kpop-native")),
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
                let mut body = body.into_owned();
                if let Some((prefix, rest)) = body.split_once("objects:\n")
                    && let Some((objects, suffix)) = rest.split_once("operation:\n")
                {
                    let mut blocks = objects
                        .split("- id: ")
                        .skip(1)
                        .map(|v| format!("- id: {v}"))
                        .collect::<Vec<_>>();
                    blocks.sort();
                    body = format!("{prefix}objects:\n{}operation:\n{suffix}", blocks.concat());
                }
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
