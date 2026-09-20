use serde_json::Value as J;
use std::{fs, path::Path, process::Command};
fn cli(root: &Path, args: &[&str], private: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .current_dir(root)
        .args(args)
        .env_remove("KPOPPER_NATIVE_RESOURCES")
        .env_remove("KPOPPER_READ_MODE")
        .env("KPOPPER_PRIVATE_HOME", private)
        .env("XDG_STATE_HOME", root.join("private-state"))
        .output()
        .unwrap()
}
const ORDINARY: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.load: {v: 61, from: s.note}\n  s.note: {name: source}\njudgments:\n  d.work:\n    verdict: continue\n    rests_on: [p.load]\n    seen: {p.load: 44}\n    wrong_if: p.load > 80\n";

#[test]
fn recursive_aliases_fail_closed_in_core_and_custom_collections() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let private = root.join("private");
    let commands = [
        vec!["--json", "--frozen", "--no-cache", "open"],
        vec!["--json", "--frozen", "--no-cache", "check"],
        vec!["--json", "--frozen", "--no-cache", "pull", "venue.status"],
        vec![
            "--json",
            "--frozen",
            "--no-cache",
            "affects",
            "venue.status",
        ],
        vec!["--json", "--frozen", "--no-cache", "export", "venue.status"],
    ];
    for semantic_cycle in [
        "x: &x {next: *x}\n",
        "x: &x [*x]\n",
        "known: &x {venue.status: *x}\n",
        "known: &x [*x]\n",
        "schema: &x {deps: *x}\n",
        "parameters: &x {p.value: *x}\n",
    ] {
        fs::write(root.join("GROUNDING.yaml"), semantic_cycle).unwrap();
        for args in &commands {
            let output = cli(root, args, &private);
            assert!(!output.status.success(), "{args:?}");
            let diagnostic = [output.stdout, output.stderr].concat();
            assert!(
                String::from_utf8_lossy(&diagnostic).contains("invalid_history_yaml"),
                "{args:?}: {}",
                String::from_utf8_lossy(&diagnostic)
            );
        }
    }
}
#[test]
fn contested_check_uses_the_reference_forty_character_claim_width() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), ORDINARY).unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    for (name, claim) in [
        (
            "alpha",
            "a very long alternative reading that definitely exceeds forty characters",
        ),
        (
            "beta",
            "another equally long alternative reading exceeding forty characters",
        ),
    ] {
        fs::write(
            root.join(".kpopper/hypotheses")
                .join(format!("{name}.yaml")),
            format!("known:\n  p.load: {{quoted: {claim:?}}}\n"),
        )
        .unwrap();
    }
    let output = cli(root, &["check"], &root.join("private"));
    assert!(String::from_utf8(output.stdout).unwrap().contains("CONTESTED p.load: alpha says a very long alternative reading that de…, beta says another equally long alternative readin… - one of them folds, or neither; a person decides"));
}

#[test]
fn ordinary_check_note_families_match_python_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        include_str!("fixtures/ordinary-note-parity.yaml"),
    )
    .unwrap();
    let output = cli(root, &["--frozen", "check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stdout,
        include_bytes!("fixtures/ordinary-note-parity.stdout")
    );
}

#[test]
fn declared_blocked_note_does_not_change_the_flagged_counter() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.x: {v: 1}\n  p.total: {rule: graph.flagged}\njudgments:\n  d.wait:\n    verdict: wait\n    rests_on: [p.x]\n    seen: {p.x: 1}\n    blocked_on: waiting for evidence\n").unwrap();
    let assessment = cli(
        root,
        &["--frozen", "assess", "graph.flagged"],
        &root.join("private"),
    );
    assert!(
        assessment.status.success(),
        "{}",
        String::from_utf8_lossy(&assessment.stderr)
    );
    let report: J = serde_json::from_slice(&assessment.stdout).unwrap();
    assert_eq!(report["nodes"]["graph.flagged"]["body"]["v"], 0);
    let check = cli(root, &["--frozen", "check"], &root.join("private"));
    assert!(check.status.success());
    assert!(
        String::from_utf8(check.stdout)
            .unwrap()
            .contains("NOTE d.wait: no predicate at all (declared: waiting for evidence)")
    );
}

