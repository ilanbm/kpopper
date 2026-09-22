//! Immutable Python/native CLI and durable-image comparison for `config`.
//! The oracle is opt-in because release source and interpreter paths are local inputs.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn git(root: &Path, args: &[&str]) {
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
}
fn fixture(root: &Path, repository: bool) {
    fs::create_dir_all(root.join("repo")).unwrap();
    if repository {
        git(&root.join("repo"), &["init", "-q", "-b", "main"]);
    }
    fs::write(root.join("repo/GROUNDING.yaml"), b"known: {}\n").unwrap();
    fs::write(root.join("shared.yaml"), b"known: {}\n").unwrap();
    fs::write(root.join("different.yaml"), b"known: {answer: {v: 42}}\n").unwrap();
}
fn invoke(program: &Path, oracle_root: Option<&Path>, root: &Path, args: &[String]) -> Output {
    let mut command = Command::new(program);
    if let Some(oracle_root) = oracle_root {
        command.arg(oracle_root.join("scripts/cli.py"));
    }
    command
        .arg("--workspace")
        .arg(root.join("repo"))
        .arg("config")
        .args(args)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("KPOPPER_NATIVE_CACHE", root.join("native-cache"))
        .env("KPOPPER_SESSION_DISABLE", "1")
        .output()
        .unwrap()
}
fn migrate_with_python(python: &Path, oracle_root: &Path, root: &Path) {
    let output = Command::new(python)
        .arg(oracle_root.join("scripts/cli.py"))
        .arg("--workspace")
        .arg(root.join("repo"))
        .args(["expressions", "migrate", "--record"])
        .arg(root.join("repo/GROUNDING.yaml"))
        .args(["--profile", "core/v1", "--destination"])
        .arg(root.join("migrated"))
        .args(["--apply", "--json"])
        .env("XDG_STATE_HOME", root.join("state"))
        .env("KPOPPER_SESSION_DISABLE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "migration setup: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn materialize_incomplete_with_python(python: &Path, oracle_root: &Path, root: &Path) {
    fs::write(
        root.join("repo/GROUNDING.yaml"),
        b"known:\n  p.a: {v: 2}\n  p.b: {rule: missing.input + 1}\njudgments:\n  d.x: {rests_on: [p.b], wrong_if: p.b > 4, seen: {p.b: 3}, verdict: Fine}\n",
    )
    .unwrap();
    let script = r#"
import pathlib, sys
from scripts import core_migration as C
root = pathlib.Path(sys.argv[1])
plan = C.prepare(root / 'repo' / 'GROUNDING.yaml', route=False)
assert plan.problems
destination = root / 'migrated'
for name, data in plan.files.items():
    path = destination / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
"#;
    let output = Command::new(python)
        .args(["-c", script])
        .arg(root)
        .current_dir(root.join("repo"))
        .env("PYTHONPATH", oracle_root)
        .env("KPOPPER_SESSION_DISABLE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn normalized(raw: &[u8], root: &Path) -> Vec<u8> {
    String::from_utf8_lossy(raw)
        .replace(root.canonicalize().unwrap().to_str().unwrap(), "$ROOT")
        .into_bytes()
}
fn compare(
    native: &Path,
    python: &Path,
    oracle_root: &Path,
    native_root: &Path,
    oracle_fixture: &Path,
    args: &[&str],
) {
    let native_args = args
        .iter()
        .map(|arg| arg.replace("$ROOT", native_root.to_str().unwrap()))
        .collect::<Vec<_>>();
    let oracle_args = args
        .iter()
        .map(|arg| arg.replace("$ROOT", oracle_fixture.to_str().unwrap()))
        .collect::<Vec<_>>();
    let actual = invoke(native, None, native_root, &native_args);
    let expected = invoke(python, Some(oracle_root), oracle_fixture, &oracle_args);
    assert_eq!(
        actual.status.code(),
        expected.status.code(),
        "exit: {args:?}"
    );
    let actual_stdout = normalized(&actual.stdout, native_root);
    let expected_stdout = normalized(&expected.stdout, oracle_fixture);
    assert_eq!(actual_stdout, expected_stdout, "stdout: {args:?}");
    assert_eq!(
        normalized(&actual.stderr, native_root),
        normalized(&expected.stderr, oracle_fixture),
        "stderr: {args:?}"
    );
}
fn visit(root: &Path, path: &Path, output: &mut BTreeMap<String, Vec<u8>>) {
    if !path.exists() {
        return;
    }
    for entry in fs::read_dir(path).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            visit(root, &path, output);
            continue;
        }
        if path.file_name().and_then(|n| n.to_str()) == Some("project.lock") {
            continue;
        }
        let relative = path.strip_prefix(root).unwrap().to_string_lossy();
        let key = if relative.contains("mode-history/") {
            let bytes = normalized(&fs::read(&path).unwrap(), root);
            relative.split("mode-history/").next().unwrap().to_owned()
                + "mode-history/"
                + &kpop_native::identity::sha256(&bytes)
                + ".json"
        } else {
            relative.into_owned()
        };
        output.insert(key, normalized(&fs::read(path).unwrap(), root));
    }
}
fn images(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    visit(root, &root.join("state/kpopper/first-use"), &mut result);
    visit(root, &root.join("repo/.git/kpopper"), &mut result);
    result
}
fn migration_images(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = images(root);
    visit(root, &root.join("migrated"), &mut result);
    result.insert(
        "repo/GROUNDING.yaml".into(),
        fs::read(root.join("repo/GROUNDING.yaml")).unwrap(),
    );
    result
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and immutable KPOP_SESSION_ORACLE_ROOT"]
fn empty_and_relative_xdg_state_home_match_python() {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    for state in ["", "relative-state"] {
        let native_temp = tempfile::tempdir().unwrap();
        let oracle_temp = tempfile::tempdir().unwrap();
        let native_root = native_temp.path().canonicalize().unwrap();
        let oracle_fixture = oracle_temp.path().canonicalize().unwrap();
        fixture(&native_root, false);
        fixture(&oracle_fixture, false);
        let call = |program: &Path, oracle: Option<&Path>, root: &Path| {
            let mut command = Command::new(program);
            if let Some(oracle) = oracle {
                command.arg(oracle.join("scripts/cli.py"));
            }
            command
                .arg("--workspace")
                .arg(root.join("repo"))
                .args(["config", "--guidance", "off", "--json"])
                .current_dir(root.join("repo"))
                .env("XDG_STATE_HOME", state)
                .env("HOME", root.join("home"))
                .env("KPOPPER_SESSION_DISABLE", "1")
                .output()
                .unwrap()
        };
        fs::create_dir(native_root.join("home")).unwrap();
        fs::create_dir(oracle_fixture.join("home")).unwrap();
        let actual = call(&native, None, &native_root);
        let expected = call(&python, Some(&oracle_root), &oracle_fixture);
        assert_eq!(actual.status.code(), expected.status.code());
        assert_eq!(
            normalized(&actual.stdout, &native_root),
            normalized(&expected.stdout, &oracle_fixture)
        );
        assert_eq!(actual.stderr, expected.stderr);
        let relative = if state.is_empty() {
            PathBuf::from("home/.local/state/kpopper/first-use/guidance.json")
        } else {
            PathBuf::from("repo/relative-state/kpopper/first-use/guidance.json")
        };
        assert_eq!(
            fs::read(native_root.join(&relative)).unwrap(),
            fs::read(oracle_fixture.join(relative)).unwrap()
        );
    }
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and immutable KPOP_SESSION_ORACLE_ROOT"]
fn python_and_native_config_match_cli_and_durable_images() {
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    for repository in [false, true] {
        let native_temp = tempfile::tempdir().unwrap();
        let oracle_temp = tempfile::tempdir().unwrap();
        let native_root = native_temp.path().canonicalize().unwrap();
        let oracle_fixture = oracle_temp.path().canonicalize().unwrap();
        fixture(&native_root, repository);
        fixture(&oracle_fixture, repository);
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &["--json"],
        );
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &[],
        );
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &["--guidance", "off", "--json"],
        );
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &["--json"],
        );
        fs::write(
            native_root.join("state/kpopper/first-use/guidance.json"),
            b"{\"schema\":1,\"enabled\":\"invalid\"}\n",
        )
        .unwrap();
        fs::write(
            oracle_fixture.join("state/kpopper/first-use/guidance.json"),
            b"{\"schema\":1,\"enabled\":\"invalid\"}\n",
        )
        .unwrap();
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &["--json"],
        );
        fs::write(
            native_root.join("state/kpopper/first-use/guidance.json"),
            b"{\"schema\": 1, \"enabled\": false}\n",
        )
        .unwrap();
        fs::write(
            oracle_fixture.join("state/kpopper/first-use/guidance.json"),
            b"{\"schema\": 1, \"enabled\": false}\n",
        )
        .unwrap();
        if repository {
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "simple",
                    "--record",
                    "$ROOT/missing.yaml",
                    "--check",
                    "--json",
                ],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "simple",
                    "--record",
                    "$ROOT/missing.yaml",
                    "--check",
                ],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "simple",
                    "--record",
                    "$ROOT/different.yaml",
                    "--check",
                    "--json",
                ],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "simple",
                    "--record",
                    "$ROOT/shared.yaml",
                    "--check",
                    "--json",
                ],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "simple",
                    "--record",
                    "$ROOT/shared.yaml",
                    "--json",
                ],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &["--json"],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "advanced",
                    "--record",
                    "GROUNDING.yaml",
                    "--check",
                    "--json",
                ],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &["--mode", "advanced", "--record", "GROUNDING.yaml", "--json"],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "advanced",
                    "--expected-generation",
                    "999",
                    "--json",
                ],
            );
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "simple",
                    "--record",
                    "$ROOT/repo/GROUNDING.yaml",
                    "--check",
                    "--json",
                ],
            );
        }
        assert_eq!(
            images(&native_root),
            images(&oracle_fixture),
            "durable images repository={repository}"
        );
    }
}

