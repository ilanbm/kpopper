mod support;

use kpop_native::{
    pending_control,
    project_modes::Project,
    public_consolidation::{self, CommandOutput, Options},
};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use support::pending::{fixture, git};

const SAYS_TEN: &str = "8887db9b513dddd4f3d7d26155713251effcfaa5e97a8edbcb1d7ced6d7297dd";
const SAYS_ELEVEN: &str = "b5bea064401e23c2f7f3b95022324ff396479ea22a7663c3da58f7871d4c36cc";
const RECORD: &str = "sources:\n  s.vendor: {name: Vendor, file: evidence/vendor.txt, read: '2026-09-14'}\nknown:\n  api.limit: {name: Limit, v: 10, from: s.vendor, at: table 1}\njudgments:\n  d.x:\n    verdict: the client stays under the vendor limit\n    rests_on: [api.limit]\n    wrong_if: \"api.limit > 10\"\n";
const NO_HYPOTHESES: &str = "no hypotheses beside the record - nothing to consolidate\n";
const RED: &str = "  a person decides each: one that is wrong is rejected with its reason (pending reject <revision> --reason \"<why>\"); one that stands is read again and set in the base, or accepted in the knowledge PR\n";

/// A ledger case of the pending-state fixtures, with its record replaced.
fn ledger(name: &str, record: &str) -> (tempfile::TempDir, PathBuf) {
    let ledgers: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pending-state.json")).unwrap();
    let mut case = ledgers
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap()
        .clone();
    case["files"] = json!({ "GROUNDING.yaml": record });
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    fixture(&root, &case);
    fs::create_dir(root.join("evidence")).unwrap();
    fs::write(root.join("evidence/vendor.txt"), "The limit is 10.\n").unwrap();
    (temp, root)
}

fn dry_run(root: &Path, names: &[&str], frozen: bool) -> CommandOutput {
    public_consolidation::dispatch(
        &Options {
            names: names.iter().map(|name| (*name).into()).collect(),
            dry_run: true,
            frozen,
            ..Default::default()
        },
        root,
    )
}

fn ledger_head(root: &Path) -> Vec<u8> {
    git(root, &["rev-parse", "refs/kpopper/pending_grounding"], None)
}

#[test]
fn a_pending_value_that_breaks_a_falsifier_makes_the_live_dry_run_red() {
    let (_temp, root) = ledger("two", RECORD);
    let head = ledger_head(&root);
    let output = dry_run(&root, &[], false);
    assert_eq!(output.stderr, "");
    assert_eq!(
        output.stdout,
        NO_HYPOTHESES.to_owned()
            + "\n"
            + "pending (2): shared findings, each laid over the base alone - tested here, never written\n"
            + "  pending-8887db9b513d · captured locally · external: API v2 · api.limit\n"
            + "  pending-b5bea064401e · captured locally · external: API v2 · api.limit\n"
            + "\n"
            + "contested (1): an id two pending findings hold differently - at most one of them is accepted\n"
            + "  api.limit:\n"
            + "    the base holds api.limit: 10 (Limit) <- s.vendor, at table 1\n"
            + "    pending-8887db9b513d says api.limit: 10 (Limit) <- s.vendor, at table 1\n"
            + "    pending-b5bea064401e says api.limit: 11 (Limit) <- s.vendor, at table 1\n"
            + "pending-8887db9b513d, if accepted: nothing changes - the base already holds what it says\n"
            + "pending-b5bea064401e, if accepted:\n"
            + "  api.limit: 10 -> 11\n"
            + "    a reading of the same day - the base keeps what it holds\n"
            + "  FALSIFIED d.x: wrong_if holds (api.limit > 10) - broken by its own condition\n"
            + "\n"
            + "pending not clean: an id two findings contest, a falsifier holds on a pending value, a contested reading - the pending findings alone make this run red; no fold, commit or merge waits on it\n"
            + RED
    );
    assert_eq!(output.code, 1);
    // Nothing is adopted: the record and the ledger stay exactly as they were.
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        RECORD
    );
    assert_eq!(ledger_head(&root), head);
}

#[test]
fn a_frozen_dry_run_has_no_pending_part() {
    let (_temp, root) = ledger("two", RECORD);
    let output = dry_run(&root, &[], true);
    assert_eq!((output.stdout.as_str(), output.code), (NO_HYPOTHESES, 0));
}