#[test]
fn ordinary_check_rejects_manual_expression_and_dependency_bypasses() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        concat!(
            "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n",
            "known:\n",
            "  p.a: {v: 1}\n",
            "  p.b: {v: 2}\n",
            "  p.invalid: {rule: {op: add, of: [p.b]}}\n",
            "  p.mixed: {v: 9, rule: {op: add, args: [{ref: p.a}, {num: '1'}]}}\n",
            "  p.unknown: {rule: {op: add, args: [{ref: p.ghost}, {num: '1'}]}}\n",
            "judgments:\n",
            "  d.reopened: {verdict: wait, rests_on: [p.a], seen: {p.a: 1}, reopened_by: 'p.a > 9'}\n",
            "  d.structured: {verdict: wait, rests_on: [p.a], seen: {p.a: 1}, wrong_if: {op: gt, args: [{ref: p.b}, {num: '9'}]}}\n",
            "  d.text: {verdict: wait, rests_on: [p.a], seen: {p.a: 1}, wrong_if: 'p.b > 9'}\n",
        ),
    )
    .unwrap();
    let output = cli(root, &["--frozen", "check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    let failures = text
        .lines()
        .filter(|line| line.starts_with("FAIL "))
        .collect::<Vec<_>>();
    assert_eq!(
        failures,
        vec![
            "FAIL d.structured: predicate reads undeclared references: p.b",
            "FAIL p.invalid: rule: invalid expression fields",
            "FAIL p.mixed: a structured rule cannot also store v or quoted",
            "FAIL p.unknown: rule: unknown references: p.ghost",
            "FAIL d.reopened: reopened_by reads as a comparison (p.a > 9) - a predicate belongs in wrong_if, where it is evaluated; a re-opener is the sign a person reads",
            "FAIL d.structured: predicate reads p.b, which it does not declare as a dependency - a change to it would never reach this",
            "FAIL d.text: predicate reads p.b, which it does not declare as a dependency - a change to it would never reach this",
        ]
    );
    assert!(text.ends_with("3 judgments, 8 entries, 7 problems, 1 declared\n"));
}
#[test]
fn actual_ordinary_cli_reads_physical_hypotheses_and_private_metadata_without_writes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let private = root.join("private");
    fs::write(root.join("GROUNDING.yaml"), ORDINARY).unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    let hyp = "known: {p.load: {v: 75}}\n";
    fs::write(root.join(".kpopper/hypotheses/a.yaml"), hyp).unwrap();
    let project = kpop_native::project_modes::Project::open(&root).unwrap();
    let key = kpop_native::identity::sha256(
        project
            .common
            .as_ref()
            .unwrap_or(&project.root)
            .to_string_lossy()
            .as_bytes(),
    );
    let dir = private.join(key);
    fs::create_dir_all(&dir).unwrap();
    // Only the retained obligation is public; its malformed body is never read.
    fs::write(dir.join("retained.json"), b"PRIVATE: not JSON").unwrap();
    fs::create_dir(dir.join("directory.json")).unwrap();
    let output = cli(&root, &["pull", "p"], &private);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("1 private drafts retained; inspect `kpop knowledge status`\n"));
    assert!(text.contains("p.load: 61 <- s.note\n    proposes 61 -> 75, from a\n"));
    assert!(text.contains("moved since review: p.load 44 -> 61 - within wrong_if"));
    let frozen = cli(&root, &["--frozen", "pull", "p"], &private);
    assert!(frozen.status.success());
    assert!(!String::from_utf8_lossy(&frozen.stdout).contains("private drafts"));
    let affects = cli(&root, &["affects", "p"], &private);
    assert!(affects.status.success());
    assert_eq!(
        String::from_utf8(affects.stdout).unwrap(),
        "# p -> 1 entries: p.load\nd.work\n    via p.load -> evaluate the predicate against p.load\n    predicate: p.load > 80\n\n1 judgments reached\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        ORDINARY
    );
    assert_eq!(
        fs::read_to_string(root.join(".kpopper/hypotheses/a.yaml")).unwrap(),
        hyp
    );
    assert_eq!(
        fs::read(dir.join("retained.json")).unwrap(),
        b"PRIVATE: not JSON"
    );
}
#[test]
fn actual_core_cli_opens_exact_history_without_checking_the_optional_brief() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let record = format!(
        "meta:\n  name: תיעוד\n  reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n{ORDINARY}\n"
    );
    fs::write(root.join("GROUNDING.yaml"), &record).unwrap();
    let open = cli(
        &root,
        &["--frozen", "open", "--json"],
        &root.join("private"),
    );
    assert!(
        open.status.success(),
        "{}",
        String::from_utf8_lossy(&open.stderr)
    );
    let value: J = serde_json::from_slice(&open.stdout).unwrap();
    assert_eq!(value["assessment_profile"], "core/v1");
    assert_eq!(value["nodes"], 3);
    let pull = cli(&root, &["--frozen", "pull", "p"], &root.join("private"));
    assert!(
        pull.status.success(),
        "{}",
        String::from_utf8_lossy(&pull.stderr)
    );
    let pulled: J = serde_json::from_slice(&pull.stdout).unwrap();
    assert_eq!(pulled["snapshot_id"], value["snapshot_id"]);
    assert_eq!(pulled["findings_revision"], value["findings_revision"]);
    assert_eq!(pulled["selection"], serde_json::json!(["p.load"]));
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/view.yaml"),
        "sections: [{pick: missing}]\n",
    )
    .unwrap();
    let check = cli(&root, &["--frozen", "check"], &root.join("private"));
    assert_eq!(check.status.code(), Some(1));
    let text = String::from_utf8_lossy(&check.stdout);
    assert!(!text.contains("FAIL page selectors"));
    assert!(text.starts_with("NOTE page layout not checked; use kpop experimental hub --verify\n"));
    let unsupported = cli(&root, &["pull", "p", "--history"], &root.join("private"));
    assert!(!unsupported.status.success());
    assert!(
        String::from_utf8_lossy(&unsupported.stderr).contains("core_profile_option_unsupported")
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        record
    );
}

