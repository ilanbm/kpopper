use serde_json::Value as J;
use std::{fs, path::Path, process::Command};
fn cli(root: &Path, args: &[&str], private: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
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
            assert_eq!(
                fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
                semantic_cycle,
                "{args:?} changed a refused record"
            );
            let diagnostic = [output.stdout, output.stderr].concat();
            assert!(
                String::from_utf8_lossy(&diagnostic).contains("recursive_yaml_alias: anchor=x")
                    && String::from_utf8_lossy(&diagnostic).contains("line 1 column")
                    && String::from_utf8_lossy(&diagnostic).contains(
                        "records are acyclic; recursive YAML aliases cannot be represented"
                    ),
                "{args:?}: {}",
                String::from_utf8_lossy(&diagnostic)
            );
        }
    }
}

#[test]
fn recursive_aliases_refuse_writes_to_the_record_or_selected_hypothesis() {
    for cycle_in_hypothesis in [false, true] {
        for mut args in [
            vec!["add", "p.new", "v=7"],
            vec!["set", "p.load", "62", "--why", "new observation"],
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            let entry = root.join("GROUNDING.yaml");
            let hypothesis = root.join(".kpopper/hypotheses/candidate.yaml");
            fs::create_dir_all(hypothesis.parent().unwrap()).unwrap();
            let cycle = "custom: &loop {p.cycle: *loop}\n";
            let record = if cycle_in_hypothesis {
                ORDINARY.to_owned()
            } else {
                format!("{ORDINARY}{cycle}")
            };
            let proposal = if cycle_in_hypothesis {
                cycle
            } else {
                "known:\n  p.proposed: {v: 7}\n"
            };
            fs::write(&entry, &record).unwrap();
            fs::write(&hypothesis, proposal).unwrap();
            if cycle_in_hypothesis {
                args.extend(["--hypothesis", "candidate"]);
            }

            let output = cli(root, &args, &root.join("private"));
            assert!(!output.status.success(), "{args:?}");
            let diagnostic = [output.stdout, output.stderr].concat();
            assert!(
                String::from_utf8_lossy(&diagnostic).contains("recursive_yaml_alias"),
                "{args:?}: {}",
                String::from_utf8_lossy(&diagnostic)
            );
            assert_eq!(fs::read(&entry).unwrap(), record.as_bytes());
            assert_eq!(fs::read(&hypothesis).unwrap(), proposal.as_bytes());
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

/// `check` on a record, against what the Python reader prints for the same record.
fn assert_check_matches_reference(record: &str, reference: &str, code: i32) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let output = cli(root, &["--frozen", "check"], &root.join("private"));
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), reference);
}

#[test]
fn check_notes_a_declared_hole_and_a_comparison_nothing_decides_as_python_does() {
    assert_check_matches_reference(
        include_str!("fixtures/ordinary-note-holes.yaml"),
        include_str!("fixtures/ordinary-note-holes.stdout"),
        1,
    );
}

#[test]
fn check_clips_a_moved_reading_at_the_reference_forty_characters() {
    assert_check_matches_reference(
        include_str!("fixtures/ordinary-note-moved.yaml"),
        include_str!("fixtures/ordinary-note-moved.stdout"),
        0,
    );
}

#[test]
fn check_asks_no_review_of_a_replaced_arrangement_and_counts_judgments_on_priors() {
    assert_check_matches_reference(
        include_str!("fixtures/ordinary-note-arrangement.yaml"),
        include_str!("fixtures/ordinary-note-arrangement.stdout"),
        0,
    );
}

#[test]
fn a_structured_condition_nothing_can_compute_fails_check_unless_its_hole_is_declared() {
    // Without the ordinary program a structured condition cannot be computed, and check
    // says so rather than passing it; a declared hole turns the failure into a note.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let record = concat!(
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\n",
        "known:\n",
        "  order.price: {v: 20}\n",
        "judgments:\n",
        "  c.budget:\n",
        "    verdict: within budget\n",
        "    rests_on: [order.price]\n",
        "    seen: {order.price: 20}\n",
        "    wrong_if: {op: gt, args: [{ref: order.price}, {num: '10'}]}\n",
    );
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let output = cli(root, &["--frozen", "check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "FAIL c.budget: condition cannot be computed: ordinary expression program is not configured\n",
            "\n",
            "1 judgments, 2 entries, 1 problems\n",
        )
    );

    fs::write(
        root.join("GROUNDING.yaml"),
        format!("{record}    blocked_on: the evaluator is not installed here yet\n"),
    )
    .unwrap();
    let output = cli(root, &["--frozen", "check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "NOTE c.budget: condition cannot be computed: ordinary expression program is not configured\n",
            "\n",
            "1 judgments, 2 entries, 0 problems, 1 declared\n",
        )
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
    // This reads without the ordinary program, so the structured condition cannot be
    // computed and says so; with the program it does not hold, and that line is absent.
    assert_eq!(
        failures,
        vec![
            "FAIL d.structured: predicate reads undeclared references: p.b",
            "FAIL p.invalid: rule: invalid expression fields",
            "FAIL p.mixed: a structured rule cannot also store v or quoted",
            "FAIL p.unknown: rule: unknown references: p.ghost",
            "FAIL d.reopened: reopened_by reads as a comparison (p.a > 9) - a predicate belongs in wrong_if, where it is evaluated; a re-opener is the sign a person reads",
            "FAIL d.structured: predicate reads p.b, which it does not declare as a dependency - a change to it would never reach this",
            "FAIL d.structured: condition cannot be computed: ordinary expression program is not configured",
            "FAIL d.text: predicate reads p.b, which it does not declare as a dependency - a change to it would never reach this",
        ]
    );
    assert!(text.ends_with("3 judgments, 8 entries, 8 problems, 1 declared\n"));
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
fn open_refuses_a_named_record_that_is_not_there_on_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let output = cli(&root, &["open", "missing.yaml"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "The requested record is unavailable: {}\n",
            root.join("missing.yaml").display()
        )
    );
}

fn no_record_here(name: &str) -> String {
    format!(
        "{name}: no record here. Run this from the directory the record sits in, or name the record file as an argument.\n"
    )
}

