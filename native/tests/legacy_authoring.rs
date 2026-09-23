use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/legacy-authoring"
);

fn fixture(name: &str) -> Vec<u8> {
    fs::read(Path::new(FIXTURES).join(name)).unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn assert_no_history(root: &Path) {
    assert!(!root.join(".kpopper/history.yaml").exists());
    assert!(!root.join(".kpopper/history-commits").exists());
    assert!(!root.join(".kpopper/history").exists());
}

#[test]
fn add_set_and_review_match_python_18_source_images_and_output() {
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &[
                "add",
                "p.gamma",
                "v=3",
                "note=new value",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
            "add-after.yaml",
            "add.stdout",
        ),
        (
            &[
                "set",
                "p.alpha",
                "new text",
                "--why",
                "correction",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
            "set-after.yaml",
            "set.stdout",
        ),
        (
            &[
                "review",
                "d.keep",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
            "review-after.yaml",
            "review.stdout",
        ),
    ];
    for (args, expected_image, expected_output) in cases {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        write(&root.join("GROUNDING.yaml"), &fixture("simple-before.yaml"));
        let output = success(run(&root, args));
        assert_eq!(output.as_bytes(), fixture(expected_output), "{args:?}");
        assert_eq!(
            fs::read(root.join("GROUNDING.yaml")).unwrap(),
            fixture(expected_image),
            "{args:?}"
        );
        assert_no_history(&root);
    }
}

#[test]
fn an_added_scope_text_is_written_as_given() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root.join("GROUNDING.yaml"), &fixture("simple-before.yaml"));
    let output = success(run(
        &root,
        &[
            "add",
            "p.gamma",
            "v=3",
            "scope=local experiment",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    ));
    assert_eq!(output.as_bytes(), fixture("add.stdout"));
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        String::from_utf8(fixture("add-after.yaml"))
            .unwrap()
            .replace(
                "    note: \"new value\"\n",
                "    scope: \"local experiment\"\n"
            )
    );
    assert_no_history(&root);
}

#[test]
fn set_replaces_source_citation_in_the_same_guarded_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    write(&root.join("GROUNDING.yaml"), &fixture("source-before.yaml"));
    let output = success(run(
        &root,
        &[
            "set",
            "p.input",
            "2",
            "--source",
            "s.new",
            "--at",
            "page-2",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    ));
    assert_eq!(output.as_bytes(), fixture("source.stdout"));
    assert_eq!(
        fs::read(root.join("GROUNDING.yaml")).unwrap(),
        fixture("source-after.yaml")
    );
    assert_no_history(&root);
}

#[test]
fn same_day_contradiction_is_refused_without_a_byte_change() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let before = fixture("simple-before.yaml");
    write(&root.join("GROUNDING.yaml"), &before);
    let output = run(
        &root,
        &[
            "set",
            "p.beta",
            "9",
            "--as-of",
            "2026-09-02",
            "GROUNDING.yaml",
        ],
    );
    assert!(!output.status.success());
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, fixture("refused.stderr"));
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert_no_history(&root);
}

#[test]
fn identical_set_without_new_evidence_is_a_byte_exact_noop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let before = fixture("simple-before.yaml");
    write(&root.join("GROUNDING.yaml"), &before);
    let output = success(run(&root, &["set", "p.beta", "2", "GROUNDING.yaml"]));
    assert_eq!(output, "p.beta is already 2; nothing written\n");
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert_no_history(&root);
}

#[test]
fn pointer_add_changes_only_the_owning_shard() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let index = fixture("pointer-before.yaml");
    let beta = fixture("pointer-beta-before.yaml");
    write(&root.join("GROUNDING.yaml"), &index);
    write(
        &root.join("shards/alpha.yaml"),
        &fixture("pointer-alpha-before.yaml"),
    );
    write(&root.join("shards/beta.yaml"), &beta);
    let output = success(run(
        &root,
        &[
            "add",
            "p.gamma",
            "v=3",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    ));
    assert_eq!(output.as_bytes(), fixture("pointer.stdout"));
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), index);
    assert_eq!(fs::read(root.join("shards/beta.yaml")).unwrap(), beta);
    assert_eq!(
        fs::read(root.join("shards/alpha.yaml")).unwrap(),
        fixture("pointer-alpha-after.yaml")
    );
    assert_no_history(&root);
}

#[test]
fn private_dependency_closure_goes_to_a_draft_and_keeps_the_record() {
    let temp = tempfile::tempdir().unwrap();
    let private = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let before = fixture("private-before.yaml");
    write(&root.join("GROUNDING.yaml"), &before);
    let output = Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(&root)
        .env("KPOPPER_PRIVATE_HOME", private.path())
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .args([
            "add",
            "d.private",
            "verdict=stop",
            "rests_on=[p.public]",
            "wrong_if=p.public > 3",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ])
        .output()
        .unwrap();
    let result: Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(result["state"], "private draft");
    assert_eq!(result["reason"], "private or unclear source permission");
    assert!(Path::new(result["path"].as_str().unwrap()).is_file());
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
    assert_no_history(&root);
}