#[test]
fn ordinary_and_core_reads_do_not_parse_or_render_optional_layouts() {
    for profile in [
        "",
        "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::write(root.join("GROUNDING.yaml"), format!("{profile}{ORDINARY}")).unwrap();
        let mut original = Vec::new();
        for args in [
            vec!["--frozen", "open"],
            vec!["--frozen", "check"],
            vec!["--frozen", "pull", "p"],
            vec!["--frozen", "affects", "p"],
        ] {
            original.push((args.clone(), cli(&root, &args, &root.join("private"))));
        }
        fs::create_dir(root.join(".kpopper")).unwrap();
        for raw in [
            "not: [valid YAML",
            "sections: [{pick: missing, as: unknown}]\n",
        ] {
            fs::write(root.join(".kpopper/view.yaml"), raw).unwrap();
            for (args, before) in &original {
                let after = cli(&root, args, &root.join("private"));
                assert_eq!(
                    after.status.code(),
                    before.status.code(),
                    "{args:?}: {}",
                    String::from_utf8_lossy(&after.stderr)
                );
                assert_eq!(after.stderr, before.stderr);
                let expected = if args[1] == "check" {
                    [
                        b"NOTE page layout not checked; use kpop experimental hub --verify\n"
                            .as_slice(),
                        before.stdout.as_slice(),
                    ]
                    .concat()
                } else {
                    before.stdout.clone()
                };
                assert_eq!(after.stdout, expected, "{args:?}");
            }
        }
    }
}

#[test]
fn ordinary_open_reports_pointer_private_and_hypothesis_orientation_without_panicking() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("GROUNDING.yaml"), "record: facts.yaml\n").unwrap();
    fs::write(root.join("facts.yaml"), ORDINARY).unwrap();
    let open = cli(&root, &["--frozen", "open"], &root.join("private"));
    assert!(open.status.success());
    assert!(String::from_utf8_lossy(&open.stdout).starts_with("  record: facts.yaml\n"));
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    for (name, value) in [("a", 70), ("b", 90)] {
        fs::write(
            root.join(format!(".kpopper/hypotheses/{name}.yaml")),
            format!("known: {{p.load: {{v: {value}}}}}\n"),
        )
        .unwrap();
    }
    let open = cli(&root, &["--frozen", "open"], &root.join("private"));
    assert!(
        open.status.success(),
        "{}",
        String::from_utf8_lossy(&open.stderr)
    );
    let text = String::from_utf8_lossy(&open.stdout);
    assert!(
        text.contains(
            "2 hypotheses wait - a (undated, 1 rests on it) · b (undated, 1 rests on it)"
        )
    );
    assert!(text.contains("p.load: CONTESTED - a says 70, b says 90"));
    let check = cli(&root, &["--frozen", "check"], &root.join("private"));
    assert!(check.status.success());
    assert!(String::from_utf8_lossy(&check.stdout).contains(
        "CONTESTED p.load: a says 70, b says 90 - one of them folds, or neither; a person decides"
    ));
}
#[test]
fn actual_open_reports_an_empty_workspace_and_explicit_missing_record() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let open = cli(&root, &["open", "--json"], &root.join("private"));
    assert!(open.status.success());
    let data: J = serde_json::from_slice(&open.stdout).unwrap();
    assert_eq!(data["status"], "missing");
    assert!(!root.join("GROUNDING.yaml").exists());
    let missing = cli(
        &root,
        &["open", "missing.yaml", "--json"],
        &root.join("private"),
    );
    assert_eq!(missing.status.code(), Some(1));
    let data: J = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(data["status"], "unavailable");
}