#[test]
fn a_record_that_is_not_there_is_refused_as_the_python_reader_refuses_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let private = root.join("private");
    fs::write(root.join("other.yaml"), "known:\n  x.y: {v: 1}\n").unwrap();
    for (args, named) in [
        (vec!["check"], "GROUNDING.yaml"),
        (vec!["--frozen", "check"], "GROUNDING.yaml"),
        (vec!["pull", "x.y"], "GROUNDING.yaml"),
        (vec!["--frozen", "pull", "x.y"], "GROUNDING.yaml"),
        (vec!["affects", "x.y"], "GROUNDING.yaml"),
        (vec!["--frozen", "affects", "x.y"], "GROUNDING.yaml"),
        (vec!["assess", "x.y"], "GROUNDING.yaml"),
        (vec!["export", "x.y"], "GROUNDING.yaml"),
        (vec!["set", "x.y", "2"], "GROUNDING.yaml"),
        (vec!["review", "d.x"], "GROUNDING.yaml"),
        (vec!["consolidate", "--dry-run"], "GROUNDING.yaml"),
        (vec!["check", "missing.yaml"], "missing.yaml"),
        (vec!["check", "./missing.yaml"], "./missing.yaml"),
        (vec!["check", "MISSING.YAML"], "MISSING.YAML"),
        (vec!["check", "other.yaml", "missing.yaml"], "missing.yaml"),
        (vec!["pull", "x.y", "missing.yaml"], "missing.yaml"),
        (vec!["affects", "x.y", "missing.yaml"], "missing.yaml"),
        (
            vec!["assess", "x.y", "--record", "missing.yaml"],
            "missing.yaml",
        ),
        (
            vec!["export", "x.y", "--record", "missing.yaml"],
            "missing.yaml",
        ),
        (vec!["add", "x.z", "v=1", "missing.yaml"], "missing.yaml"),
        (
            vec!["consolidate", "--dry-run", "missing.yaml"],
            "missing.yaml",
        ),
    ] {
        let output = cli(root, &args, &private);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            no_record_here(named),
            "{args:?}"
        );
    }
    assert!(!root.join("GROUNDING.yaml").exists());
    assert!(!root.join("missing.yaml").exists());

    let output = cli(root, &["--json", "check"], &private);
    assert_eq!(output.status.code(), Some(1));
    let result: J = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 1);
    assert_eq!(result["error"], no_record_here("GROUNDING.yaml"));
    // The Hub keeps its own framing and exit status around the same reason.
    let output = cli(root, &["experimental", "hub"], &private);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "kpop experimental hub: {}",
            no_record_here("GROUNDING.yaml")
        )
    );
}

#[test]
fn a_configured_simple_record_that_is_not_there_is_named_in_full() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let private = root.join("private");
    let git = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["init", "-q", "-b", "main"])
        .output()
        .unwrap();
    assert!(git.status.success());
    fs::create_dir_all(root.join(".git/kpopper/project")).unwrap();
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(
        root.join(".git/kpopper/project/project.json"),
        r#"{"version": 1, "mode": "simple", "record": "notes/knowledge.yaml", "publication": null, "generation": 0}"#,
    )
    .unwrap();
    // A live read takes the project's one record for the entry names at its root, and for
    // the record named as it is.
    let configured = root.join("notes/knowledge.yaml");
    for args in [
        vec!["check"],
        vec!["check", "GROUNDING.yaml"],
        vec!["check", "notes/knowledge.yaml"],
        vec!["pull", "x.y"],
        vec!["assess", "x.y"],
        vec!["set", "x.y", "2"],
    ] {
        let output = cli(&root, &args, &private);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            no_record_here(configured.to_str().unwrap()),
            "{args:?}"
        );
    }
    // A frozen read takes the files as the command gave them, and so does a read from
    // below the project's root.
    let output = cli(&root, &["--frozen", "check"], &private);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        no_record_here("GROUNDING.yaml")
    );
    let output = cli(&root.join("notes"), &["check"], &private);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        no_record_here("GROUNDING.yaml")
    );
    assert!(!configured.exists());
    // The record keeps the root's entry name: it is still named in full.
    fs::write(
        root.join(".git/kpopper/project/project.json"),
        r#"{"version": 1, "mode": "simple", "record": "GROUNDING.yaml", "publication": null, "generation": 0}"#,
    )
    .unwrap();
    let output = cli(&root, &["check"], &private);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        no_record_here(root.join("GROUNDING.yaml").to_str().unwrap())
    );
}

const BROKEN_YAML: &str = "known:\n  a.b: {v: 1\n  c.d: [unclosed\n";
fn broken_yaml_refusal(name: &str) -> String {
    format!(
        "{name}: the record is not valid YAML.\nwhile parsing a flow mapping\n  in \"{name}\", line 2, column 8:\n      a.b: {{v: 1\n           ^\nexpected ',' or '}}', but got ':'\n  in \"{name}\", line 3, column 6:\n      c.d: [unclosed\n         ^\n"
    )
}

#[test]
fn a_record_that_does_not_parse_is_refused_with_the_line_and_column() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let private = root.join("private");
    let entry = root.join("GROUNDING.yaml");
    fs::write(&entry, BROKEN_YAML).unwrap();
    let baseline = root.join("baseline.json");
    let (baseline, record) = (baseline.to_str().unwrap(), entry.to_str().unwrap());
    for args in [
        vec!["check"],
        vec!["--frozen", "check"],
        vec!["pull", "a.b"],
        vec!["--frozen", "pull", "a.b"],
        vec!["affects", "a.b"],
        vec!["open"],
        vec!["--frozen", "open"],
        vec!["assess", "a.b"],
        vec!["export", "a.b"],
        vec!["add", "x.y", "v=1"],
        vec!["set", "a.b", "2"],
        vec!["consolidate", "--dry-run"],
        vec!["mark", baseline, record],
    ] {
        let output = cli(root, &args, &private);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            broken_yaml_refusal("GROUNDING.yaml"),
            "{args:?}"
        );
    }
    assert_eq!(fs::read_to_string(&entry).unwrap(), BROKEN_YAML);

    let output = cli(root, &["--json", "check"], &private);
    assert_eq!(output.status.code(), Some(1));
    let result: J = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 1);
    assert_eq!(result["error"], broken_yaml_refusal("GROUNDING.yaml"));
    let output = cli(root, &["experimental", "hub"], &private);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "kpop experimental hub: {}",
            broken_yaml_refusal("GROUNDING.yaml")
        )
    );

    // A file the record points to is named as the command line would name it, and a
    // record found above the current directory by its full path.
    fs::write(&entry, "also: notes/more.yaml\nknown:\n  x.y: {v: 1}\n").unwrap();
    fs::create_dir_all(root.join("notes")).unwrap();
    fs::write(root.join("notes/more.yaml"), BROKEN_YAML).unwrap();
    let output = cli(root, &["check"], &private);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        broken_yaml_refusal("notes/more.yaml")
    );
    fs::write(&entry, BROKEN_YAML).unwrap();
    let output = cli(&root.join("notes"), &["check"], &private);
    assert_eq!(output.status.code(), Some(1));
    let full = root.canonicalize().unwrap().join("GROUNDING.yaml");
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        broken_yaml_refusal(full.to_str().unwrap())
    );

    // A tab where PyYAML takes none is refused where it stands.
    fs::write(&entry, "known:\n\tp.a: 1\n").unwrap();
    let output = cli(root, &["check"], &private);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "GROUNDING.yaml: the record is not valid YAML.\nwhile scanning for the next token\nfound character '\\t' that cannot start any token\n  in \"GROUNDING.yaml\", line 2, column 1:\n    \tp.a: 1\n    ^\n"
    );
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

