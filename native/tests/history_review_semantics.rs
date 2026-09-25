use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn run(root: &Path, actor: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kpop"));
    cmd.env(
        "KPOPPER_NATIVE_RESOURCES",
        root.parent().unwrap().join("test-resources"),
    )
    .env(
        "KPOPPER_NATIVE_CACHE",
        root.parent().unwrap().join("test-cache"),
    );
    cmd.current_dir(root)
        .env_remove("CODEX_THREAD_ID")
        .env_remove("KPOPPER_AGENT_SESSION")
        .args(args);
    if let Some(actor) = actor {
        cmd.env("KPOPPER_AGENT_SESSION", actor);
    }
    cmd.output().unwrap()
}
fn ok(root: &Path, actor: Option<&str>, args: &[&str]) -> String {
    let out = run(root, actor, args);
    assert!(
        out.status.success(),
        "{args:?}: {} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
fn seed(root: &Path, node: bool, actor: Option<&str>) -> PathBuf {
    let resources = root.join("test-resources/reasoning");
    fs::create_dir_all(&resources).unwrap();
    let archive = format!(
        "{}.kpopper-runtime",
        kpop_native::reasoning_runtime::target_name().unwrap()
    );
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(&archive),
        resources.join(&archive),
    )
    .unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    ok(
        &source,
        actor,
        &["add", "p.runs", "v=0", "--as-of", "2026-09-01"],
    );
    ok(
        &source,
        actor,
        &[
            "add",
            "d.done",
            "verdict=not demonstrated",
            "because=no brief arrived",
            "rests_on=[p.runs]",
            "wrong_if={expr: 'p.runs > 0'}",
        ],
    );
    if node {
        let copy = root.join("node");
        ok(
            &source,
            actor,
            &[
                "history",
                "migrate",
                "--node-history",
                "--to",
                copy.to_str().unwrap(),
            ],
        );
        copy
    } else {
        source
    }
}
fn reverse(root: &Path, actor: Option<&str>) {
    ok(
        root,
        actor,
        &[
            "set",
            "p.runs",
            "1",
            "--as-of",
            "2026-09-02",
            "--why",
            "fixture: one brief rendered",
        ],
    );
    ok(
        root,
        actor,
        &[
            "add",
            "d.done",
            "verdict=demonstrated",
            "because=one rendered brief",
            "rests_on=[p.runs]",
            "wrong_if={expr: 'p.runs < 1'}",
        ],
    );
}

#[test]
fn a_replacement_needs_another_recorded_session_review_on_both_formats() {
    for node in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = seed(temp.path(), node, Some("fixture-author"));
        let initial = ok(&root, Some("fixture-author"), &["open"]);
        assert!(!initial.contains("unreviewed"), "{initial}");
        reverse(&root, Some("fixture-author"));
        let replaced = ok(&root, Some("fixture-author"), &["open"]);
        assert!(replaced.contains("unreviewed"), "{node}: {replaced}");
        ok(&root, Some("fixture-author"), &["review", "d.done"]);
        let own = ok(&root, Some("fixture-author"), &["open"]);
        assert!(own.contains("unreviewed"), "self-review cleared it: {own}");
        ok(&root, Some("fixture-reviewer"), &["review", "d.done"]);
        let other = ok(&root, Some("fixture-reviewer"), &["open"]);
        assert!(
            !other.contains("unreviewed"),
            "other exact review did not clear it: {other}"
        );
    }
}

#[test]
fn an_unreviewed_replacement_reserves_its_dependents_without_changing_acceptance() {
    for node in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = seed(temp.path(), node, Some("fixture-author"));
        reverse(&root, Some("fixture-author"));
        ok(
            &root,
            Some("fixture-author"),
            &[
                "add",
                "d.ship",
                "verdict=ship",
                "because=delivery demonstrated",
                "rests_on=[d.done, p.runs]",
                "wrong_if={expr: 'p.runs < 1'}",
            ],
        );
        let pull: serde_json::Value =
            serde_json::from_str(&ok(&root, None, &["pull", "d.ship"])).unwrap();
        assert_eq!(pull["nodes"]["d.ship"]["acceptance"]["status"], "accepted");
        let reservations = pull["nodes"]["d.ship"]["support"]["reservations"]
            .as_array()
            .unwrap();
        assert!(
            reservations
                .iter()
                .any(|r| r["subject"] == "d.done" && r["state"] == "unreviewed"),
            "reservations={reservations:?}"
        );
    }
}

