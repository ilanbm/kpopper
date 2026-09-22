use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("TZ", "UTC")
        .args(args)
        .output()
        .unwrap()
}
fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/legacy-review")
            .join(name),
    )
    .unwrap()
}
fn success(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[test]
fn legacy_supersede_replaces_in_place_and_keeps_the_old_body() {
    let temp = tempfile::tempdir().unwrap();
    let before = fixture("supersede-before.yaml");
    fs::write(temp.path().join("GROUNDING.yaml"), &before).unwrap();
    let output = run(
        temp.path(),
        &[
            "add",
            "d.keep",
            "verdict=stop",
            "rests_on=[p.alpha, p.beta]",
            "wrong_if=p.alpha > 9",
            "because=new evidence",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    );
    let stdout = String::from_utf8(success(output)).unwrap();
    assert!(
        stdout.contains("supersede d.keep: continue -> stop"),
        "{stdout}"
    );
    let record = fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap();
    assert_eq!(record.matches("  d.keep:").count(), 1, "{record}");
    assert!(record.contains("verdict: stop"), "{record}");
    assert!(record.contains("replaced:"), "{record}");
    let retained = fs::read_to_string(temp.path().join(".kpopper/replaced.yaml")).unwrap();
    assert!(retained.contains("d.keep:"), "{retained}");
    assert!(retained.contains("verdict: continue"), "{retained}");
    assert!(retained.contains("ended:"), "{retained}");
    assert!(retained.contains("day: '2026-09-19'"), "{retained}");
}

#[test]
fn review_preserves_unchanged_snapshots_and_acknowledges_reversal_trails() {
    for case in ["replaced", "reviewnochange"] {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("GROUNDING.yaml"),
            fixture(&format!("{case}-before.yaml")),
        )
        .unwrap();
        let output = success(run(
            temp.path(),
            &[
                "review",
                "d.keep",
                "--as-of",
                "2026-09-19",
                "GROUNDING.yaml",
            ],
        ));
        assert_eq!(output, fixture(&format!("{case}.stdout")), "{case}");
        assert_eq!(
            fs::read(temp.path().join("GROUNDING.yaml")).unwrap(),
            fixture(&format!("{case}-after.yaml")),
            "{case}"
        );
    }
}

#[test]
fn changing_set_without_as_of_uses_the_current_day() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("GROUNDING.yaml"),
        "known:\n  p.a:\n    v: 1\n    of: 2026-01-01\n",
    )
    .unwrap();
    let day_before = chrono::Utc::now().date_naive().to_string();
    let output = String::from_utf8(success(run(
        temp.path(),
        &["set", "p.a", "2", "GROUNDING.yaml"],
    )))
    .unwrap();
    let day_after = chrono::Utc::now().date_naive().to_string();
    assert!(
        output.contains(&format!("(as of {day_before})"))
            || output.contains(&format!("(as of {day_after})")),
        "{output}"
    );
    let written = fs::read_to_string(temp.path().join("GROUNDING.yaml")).unwrap();
    assert!(written.contains("v: 2"));
    assert!(written.contains(&day_before) || written.contains(&day_after));
}

#[test]
fn advanced_recovery_without_a_legacy_journal_reaches_the_normal_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        temp.path().join("GROUNDING.yaml"),
        "known:\n  p.a: {v: 1}\n",
    )
    .unwrap();
    let output = run(temp.path(), &["recover"]);
    assert!(!output.status.success());
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(message.contains("no_recovery_pending"), "{message}");
}

#[test]
fn pointer_add_uses_record_file_order_before_lexical_entry_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("shards")).unwrap();
    let index = "record: [shards/alpha.yaml, shards/beta.yaml]\n";
    let beta = "known:\n  a.one: {v: 1}\n";
    fs::write(root.join("GROUNDING.yaml"), index).unwrap();
    fs::write(root.join("shards/alpha.yaml"), "known:\n  z.one: {v: 1}\n").unwrap();
    fs::write(root.join("shards/beta.yaml"), beta).unwrap();
    let output = String::from_utf8(success(run(
        root,
        &[
            "add",
            "m.new",
            "v=2",
            "--as-of",
            "2026-09-19",
            "GROUNDING.yaml",
        ],
    )))
    .unwrap();
    assert!(output.contains("before z.one"), "{output}");
    assert!(
        fs::read_to_string(root.join("shards/alpha.yaml"))
            .unwrap()
            .contains("m.new:")
    );
    assert_eq!(
        fs::read_to_string(root.join("shards/beta.yaml")).unwrap(),
        beta
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        index
    );
}