/// Every reason the Python opener gives a person, in its order: broken, undeclared and
/// blocked references, a misfiled re-opener, a falsified predicate, a reversed verdict and
/// a moved reading; and in the head the legend, the priors line, a reading only a replaced
/// judgment listened to, and a file left under the earlier layout. The expected outputs
/// are the Python reader's own.
#[test]
fn ordinary_open_names_every_reason_the_python_opener_gives_a_person() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        include_str!("fixtures/opener/record.yaml"),
    )
    .unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/replaced.yaml"),
        include_str!("fixtures/opener/replaced.yaml"),
    )
    .unwrap();
    fs::write(root.join("PROVENANCE.view.yaml"), "title: left behind\n").unwrap();
    for (args, expected) in [
        (
            &["--frozen", "open", "--chars", "40000"][..],
            include_str!("fixtures/opener/reasons.stdout"),
        ),
        (
            &["--frozen", "open", "--budget", "2"][..],
            include_str!("fixtures/opener/reasons-budget.stdout"),
        ),
        (
            &["--frozen", "open", "--chars", "700"][..],
            include_str!("fixtures/opener/reasons-chars.stdout"),
        ),
    ] {
        let output = cli(&root, args, &root.join("private"));
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected,
            "{args:?}"
        );
    }
}

/// A bare `open` fills the 2000-character slot a session hook gets and counts what it cut,
/// exactly as `--chars 2000` does; a caller naming only an item budget gets neither the
/// standing verdicts nor a character cut.
#[test]
fn ordinary_open_fills_the_session_slot_unless_its_caller_names_a_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        include_str!("fixtures/opener/slot.yaml"),
    )
    .unwrap();
    let private = root.join("private");
    for args in [
        &["--frozen", "open"][..],
        &["--frozen", "open", "--chars", "2000"][..],
    ] {
        let output = cli(&root, args, &private);
        assert!(output.status.success(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            include_str!("fixtures/opener/slot.stdout"),
            "{args:?}"
        );
    }
    let output = cli(&root, &["--frozen", "open", "--budget", "25"], &private);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        include_str!("fixtures/opener/slot-budget.stdout")
    );
    let refused = cli(&root, &["--frozen", "open", "--chars", "0"], &private);
    assert_eq!(refused.status.code(), Some(2));
    assert!(refused.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("--chars must be positive"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

/// The next moves are named as the host invokes a skill, consolidation among them while a
/// hypothesis waits; a judgment resting on a prior is counted in the head, in the singular.
#[test]
fn ordinary_open_names_the_next_moves_as_the_host_invokes_them() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.x: {v: 1}\n  prior.sure: {v: 0.9}\njudgments:\n  d.one:\n    verdict: stands\n    rests_on: [p.x, prior.sure]\n    seen: {p.x: 1, prior.sure: 0.9}\n    wrong_if: p.x > 5\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/alt.yaml"),
        "known:\n  p.y: {v: 2}\n",
    )
    .unwrap();
    let opening = concat!(
        "3 entries, 1 judgments\n",
        "1 judgment rests on prior.* claims, 1 of them on a prior at 0.8 or above\n",
        "1 hypothesis waits - alt (undated, 0 rest on it)\n",
        "\n",
        "nothing needs a person right now.\n",
        "\n",
        "standing:\n",
        "  = d.one: stands\n",
        "\n",
    );
    for (host, next) in [
        (
            Some("claude"),
            "next: /kpopper:ground <entry|prefix> (values with sources, what a change reaches) · /kpopper:record (what this session found) · check · /kpopper:consolidate (1 hypothesis waits)\n",
        ),
        (
            Some("codex"),
            "next: $ground <entry|prefix> (values with sources, what a change reaches) · $record (what this session found) · check · $consolidate (1 hypothesis waits)\n",
        ),
        (
            None,
            "next: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check\n",
        ),
    ] {
        let mut args = vec!["--frozen", "open"];
        args.extend(host.map(|host| ["--host", host]).into_iter().flatten());
        let output = cli(&root, &args, &root.join("private"));
        assert!(output.status.success(), "{host:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{opening}{next}"),
            "{host:?}"
        );
    }
}

#[test]
fn a_record_with_no_snapshot_field_says_drift_cannot_be_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let judgment = "judgments:\n  d.w: {verdict: known, rests_on: [api.limit], wrong_if: \"api.limit > 100\"}\n";
    fs::write(
        root.join("GROUNDING.yaml"),
        format!("known:\n  api.limit: {{v: 10}}\n{judgment}"),
    )
    .unwrap();
    for mode in [&[][..], &["--frozen"][..]] {
        let check = cli(
            &root,
            &[mode, &["check"][..]].concat(),
            &root.join("private"),
        );
        assert!(
            check.status.success(),
            "{}",
            String::from_utf8_lossy(&check.stderr)
        );
        assert_eq!(
            String::from_utf8(check.stdout).unwrap(),
            concat!(
                "NOTE no snapshot field anywhere: dependencies are declared but never captured, so drift can never be detected\n",
                "\n",
                "1 judgments, 2 entries, 0 problems, 1 declared\n",
            ),
            "{mode:?}"
        );
        let open = cli(
            &root,
            &[mode, &["open"][..]].concat(),
            &root.join("private"),
        );
        assert!(
            open.status.success(),
            "{}",
            String::from_utf8_lossy(&open.stderr)
        );
        assert_eq!(
            String::from_utf8(open.stdout).unwrap(),
            concat!(
                "2 entries, 1 judgments\n",
                "no snapshot field: drift cannot be detected in this record\n",
                "\n",
                "nothing needs a person right now.\n",
                "\n",
                "standing:\n",
                "  = d.w: known\n",
                "\n",
                "next: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check\n",
            ),
            "{mode:?}"
        );
    }

    // A snapshot field the schema names is a field, even before any judgment carries it:
    // the missing snapshot is then a problem with the judgment, not with the record.
    fs::write(
        root.join("GROUNDING.yaml"),
        format!("schema: {{snapshot: saw}}\nknown:\n  api.limit: {{v: 10}}\n{judgment}"),
    )
    .unwrap();
    let check = cli(&root, &["--frozen", "check"], &root.join("private"));
    assert_eq!(check.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(check.stdout).unwrap(),
        concat!(
            "FAIL d.w: no snapshot for api.limit - never checked against it\n",
            "\n",
            "1 judgments, 2 entries, 1 problems\n",
        )
    );
    let open = cli(&root, &["--frozen", "open"], &root.join("private"));
    assert_eq!(
        String::from_utf8(open.stdout).unwrap(),
        concat!(
            "2 entries, 1 judgments\n",
            "\n",
            "needs a person (1):\n",
            "  d.w: never checked against api.limit\n",
            "\n",
            "next: pull <entry|prefix> (values with sources) · affects <entry> (what a change reaches) · check\n",
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

/// A judgment resting on a name the record does not hold - a shared finding still
/// pending elsewhere, say - leaves no field that reads as a dependency list.
const BROKEN_REFERENCE: &str = "known:\n  local.one: {v: 1}\njudgments:\n  d.use_limit:\n    verdict: Batch requests at the vendor limit\n    rests_on: [api.limit]\n    seen: {api.limit: 10}\n    wrong_if: api.limit > 20\n";
const BROKEN_REFERENCE_REFUSAL: &str = concat!(
    "no dependency field found: no field lists names that are all entries in this record, so there is no graph to walk.\n",
    "These list names that are not entries:\n",
    "  rests_on: api.limit (in d.use_limit)\n",
    "\n",
    "Either those names are wrong, or one of these is a dependency field this reader cannot see by shape - and it does not guess between them. Fix the names, or say which:\n",
    "\n",
    "schema:\n",
    "  deps: <field name>\n",
);

#[test]
fn a_record_without_a_readable_dependency_field_is_refused_with_the_reason() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let entry = root.join("GROUNDING.yaml");
    fs::write(&entry, BROKEN_REFERENCE).unwrap();
    let baseline = root.join("baseline.json");
    let (baseline, record) = (baseline.to_str().unwrap(), entry.to_str().unwrap());
    for args in [
        vec!["check"],
        vec!["--frozen", "check"],
        vec!["pull", "d.use_limit"],
        vec!["--frozen", "pull", "d.use_limit"],
        vec!["affects", "api.limit"],
        vec!["--frozen", "affects", "api.limit"],
        vec!["open"],
        vec!["assess", "local.one"],
        vec!["export", "local.one"],
        vec!["add", "local.two", "2"],
        vec!["set", "local.one", "3"],
        vec!["mark", baseline, record],
    ] {
        let output = cli(root, &args, &root.join("private"));
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            BROKEN_REFERENCE_REFUSAL,
            "{args:?}"
        );
    }
    assert_eq!(fs::read_to_string(&entry).unwrap(), BROKEN_REFERENCE);

    let output = cli(root, &["--json", "check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    let result: J = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 1);
    assert_eq!(result["error"], BROKEN_REFERENCE_REFUSAL);
    let output = cli(
        root,
        &["--json", "export", "local.one"],
        &root.join("private"),
    );
    assert_eq!(output.status.code(), Some(1));
    let result: J = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 1);
    assert_eq!(result["error"], BROKEN_REFERENCE_REFUSAL);
    // The Hub keeps its own framing and exit status around the same reason.
    let output = cli(root, &["experimental", "hub"], &root.join("private"));
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("kpop experimental hub: {BROKEN_REFERENCE_REFUSAL}")
    );
}

