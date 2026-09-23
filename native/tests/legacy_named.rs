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

fn kpop(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("TZ", "UTC");
    command
}

fn success(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
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
    let config: Value = serde_json::from_str(&success(
        kpop(root).args(["config", "--json"]).output().unwrap(),
    ))
    .unwrap();
    config["project"]["mode"].as_str().unwrap().to_owned()
}

fn semantic_files(root: &Path) -> std::collections::BTreeMap<String, String> {
    fn walk(root: &Path, path: &Path, out: &mut std::collections::BTreeMap<String, String>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == ".git") {
                    continue;
                }
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

fn assert_named_cases_match_python(advanced: bool) {
    let cases: Value =
        serde_json::from_slice(include_bytes!("fixtures/legacy-named.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        copy_resources(root);
        if advanced {
            advanced_project(root);
        }
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
        let output = kpop(root).args(&args).output().unwrap();
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
        if advanced {
            assert_eq!(project_mode(root), "advanced", "{}", case["name"]);
        }
    }
}

#[test]
fn named_writes_match_complete_python_commands_and_record_images() {
    assert_named_cases_match_python(false);
}

#[test]
fn named_writes_in_an_advanced_project_match_the_same_python_images() {
    assert_named_cases_match_python(true);
}

#[test]
fn a_same_day_refusal_names_a_hypothesis_write_an_advanced_project_folds() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    copy_resources(root);
    advanced_project(root);
    let record = root.join("GROUNDING.yaml");
    let before = "sources:\n  s.doc:\n    name: \"A document\"\n    file: \"doc.txt\"\n    read: \"2026-09-02\"\nknown:\n  p.hours:\n    name: \"Hours\"\n    v: 10\n    from: s.doc\n    at: \"line 1\"\n";
    fs::write(&record, before).unwrap();
    assert_eq!(project_mode(root), "advanced");

    let refused = kpop(root)
        .args([
            "set",
            "p.hours",
            "12",
            "--source",
            "s.doc",
            "--at",
            "line 2",
            "--as-of",
            "2026-09-02",
            "--why",
            "reread",
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    let stderr = String::from_utf8(refused.stderr).unwrap();
    let (_, suggestion) = stderr
        .trim_end()
        .split_once("a hypothesis holds the other: ")
        .unwrap_or_else(|| panic!("{stderr}"));
    let args = shlex::split(suggestion).unwrap();
    let name = args
        .iter()
        .skip_while(|arg| *arg != "--hypothesis")
        .nth(1)
        .unwrap()
        .clone();

    success(kpop(root).args(&args).output().unwrap());
    assert_eq!(fs::read_to_string(&record).unwrap(), before);
    let hypothesis = root
        .join(".kpopper/hypotheses")
        .join(format!("{name}.yaml"));
    assert!(
        fs::read_to_string(&hypothesis)
            .unwrap()
            .contains("    v: 12\n")
    );

    // The same-day reading is contested; the dry run names a later reading,
    // set in the hypothesis, as the way on.
    let contested = kpop(root)
        .args(["consolidate", "--dry-run"])
        .output()
        .unwrap();
    assert!(!contested.status.success());
    assert!(
        String::from_utf8_lossy(&contested.stdout)
            .contains("set it in the base or in the hypothesis with --as-of")
    );
    success(
        kpop(root)
            .args([
                "set",
                "p.hours",
                "12",
                "--as-of",
                "2026-09-03",
                "--hypothesis",
                &name,
            ])
            .output()
            .unwrap(),
    );
    success(kpop(root).args(["consolidate", &name]).output().unwrap());
    assert_eq!(
        fs::read_to_string(&record).unwrap(),
        "sources:\n  s.doc:\n    name: \"A document\"\n    file: \"doc.txt\"\n    read: \"2026-09-02\"\nknown:\n  p.hours:\n    name: \"Hours\"\n    v: 12\n    of: \"2026-09-03\"\n    # set 2026-09-02: reread\n    from: s.doc\n    at: \"line 2\"\n"
    );
    assert!(!hypothesis.exists());
}

#[test]
fn feature_scoped_named_writes_match_python() {
    for advanced in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        copy_resources(root);
        if advanced {
            advanced_project(root);
        }
        let record = root.join("GROUNDING.yaml");
        let before = include_bytes!("fixtures/legacy-authoring/simple-before.yaml");
        fs::write(&record, before).unwrap();
        let scoped = [
            "--as-of",
            "2026-09-03",
            "--hypothesis",
            "scoped",
            "--shareability",
            "project",
            "--scope",
            "feature",
            "--environment",
            "checkout",
        ];
        assert_eq!(
            success(
                kpop(root)
                    .args(["set", "p.beta", "3"])
                    .args(scoped)
                    .output()
                    .unwrap()
            ),
            "carry p.beta into known, its first entry of hypothesis scoped\nset p.beta in hypothesis scoped: 2 -> 3 (as of 2026-09-03)\nrests on it, under scoped:\n  MUTED     d.keep: p.beta moved 2 -> 3, inside wrong_if (p.beta > 3) - nothing is asked\n\nthe base is untouched; scoped holds 1 entry and 0 judgments\n"
        );
        assert_eq!(
            success(
                kpop(root)
                    .args(["add", "p.gamma", "v=5"])
                    .args(scoped)
                    .output()
                    .unwrap()
            ),
            "add p.gamma into known, after p.beta of hypothesis scoped\n\nthe base is untouched; scoped holds 2 entries and 0 judgments\n"
        );
        assert_eq!(fs::read(&record).unwrap(), before);
        assert_eq!(
            fs::read_to_string(root.join(".kpopper/hypotheses/scoped.yaml")).unwrap(),
            "hypothesis: {born: \"2026-09-03\"}\n\nknown:\n  p.beta:\n    v: 3\n    of: 2026-09-03\n\n  p.gamma:\n    v: 5\n    scope: {kind: feature, environment: checkout}\n"
        );
        assert_eq!(
            project_mode(root),
            if advanced { "advanced" } else { "simple" }
        );
    }
}

#[test]
fn a_named_add_keeps_its_scope_text() {
    for advanced in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        copy_resources(root);
        if advanced {
            advanced_project(root);
        }
        let record = root.join("GROUNDING.yaml");
        let before = include_bytes!("fixtures/legacy-authoring/simple-before.yaml");
        fs::write(&record, before).unwrap();
        assert_eq!(
            success(
                kpop(root)
                    .args([
                        "add",
                        "p.gamma",
                        "v=3",
                        "scope=local experiment",
                        "--as-of",
                        "2026-09-19",
                        "--hypothesis",
                        "trial",
                    ])
                    .output()
                    .unwrap()
            ),
            "add p.gamma into known, its first entry of hypothesis trial\n\nthe base is untouched; trial holds 1 entry and 0 judgments\n"
        );
        assert_eq!(fs::read(&record).unwrap(), before);
        assert_eq!(
            fs::read_to_string(root.join(".kpopper/hypotheses/trial.yaml")).unwrap(),
            "hypothesis: {born: \"2026-09-19\"}\n\nknown:\n  p.gamma:\n    v: 3\n    scope: \"local experiment\"\n"
        );
    }
}

/// An entry carried into a hypothesis brings the comment under it, and the
/// next one carried after it goes below that comment.
#[test]
fn a_carried_entry_keeps_the_comment_under_it() {
    for advanced in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        copy_resources(root);
        if advanced {
            advanced_project(root);
        }
        let record = root.join("GROUNDING.yaml");
        let before = "known:\n  p.a: {v: 1, of: \"2026-09-01\"}\n    # set 2026-09-01: seed reading\n  p.b: {v: 2, of: \"2026-09-01\"}\n";
        fs::write(&record, before).unwrap();
        success(
            kpop(root)
                .args([
                    "set",
                    "p.a",
                    "5",
                    "--hypothesis",
                    "h",
                    "--as-of",
                    "2026-09-10",
                ])
                .output()
                .unwrap(),
        );
        assert_eq!(
            success(
                kpop(root)
                    .args([
                        "set",
                        "p.b",
                        "7",
                        "--hypothesis",
                        "h",
                        "--as-of",
                        "2026-09-10"
                    ])
                    .output()
                    .unwrap()
            ),
            "carry p.b into known, after p.a of hypothesis h\nset p.b in hypothesis h: 2 -> 7 (as of 2026-09-10)\nnothing rests on it\n\nthe base is untouched; h holds 2 entries and 0 judgments\n"
        );
        assert_eq!(fs::read_to_string(&record).unwrap(), before);
        assert_eq!(
            fs::read_to_string(root.join(".kpopper/hypotheses/h.yaml")).unwrap(),
            "hypothesis: {born: \"2026-09-10\"}\n\nknown:\n  p.a: {v: 5, of: \"2026-09-10\"}\n    # set 2026-09-01: seed reading\n  p.b: {v: 7, of: \"2026-09-10\"}\n"
        );
        assert_eq!(
            project_mode(root),
            if advanced { "advanced" } else { "simple" }
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
