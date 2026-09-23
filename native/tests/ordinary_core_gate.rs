//! An ordinary reader leaves alone what only a core/v1 consumer reads, as the Python reader
//! does. A publication target that declares core/v1, or is kept by its history, stays
//! unverified and contests nothing, while the core/v1 snapshot still compares it. A record,
//! or a layer beside an ordinary record, that declares core/v1 is refused by every ordinary
//! command.
mod support;

use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use support::pending::{fixture, git};

const REFUSAL: &str = "unsupported_capability: use core/v1 consumer";

struct Env {
    private: tempfile::TempDir,
    resources: PathBuf,
}
impl Env {
    fn new() -> Self {
        let private = tempfile::tempdir().unwrap();
        let resources = private.path().join("resources");
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        fs::create_dir_all(resources.join("reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.kpopper-runtime")),
            resources.join("reasoning").join(format!("{target}.zip")),
        )
        .unwrap();
        let ordinary = resources.join("ordinary").join(&target);
        fs::create_dir_all(&ordinary).unwrap();
        let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(
                    std::env::var_os("HOME")
                        .or_else(|| std::env::var_os("USERPROFILE"))
                        .unwrap(),
                )
                .join(".cache/kpopper/lean")
                .join(if cfg!(windows) {
                    "windows-amd64"
                } else {
                    &target
                })
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
            fs::copy(program.join(name), ordinary.join(name)).unwrap();
        }
        Self { private, resources }
    }
    fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
        command
            .current_dir(cwd)
            .env("KPOPPER_NATIVE_RESOURCES", &self.resources)
            .env("KPOPPER_NATIVE_CACHE", self.private.path().join("cache"))
            .env("KPOPPER_PRIVATE_HOME", self.private.path().join("private"))
            .env("XDG_STATE_HOME", self.private.path().join("state"))
            .env("TMPDIR", self.private.path())
            .env_remove("KPOPPER_READ_MODE");
        command
    }
    fn kpop(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd).args(args).output().unwrap()
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn ledger_case(name: &str) -> Value {
    let cases: Value = serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
        .clone()
}

/// A workspace set up from one case of the pending-ledger corpus.
fn pending_workspace(name: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("repo");
    fs::create_dir(&root).unwrap();
    fixture(&root, &ledger_case(name));
    (temp, root)
}

/// A committed Git workspace holding the given files.
fn committed(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("repo");
    fs::create_dir(&root).unwrap();
    for (path, raw) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw).unwrap();
    }
    git(&root, &["init", "-q", "-b", "main"], None);
    git(&root, &["add", "-A"], None);
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "-m",
            "record",
        ],
        None,
    );
    (temp, root)
}

fn core_page() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "GROUNDING.yaml",
            include_str!("../../tests/fixtures/core-page/GROUNDING.yaml"),
        ),
        (
            ".kpopper/view.yaml",
            include_str!("../../tests/fixtures/core-page/view.yaml"),
        ),
    ]
}

const CORE_HYPOTHESIS: &str = "meta:\n  reasoning:\n    version: 2\n    profile: core/v1\n    requires: [arithmetic/v1]\nhypothesis:\n  claim: one is three\nknown:\n  a.one:\n    v: 3\n";
const ORDINARY_RECORD: &str = "sources:\n  s.notes:\n    name: Notes\n    file: notes.txt\n    read: 2026-09-20\nknown:\n  a.one:\n    v: 1\n    from: s.notes\n    at: line 1\n  a.two:\n    v: 2\n    from: s.notes\n    at: line 2\njudgments:\n  d.small:\n    verdict: a.one stays small\n    because: the notes say so\n    rests_on: [a.one]\n    wrong_if: a.one > 5\n    seen:\n      a.one: 1\n";