#[test]
fn the_refusal_names_the_fields_that_listed_names_in_the_record_s_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (record, refusal) in [
        (
            "known:\n  local.one: {v: 1}\njudgments:\n  d.use_limit:\n    verdict: Batch requests at the vendor limit\n    wrong_if: local.one > 20\n",
            "no dependency field found: nothing declares what it rests on, so there is no graph to walk\n",
        ),
        (
            concat!(
                "zeta:\n",
                "  d.first:\n",
                "    verdict: first in the file\n",
                "    rests_on: [api.limit, api.quota, api.region, api.burst]\n",
                "    tags: [urgent]\n",
                "alpha:\n",
                "  d.second:\n",
                "    verdict: second\n",
                "    rests_on: [api.second]\n",
                "    tags: [later]\n",
                "    also_named: [a_dependency_whose_name_runs_on.well_past_the_ninety_characters_a_line_of_this_report_shows]\n",
                "  d.third:\n",
                "    verdict: third\n",
                "    rests_on: [api.third, local.one]\n",
                "    zz_first_of_the_quiet_fields: [x.one]\n",
                "    zz_second_of_the_quiet_fields: [x.two]\n",
                "    zz_third_of_the_quiet_fields: [x.three]\n",
                "    zz_fourth_of_the_quiet_fields: [x.four]\n",
                "  d.fourth:\n",
                "    verdict: fourth\n",
                "    rests_on: [api.fourth]\n",
                "known:\n",
                "  local.one: {v: 1}\n",
            ),
            concat!(
                "no dependency field found: no field lists names that are all entries in this record, so there is no graph to walk.\n",
                "These list names that are not entries:\n",
                "  rests_on: api.limit, api.quota, api.region ... (in d.first, and 3 more)\n",
                "  tags: urgent (in d.first, and 1 more)\n",
                "  also_named: a_dependency_whose_name_runs_on.well_past_the_ninety_characters_a_line_of_this_report_show ... (in d.second)\n",
                "  ... and 4 more: zz_first_of_the_quiet_fields, zz_fourth_of_the_quiet_fields, zz_second_of_the_quiet_fields ...\n",
                "\n",
                "Either those names are wrong, or one of these is a dependency field this reader cannot see by shape - and it does not guess between them. Fix the names, or say which:\n",
                "\n",
                "schema:\n",
                "  deps: <field name>\n",
            ),
        ),
        (
            "judgments:\n  d.plan:\n    verdict: plan the release\n    rests_on: [ספק.מחיר_ליחידה_בשקלים_לפני_מע״מ_כפי_שנמסר_בהצעת_המחיר_האחרונה_מהספק_הראשי_של_החברה_בחודש_שעבר]\nknown:\n  local.one: {v: 1}\n",
            concat!(
                "no dependency field found: no field lists names that are all entries in this record, so there is no graph to walk.\n",
                "These list names that are not entries:\n",
                "  rests_on: ספק.מחיר_ליחידה_בשקלים_לפני_מע״מ_כפי_שנמסר_בהצעת_המחיר_האחרונה_מהספק_הראשי_של_החברה_בחודש_ ... (in d.plan)\n",
                "\n",
                "Either those names are wrong, or one of these is a dependency field this reader cannot see by shape - and it does not guess between them. Fix the names, or say which:\n",
                "\n",
                "schema:\n",
                "  deps: <field name>\n",
            ),
        ),
        (
            "schema: {deps: depends_on}\nknown:\n  local.one: {v: 1}\njudgments:\n  d.use_limit:\n    verdict: Batch requests at the vendor limit\n    rests_on: [local.one]\n    seen: {local.one: 1}\n    wrong_if: local.one > 20\n",
            "schema names 'depends_on' for 'deps', and nothing this reader can see carries it: no judgment would be found, and the record would pass by having nothing left to check.\nFields it can see: rests_on, seen, v, verdict, wrong_if\n",
        ),
    ] {
        fs::write(root.join("GROUNDING.yaml"), record).unwrap();
        let output = cli(root, &["check"], &root.join("private"));
        assert_eq!(output.status.code(), Some(1), "{record}");
        assert_eq!(String::from_utf8(output.stderr).unwrap(), refusal);
    }
}

/// A record before its first judgment, keeping its values under a section name of its own.
const YOUNG_FACTS: &str = "meta:\n  updated: 2026-09-22\nsources:\n  pricing: {name: \"Acme's pricing page\", url: \"https://example.test/pricing\", read: \"2026-09-20\"}\nfacts:\n  acme.seat_price: {v: 42, from: pricing}\n  acme.seats: {v: 120, from: pricing}\n";

fn check_young(root: &Path, record: &str) -> std::process::Output {
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    cli(root, &["check"], &root.join("private"))
}

