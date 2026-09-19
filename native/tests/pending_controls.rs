mod support;

use fs2::FileExt;
use kpop_native::{
    pending_control::{self, Configure},
    project_modes::Project,
    public_pending::{self, Command, ConfigureOptions, ControlOptions, Options},
};
use serde_json::Value;
use std::{collections::BTreeMap, fs::OpenOptions, path::Path, process::Command as Process};
use support::pending::{fixture, git};

fn fixtures() -> (Value, Value) {
    (
        serde_json::from_str(include_str!("fixtures/pending-controls-oracle.json")).unwrap(),
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap(),
    )
}

fn named<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap()
}

fn expected_output(entry: &Value, replacements: &[(&str, &Path)]) -> Value {
    let mut text = entry["actual"]["stdout"].as_str().unwrap().to_owned();
    for (token, path) in replacements {
        text = text.replace(token, path.to_str().unwrap());
    }
    serde_json::from_str(&text).unwrap()
}

fn state_files(project: &Project) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    if !project.state.exists() {
        return files;
    }
    for entry in std::fs::read_dir(&project.state).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            files.insert(
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(path).unwrap(),
            );
        }
    }
    files
}

fn expected_files(entry: &Value, remote: Option<&Path>) -> BTreeMap<String, String> {
    entry["state_files"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, value)| {
            let mut text = value.as_str().unwrap().to_owned();
            if let Some(remote) = remote {
                text = text.replace("$REMOTE", remote.to_str().unwrap());
            }
            (name.clone(), text)
        })
        .collect()
}

#[test]
fn configure_cli_matches_python_output_and_exact_policy_files() {
    let (oracle, _) = fixtures();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "trunk"], None);
    git(
        temp.path(),
        &["init", "--bare", remote.to_str().unwrap()],
        None,
    );
    git(
        &root,
        &["remote", "add", "team", remote.to_str().unwrap()],
        None,
    );
    let binary = env!("CARGO_BIN_EXE_kpop-native");
    for name in ["configure_initial", "configure_grant", "configure_revoke"] {
        let entry = named(&oracle, name);
        let mut command = Process::new(binary);
        command.args(["--workspace", root.to_str().unwrap(), "pending"]);
        command.args(
            entry["argv"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap()),
        );
        let result = command.output().unwrap();
        assert_eq!(
            result.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&result.stdout).unwrap(),
            expected_output(entry, &[("$REMOTE", &remote)]),
            "{name}"
        );
        assert_eq!(
            state_files(&Project::open(&root).unwrap()),
            expected_files(entry, Some(&remote))
        );
    }
}

#[test]
fn pause_and_resume_match_python_outputs_and_exact_state_files() {
    let (oracle, ledgers) = fixtures();
    for setup in ["one", "retired"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        fixture(&root, named(&ledgers, setup));
        let project = Project::open(&root).unwrap();
        let pause = named(&oracle, &format!("{setup}_pause"));
        let value = pending_control::decision_at(&project, "pause", &[], "", 123.5).unwrap();
        assert_eq!(value.to_json().unwrap(), expected_output(pause, &[]));
        assert_eq!(state_files(&project), expected_files(pause, None));

        let resume_name = if setup == "one" {
            "one_resume_empty"
        } else {
            "retired_resume_revision"
        };
        let resume = named(&oracle, resume_name);
        let revisions = resume["argv"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|value| value.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let value =
            pending_control::decision_at(&project, "resume", &revisions, "", 123.5).unwrap();
        assert_eq!(value.to_json().unwrap(), expected_output(resume, &[]));
        assert_eq!(state_files(&project), expected_files(resume, None));
    }
}

#[test]
fn pause_and_resume_are_real_cli_commands_and_never_touch_a_remote() {
    let (_, ledgers) = fixtures();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, named(&ledgers, "one"));
    let before = git(&root, &["show-ref"], None);
    let binary = env!("CARGO_BIN_EXE_kpop-native");
    for action in ["pause", "resume"] {
        let result = Process::new(binary)
            .args(["--workspace", root.to_str().unwrap(), "pending", action])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(value["paused"], action == "pause");
    }
    assert_eq!(git(&root, &["show-ref"], None), before);
}

#[test]
fn configure_refuses_scope_change_with_a_pending_ledger_and_preserves_policy() {
    let (oracle, ledgers) = fixtures();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, named(&ledgers, "one"));
    git(
        temp.path(),
        &["init", "--bare", remote.to_str().unwrap()],
        None,
    );
    git(
        &root,
        &["remote", "add", "team", remote.to_str().unwrap()],
        None,
    );
    let project = Project::open(&root).unwrap();
    pending_control::configure(
        &project,
        &Configure {
            remote: Some("team"),
            target: Some("trunk"),
            branch: None,
            grant: false,
            revoke: false,
        },
    )
    .unwrap();
    let before = std::fs::read(&project.config_path).unwrap();
    assert_eq!(
        pending_control::configure(
            &project,
            &Configure {
                remote: None,
                target: Some("other"),
                branch: None,
                grant: false,
                revoke: false,
            }
        )
        .unwrap_err()
        .0,
        "pending store belongs to the existing publication scope"
    );
    assert_eq!(std::fs::read(&project.config_path).unwrap(), before);
    let expected = named(&oracle, "configure_pending_scope_change");
    assert_eq!(
        state_files(&project),
        expected_files(expected, Some(&remote))
    );
    assert_eq!(
        public_pending::dispatch(
            &Options {
                command: Command::Configure(ConfigureOptions {
                    target: Some("other".into()),
                    ..Default::default()
                }),
            },
            &root,
            false,
        )
        .stderr,
        expected["actual"]["stderr"]
    );
}