#[test]
#[ignore = "requires immutable Python oracle and KPOPPER_NATIVE_RESOURCES"]
fn python_core_migration_forward_and_rollback_match_native_config() {
    assert!(
        std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_some(),
        "set KPOPPER_NATIVE_RESOURCES"
    );
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    let native_temp = tempfile::tempdir().unwrap();
    let oracle_temp = tempfile::tempdir().unwrap();
    let native_root = native_temp.path().canonicalize().unwrap();
    let oracle_fixture = oracle_temp.path().canonicalize().unwrap();
    fixture(&native_root, true);
    fixture(&oracle_fixture, true);
    migrate_with_python(&python, &oracle_root, &native_root);
    migrate_with_python(&python, &oracle_root, &oracle_fixture);
    let receipt = "$ROOT/migrated/.kpopper-migration/receipt.json";
    let migrated = "$ROOT/migrated/GROUNDING.yaml";
    for args in [
        vec![
            "--mode",
            "simple",
            "--record",
            migrated,
            "--migration-receipt",
            receipt,
            "--check",
            "--json",
        ],
        vec![
            "--mode",
            "simple",
            "--record",
            migrated,
            "--migration-receipt",
            receipt,
            "--json",
        ],
        vec![
            "--mode",
            "advanced",
            "--record",
            "GROUNDING.yaml",
            "--migration-receipt",
            receipt,
            "--rollback",
            "--check",
            "--json",
        ],
        vec![
            "--mode",
            "advanced",
            "--record",
            "GROUNDING.yaml",
            "--migration-receipt",
            receipt,
            "--rollback",
            "--json",
        ],
    ] {
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &args,
        );
    }
    assert_eq!(
        migration_images(&native_root),
        migration_images(&oracle_fixture)
    );
}