#[test]
fn a_young_record_reads_a_section_of_values_under_any_name() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for name in ["known", "facts", "parameters", "readings"] {
        let output = check_young(
            root,
            &YOUNG_FACTS.replace("\nfacts:\n", &format!("\n{name}:\n")),
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{name}: {stderr}");
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("0 judgments, 3 entries, 0 problems"),
            "{name}"
        );
    }
    for section in [
        "  acme.annual: {rule: \"acme.seat_price * acme.seats * 12\"}\n  contract.exit: {quoted: \"Either party may end it on 90 days' notice.\", from: pricing}\n",
        "  acme.discount: 0.1\n",
        "  memo: {name: the planning memo, file: memo.md, read: \"2026-09-19\"}\n  acme.term: {v: 12, from: memo}\n",
    ] {
        let output = check_young(root, &format!("{YOUNG_FACTS}{section}"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{section}: {stderr}");
    }
}

#[test]
fn a_young_record_takes_its_first_judgment_under_a_custom_section() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let entry = root.join("GROUNDING.yaml");
    fs::write(&entry, YOUNG_FACTS).unwrap();
    let private = root.join("private");
    let output = cli(
        root,
        &[
            "add",
            "why_acme",
            "rests_on=[acme.seats]",
            "verdict=prefer Acme",
            "because=cheaper above 100 seats",
            "wrong_if=acme.seats < 100",
            "--as-of",
            "2026-09-22",
        ],
        &private,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = fs::read_to_string(&entry).unwrap();
    assert!(saved.contains(
        "facts:\n  acme.seat_price: {v: 42, from: pricing}\n  acme.seats: {v: 120, from: pricing}\n"
    ));
    assert!(saved.contains("judgments:\n  why_acme:\n    rests_on: [acme.seats]\n"));
    assert!(saved.contains("    seen: {acme.seats: 120}\n"));
    let output = cli(root, &["check"], &private);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("1 judgments, 4 entries, 0 problems")
    );
    // Once a judgment exists, a new value still goes where the values are.
    let output = cli(
        root,
        &[
            "add",
            "acme.discount",
            "v=0.1",
            "from=pricing",
            "--as-of",
            "2026-09-22",
        ],
        &private,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(&entry)
            .unwrap()
            .contains("facts:\n  acme.discount: {v: 0.1, from: pricing}\n")
    );
}

#[test]
fn a_role_the_schema_names_is_the_one_a_young_record_s_first_judgment_takes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let entry = root.join("GROUNDING.yaml");
    fs::write(
        &entry,
        format!("schema: {{snapshot: reviewed}}\n{YOUNG_FACTS}"),
    )
    .unwrap();
    let private = root.join("private");
    let output = cli(
        root,
        &[
            "add",
            "why_acme",
            "rests_on=[acme.seats]",
            "verdict=prefer Acme",
            "because=cheaper above 100 seats",
            "wrong_if=acme.seats < 100",
            "--as-of",
            "2026-09-22",
        ],
        &private,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = fs::read_to_string(&entry).unwrap();
    assert!(
        saved.contains("    reviewed: {acme.seats: 120}\n"),
        "{saved}"
    );
    assert!(!saved.contains("    seen:"), "{saved}");
    let output = cli(root, &["check"], &private);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("1 judgments, 4 entries, 0 problems")
    );
}

#[test]
fn a_value_added_by_position_keeps_a_young_record_readable() {
    // `add <id> <value>` writes the bare value into the section its prefix already holds.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let entry = root.join("GROUNDING.yaml");
    fs::write(&entry, YOUNG_FACTS).unwrap();
    let private = root.join("private");
    let output = cli(
        root,
        &["add", "acme.term", "12", "--as-of", "2026-09-22"],
        &private,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(&entry)
            .unwrap()
            .contains("  acme.term: \"12\"\n")
    );
    let output = cli(root, &["check"], &private);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("0 judgments, 4 entries, 0 problems")
    );
}

#[test]
fn a_misspelled_dependency_declaration_is_still_refused_in_a_young_record() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let entry = root.join("GROUNDING.yaml");
    for claim in [
        // The dependency field misspelled, the judgment's other fields still its own.
        "{restson: acme.seats, verdict: prefer Acme, wrong_if: \"acme.seats < 100\"}",
        // A dependency list naming what is not an entry, under either name.
        "{rests_on: [acme.seat], verdict: prefer Acme}",
        "{depends_on: [acme.seat], conclusion: prefer Acme}",
        // A judgment in names of the record's own, holding no value.
        "{depends: acme.seats, conclusion: prefer Acme, falsified_when: \"acme.seats < 100\"}",
    ] {
        let record = format!("{YOUNG_FACTS}claims:\n  why_acme: {claim}\n");
        let output = check_young(root, &record);
        assert_eq!(output.status.code(), Some(1), "{claim}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.starts_with("no dependency field found: "),
            "{claim}: {stderr}"
        );
        let output = cli(
            root,
            &["add", "acme.discount", "v=0.1", "--as-of", "2026-09-22"],
            &root.join("private"),
        );
        assert_eq!(output.status.code(), Some(1), "{claim}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.starts_with("no dependency field found: "),
            "{claim}: {stderr}"
        );
        assert_eq!(fs::read_to_string(&entry).unwrap(), record);
    }
    let output = check_young(root, &format!("schema: {{deps: restson}}\n{YOUNG_FACTS}"));
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8(output.stderr).unwrap().starts_with(
        "schema names 'restson' for 'deps', and nothing this reader can see carries it"
    ));
}

#[test]
fn a_hypothesis_the_base_cannot_read_is_named_with_the_reason_on_one_line() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        BROKEN_REFERENCE
            .replace("[api.limit]", "[local.one]")
            .replace("{api.limit: 10}", "{local.one: 1}")
            .replace("api.limit > 20", "local.one > 20"),
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/vendor.yaml"),
        BROKEN_REFERENCE.replace("known:\n  local.one: {v: 1}\n", ""),
    )
    .unwrap();
    let why = "cannot be read over the base: no dependency field found: no field lists names that are all entries in this record, so there is no graph to walk. These list names that are not entries: rests_";

    let output = cli(root, &["check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("FAIL hypothesis vendor {why}\n\n1 judgments, 2 entries, 1 problems\n")
    );
    let output = cli(root, &["pull", "d.use_limit"], &root.join("private"));
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with(&format!("! hypothesis vendor {why}\nlocal.one: 1\n"))
    );
    let output = cli(
        root,
        &["consolidate", "--dry-run", "--as-of", "2026-01-01"],
        &root.join("private"),
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("refused - hypothesis vendor {why}\n")
    );
}

/// Two judgments that say what they rest on under different names: each field lists
/// only entries, so each gets one vote, and the reader does not choose between them.
const TIED_ROLES: &str = "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n  d.a:\n    verdict: a\n    rests_on: [local.one]\n  d.b:\n    verdict: b\n    depends: [local.two]\n";
fn tied(role: &str, first: &str, second: &str) -> String {
    format!(
        "two fields fit '{role}' ({first}, {second}) and this tool does not guess.\nAdd to the record:\n\nschema:\n  {role}: <field name>\n"
    )
}