#[test]
fn ordinary_readers_leave_a_core_target_unverified_where_the_core_snapshot_compares_it() {
    let env = Env::new();
    let (_temp, root) = pending_workspace("target_core");

    let open = env.kpop(&root, &["open"]);
    assert!(open.status.success(), "{}", text(&open.stderr));
    assert_eq!(
        text(&open.stdout),
        "1 entries, 0 judgments\nPENDING 8887db9b513d · captured locally · external: API v2 · api.limit\nTARGET UNVERIFIED: unsupported_capability: use core/v1 consumer\n\nnothing needs a person right now.\n\nnext: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check\n"
    );
    let check = env.kpop(&root, &["check"]);
    assert!(check.status.success(), "{}", text(&check.stderr));
    assert_eq!(
        text(&check.stdout),
        "NOTE PENDING 8887db9b513d · captured locally · external: API v2 · api.limit\nNOTE TARGET UNVERIFIED: unsupported_capability: use core/v1 consumer\n\n0 judgments, 1 entries, 0 problems, 2 declared\n"
    );
    let status = env.kpop(&root, &["knowledge", "status"]);
    assert!(status.status.success(), "{}", text(&status.stderr));
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["target_unavailable"], REFUSAL);
    assert_eq!(status["conflicts"], json!({}));

    let state = env.private.path().join("session-state");
    let session = env.kpop(
        &root,
        &[
            "session",
            "open",
            "--no-settings",
            "--input",
            "GROUNDING.yaml",
            "--project",
            "gate",
            "--state",
            state.to_str().unwrap(),
        ],
    );
    assert!(session.status.success(), "{}", text(&session.stderr));
    let opener = text(&session.stdout);
    assert!(opener.contains("contested=0"), "{opener}");
    assert!(!opener.contains("CONTESTED"), "{opener}");

    // The core/v1 consumer reads the target whatever its profile, so its snapshot still
    // holds both ids the pending contribution contests.
    let cache = tempfile::tempdir().unwrap();
    let runtime = kpop_native::reasoning_runtime::Runtime::open(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!(
                "{}.kpopper-runtime",
                kpop_native::reasoning_runtime::target_name().unwrap()
            )),
        cache.path(),
        Default::default(),
    )
    .unwrap();
    let captured = kpop_native::source_capture::capture_source_with_runtime(
        &[root.join("GROUNDING.yaml")],
        &root,
        kpop_native::source_capture::ReadMode::Live,
        None,
        Some(&runtime),
    )
    .unwrap();
    let snapshot = captured.snapshot().unwrap().to_data().to_json().unwrap();
    let conflicts = snapshot["context"]["conflicts"].as_object().unwrap();
    assert_eq!(
        conflicts.keys().collect::<Vec<_>>(),
        ["api.limit", "s.vendor"]
    );
    assert_eq!(snapshot["context"]["target"]["status"], "observed");
    assert_eq!(snapshot["context"]["target"]["reason"], Value::Null);
}

#[test]
fn ordinary_readers_leave_a_history_target_unverified() {
    let env = Env::new();
    let (_temp, root) = pending_workspace("target_history");
    let reason = "unsupported_history_consumer: history target needs captured consumer";

    let open = env.kpop(&root, &["open"]);
    assert!(open.status.success(), "{}", text(&open.stderr));
    assert_eq!(
        text(&open.stdout),
        format!(
            "1 entries, 0 judgments\nPENDING 8887db9b513d · captured locally · external: API v2 · api.limit\nTARGET UNVERIFIED: {reason}\n\nnothing needs a person right now.\n\nnext: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check\n"
        )
    );
    let status = env.kpop(&root, &["knowledge", "status"]);
    assert!(status.status.success(), "{}", text(&status.stderr));
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["target_unavailable"], reason);
    assert_eq!(status["conflicts"], json!({}));
}

#[test]
fn an_ordinary_reader_needs_no_runtime_to_leave_a_core_target_unverified() {
    let (_temp, root) = pending_workspace("target_core");
    let private = tempfile::tempdir().unwrap();
    // No resources are configured and none sit beside the test binary, so the computed
    // target cannot be replayed; its declaration alone keeps it from an ordinary reader.
    let kpop = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_kpop"))
            .current_dir(&root)
            .args(args)
            .env_remove("KPOPPER_NATIVE_RESOURCES")
            .env_remove("KPOPPER_READ_MODE")
            .env("KPOPPER_NATIVE_CACHE", private.path().join("cache"))
            .env("KPOPPER_PRIVATE_HOME", private.path().join("private"))
            .env("XDG_STATE_HOME", private.path().join("state"))
            .output()
            .unwrap()
    };
    let open = kpop(&["open"]);
    assert!(open.status.success(), "{}", text(&open.stderr));
    assert!(
        text(&open.stdout).contains(&format!("TARGET UNVERIFIED: {REFUSAL}\n")),
        "{}",
        text(&open.stdout)
    );
    let status = kpop(&["knowledge", "status"]);
    assert!(status.status.success(), "{}", text(&status.stderr));
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["target_unavailable"], REFUSAL);
    assert_eq!(status["conflicts"], json!({}));
}