#[test]
#[ignore = "requires immutable Python oracle and KPOPPER_NATIVE_RESOURCES"]
fn python_core_migration_tampering_refusals_match_native_config() {
    assert!(std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_some());
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    for kind in ["receipt", "inventory", "source", "authorship"] {
        let native_temp = tempfile::tempdir().unwrap();
        let oracle_temp = tempfile::tempdir().unwrap();
        let native_root = native_temp.path().canonicalize().unwrap();
        let oracle_fixture = oracle_temp.path().canonicalize().unwrap();
        fixture(&native_root, true);
        fixture(&oracle_fixture, true);
        migrate_with_python(&python, &oracle_root, &native_root);
        migrate_with_python(&python, &oracle_root, &oracle_fixture);
        let receipt = "$ROOT/migrated/.kpopper-migration/receipt.json";
        let migrated = "$ROOT/migrated/GROUNDING.yaml";
        if kind == "authorship" {
            compare(
                &native,
                &python,
                &oracle_root,
                &native_root,
                &oracle_fixture,
                &[
                    "--mode",
                    "simple",
                    "--record",
                    migrated,
                    "--migration-receipt",
                    receipt,
                    "--json",
                ],
            );
        }
        for root in [&native_root, &oracle_fixture] {
            match kind {
                "receipt" => {
                    writeln!(
                        fs::OpenOptions::new()
                            .append(true)
                            .open(root.join("migrated/.kpopper-migration/receipt.json"))
                            .unwrap()
                    )
                    .unwrap();
                }
                "inventory" => fs::write(root.join("migrated/unexpected"), b"extra").unwrap(),
                "source" => fs::write(
                    root.join("repo/GROUNDING.yaml"),
                    b"known: {changed: {v: true}}\n",
                )
                .unwrap(),
                "authorship" => {
                    writeln!(
                        fs::OpenOptions::new()
                            .append(true)
                            .open(root.join("migrated/GROUNDING.yaml"))
                            .unwrap(),
                        "# further core authorship"
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
        }
        let args = if kind == "authorship" {
            vec![
                "--mode",
                "advanced",
                "--record",
                "GROUNDING.yaml",
                "--migration-receipt",
                receipt,
                "--rollback",
                "--check",
                "--json",
            ]
        } else {
            vec![
                "--mode",
                "simple",
                "--record",
                migrated,
                "--migration-receipt",
                receipt,
                "--check",
                "--json",
            ]
        };
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &args,
        );
        assert_eq!(
            migration_images(&native_root),
            migration_images(&oracle_fixture),
            "tamper case {kind}"
        );
    }
}

#[test]
#[ignore = "requires immutable Python oracle and KPOPPER_NATIVE_RESOURCES"]
fn incomplete_core_plan_precedes_receipt_checks_in_both_directions() {
    assert!(std::env::var_os("KPOPPER_NATIVE_RESOURCES").is_some());
    let python =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("oracle python"));
    let oracle_root =
        PathBuf::from(std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("oracle root"));
    let native = PathBuf::from(env!("CARGO_BIN_EXE_kpop"));
    for rollback in [false, true] {
        let native_temp = tempfile::tempdir().unwrap();
        let oracle_temp = tempfile::tempdir().unwrap();
        let native_root = native_temp.path().canonicalize().unwrap();
        let oracle_fixture = oracle_temp.path().canonicalize().unwrap();
        fixture(&native_root, true);
        fixture(&oracle_fixture, true);
        materialize_incomplete_with_python(&python, &oracle_root, &native_root);
        materialize_incomplete_with_python(&python, &oracle_root, &oracle_fixture);
        fs::OpenOptions::new()
            .append(true)
            .open(native_root.join("migrated/.kpopper-migration/receipt.json"))
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(oracle_fixture.join("migrated/.kpopper-migration/receipt.json"))
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        if rollback {
            for root in [&native_root, &oracle_fixture] {
                let project = root.join("repo/.git/kpopper/project");
                fs::create_dir_all(&project).unwrap();
                fs::write(
                    project.join("project.json"),
                    format!(
                        "{{\"generation\":1,\"mode\":\"simple\",\"publication\":null,\"record\":{},\"version\":1}}\n",
                        serde_json::to_string(
                            &root.join("migrated/GROUNDING.yaml").to_string_lossy()
                        )
                        .unwrap()
                    ),
                )
                .unwrap();
            }
        }
        let args = if rollback {
            vec![
                "--mode",
                "advanced",
                "--record",
                "GROUNDING.yaml",
                "--migration-receipt",
                "$ROOT/migrated/.kpopper-migration/receipt.json",
                "--rollback",
                "--check",
                "--json",
            ]
        } else {
            vec![
                "--mode",
                "simple",
                "--record",
                "$ROOT/migrated/GROUNDING.yaml",
                "--migration-receipt",
                "$ROOT/migrated/.kpopper-migration/receipt.json",
                "--check",
                "--json",
            ]
        };
        compare(
            &native,
            &python,
            &oracle_root,
            &native_root,
            &oracle_fixture,
            &args,
        );
    }
}