#[test]
fn a_record_whose_field_roles_tie_is_refused_with_the_fields_that_tie() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let entry = root.join("GROUNDING.yaml");
    fs::write(&entry, TIED_ROLES).unwrap();
    let baseline = root.join("baseline.json");
    let refusal = tied("deps", "rests_on", "depends");
    for args in [
        vec!["check"],
        vec!["--frozen", "check"],
        vec!["pull", "d.a"],
        vec!["--frozen", "pull", "d.a"],
        vec!["affects", "local.one"],
        vec!["--frozen", "affects", "local.one"],
        vec!["open"],
        vec!["assess", "d.a"],
        vec!["export", "d.a"],
        vec!["add", "local.three", "3"],
        vec!["set", "local.one", "5"],
        vec!["review", "d.a"],
        vec!["mark", baseline.to_str().unwrap(), entry.to_str().unwrap()],
    ] {
        let output = cli(root, &args, &root.join("private"));
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            refusal,
            "{args:?}"
        );
    }
    assert_eq!(fs::read_to_string(&entry).unwrap(), TIED_ROLES);
    assert!(!baseline.exists());

    let output = cli(root, &["--json", "check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    let result: J = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 1);
    assert_eq!(result["error"], refusal);
    let output = cli(root, &["--json", "export", "d.a"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    let result: J = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["exit_code"], 1);
    assert_eq!(result["error"], refusal);
    let output = cli(root, &["experimental", "hub"], &root.join("private"));
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("kpop experimental hub: {refusal}")
    );
}

#[test]
fn tied_fields_are_named_in_the_order_the_record_first_gives_them() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (record, refusal) in [
        // The record's own order, collections included - not the order of the names.
        (
            "zeta:\n  d.a: {verdict: a, zz_deps: [local.one]}\nalpha:\n  d.b: {verdict: b, aa_deps: [local.two]}\nknown:\n  local.one: {v: 1}\n  local.two: {v: 2}\n",
            tied("deps", "zz_deps", "aa_deps"),
        ),
        // A list naming something that is not an entry casts no vote, so a field is
        // placed by its first vote.
        (
            "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n  d.x: {verdict: x, zz_deps: [nowhere.at_all]}\n  d.a: {verdict: a, aa_deps: [local.one]}\n  d.b: {verdict: b, zz_deps: [local.two]}\n",
            tied("deps", "aa_deps", "zz_deps"),
        ),
        // Only the fields with the most votes tie.
        (
            "known:\n  local.one: {v: 1}\njudgments:\n  d.a: {verdict: a, mm: [local.one]}\n  d.b: {verdict: b, zz: [local.one]}\n  d.c: {verdict: c, aa: [local.one]}\n  d.d: {verdict: d, aa: [local.one]}\n  d.e: {verdict: e, zz: [local.one]}\n",
            tied("deps", "zz", "aa"),
        ),
        // The snapshot and the predicate are voted on among the judgments, and a role
        // the schema names is not voted on at all.
        (
            "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n  d.a: {verdict: a, rests_on: [local.one], was: {local.one: 1}}\n  d.b: {verdict: b, rests_on: [local.two], seen: {local.two: 2}}\n",
            tied("snapshot", "was", "seen"),
        ),
        (
            "schema: {deps: basis, snapshot: seen}\nknown:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n  d.a: {verdict: a, basis: [local.one], seen: {local.one: 1}, wrong_if: local.one > 5}\n  d.b: {verdict: b, basis: [local.two], seen: {local.two: 2}, fails_if: local.two > 5}\n",
            tied("predicate", "wrong_if", "fails_if"),
        ),
    ] {
        fs::write(root.join("GROUNDING.yaml"), record).unwrap();
        let output = cli(root, &["check"], &root.join("private"));
        assert_eq!(output.status.code(), Some(1), "{record}");
        assert!(output.stdout.is_empty(), "{record}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            refusal,
            "{record}"
        );
    }
}