#[test]
fn a_rejected_finding_is_no_longer_laid_over_the_record() {
    let (_temp, root) = ledger("two", RECORD);
    pending_control::decision(
        &Project::open(&root).unwrap(),
        "reject",
        &[SAYS_ELEVEN.into()],
        "the vendor table says 10",
    )
    .unwrap();
    let output = dry_run(&root, &[], false);
    assert_eq!(
        output.stdout,
        NO_HYPOTHESES.to_owned()
            + "\n"
            + "pending (1): shared findings, each laid over the base alone - tested here, never written\n"
            + "  pending-8887db9b513d · captured locally · external: API v2 · api.limit\n"
            + "\n"
            + "pending-8887db9b513d, if accepted: nothing changes - the base already holds what it says\n"
            + "\n"
            + "pending clean: accepting it breaks nothing the base holds\n"
    );
    assert_eq!(output.code, 0);
    // A withdrawn finding is retired the same way; with none left, the run is as it was.
    pending_control::decision(
        &Project::open(&root).unwrap(),
        "withdraw",
        &[SAYS_TEN.into()],
        "superseded by a later reading",
    )
    .unwrap();
    let output = dry_run(&root, &[], false);
    assert_eq!((output.stdout.as_str(), output.code), (NO_HYPOTHESES, 0));
}

#[test]
fn hypotheses_come_first_and_a_named_run_is_about_them_alone() {
    let (_temp, root) = ledger("two", RECORD);
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/trial.yaml"),
        "known:\n  api.window: {v: 5}\n",
    )
    .unwrap();
    let named = dry_run(&root, &["trial"], false);
    assert!(
        named
            .stdout
            .ends_with("clean: trial may fold - consolidate trial\n")
    );
    assert!(!named.stdout.contains("pending"), "{}", named.stdout);
    assert_eq!(named.code, 0);
    let every = dry_run(&root, &[], false);
    assert!(every.stdout.starts_with(&named.stdout), "{}", every.stdout);
    let pending = &every.stdout[named.stdout.len()..];
    assert!(
        pending.starts_with("\npending (2): shared findings, each laid over the base alone"),
        "{pending}"
    );
    assert!(pending.contains("  FALSIFIED d.x: wrong_if holds (api.limit > 10)"));
    assert!(pending.ends_with(RED));
    assert_eq!(every.code, 1);
}

#[test]
fn a_hole_names_no_pending_finding_to_consolidate_with() {
    let (_temp, root) = ledger(
        "one",
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  local.one: {v: 1}\n",
    );
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/trial.yaml"),
        "judgments:\n  d.h: {verdict: fits, rests_on: [api.limit], wrong_if: \"api.limit > 100\"}\n",
    )
    .unwrap();
    let output = dry_run(&root, &[], false);
    assert!(
        output
            .stdout
            .contains("  FAIL d.h: rests on api.limit, which is not an entry\n"),
        "{}",
        output.stdout
    );
    assert!(!output.stdout.contains("held by"), "{}", output.stdout);
    assert!(output.stdout.contains(
        "pending-8887db9b513d, if accepted: nothing breaks - it adds api.limit, s.vendor\n"
    ));
    assert_eq!(output.code, 1);
}

fn kpop(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env_remove("KPOPPER_NATIVE_RESOURCES");
    command
}