#[test]
fn a_layer_whose_declaration_cannot_be_read_is_refused_in_the_contracts_words() {
    let env = Env::new();
    for (declaration, refusal) in [
        (
            "meta:\n  reasoning:\n    version: 2\n    profile: core/v1\n",
            "invalid_capability: invalid meta.reasoning capability declaration",
        ),
        (
            "meta:\n  reasoning:\n    version: 3\n    profile: core/v1\n    requires: [arithmetic/v1]\n",
            "unsupported_capability: unsupported reasoning profile or metadata version",
        ),
        (
            "meta:\n  reasoning:\n    version: 2\n    profile: core/v1\n    requires: [arithmetic/v1, bogus/v1]\n",
            "unsupported_capability: unsupported modules: bogus/v1",
        ),
        (
            "meta:\n  reasoning:\n    version: 2\n    profile: core/v1\n    requires: [composition/v1]\n",
            "invalid_capability: core/v1 requires arithmetic/v1",
        ),
    ] {
        let (_temp, root) = committed(&[
            ("GROUNDING.yaml", ORDINARY_RECORD),
            ("notes.txt", "one\ntwo\n"),
            (
                ".kpopper/hypotheses/trial.yaml",
                &format!("{declaration}known:\n  a.one:\n    v: 3\n"),
            ),
        ]);
        for (args, code) in [
            (vec!["open"], 1),
            (vec!["assess", "d.small"], 1),
            (vec!["export", "d.small"], 1),
            (vec!["search", "one"], 2),
            (vec!["knowledge", "status"], 1),
            (vec!["set", "a.one", "3"], 1),
        ] {
            let output = env.kpop(&root, &args);
            assert_eq!(output.status.code(), Some(code), "{refusal} {args:?}");
            assert_eq!(text(&output.stderr), format!("{refusal}\n"), "{args:?}");
        }
    }
}

#[test]
fn knowledge_status_refuses_a_core_record_as_an_ordinary_reader() {
    let env = Env::new();
    let (_temp, root) = committed(&core_page());
    for args in [
        vec!["knowledge", "status"],
        vec!["--frozen", "knowledge", "status"],
    ] {
        let output = env.kpop(&root, &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&output.stdout), "", "{args:?}");
        assert_eq!(text(&output.stderr), format!("{REFUSAL}\n"), "{args:?}");
    }
}

#[test]
fn consolidate_over_a_core_record_reads_what_sits_beside_it_first() {
    let env = Env::new();
    let (_temp, root) = committed(&core_page());
    let dry = env.kpop(&root, &["consolidate", "--dry-run"]);
    assert_eq!(dry.status.code(), Some(0), "{}", text(&dry.stderr));
    assert_eq!(
        text(&dry.stdout),
        "no hypotheses beside the record - nothing to consolidate\n"
    );
    for args in [
        vec!["consolidate", "--dry-run", "nope"],
        vec!["consolidate", "nope"],
    ] {
        let refused = env.kpop(&root, &args);
        assert_eq!(refused.status.code(), Some(1), "{args:?}");
        assert_eq!(
            text(&refused.stderr),
            "refused - no hypothesis named nope beside the record (there: none)\n",
            "{args:?}"
        );
    }

    // A hypothesis beside a core/v1 record is named when another is asked for, and laying it
    // over the record is left to the core/v1 consolidation: nothing is folded or deleted.
    let (_temp, root) = committed(
        &core_page()
            .into_iter()
            .chain([(
                ".kpopper/hypotheses/trial.yaml",
                "hypothesis:\n  claim: zero is one\nknown:\n  v.zero:\n    v: 1\n",
            )])
            .collect::<Vec<_>>(),
    );
    let missing = env.kpop(&root, &["consolidate", "--dry-run", "nope"]);
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        text(&missing.stderr),
        "refused - no hypothesis named nope beside the record (there: trial)\n"
    );
    for args in [
        vec!["consolidate", "--dry-run"],
        vec!["consolidate", "trial"],
    ] {
        let refused = env.kpop(&root, &args);
        assert_eq!(refused.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&refused.stderr), format!("{REFUSAL}\n"), "{args:?}");
        let changed = git(&root, &["status", "--porcelain"], None);
        assert!(changed.is_empty(), "{args:?}: {}", text(&changed));
    }
}