#[test]
fn actual_seed_case_and_legacy_negative_budget_remain_distinct_from_filenames() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let body = ORDINARY.replace("known:\n", "known:\n  p.YAML: {v: 7}\n");
    fs::write(root.join("GROUNDING.yaml"), &body).unwrap();
    let output = cli(
        &root,
        &["--frozen", "pull", "p.YAML"],
        &root.join("private"),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "p.YAML: 7\n\naffects <entry> shows what a change reaches\n"
    );
    let output = cli(
        &root,
        &["--frozen", "pull", "p.YAML", "--budget", "-1"],
        &root.join("private"),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "... 2 more lines - raise the budget\n\naffects <entry> shows what a change reaches\n"
    );
}

#[test]
fn actual_ordinary_cli_opens_and_checks_the_record() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let record = include_str!("../../examples/offer-review/after/GROUNDING.yaml");
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();

    let open = cli(&root, &["--frozen", "open"], &root.join("private"));
    assert!(
        open.status.success(),
        "{}",
        String::from_utf8_lossy(&open.stderr)
    );
    assert_eq!(
        String::from_utf8(open.stdout).unwrap(),
        concat!(
            "Illustrative mortgage plan after an agent records a replacement offer, before reviewing the old plan. All details are fictional.\n",
            "holds: offer (2) · and 2 standalone\n",
            "5 entries, 1 judgments\n",
            "\n",
            "needs a person (1):\n",
            "  plan.application_deadline: offer.current_id differs from what it last saw: A -> B\n",
            "      because: The plan uses Offer A and its written validity period; its application conditions sti ...\n",
            "\n",
            "next: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check\n",
        )
    );

    let check = cli(&root, &["--frozen", "check"], &root.join("private"));
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert_eq!(
        String::from_utf8(check.stdout).unwrap(),
        concat!(
            "NOTE plan.application_deadline: no predicate at all - decided; reopened by: A replacement offer or written bank clarification changes the terms this plan relies on. R\n",
            "MOVED plan.application_deadline: offer.current_id differs from its snapshot (A -> B) - re-review, or refresh seen\n",
            "\n",
            "1 judgments, 5 entries, 0 problems, 1 moved, 1 declared\n",
        )
    );
}

#[test]
fn actual_ordinary_pull_history_reads_the_retained_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(root.join("GROUNDING.yaml"), ORDINARY).unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/replaced.yaml"),
        "d.work:\n- verdict: before\n  because: the earlier reason\n  rests_on: [p.load]\n  wrong_if: p.load > 70\n  request: s.note\n  dropped: {p.old: superseded}\n  ended: the standing judgment broke\n  day: '2026-09-18'\n- same_as: 1\n  ended: a person restored it\n  day: '2026-09-19'\n",
    )
    .unwrap();
    let output = cli(
        &root,
        &["--frozen", "pull", "d.work", "--history"],
        &root.join("private"),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "p.load: 61 <- s.note\n",
            "+ d.work: continue\n",
            "    holds\n",
            "    wrong_if: p.load > 80\n",
            "    moved since review: p.load 44 -> 61 - within wrong_if\n",
            "\n",
            "affects <entry> shows what a change reaches\n",
            "\n",
            "history of d.work: 2 versions kept in .kpopper/replaced.yaml\n",
            "  1. until 2026-09-18 - the standing judgment broke\n",
            "     verdict: before\n",
            "     because: the earlier reason\n",
            "     rests_on: [p.load]\n",
            "     wrong_if: p.load > 70\n",
            "     request: s.note\n",
            "     no longer rested on p.old: superseded\n",
            "  2. until 2026-09-19 - a person restored it (the same decision as version 1)\n",
        ).replacen("kept in .kpopper/replaced.yaml",
            &format!("kept in {}", Path::new(".kpopper").join("replaced.yaml").display()), 1)
    );
}