/// A repository whose pending ledger holds an older shared record, imported as one finding.
fn imported(shared_record: &str, record: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"], None);
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let file = temp.path().join("shared.yaml");
    fs::write(&file, shared_record).unwrap();
    let captured = kpop(&root)
        .args([
            "knowledge",
            "import",
            "--shareability",
            "project",
            "--scope",
            "external",
            "--environment",
            "API v2",
            "--event-id",
            "imported",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        captured.status.success(),
        "{}",
        String::from_utf8_lossy(&captured.stdout)
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        record
    );
    (temp, root)
}

/// A repository whose pending ledger holds what a session shared from each record in turn,
/// captured through the command line; the checkout then holds the record under test.
fn shared(captures: &[(&str, &[&str])], record: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"], None);
    for (event, (from, args)) in captures.iter().enumerate() {
        fs::write(root.join("GROUNDING.yaml"), from).unwrap();
        let event = format!("finding-{event}");
        let captured = kpop(&root)
            .args(*args)
            .args([
                "--shareability",
                "project",
                "--scope",
                "external",
                "--environment",
                "API v2",
                "--event-id",
                &event,
            ])
            .output()
            .unwrap();
        assert!(
            captured.status.success(),
            "{}",
            String::from_utf8_lossy(&captured.stderr)
        );
        // Sharing a finding leaves the record as it was.
        assert_eq!(
            fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
            *from
        );
    }
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    (temp, root)
}

/// The lines after the pending part's list of findings.
fn verdicts(output: &CommandOutput) -> Vec<&str> {
    output
        .stdout
        .lines()
        .skip_while(|line| !line.starts_with("pending ("))
        .skip_while(|line| !line.is_empty())
        .skip(1)
        .collect()
}

/// A finding captured the way a session shares one: a newer reading of an id the record holds.
#[test]
fn a_captured_newer_reading_is_tested_as_an_update_to_the_record() {
    let record = "sources:\n  s.vendor: {name: Vendor, read: '2026-09-14'}\nknown:\n  api.limit: {name: Limit, v: 10, from: s.vendor, of: '2026-09-14'}\n  api.window: {name: Window, v: 5, from: s.vendor, of: '2026-09-14'}\njudgments:\n  d.x:\n    verdict: the client stays under the vendor limit\n    rests_on: [api.limit]\n    wrong_if: \"api.limit > 10\"\n  d.y:\n    verdict: one burst fits the window\n    rests_on: [api.limit, api.window]\n    seen: {api.limit: 10, api.window: 5}\n    wrong_if: \"api.window > 20\"\n";
    let (_temp, root) = shared(
        &[(record, &["set", "api.limit", "11", "--as-of", "2026-09-20"])],
        record,
    );
    let output = dry_run(&root, &[], false);
    let lines = verdicts(&output);
    assert_eq!(
        lines[1..],
        [
            "  api.limit: 10 -> 11",
            "    a reading from 2026-09-20 that is newer than the base's",
            "  FALSIFIED d.x: wrong_if holds (api.limit > 10) - broken by its own condition",
            "  MOVED d.y: api.limit differs from its snapshot (10 -> 11) - re-review, or refresh seen",
            "",
            "pending not clean: a falsifier holds on a pending value - the pending findings alone make this run red; no fold, commit or merge waits on it",
            RED.trim_end(),
        ],
        "{}",
        output.stdout
    );
    assert!(lines[0].ends_with(", if accepted:"), "{}", output.stdout);
    assert_eq!(output.code, 1);
    assert_eq!(dry_run(&root, &[], true).stdout, NO_HYPOTHESES);
}

#[test]
fn one_value_from_two_sources_is_two_versions() {
    let (_temp, root) = shared(
        &[
            (
                "sources:\n  s.a: {name: Vendor a, read: '2026-09-14'}\n",
                &["add", "api.limit", "v=10", "name=Limit", "from=s.a"],
            ),
            (
                "sources:\n  s.b: {name: Vendor b, read: '2026-09-14'}\n",
                &["add", "api.limit", "v=10", "name=Limit", "from=s.b"],
            ),
        ],
        RECORD,
    );
    let output = dry_run(&root, &[], false);
    let lines = verdicts(&output);
    assert_eq!(
        lines[..3],
        [
            "contested (1): an id two pending findings hold differently - at most one of them is accepted",
            "  api.limit:",
            "    the base holds api.limit: 10 (Limit) <- s.vendor, at table 1",
        ],
        "{}",
        output.stdout
    );
    assert!(lines[3].ends_with(" says api.limit: 10 (Limit) <- s.a"));
    assert!(lines[4].ends_with(" says api.limit: 10 (Limit) <- s.b"));
    assert!(
        output
            .stdout
            .contains("\npending not clean: an id two findings contest - ")
    );
    assert_eq!(output.code, 1);
}

/// The scope an imported finding's roots carry.
const SHARED_SCOPE: &str = "scope: {kind: external, environment: API v2}";

#[test]
fn a_finding_read_by_other_fields_is_reported_and_never_passes() {
    let (_temp, root) = imported(
        &format!(
            "schema: {{deps: requires, predicate: fails_if}}\nsources:\n  s.vendor: {{name: Vendor, read: '2026-09-14'}}\nknown:\n  api.limit: {{name: Limit, v: 10, from: s.vendor, {SHARED_SCOPE}}}\njudgments:\n  d.z: {{verdict: the limit leaves room, requires: [api.limit], fails_if: \"api.limit > 50\", {SHARED_SCOPE}}}\n"
        ),
        &("schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n".to_owned() + RECORD),
    );
    let output = dry_run(&root, &[], false);
    let lines = verdicts(&output);
    assert!(
        lines[0].ends_with(
            ", if accepted: it cannot be read over the base - it holds d.z as a judgment, and the base would read an entry"
        ),
        "{}",
        output.stdout
    );
    assert!(
        lines[2].starts_with("pending not clean: a finding that cannot be read over the base - ")
    );
    assert_eq!(output.code, 1);
}

#[test]
fn a_finding_the_base_cannot_read_is_reported_and_the_run_goes_on() {
    let (_temp, root) = imported(
        &format!(
            "schema: {{deps: depends_on}}\nsources:\n  s.vendor: {{name: Vendor, read: '2026-09-14'}}\nknown:\n  api.limit: {{name: Limit, v: 10, from: s.vendor, {SHARED_SCOPE}}}\njudgments:\n  d.z: {{verdict: the limit is enough, depends_on: [api.limit], wrong_if: \"api.limit < 5\", {SHARED_SCOPE}}}\n"
        ),
        RECORD,
    );
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/trial.yaml"),
        "known:\n  api.window: {v: 5}\n",
    )
    .unwrap();
    let output = dry_run(&root, &[], false);
    assert!(
        output
            .stdout
            .contains("\nclean: trial may fold - consolidate trial\n\npending (1): "),
        "{}",
        output.stdout
    );
    let lines = verdicts(&output);
    assert!(
        lines[0].ends_with(", if accepted: it cannot be read over the base - check says why"),
        "{}",
        output.stdout
    );
    assert!(
        lines[2].starts_with("pending not clean: a finding that cannot be read over the base - ")
    );
    assert_eq!(output.code, 1);
}