#[test]
fn every_ordinary_command_refuses_a_record_with_a_core_hypothesis_beside_it() {
    let env = Env::new();
    let (_temp, root) = committed(&[
        ("GROUNDING.yaml", ORDINARY_RECORD),
        ("notes.txt", "one\ntwo\n"),
        (".kpopper/hypotheses/trial.yaml", CORE_HYPOTHESIS),
    ]);
    let state = env.private.path().join("session-state");
    let commands: Vec<(Vec<&str>, i32)> = vec![
        (vec!["open"], 1),
        (vec!["check"], 1),
        (vec!["pull", "a.one"], 1),
        (vec!["pull", "--from", "HEAD", "a.one"], 1),
        (vec!["affects", "a.one"], 1),
        (vec!["search", "one"], 2),
        (vec!["assess", "d.small"], 1),
        (vec!["export", "d.small"], 1),
        (vec!["knowledge", "status"], 1),
        (vec!["consolidate", "--dry-run"], 1),
        (vec!["consolidate", "trial"], 1),
        (vec!["consolidate", "--refute", "trial", "not so"], 1),
        (vec!["remeasure"], 1),
        (vec!["set", "a.one", "3"], 1),
        (vec!["add", "a.three", "v=3"], 1),
        (vec!["review", "d.small"], 1),
        (vec!["same", "a.one", "a.two"], 1),
        (vec!["distinct", "a.one", "a.two", "they differ"], 1),
        (vec!["experimental", "hub", "--out", "page.html"], 1),
        (
            vec![
                "session",
                "open",
                "--no-settings",
                "--input",
                "GROUNDING.yaml",
                "--project",
                "gate",
                "--state",
                state.to_str().unwrap(),
            ],
            1,
        ),
    ];
    for (args, code) in &commands {
        let output = env.kpop(&root, args);
        assert_eq!(
            output.status.code(),
            Some(*code),
            "{args:?}: {}",
            text(&output.stderr)
        );
        assert_eq!(text(&output.stdout), "", "{args:?}");
        assert_eq!(text(&output.stderr), format!("{REFUSAL}\n"), "{args:?}");
        let changed = git(&root, &["status", "--porcelain"], None);
        assert!(changed.is_empty(), "{args:?}: {}", text(&changed));
    }

    let payload = json!({
        "session_id": "gate",
        "cwd": root,
        "hook_event_name": "UserPromptSubmit",
        "prompt": "what is a.one?",
    });
    let mut hook = env
        .command(&root)
        .args(["_hook", "ground", "claude", "prompt"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    hook.stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let hook = hook.wait_with_output().unwrap();
    assert!(hook.status.success());
    assert_eq!(text(&hook.stdout), "");
    assert_eq!(
        text(&hook.stderr),
        format!("kpopper grounding unavailable: {REFUSAL}\n")
    );

    // The refusal is the layer's: the same record without it is read.
    fs::remove_file(root.join(".kpopper/hypotheses/trial.yaml")).unwrap();
    let open = env.kpop(&root, &["open"]);
    assert!(open.status.success(), "{}", text(&open.stderr));
    assert!(text(&open.stdout).contains("4 entries, 1 judgments"));
}

#[test]
fn a_simple_project_refuses_a_core_hypothesis_beside_its_record() {
    let env = Env::new();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("GROUNDING.yaml"), ORDINARY_RECORD).unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(root.join(".kpopper/hypotheses/trial.yaml"), CORE_HYPOTHESIS).unwrap();
    for args in [
        vec!["open"],
        vec!["set", "a.one", "3"],
        vec!["same", "a.one", "a.two"],
    ] {
        let output = env.kpop(&root, &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&output.stderr), format!("{REFUSAL}\n"), "{args:?}");
    }
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        ORDINARY_RECORD
    );
}