#[test]
fn a_hypothesis_whose_fields_tie_over_the_base_is_named_with_the_fields() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n  d.a:\n    verdict: a\n    rests_on: [local.one]\n    seen: {local.one: 1}\n    wrong_if: local.one > 5\n",
    )
    .unwrap();
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    let hypothesis = root.join(".kpopper/hypotheses/vendor.yaml");
    fs::write(
        &hypothesis,
        "judgments:\n  d.b:\n    verdict: b\n    depends: [local.two]\n",
    )
    .unwrap();
    let why = "cannot be read over the base: two fields fit 'deps' (rests_on, depends) and this tool does not guess. Add to the record: schema: deps: <field name>";

    let output = cli(root, &["check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("FAIL hypothesis vendor {why}\n\n1 judgments, 3 entries, 1 problems\n")
    );
    let output = cli(root, &["pull", "d.a"], &root.join("private"));
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with(&format!("! hypothesis vendor {why}\nlocal.one: 1\n"))
    );
    let output = cli(root, &["open"], &root.join("private"));
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains(
        "\n1 hypothesis waits - vendor (unreadable over the base: two fields fit 'deps' (rests_on, depends) and this tool ...\n"
    ));
    let output = cli(
        root,
        &["consolidate", "--dry-run", "--as-of", "2026-01-01"],
        &root.join("private"),
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("refused - hypothesis vendor {why}\n")
    );

    // A hypothesis's own collections are laid over the base in the order its file
    // holds them, so a tie between them reads as it would in that file.
    fs::write(
        &hypothesis,
        "zeta:\n  d.z: {verdict: z, zz_deps: [local.one]}\n  d.y: {verdict: y, zz_deps: [local.two]}\nalpha:\n  d.x: {verdict: x, aa_deps: [local.two]}\n  d.w: {verdict: w, aa_deps: [local.one]}\n",
    )
    .unwrap();
    let output = cli(root, &["check"], &root.join("private"));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "FAIL hypothesis vendor cannot be read over the base: two fields fit 'deps' (zz_deps, aa_deps) and this tool does not guess. Add to the record: schema: deps: <field name>\n\n1 judgments, 3 entries, 1 problems\n"
    );
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn commit_record(root: &Path, record: &str, message: &str) {
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    git(root, &["add", "GROUNDING.yaml"]);
    git(
        root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

#[test]
fn a_branch_record_whose_fields_tie_is_read_over_this_one_as_a_hypothesis_is() {
    let temp = tempfile::tempdir().unwrap();
    let root = &temp.path().canonicalize().unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    let base = "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n  d.a:\n    verdict: a\n    rests_on: [local.one]\n    seen: {local.one: 1}\n    wrong_if: local.one > 5\n";
    // The other branch adds a judgment that says what it rests on under another name, so
    // on its own its record cannot say which field is its dependency field.
    commit_record(
        root,
        &format!("{base}  d.b:\n    verdict: b\n    depends: [local.two]\n"),
        "branch",
    );
    git(root, &["branch", "other"]);
    commit_record(root, base, "base");

    // Laid over this record the two fields still tie: the branch is named with the
    // reason, and this record's own pull follows.
    let output = cli(
        root,
        &["pull", "--from", "other", "d.a"],
        &root.join("private"),
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "! hypothesis other cannot be read over the base: two fields fit 'deps' (rests_on, depends) and this tool does not guess. Add to the record: schema: deps: <field name>\nlocal.one: 1\n+ d.a: a\n    holds\n    wrong_if: local.one > 5\n\naffects <entry> shows what a change reaches\n"
    );

    // Once this record rests a second judgment on its own field, the tie is the
    // branch's alone: over this record its roles read, and what it adds is shown.
    let current = format!(
        "{base}  d.c:\n    verdict: c\n    rests_on: [local.two]\n    seen: {{local.two: 2}}\n    wrong_if: local.two > 5\n"
    );
    commit_record(root, &current, "second judgment");
    let output = cli(
        root,
        &["pull", "--from", "other", "d"],
        &root.join("private"),
    );
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "d.b:  - held by other\nlocal.one: 1\nlocal.two: 2\n+ d.a: a\n    holds\n    wrong_if: local.one > 5\n+ d.c: c\n    holds\n    wrong_if: local.two > 5\n\naffects <entry> shows what a change reaches\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        current
    );
}

#[test]
fn another_branch_s_record_is_laid_over_this_one_as_what_it_holds_differently() {
    let temp = tempfile::tempdir().unwrap();
    let root = &temp.path().canonicalize().unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    // Checking a branch out keeps the record's bytes as written, on every platform.
    git(root, &["config", "core.autocrlf", "false"]);
    let base = "known:\n  local.one: {v: 1}\n  local.two: {v: 2, of: 2026-09-10}\n  local.three: {v: two  words}\njudgments:\n  d.a:\n    verdict: a\n    rests_on: [local.one]\n    seen: {local.one: 1}\n    wrong_if: local.one > 5\n";
    commit_record(root, base, "base");
    git(root, &["branch", "same"]);
    let branches = [
        // an unchanged claim that gains a list of names
        (
            "listed",
            base.replace("{v: 1}", "{v: 1, tags: [local.two]}"),
        ),
        // an id held a second time, in another collection
        (
            "twice",
            format!(
                "{base}  local.two: {{verdict: dup, rests_on: [local.one], seen: {{local.one: 1}}}}\n"
            ),
        ),
        ("schema", format!("schema: 5\n{base}")),
        // the same claims written differently
        (
            "rewritten",
            base.replace("{v: 1}", "{v: 1.0}")
                .replace("two  words", "two words"),
        ),
        ("older", base.replace("of: 2026-09-10", "of: 2026-09-01")),
        ("value", base.replace("{v: 1}", "{v: 9}")),
        ("newer", base.replace("of: 2026-09-10", "of: 2026-09-12")),
    ];
    for (name, record) in &branches {
        git(root, &["checkout", "-q", "-b", name, "main"]);
        commit_record(root, record, name);
        git(root, &["checkout", "-q", "main"]);
    }
    // A hypothesis file the branch carries, beside a record that holds what this one does.
    git(root, &["checkout", "-q", "-b", "idea", "main"]);
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/idea.yaml"),
        "known:\n  local.one: {v: 4}\n",
    )
    .unwrap();
    git(root, &["add", ".kpopper"]);
    commit_record(root, base, "idea");
    git(root, &["checkout", "-q", "main"]);

    // What this record already holds is not laid again: a list added to an unchanged
    // claim casts no vote for a field role, a second holder of an id does not contest
    // the first, the branch's own schema stays the branch's, a claim written another
    // way or read on an earlier day is the same reading, and a hypothesis file the
    // branch carries is not contested by the branch's copy of this record.
    let holds = "+ d.a: a\n    holds\n    wrong_if: local.one > 5\n";
    let unchanged =
        format!("local.one: 1\nlocal.three: two  words\nlocal.two: 2 as of 2026-09-10\n{holds}");
    for (name, expected) in [
        ("same", unchanged.clone()),
        ("listed", unchanged.clone()),
        ("schema", unchanged.clone()),
        ("rewritten", unchanged.clone()),
        ("older", unchanged.clone()),
        (
            "twice",
            format!(
                "local.one: 1\nlocal.three: two  words\n{holds}+ local.two (in hypothesis twice): dup\n    holds\n"
            ),
        ),
        (
            "value",
            format!(
                "local.one: 1\n    proposes 1 -> 9, from value\nlocal.three: two  words\nlocal.two: 2 as of 2026-09-10\n{holds}"
            ),
        ),
        (
            "newer",
            format!(
                "local.one: 1\nlocal.three: two  words\nlocal.two: 2 as of 2026-09-10\n    proposes instead, from newer: 2 as of 2026-09-12\n{holds}"
            ),
        ),
        (
            "idea",
            format!(
                "local.one: 1\n    proposes 1 -> 4, from idea:idea\nlocal.three: two  words\nlocal.two: 2 as of 2026-09-10\n{holds}"
            ),
        ),
    ] {
        let output = cli(
            root,
            &["pull", "--from", name, "local", "d"],
            &root.join("private"),
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{name}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            expected + "\naffects <entry> shows what a change reaches\n",
            "{name}"
        );
    }
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        base
    );
}

#[test]
fn a_branch_record_whose_fields_tie_is_consolidated_over_this_one() {
    let temp = tempfile::tempdir().unwrap();
    let root = &temp.path().canonicalize().unwrap();
    let private = &root.join("private");
    git(root, &["init", "-q", "-b", "main"]);
    let known = "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n";
    let b = "  d.b: {verdict: b, depends: [local.two], seen: {local.two: 2}, wrong_if: local.two > 5}\n";
    let z = "  d.z: {verdict: z, rests_on: [local.one], seen: {local.one: 1}, wrong_if: local.one > 5}\n";
    let c = "  d.c: {verdict: c, rests_on: [local.two], seen: {local.two: 2}, wrong_if: local.two > 5}\n";
    // The other branch rests d.b on a field of another name, so on its own its record
    // cannot say which field is its dependency field.
    commit_record(root, &format!("{known}{b}{z}"), "branch");
    git(root, &["branch", "other"]);
    let commit = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "other"])
        .output()
        .unwrap()
        .stdout;
    let commit = String::from_utf8(commit).unwrap();
    let base = format!("{known}{z}");
    commit_record(root, &base, "base");

    // Laid over this record the two fields still tie, so nothing is tested or folded. The
    // tie is named in the order the records give the fields, this record's first.
    for args in [
        &["consolidate", "--dry-run", "--from", "other"][..],
        &["consolidate", "--from", "other"],
    ] {
        let output = cli(root, args, private);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            tied("deps", "rests_on", "depends"),
            "{args:?}"
        );
        assert_eq!(
            fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
            base
        );
    }

    // Once this record rests a second judgment on its own field, the tie is the branch's
    // alone: over this record its roles read, and what it adds is tested and folded.
    let current = format!("{known}{z}{c}");
    commit_record(root, &current, "second judgment");
    let report = "the base with other laid over it\n  other\n\narrived (1): what the fold would add\n  d.b:  - from other\nupdates (0): what the base holds that a hypothesis replaces, and what rests on each\nreversed (0): a verdict, or other grounds, laid over a standing judgment - by its own condition, by a person's name, or waiting for one\nmoved / falsified (0): what the union moves or breaks\ncontested (0)\ncandidates (0): pairs for a person to judge as the same subject or distinct\nnew subjects (0): prefixes the base does not hold\n\nclean: other may fold - consolidate other\n";
    // The second line names the branch's commit, the day it was made and its age.
    let told = |output: std::process::Output| {
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let mut lines = stdout.split('\n').collect::<Vec<_>>();
        assert!(
            lines[1].starts_with("  other (born ")
                && lines[1].ends_with(&format!(
                    "): what other committed ({}), read as a hypothesis",
                    &commit[..7]
                )),
            "{stdout}"
        );
        lines[1] = "  other";
        lines.join("\n")
    };
    let output = cli(
        root,
        &["consolidate", "--dry-run", "--from", "other"],
        private,
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(told(output), report);
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        current
    );

    let output = cli(root, &["consolidate", "--from", "other"], private);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        told(output),
        format!(
            "{report}\ncarry d.b from other into judgments, before d.z\nfolded other: 1 entry and 0 judgments - 1 added, 0 replaced\nfiles to commit: GROUNDING.yaml\n  nothing to delete for other: another branch keeps its own record\nnext: git add GROUNDING.yaml && git commit\n  then merge other as you would - its record is folded here, and the merge carries only its code\n\nthe record needs a person on 0 judgments - check says the rest\n"
        )
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        format!("{known}{b}{z}{c}")
    );
}