#[test]
fn a_finding_that_changes_how_the_base_is_read_never_passes() {
    // Two judgments on another dependency field outvote the base's one, and d.x - whose
    // falsifier the newer limit would break - would drop out of the test unseen.
    let (_temp, root) = imported(
        &format!(
            "sources:\n  s.vendor: {{name: Vendor, read: '2026-09-20'}}\nknown:\n  api.limit: {{name: Limit, v: 11, from: s.vendor, {SHARED_SCOPE}}}\njudgments:\n  d.p: {{verdict: one, requires: [api.limit], fails_if: \"api.limit > 100\", {SHARED_SCOPE}}}\n  d.q: {{verdict: two, requires: [api.limit], fails_if: \"api.limit > 200\", {SHARED_SCOPE}}}\n"
        ),
        &RECORD.replace(", file: evidence/vendor.txt", ""),
    );
    let output = dry_run(&root, &[], false);
    let lines = verdicts(&output);
    assert!(
        lines[0].ends_with(
            ", if accepted: it cannot be read over the base - laid over the base, it would turn the base's d.x from a judgment into an entry"
        ),
        "{}",
        output.stdout
    );
    assert!(!output.stdout.contains("pending clean"));
    assert_eq!(output.code, 1);
}

#[test]
fn a_finding_the_base_cannot_read_leaves_a_brief_beside_the_record_alone() {
    let (_temp, root) = imported(
        &format!(
            "schema: {{deps: depends_on}}\nsources:\n  s.vendor: {{name: Vendor, read: '2026-09-14'}}\nknown:\n  api.limit: {{name: Limit, v: 10, from: s.vendor, {SHARED_SCOPE}}}\njudgments:\n  d.z: {{verdict: the limit is enough, depends_on: [api.limit], wrong_if: \"api.limit < 5\", {SHARED_SCOPE}}}\n"
        ),
        RECORD,
    );
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/view.yaml"), "sections: []\n").unwrap();
    let output = dry_run(&root, &[], false);
    assert_eq!(output.stderr, "");
    assert!(
        verdicts(&output)[0]
            .ends_with(", if accepted: it cannot be read over the base - check says why"),
        "{}",
        output.stdout
    );
    assert_eq!(output.code, 1);
}