#[test]
fn nontext_metadata_keys_remain_exact_and_never_leak_projection_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let before = b"meta:\n  updated: 2026-09-01\n  opaque:\n    7: null\n    false: 2026-09-02\nknown:\n  p.input: {v: 1, of: 2026-09-01}\n";
    write(&root.join("GROUNDING.yaml"), before);
    success(run(
        &root,
        &[
            "set",
            "p.input",
            "2",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    ));
    let after = fs::read(root.join("GROUNDING.yaml")).unwrap();
    assert!(
        after
            .windows(b"7: null".len())
            .any(|value| value == b"7: null")
    );
    assert!(
        after
            .windows(b"false: 2026-09-02".len())
            .any(|value| value == b"false: 2026-09-02")
    );
    assert!(!after.contains(&0));
    assert_no_history(&root);
}

#[test]
fn named_hypotheses_keep_the_base_and_bad_or_pending_authority_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let before = fixture("simple-before.yaml");
    write(&root.join("GROUNDING.yaml"), &before);
    let named = run(
        &root,
        &[
            "set",
            "p.alpha",
            "new",
            "--hypothesis",
            "proposal",
            "GROUNDING.yaml",
        ],
    );
    assert!(
        named.status.success(),
        "{}",
        String::from_utf8_lossy(&named.stderr)
    );
    assert!(root.join(".kpopper/hypotheses/proposal.yaml").is_file());
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);

    write(
        &root.join(".kpopper/history.yaml"),
        b"authority: unknown\ngeneration: 0\nprofile: history/v1\nrecord_id: record\nversion: 1\n",
    );
    let unknown = run(&root, &["set", "p.alpha", "new", "GROUNDING.yaml"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unsupported_authority"));
    fs::remove_file(root.join(".kpopper/history.yaml")).unwrap();

    write(&root.join(".kpopper/.history-local/pending.json"), b"{}\n");
    let pending = run(&root, &["set", "p.alpha", "new", "GROUNDING.yaml"]);
    assert!(!pending.status.success());
    assert!(String::from_utf8_lossy(&pending.stderr).contains("invalid_pending_journal"));
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn cli_recovers_a_retained_cross_directory_publication() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let index = fixture("pointer-before.yaml");
    let alpha = fixture("pointer-alpha-before.yaml");
    write(&root.join("GROUNDING.yaml"), &index);
    write(&root.join("shards/alpha.yaml"), &alpha);
    write(
        &root.join("shards/beta.yaml"),
        &fixture("pointer-beta-before.yaml"),
    );
    let shard_dir = root.join("shards");
    let original_mode = fs::metadata(&shard_dir).unwrap().permissions().mode();
    fs::set_permissions(&shard_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let failed = run(
        &root,
        &[
            "add",
            "p.gamma",
            "v=3",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    );
    fs::set_permissions(&shard_dir, fs::Permissions::from_mode(original_mode)).unwrap();
    assert!(!failed.status.success());
    assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), index);
    assert_eq!(fs::read(root.join("shards/alpha.yaml")).unwrap(), alpha);
    assert!(
        root.join(".kpopper/.history-local")
            .read_dir()
            .unwrap()
            .any(|item| {
                item.unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
    );

    let recovered = success(run(&root, &["recover", "--record", "GROUNDING.yaml"]));
    assert!(recovered.starts_with("recovered: direct-"));
    assert_eq!(
        fs::read(root.join("shards/alpha.yaml")).unwrap(),
        fixture("pointer-alpha-after.yaml")
    );
    assert!(
        !root
            .join(".kpopper/.history-local")
            .read_dir()
            .unwrap()
            .any(|item| {
                item.unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
    );
    assert_no_history(&root);
}

/// Without bundled resources a condition stays text, as the Python writer keeps it when
/// no expression program is configured. State lives beside the workspace, not in it.
fn run_unbundled(root: &Path, args: &[&str]) -> Output {
    let home = root.parent().unwrap();
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env("KPOPPER_PRIVATE_HOME", home.join("private"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("KPOPPER_NATIVE_CACHE", home.join("cache"))
        .args(args)
        .output()
        .unwrap()
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

const ADD_JUDGMENT: &[&str] = &[
    "add",
    "d.z",
    "verdict=x",
    "rests_on=[api.limit]",
    "wrong_if=api.limit > 50",
    "--as-of",
    "2026-01-01",
];

/// Commands on the fixtures under `indent/`, each with the image `scripts/cli.py` leaves
/// for it: the record, or with a hypothesis the file the write goes to. Fields go at the
/// column the neighbouring entry's fields use, and at the entry's column plus two only
/// beside an entry written on one line.
struct IndentCase {
    record: &'static str,
    hypothesis: Option<&'static str>,
    steps: &'static [(&'static [&'static str], &'static str)],
}

const INDENT_CASES: &[IndentCase] = &[
    IndentCase {
        record: "four-space.yaml",
        hypothesis: None,
        steps: &[(ADD_JUDGMENT, "four-space-add.yaml")],
    },
    IndentCase {
        record: "four-space.yaml",
        hypothesis: Some("four-space-hypothesis.yaml"),
        steps: &[(
            &[
                "add",
                "d.z",
                "verdict=x",
                "rests_on=[api.limit]",
                "wrong_if=api.limit > 50",
                "--as-of",
                "2026-01-01",
                "--hypothesis",
                "prop",
            ],
            "four-space-hypothesis-add.yaml",
        )],
    },
    IndentCase {
        record: "three-space.yaml",
        hypothesis: None,
        steps: &[(
            &[
                "add",
                "s.zzz",
                "asked=what now",
                "read=2026-01-02",
                "--as-of",
                "2026-01-01",
            ],
            "three-space-add.yaml",
        )],
    },
    IndentCase {
        record: "unreadable-ids.yaml",
        hypothesis: None,
        steps: &[(
            &["add", "api.window", "v=5", "--as-of", "2026-01-01"],
            "unreadable-ids-add.yaml",
        )],
    },
    IndentCase {
        record: "four-space-flow.yaml",
        hypothesis: None,
        steps: &[(ADD_JUDGMENT, "four-space-flow-add.yaml")],
    },
    IndentCase {
        record: "four-space-broken.yaml",
        hypothesis: None,
        steps: &[
            (
                &[
                    "add",
                    "d.w",
                    "verdict=y",
                    "rests_on=[api.limit]",
                    "wrong_if=api.limit > 50",
                    "--as-of",
                    "2026-01-01",
                ],
                "four-space-superseded.yaml",
            ),
            (
                &["review", "d.w", "--as-of", "2026-01-01"],
                "four-space-reviewed.yaml",
            ),
        ],
    },
    // A judgment the Python writer replaced: its fields sit at eight.
    IndentCase {
        record: "four-space-superseded.yaml",
        hypothesis: None,
        steps: &[(
            &["review", "d.w", "--as-of", "2026-01-01"],
            "four-space-reviewed.yaml",
        )],
    },
];

/// Runs the case's commands in a fresh workspace and compares the written file with each
/// image. A hypothesis is named `prop`, and the base must stay as it was.
fn check_indent_case(case: &IndentCase, run: impl Fn(&Path, &[&str]) -> Output) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("repo");
    let base = fixture(&format!("indent/{}", case.record));
    write(&root.join("GROUNDING.yaml"), &base);
    let target = match case.hypothesis {
        Some(hypothesis) => {
            let path = root.join(".kpopper/hypotheses/prop.yaml");
            write(&path, &fixture(&format!("indent/{hypothesis}")));
            path
        }
        None => root.join("GROUNDING.yaml"),
    };
    for (args, expected) in case.steps {
        success(run(&root, args));
        assert_eq!(
            text(fs::read(&target).unwrap()),
            text(fixture(&format!("indent/{expected}"))),
            "{}: {args:?}",
            case.record
        );
    }
    if case.hypothesis.is_some() {
        assert_eq!(fs::read(root.join("GROUNDING.yaml")).unwrap(), base);
    }
    assert_no_history(&root);
}

#[test]
fn written_entries_take_the_field_indent_of_their_neighbours_like_python() {
    for case in INDENT_CASES {
        check_indent_case(case, run_unbundled);
    }
}

#[test]
#[ignore = "requires KPOP_SESSION_ORACLE_PYTHON and KPOP_SESSION_ORACLE_ROOT pointing at an immutable Python oracle"]
fn the_field_indent_images_are_what_the_python_writer_leaves() {
    let python =
        std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("set KPOP_SESSION_ORACLE_PYTHON");
    let cli = Path::new(
        &std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("set KPOP_SESSION_ORACLE_ROOT"),
    )
    .join("scripts/cli.py");
    for case in INDENT_CASES {
        check_indent_case(case, |root, args| {
            // No checked-session core, as for the unbundled native binary.
            let home = root.parent().unwrap();
            Command::new(&python)
                .arg(&cli)
                .args(args)
                .current_dir(root)
                .env_remove("KPOPPER_AGENT_SESSION")
                .env_remove("CODEX_THREAD_ID")
                .env("XDG_CACHE_HOME", home.join("cache"))
                .env("XDG_STATE_HOME", home.join("state"))
                .env("KPOPPER_PRIVATE_HOME", home.join("private"))
                .output()
                .unwrap()
        });
    }
}