#[test]
fn a_tie_a_branch_brings_in_new_collections_is_named_in_its_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = &temp.path().canonicalize().unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    let known = "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\n";
    // Two collections this record does not hold, zeta before alpha, each resting a judgment
    // on a field of its own name: over this record three fields fit the dependency role.
    commit_record(
        root,
        &format!(
            "{known}zeta:\n  d.y: {{verdict: y, depends: [local.two], seen: {{local.two: 2}}, wrong_if: local.two > 5}}\nalpha:\n  d.x: {{verdict: x, needs: [local.one], seen: {{local.one: 1}}, wrong_if: local.one > 5}}\n"
        ),
        "branch",
    );
    git(root, &["branch", "other"]);
    let base = format!(
        "{known}judgments:\n  d.z: {{verdict: z, rests_on: [local.one], seen: {{local.one: 1}}, wrong_if: local.one > 5}}\n"
    );
    commit_record(root, &base, "base");
    for args in [
        &["consolidate", "--dry-run", "--from", "other"][..],
        &["consolidate", "--from", "other"],
    ] {
        let output = cli(root, args, &root.join("private"));
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            tied("deps", "rests_on", "depends"),
            "{args:?}"
        );
        assert_eq!(
            fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
            base
        );
    }
}

#[test]
fn a_tie_over_what_the_branch_holds_differently_is_named_over_that() {
    let temp = tempfile::tempdir().unwrap();
    let root = &temp.path().canonicalize().unwrap();
    git(root, &["init", "-q", "-b", "main"]);
    let a = "  d.a: {verdict: a, rests_on: [local.one], seen: {local.one: 1}, wrong_if: local.one > 5}\n";
    let base = format!("known:\n  local.one: {{v: 1}}\n  local.two: {{v: 2}}\njudgments:\n{a}");
    // The branch adds a list of names to an entry whose claim it leaves alone, which is not
    // laid again, and a judgment resting on a field of another name, which is.
    commit_record(
        root,
        &format!(
            "known:\n  local.one: {{v: 1, tags: [local.two]}}\n  local.two: {{v: 2}}\njudgments:\n{a}  d.b: {{verdict: b, depends: [local.two], seen: {{local.two: 2}}, wrong_if: local.two > 5}}\n"
        ),
        "branch",
    );
    git(root, &["branch", "other"]);
    commit_record(root, &base, "base");
    for args in [
        &["consolidate", "--dry-run", "--from", "other"][..],
        &["consolidate", "--from", "other"],
    ] {
        let output = cli(root, args, &root.join("private"));
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            tied("deps", "rests_on", "depends"),
            "{args:?}"
        );
        assert_eq!(
            fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
            base
        );
    }
}

#[test]
fn a_branch_record_consolidation_would_lose_or_misread_is_refused() {
    let known = "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\n";
    let judgments = "judgments:\n  d.a: {verdict: a, rests_on: [local.one], seen: {local.one: 1}, wrong_if: local.one > 5}\n  d.c: {verdict: c, rests_on: [local.two], seen: {local.two: 2}, wrong_if: local.two > 5}\n";
    let e = "  d.e: {verdict: e, rests_on: [local.two], seen: {local.two: 2}, wrong_if: local.two > 5}\n";
    let base = format!("{known}{judgments}");
    for (branch, refusal) in [
        // One id in two collections: laid over this record, one of its bodies would go.
        (
            format!("{known}  d.e: {{v: 9}}\n{judgments}{e}"),
            "duplicate_entry",
        ),
        // A record the core computes is not read as an ordinary one.
        (
            format!(
                "meta:\n  reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n{known}{judgments}{e}"
            ),
            "unsupported_capability: use core/v1 consumer",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = &temp.path().canonicalize().unwrap();
        let private = &root.join("private");
        git(root, &["init", "-q", "-b", "main"]);
        commit_record(root, &branch, "branch");
        git(root, &["branch", "other"]);
        commit_record(root, &base, "base");
        for args in [
            &["consolidate", "--dry-run", "--from", "other"][..],
            &["consolidate", "--from", "other"],
        ] {
            let output = cli(root, args, private);
            assert_eq!(output.status.code(), Some(1), "{args:?}");
            assert!(output.stdout.is_empty(), "{args:?}");
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                format!("{refusal}\n"),
                "{args:?}"
            );
            assert_eq!(
                fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
                base
            );
        }
    }
}

#[test]
fn export_and_the_write_commands_tell_the_refusal_in_the_record_s_order() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    for (record, refusal) in [
        // Entries out of name order: the first to vote leads.
        (
            "known:\n  local.one: {v: 1}\n  local.two: {v: 2}\njudgments:\n  d.zeta:\n    verdict: z\n    rests_on: [local.one]\n  d.alpha:\n    verdict: a\n    depends: [local.two]\n".to_owned(),
            tied("deps", "rests_on", "depends"),
        ),
        // Names that are not entries, held first by the record's first entry.
        (
            "zeta:\n  d.first:\n    verdict: first in the file\n    rests_on: [api.limit]\nalpha:\n  d.second:\n    verdict: second\n    rests_on: [api.second]\nknown:\n  local.one: {v: 1}\n  local.two: {v: 2}\n".to_owned(),
            concat!(
                "no dependency field found: no field lists names that are all entries in this record, so there is no graph to walk.\n",
                "These list names that are not entries:\n",
                "  rests_on: api.limit (in d.first, and 1 more)\n",
                "\n",
                "Either those names are wrong, or one of these is a dependency field this reader cannot see by shape - and it does not guess between them. Fix the names, or say which:\n",
                "\n",
                "schema:\n",
                "  deps: <field name>\n",
            )
            .to_owned(),
        ),
    ] {
        let entry = root.join("GROUNDING.yaml");
        fs::write(&entry, &record).unwrap();
        for args in [
            vec!["export", "local.one"],
            vec!["add", "local.three", "3"],
            vec!["set", "local.one", "5"],
            vec!["review", "local.one"],
            vec!["same", "local.one", "local.two"],
            vec!["distinct", "local.one", "local.two", "because"],
        ] {
            let output = cli(root, &args, &root.join("private"));
            assert_eq!(output.status.code(), Some(1), "{args:?}");
            assert!(output.stdout.is_empty(), "{args:?}");
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                refusal,
                "{args:?}"
            );
        }
        assert_eq!(fs::read_to_string(&entry).unwrap(), record);
    }
}