#[test]
fn a_legacy_unknown_actor_reversal_names_the_provenance_gap() {
    for node in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = seed(temp.path(), node, None);
        reverse(&root, None);
        let output = ok(&root, None, &["open"]);
        assert!(
            output.contains("review_provenance_missing"),
            "{node}: {output}"
        );
        let pull: serde_json::Value =
            serde_json::from_str(&ok(&root, None, &["pull", "d.done"])).unwrap();
        assert_eq!(pull["nodes"]["d.done"]["acceptance"]["status"], "accepted");
        assert!(
            !pull["nodes"]["d.done"]["support"]["reservations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["state"] == "unreviewed")
        );
    }
}

#[test]
fn a_review_expires_when_its_observed_inputs_change_and_can_be_renewed() {
    for node in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = seed(temp.path(), node, Some("fixture-author"));
        reverse(&root, Some("fixture-author"));
        ok(&root, Some("fixture-reviewer"), &["review", "d.done"]);
        ok(
            &root,
            Some("fixture-author"),
            &[
                "set",
                "p.runs",
                "2",
                "--as-of",
                "2026-09-03",
                "--why",
                "another rendered brief",
            ],
        );
        let stale = ok(&root, None, &["open"]);
        assert!(
            stale.contains("unreviewed"),
            "old review certified changed input: {stale}"
        );
        ok(&root, Some("fixture-reviewer"), &["review", "d.done"]);
        let renewed = ok(&root, None, &["open"]);
        assert!(
            !renewed.contains("unreviewed"),
            "new exact review did not acknowledge current inputs: {renewed}"
        );
    }
}

#[test]
fn a_terminal_reviewer_can_name_the_recorded_actor_without_a_session() {
    for node in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = seed(temp.path(), node, Some("fixture-author"));
        reverse(&root, Some("fixture-author"));
        ok(
            &root,
            None,
            &["review", "d.done", "--by", "session:fixture-author"],
        );
        assert!(ok(&root, None, &["open"]).contains("unreviewed"));
        ok(&root, None, &["review", "d.done", "--by", "human-reviewer"]);
        assert!(!ok(&root, None, &["open"]).contains("unreviewed"));
    }
}

#[test]
fn legacy_review_acknowledges_the_gap_without_backfilling_an_actor() {
    for node in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = seed(temp.path(), node, None);
        reverse(&root, None);
        ok(&root, None, &["review", "d.done"]);
        assert!(!ok(&root, None, &["open"]).contains("review_provenance_missing"));
        let pull: serde_json::Value =
            serde_json::from_str(&ok(&root, None, &["pull", "d.done"])).unwrap();
        let reviews = pull["history_subjects"]["d.done"]["reviews"]
            .as_array()
            .unwrap();
        assert!(reviews.iter().all(|r| r["by"].is_null()));
    }
}

fn persistent(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, at: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        if !at.exists() { return }
        for item in fs::read_dir(at).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() { visit(root, &path, out); }
            else { out.insert(path.strip_prefix(root).unwrap().to_owned(), fs::read(path).unwrap()); }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    out.insert(PathBuf::from("GROUNDING.yaml"), fs::read(root.join("GROUNDING.yaml")).unwrap());
    visit(root, &root.join(".kpopper/history"), &mut out);
    visit(root, &root.join(".kpopper/history-commits"), &mut out);
    out
}

#[test]
fn review_findings_are_transient_and_readers_do_not_grow_history() {
    for node in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = seed(temp.path(), node, Some("fixture-author"));
        reverse(&root, Some("fixture-author"));
        let before = persistent(&root);
        for bytes in before.values() {
            let text = String::from_utf8_lossy(bytes);
            assert!(!text.contains("\"resolutions\"") && !text.contains("\nresolutions:"), "transient resolution evidence entered permanent storage");
        }
        for args in [vec!["open"], vec!["pull", "d.done"], vec!["check"]] {
            ok(&root, Some("fixture-author"), &args);
        }
        assert_eq!(persistent(&root), before, "reading modified stored history");
    }
}
