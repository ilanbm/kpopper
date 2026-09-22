use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn copy_resources(root: &Path) {
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
    fs::create_dir_all(root.join("resources/ordinary").join(&target)).unwrap();
    let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(
            ordinary.join(name),
            root.join("resources/ordinary").join(&target).join(name),
        )
        .unwrap();
    }
}

fn semantic_files(root: &Path) -> std::collections::BTreeMap<String, String> {
    fn walk(root: &Path, path: &Path, out: &mut std::collections::BTreeMap<String, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
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
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn named_writes_match_complete_python_commands_and_record_images() {
    let cases: Value =
        serde_json::from_slice(include_bytes!("fixtures/legacy-named.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        copy_resources(root);
        fs::write(
            root.join(case["record"].as_str().unwrap()),
            case["before"].as_str().unwrap(),
        )
        .unwrap();
        if let Some(extra) = case["extra_files"].as_object() {
            for (path, content) in extra {
                let path = root.join(path);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, content.as_str().unwrap()).unwrap();
            }
        }
        if let Some(hyp) = case["hypothesis"].as_str() {
            let path = root.join(case["hypothesis_path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, hyp).unwrap();
        }
        let args = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect::<Vec<_>>();
        let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root)
            .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
            .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
            .args(&args)
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env("TZ", "UTC")
            .output()
            .unwrap();
        assert_eq!(
            output.status.code().map(i64::from),
            case["code"].as_i64(),
            "{}: {} {}",
            case["name"],
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            case["stdout"],
            "{}: stdout",
            case["name"]
        );
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            case["stderr"],
            "{}: stderr",
            case["name"]
        );
        assert_eq!(
            serde_json::to_value(semantic_files(root)).unwrap(),
            case["files"],
            "{}: images",
            case["name"]
        );
    }
}

#[test]
fn private_dependencies_stay_out_of_shared_hypotheses() {
    for existing in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        let root = temp.path();
        let before = include_bytes!("fixtures/legacy-authoring/private-before.yaml");
        fs::write(root.join("GROUNDING.yaml"), before).unwrap();
        let hyp = root.join(".kpopper/hypotheses/alpha.yaml");
        if existing {
            fs::create_dir_all(hyp.parent().unwrap()).unwrap();
            fs::write(
                &hyp,
                "hypothesis: {claim: preserve}\nknown: {p.safe: {v: 1}}\n",
            )
            .unwrap();
        }
        let original = semantic_files(root);
        let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(root)
            .env_remove("KPOPPER_AGENT_SESSION")
            .env_remove("CODEX_THREAD_ID")
            .env("KPOPPER_PRIVATE_HOME", private.path())
            .args([
                "add",
                "d.private",
                "verdict=stop",
                "rests_on=[p.public]",
                "wrong_if=p.public > 3",
                "--hypothesis",
                "alpha",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["state"], "private draft");
        assert_eq!(result["reason"], "private or unclear source permission");
        let draft = std::path::PathBuf::from(result["path"].as_str().unwrap());
        assert!(draft.starts_with(private.path().canonicalize().unwrap()));
        assert!(draft.is_file());
        assert_eq!(semantic_files(root), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(draft).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