#[test]
fn simple_pause_cli_refusal_matches_python_and_creates_no_state() {
    let (oracle, _) = fixtures();
    let temp = tempfile::tempdir().unwrap();
    let result = Process::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args([
            "--workspace",
            temp.path().to_str().unwrap(),
            "pending",
            "pause",
        ])
        .output()
        .unwrap();
    let expected = named(&oracle, "simple_pause");
    assert_eq!(
        result.status.code(),
        expected["actual"]["code"]
            .as_i64()
            .map(|value| value as i32)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        expected["actual"]["stdout"]
    );
    assert_eq!(
        String::from_utf8(result.stderr).unwrap(),
        expected["actual"]["stderr"]
    );
    assert!(!temp.path().join(".kpopper").exists());
}

#[test]
fn controls_refuse_unknown_revisions_and_a_busy_publisher_without_state_changes() {
    let (_, ledgers) = fixtures();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, named(&ledgers, "one"));
    let project = Project::open(&root).unwrap();
    assert_eq!(
        pending_control::decision(&project, "resume", &["0".repeat(64)], "")
            .unwrap_err()
            .0,
        "decision must name a captured immutable revision"
    );
    let path = project.state.join("publisher.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .unwrap();
    lock.lock_exclusive().unwrap();
    assert_eq!(
        pending_control::decision(&project, "pause", &[], "")
            .unwrap_err()
            .0,
        "another local publisher is active"
    );
    assert!(!project.state.join("publication.json").exists());
}

#[test]
fn command_enum_routes_configure_pause_and_resume() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    let remote = temp.path().join("remote.git");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init", "-b", "trunk"], None);
    git(
        temp.path(),
        &["init", "--bare", remote.to_str().unwrap()],
        None,
    );
    git(
        &root,
        &["remote", "add", "team", remote.to_str().unwrap()],
        None,
    );
    public_pending::run(
        &Options {
            command: Command::Configure(ConfigureOptions {
                remote: Some("team".into()),
                target: Some("trunk".into()),
                ..Default::default()
            }),
        },
        &root,
    )
    .unwrap();
    public_pending::run(
        &Options {
            command: Command::Pause(ControlOptions::default()),
        },
        &root,
    )
    .unwrap();
    let value = public_pending::run(
        &Options {
            command: Command::Resume(ControlOptions::default()),
        },
        &root,
    )
    .unwrap()
    .to_json()
    .unwrap();
    assert_eq!(value["paused"], false);
}

#[test]
fn terminal_decisions_and_retry_match_python_outputs_and_state_files() {
    let (oracle, ledgers) = fixtures();
    for name in ["withdraw", "reject", "supersede"] {
        let entry = named(&oracle, name);
        let setup = entry["setup"].as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        fixture(&root, named(&ledgers, setup));
        let project = Project::open(&root).unwrap();
        let argv = entry["argv"].as_array().unwrap();
        let action = argv[0].as_str().unwrap();
        let revision = argv[1].as_str().unwrap().to_owned();
        let reason = argv
            .iter()
            .position(|value| value == "--reason")
            .map(|index| argv[index + 1].as_str().unwrap())
            .unwrap_or("");
        let replacement = argv
            .iter()
            .position(|value| value == "--replacement")
            .map(|index| argv[index + 1].as_str().unwrap());
        let value =
            pending_control::action_at(&project, action, &[revision], reason, replacement, 123.5)
                .unwrap();
        assert_eq!(
            value.to_json().unwrap(),
            expected_output(entry, &[]),
            "{name}"
        );
        assert_eq!(state_files(&project), expected_files(entry, None), "{name}");
    }

    let entry = named(&oracle, "retry");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, named(&ledgers, "one"));
    let project = Project::open(&root).unwrap();
    std::fs::create_dir_all(&project.state).unwrap();
    std::fs::write(
        project.state.join("publication.json"),
        r#"{"version":1,"scope":null,"decisions":{},"receipts":[],"expected_head":null,"pr":null,"cycle":0,"proposed":[],"intent":null,"failures":4,"retry_at":9999,"paused":false,"states":{}}"#,
    ).unwrap();
    let value = pending_control::action_at(&project, "retry", &[], "", None, 123.5)
        .unwrap()
        .to_json()
        .unwrap();
    assert_eq!(value, expected_output(entry, &[]));
    assert_eq!(state_files(&project), expected_files(entry, None));
}

#[test]
fn terminal_decision_requires_exact_revision_reason_and_replacement() {
    let (oracle, ledgers) = fixtures();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    fixture(&root, named(&ledgers, "one"));
    let revision = "8887db9b513dddd4f3d7d26155713251effcfaa5e97a8edbcb1d7ced6d7297dd";
    let output = public_pending::dispatch(
        &Options {
            command: Command::Withdraw(ControlOptions {
                revisions: vec![revision.into()],
                reason: String::new(),
            }),
        },
        &root,
        false,
    );
    let expected = named(&oracle, "withdraw_missing_reason");
    assert_eq!(output.code, 2);
    assert_eq!(output.stdout, expected["actual"]["stdout"]);
    assert_eq!(output.stderr, expected["actual"]["stderr"]);
    assert_eq!(
        pending_control::action(
            &Project::open(&root).unwrap(),
            "supersede",
            &[revision.into()],
            "reason",
            Some(revision)
        )
        .unwrap_err()
        .0,
        "supersession must name a different captured revision"
    );
}
