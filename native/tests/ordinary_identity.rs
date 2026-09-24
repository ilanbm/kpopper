use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn resources(root: &Path) {
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(root.join("resources/reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        root.join("resources/reasoning")
            .join(format!("{target}.zip")),
    )
    .unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    let destination = root.join("resources/ordinary").join(&target);
    fs::create_dir_all(&destination).unwrap();
    for filename in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(program.join(filename), destination.join(filename)).unwrap();
    }
}
fn files(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            // These trees belong to this test harness or to Git, not the shared record.
            if path.parent() == Some(root)
                && ["resources", "cache", "state", ".git"]
                    .iter()
                    .any(|name| path.file_name().is_some_and(|part| part == *name))
            {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else if path == root.join(".kpopper/project.lock") {
                // Policy locking and the private journal namespace have fixed control bytes.
                assert!(fs::read(&path).unwrap().is_empty());
            } else if path.ends_with(".history-local/.gitignore") {
                assert_eq!(fs::read(&path).unwrap(), b"*\n");
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    fs::read_to_string(path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}
fn kpop(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TZ", "UTC")
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID");
    command
}
/// A Git project without a configuration is an Advanced one.
fn advanced_project(root: &Path) {
    let init = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
}
fn project_mode(root: &Path) -> String {
    let output = kpop(root).args(["config", "--json"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config: Value = serde_json::from_slice(&output.stdout).unwrap();
    config["project"]["mode"].as_str().unwrap().to_owned()
}
fn assert_identity_cases_match_python(advanced: bool) {
    let cases: Value =
        serde_json::from_slice(include_bytes!("fixtures/ordinary-identity-cli.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        resources(root);
        if advanced {
            advanced_project(root);
        }
        for (path, value) in case["before"].as_object().unwrap() {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value.as_str().unwrap()).unwrap();
        }
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        let result = kpop(root).args(args).output().unwrap();
        let normalized =
            |bytes: &[u8]| String::from_utf8_lossy(bytes).replace(root.to_str().unwrap(), "$ROOT");
        assert_eq!(
            result.status.code().map(i64::from),
            case["code"].as_i64(),
            "{}: {} {}",
            case["name"],
            normalized(&result.stdout),
            normalized(&result.stderr)
        );
        assert_eq!(
            normalized(&result.stdout),
            case["stdout"],
            "{}: stdout",
            case["name"]
        );
        assert_eq!(
            normalized(&result.stderr),
            case["stderr"],
            "{}: stderr",
            case["name"]
        );
        assert_eq!(
            serde_json::to_value(files(root)).unwrap(),
            case["files"],
            "{}: images",
            case["name"]
        );
        if advanced {
            assert_eq!(project_mode(root), "advanced", "{}", case["name"]);
        }
    }
}
#[test]
fn ordinary_identity_matches_complete_python_commands_and_images() {
    assert_identity_cases_match_python(false);
}
/// Git projects default to Advanced mode, and Python writes their own ordinary
/// record with the same `same` and `distinct` bytes as a Simple project's.
#[test]
fn ordinary_identity_in_an_advanced_project_matches_the_same_python_images() {
    assert_identity_cases_match_python(true);
}

#[test]
fn same_embeds_the_public_record_check_with_an_unserved_question() {
    for add_question in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/page");
        for entry in fs::read_dir(fixture).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
        }
        advanced_project(root);
        for args in [
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        resources(root);
        if add_question {
            let added = kpop(root)
                .args(["add", "q.frames", "asked=do the frames need replacing?"])
                .output()
                .unwrap();
            assert!(
                added.status.success(),
                "{}",
                String::from_utf8_lossy(&added.stderr)
            );
        }
        let same = kpop(root)
            .args(["same", "heat.loss_kw", "heat.deficit_kw"])
            .output()
            .unwrap();
        assert!(
            same.status.success(),
            "{}",
            String::from_utf8_lossy(&same.stderr)
        );
        let checked = kpop(root).arg("check").output().unwrap();
        assert!(
            checked.status.success(),
            "{}",
            String::from_utf8_lossy(&checked.stderr)
        );
        let check_text = String::from_utf8(checked.stdout).unwrap();
        let summary = check_text.lines().last().unwrap();
        assert_eq!(
            summary,
            format!(
                "3 judgments, {} entries, 0 problems, 2 declared",
                if add_question { 14 } else { 13 }
            )
        );
        let same_text = String::from_utf8(same.stdout).unwrap();
        assert_eq!(
            same_text
                .lines()
                .find_map(|line| line.strip_prefix("  check: ")),
            Some(summary)
        );
    }
}

#[test]
fn private_identity_closure_is_retained_outside_shared_files() {
    for command in ["same", "distinct"] {
        let temp = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        let root = temp.path();
        let source = "schema: {deps: rests_on, predicate: wrong_if, snapshot: seen}\nknown:\n  p.keep: {v: 1, from: s.secret}\n  p.old: {v: 1}\nsources:\n  s.secret: {privacy: private, note: selected}\n  s.other: {privacy: private, note: unrelated}\n";
        fs::write(root.join("GROUNDING.yaml"), source).unwrap();
        let mut args = vec![command, "p.keep", "p.old"];
        if command == "distinct" {
            args.push("different subjects");
        }
        args.push("GROUNDING.yaml");
        let result = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root)
            .env("KPOPPER_PRIVATE_HOME", private.path())
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let stderr = String::from_utf8(result.stderr).unwrap();
        let path = stderr
            .trim()
            .strip_prefix("private draft retained at ")
            .unwrap();
        let bytes = fs::read(path).unwrap();
        let tagged: Value = serde_json::from_slice(&bytes).unwrap();
        let draft = kpop_native::value::TypedValue::from_tagged(&tagged)
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(draft["action"]["kind"], command);
        assert_eq!(draft["document"]["sources"]["s.secret"]["note"], "selected");
        assert!(draft["document"]["sources"].get("s.other").is_none());
        assert_eq!(
            fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
            source
        );
    }
}

#[test]
fn unsupported_history_values_refuse_without_writes() {
    let cases: Value =
        serde_json::from_slice(include_bytes!("fixtures/ordinary-identity-refusals.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        resources(root);
        for (path, value) in case["before"].as_object().unwrap() {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, value.as_str().unwrap()).unwrap();
        }
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        let result = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root)
            .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
            .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(case["code"], 1);
        assert!(result.stdout.is_empty());
        // The native CLI reports Python's final semantic error without language-specific
        // traceback frames. Both implementations leave every record byte unchanged.
        assert!(
            case["oracle_failure"]
                .as_str()
                .unwrap()
                .ends_with(case["native_stderr"].as_str().unwrap().trim_end())
        );
        assert_eq!(
            String::from_utf8(result.stderr).unwrap(),
            case["native_stderr"],
            "{}",
            case["name"]
        );
        assert_eq!(
            serde_json::to_value(files(root)).unwrap(),
            case["files"],
            "{}",
            case["name"]
        );
        assert_eq!(case["files"], case["before"]);
    }
}
