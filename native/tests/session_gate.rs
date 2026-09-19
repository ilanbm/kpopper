use kpop_native::{
    ordinary_runtime::Program,
    reasoning_runtime::{OperationalBounds, Runtime, target_name},
    session_gate::{self, GateOptions},
    source_capture::ReadMode,
};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const GOOD: &str = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.ready: {v: true}\njudgments:\n  d.go: {verdict: go, rests_on: [p.ready], seen: {p.ready: true}, wrong_if: 'p.ready == false'}\n";

fn options<'a>(state: &'a Path, record: &'a PathBuf, root: &'a Path) -> GateOptions<'a> {
    GateOptions {
        state_path: state,
        paths: std::slice::from_ref(record),
        workspace: root,
        read_mode: ReadMode::Frozen,
        runtime: None,
        turns: 0,
        host: None,
        nudged_at: None,
        session_id: None,
        private_tmp: None,
    }
}

fn setup(text: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let record = tmp.path().join("GROUNDING.yaml");
    let state = tmp.path().join("private/mark.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&record, text).unwrap();
    (tmp, record, state)
}

fn python(root: &Path, arguments: &[&str]) -> (i32, String) {
    let output = Command::new(
        std::env::var_os("KPOP_SESSION_ORACLE_PYTHON").expect("set explicit oracle Python"),
    )
    .arg(
        PathBuf::from(
            std::env::var_os("KPOP_SESSION_ORACLE_ROOT").expect("set immutable oracle root"),
        )
        .join("scripts/provenance.py"),
    )
    .args(arguments)
    .current_dir(root)
    .output()
    .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
    )
}

fn runtime(cache: &Path) -> Runtime {
    let target = target_name().unwrap();
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{target}.zip"));
    let ordinary = PathBuf::from(std::env::var_os("HOME").unwrap())
        .join(".cache/kpopper/lean")
        .join(&target)
        .join(env!("KPOP_ORDINARY_SOURCE_SHA256"));
    Runtime::open(&archive, cache, OperationalBounds::default())
        .unwrap()
        .with_ordinary_program(Program::open(&ordinary).unwrap())
}

#[test]
fn inherited_failure_does_not_block_but_a_new_failure_does() {
    let bad = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1}\njudgments:\n  d.bad: {rests_on: [missing], wrong_if: 'p.a > 2'}\n";
    let (tmp, record, state) = setup(bad);
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    assert_eq!(
        session_gate::gate(&options(&state, &record, tmp.path()))
            .unwrap()
            .code,
        0
    );
    fs::write(
        &record,
        format!("{bad}  d.new: {{rests_on: [absent], wrong_if: 'p.a > 3'}}\n"),
    )
    .unwrap();
    let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
    assert_eq!(result.code, 2);
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.subject.starts_with("d.new:"))
    );
}

#[test]
fn inherited_predicate_reach_failure_does_not_hide_a_same_count_replacement() {
    let inherited = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1}\n  p.b: {v: 2}\njudgments:\n  d.old: {rests_on: [p.a], seen: {p.a: 1}, wrong_if: 'p.b > 9'}\n";
    let replacement = inherited.replace("d.old:", "d.new:");
    let (tmp, record, state) = setup(inherited);
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    assert_eq!(
        session_gate::gate(&options(&state, &record, tmp.path()))
            .unwrap()
            .code,
        0
    );
    fs::write(&record, replacement).unwrap();
    let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
    assert_eq!(result.code, 2, "{}", result.text);
    assert!(
        result.issues.iter().any(|issue| issue.subject
            == "d.new: predicate reads p.b, which it does not declare as a dependency - a change to it would never reach this")
    );
}

#[test]
fn updated_input_may_falsify_an_unchanged_judgment_and_remains_reported() {
    let (tmp, record, state) = setup(GOOD);
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    fs::write(
        &record,
        GOOD.replace("p.ready: {v: true}", "p.ready: {v: false}"),
    )
    .unwrap();
    let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
    assert_eq!(result.code, 0, "{}", result.text);
    assert_eq!(result.allowed_falsifiers, vec!["d.go"]);
    assert!(result.text.contains("remain flagged for review"));
}

#[test]
fn unattributed_new_entries_block_unless_they_are_not_owned_by_this_session() {
    let (tmp, record, state) = setup(GOOD);
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    fs::write(&record, format!("{GOOD}  p.extra: {{v: 2}}\n")).unwrap();
    let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
    assert_eq!(result.code, 2);
    let mut scoped = options(&state, &record, tmp.path());
    scoped.session_id = Some("another-session");
    scoped.private_tmp = Some(tmp.path());
    assert_eq!(session_gate::gate(&scoped).unwrap().code, 0);
}

#[test]
fn hypothesis_only_ids_are_counted_by_the_gate() {
    let (tmp, record, state) = setup(GOOD);
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    fs::create_dir_all(tmp.path().join(".kpopper/hypotheses")).unwrap();
    fs::write(
        tmp.path().join(".kpopper/hypotheses/new.yaml"),
        "hypothesis: {born: '2026-09-19'}\nknown:\n  p.hyp: {v: 3}\n",
    )
    .unwrap();
    let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
    assert_eq!(result.code, 2, "{}", result.text);
    assert!(result.text.contains("p.hyp"));
}

#[test]
fn count_only_marks_remain_conservative_and_nudge_is_persisted_once() {
    let (tmp, record, state) = setup(GOOD);
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, "0").unwrap();
    assert_eq!(
        session_gate::gate(&options(&state, &record, tmp.path()))
            .unwrap()
            .code,
        0
    );
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    let mut nudged = options(&state, &record, tmp.path());
    nudged.turns = 8;
    let first = session_gate::gate(&nudged).unwrap();
    assert_eq!(first.code, 2);
    assert!(first.text.contains("8 prompts in"));
    assert_eq!(session_gate::gate(&nudged).unwrap().code, 0);
}

#[test]
fn core_marks_core_ids_without_reading_optional_views() {
    let core = format!(
        "meta: {{reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}}}\n{GOOD}other:\n  p.scalar: hello\n"
    );
    let (tmp, record, state) = setup(&core);
    fs::create_dir_all(tmp.path().join(".kpopper")).unwrap();
    fs::write(tmp.path().join(".kpopper/view.yaml"), "not: [valid yaml").unwrap();
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    let mark: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    assert_eq!(mark["profile"], "core/v1");
    assert!(
        mark["ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "d.go")
    );
    assert!(
        mark["ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "p.scalar")
    );
}

#[test]
fn malformed_and_oversized_private_state_fail_closed() {
    let (tmp, record, state) = setup(GOOD);
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    fs::write(&state, "{broken").unwrap();
    // Historical non-JSON text is a count-only mark, as in Python.
    assert_eq!(
        session_gate::gate(&options(&state, &record, tmp.path()))
            .unwrap()
            .code,
        0
    );
    fs::write(&state, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert_eq!(
        session_gate::gate(&options(&state, &record, tmp.path()))
            .unwrap_err()
            .0,
        "invalid_session_mark"
    );
}

#[test]
fn mark_refuses_record_member_and_hypothesis_targets() {
    let (tmp, record, state) = setup(GOOD);
    let mut overlap = options(&state, &record, tmp.path());
    overlap.state_path = &record;
    assert_eq!(
        session_gate::mark(&overlap).unwrap_err().0,
        "session_mark_overlaps_record"
    );
    assert_eq!(fs::read_to_string(&record).unwrap(), GOOD);

    let hypotheses = tmp.path().join(".kpopper/hypotheses");
    fs::create_dir_all(&hypotheses).unwrap();
    let hypothesis = hypotheses.join("one.yaml");
    let body = "hypothesis: {born: '2026-09-19'}\nknown:\n  p.hyp: {v: 2}\n";
    fs::write(&hypothesis, body).unwrap();
    let mut overlap = options(&state, &record, tmp.path());
    overlap.state_path = &hypothesis;
    assert_eq!(
        session_gate::mark(&overlap).unwrap_err().0,
        "session_mark_overlaps_record"
    );
    assert_eq!(fs::read_to_string(hypothesis).unwrap(), body);

    let view = tmp.path().join(".kpopper/view.yaml");
    fs::write(&view, "tabs:\n  - name: main\n    shows: [p.ready]\n").unwrap();
    let mut overlap = options(&state, &record, tmp.path());
    overlap.state_path = &view;
    assert_eq!(
        session_gate::mark(&overlap).unwrap_err().0,
        "session_mark_overlaps_record"
    );
    assert_eq!(
        fs::read_to_string(view).unwrap(),
        "tabs:\n  - name: main\n    shows: [p.ready]\n"
    );
}

#[cfg(unix)]
#[test]
fn unavailable_ingestion_evidence_does_not_hide_new_failures_or_block() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let record = root.join("GROUNDING.yaml");
    let state = root.join("private/mark.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    let ingestion = root.join("ingestion");
    fs::create_dir_all(ingestion.join("sources")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(ingestion.join("record.json"))
            .status()
            .unwrap()
            .success()
    );
    let base = format!(
        "schema: {{deps: rests_on, snapshot: seen, predicate: wrong_if}}\nknown:\n  p.a: {{v: 1}}\n  s.ingest_e:\n    file: '{}/sources/e.txt'\n    recorded_for: purpose\n",
        ingestion.display()
    );
    fs::write(&record, &base).unwrap();
    session_gate::mark(&options(&state, &record, root)).unwrap();
    fs::write(&record,format!("{base}  p.new: {{v: 2, from: s.ingest_e}}\njudgments:\n  d.new: {{verdict: go, rests_on: [absent], seen: {{}}, wrong_if: 'p.a > 9'}}\n")).unwrap();
    let started = std::time::Instant::now();
    let result = session_gate::gate(&options(&state, &record, root)).unwrap();
    assert_eq!(result.code, 2, "{}", result.text);
    assert!(result.text.contains("d.new: rests on absent"));
    assert!(
        result
            .issues
            .iter()
            .any(|issue| issue.kind == "unattributed" && issue.subject == "p.new")
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
#[ignore = "requires an explicitly configured immutable Python oracle"]
fn python_and_native_marks_are_bidirectionally_compatible() {
    for python_marks in [true, false] {
        let (tmp, record, state) = setup(GOOD);
        if python_marks {
            assert_eq!(
                python(
                    tmp.path(),
                    &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
        } else {
            session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
        }
        let unchanged = if python_marks {
            session_gate::gate(&options(&state, &record, tmp.path())).unwrap()
        } else {
            let (code, text) = python(
                tmp.path(),
                &["gate", state.to_str().unwrap(), record.to_str().unwrap()],
            );
            kpop_native::session_gate::GateResult {
                code,
                text,
                issues: vec![],
                allowed_falsifiers: vec![],
            }
        };
        assert_eq!((unchanged.code, unchanged.text.as_str()), (0, ""));

        fs::write(
            &record,
            GOOD.replace("p.ready: {v: true}", "p.ready: {v: false}"),
        )
        .unwrap();
        let actual = if python_marks {
            let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
            (result.code, result.text)
        } else {
            python(
                tmp.path(),
                &["gate", state.to_str().unwrap(), record.to_str().unwrap()],
            )
        };
        assert_eq!(actual.0, 0, "{}", actual.1);
        assert_eq!(
            actual.1,
            "Updated readings falsified unchanged judgments: d.go. They remain flagged for review; check still reports their failed conditions.\n"
        );
    }
}

#[test]
#[ignore = "requires an explicitly configured immutable Python oracle"]
fn judgment_mark_preserves_nested_input_order_and_unicode() {
    let document = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.map: {v: {z: שלום, a: 2026-09-19}}\n  p.ready: {v: true}\njudgments:\n  d.go:\n    verdict: go\n    rests_on: [p.map, p.ready]\n    seen: {p.map: {z: שלום, a: 2026-09-19}, p.ready: true}\n    wrong_if: p.ready == false\n";
    let (tmp, record, state) = setup(document);
    let python_state = state.with_file_name("python.json");
    let result = python(
        tmp.path(),
        &[
            "mark",
            python_state.to_str().unwrap(),
            record.to_str().unwrap(),
        ],
    );
    assert_eq!(result.0, 0, "{}", result.1);
    session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
    let expected: Value = serde_json::from_slice(&fs::read(python_state).unwrap()).unwrap();
    let actual: Value = serde_json::from_slice(&fs::read(state).unwrap()).unwrap();
    assert_eq!(actual["judgments"], expected["judgments"]);
}

#[test]
#[ignore = "requires an explicitly configured immutable Python oracle"]
fn mixed_marks_preserve_reordered_bodies_and_structured_predicate_aliases() {
    let record_text = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.ready: {v: true}\njudgments:\n  d.go:\n    wrong_if: {expr: 'p.ready == false'}\n    because: reordered on purpose\n    seen: {p.ready: true}\n    verdict: go\n    rests_on: [p.ready]\n";
    for python_marks in [true, false] {
        let (tmp, record, state) = setup(record_text);
        let cache = tempfile::tempdir().unwrap();
        let runtime = runtime(cache.path());
        let mut native_options = options(&state, &record, tmp.path());
        native_options.runtime = Some(&runtime);
        if python_marks {
            assert_eq!(
                python(
                    tmp.path(),
                    &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
        } else {
            session_gate::mark(&native_options).unwrap();
        }
        fs::write(
            &record,
            record_text.replace("p.ready: {v: true}", "p.ready: {v: false}"),
        )
        .unwrap();
        let result = if python_marks {
            let result = session_gate::gate(&native_options).unwrap();
            (result.code, result.text)
        } else {
            python(
                tmp.path(),
                &["gate", state.to_str().unwrap(), record.to_str().unwrap()],
            )
        };
        assert_eq!(result.0, 0, "python_marks={python_marks}: {}", result.1);
        assert!(
            result
                .1
                .contains("Updated readings falsified unchanged judgments: d.go."),
            "python_marks={python_marks}: {}",
            result.1
        );
    }
}

#[test]
#[ignore = "requires an explicitly configured immutable Python oracle"]
fn mixed_marks_preserve_failure_sets_profile_changes_nulls_and_nudges() {
    let bad_one = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1}\njudgments:\n  d.one: {rests_on: [missing], wrong_if: 'p.a > 2'}\n";
    let bad_two = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1}\njudgments:\n  d.two: {rests_on: [absent], wrong_if: 'p.a > 2'}\n";
    for python_marks in [true, false] {
        let (tmp, record, state) = setup(bad_one);
        if python_marks {
            assert_eq!(
                python(
                    tmp.path(),
                    &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
        } else {
            session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
        }
        fs::write(&record, bad_two).unwrap();
        let result = if python_marks {
            let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
            (result.code, result.text)
        } else {
            python(
                tmp.path(),
                &["gate", state.to_str().unwrap(), record.to_str().unwrap()],
            )
        };
        assert_eq!(result.0, 2, "python_marks={python_marks}: {}", result.1);
        assert!(result.1.contains("d.two:"));
    }

    let null_predicate = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1}\njudgments:\n  d.open: {rests_on: [p.a], seen: {p.a: 1}, blocked_on: observational}\n";
    for python_marks in [true, false] {
        let (tmp, record, state) = setup(null_predicate);
        if python_marks {
            assert_eq!(
                python(
                    tmp.path(),
                    &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
            assert_eq!(
                session_gate::gate(&options(&state, &record, tmp.path()))
                    .unwrap()
                    .code,
                0
            );
        } else {
            session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
            assert_eq!(
                python(
                    tmp.path(),
                    &["gate", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
        }
        let mark: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
        assert!(mark["judgments"]["d.open"]["predicate"].is_null());
    }

    let core_bad = "meta: {reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.ready: {v: false}\njudgments:\n  d.go: {verdict: go, rests_on: [p.ready], seen: {p.ready: true}, wrong_if: {expr: 'p.ready == false'}}\n";
    for python_marks in [true, false] {
        let ordinary_bad = GOOD.replace("p.ready: {v: true}", "p.ready: {v: false}");
        let (tmp, record, state) = setup(&ordinary_bad);
        if python_marks {
            assert_eq!(
                python(
                    tmp.path(),
                    &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
        } else {
            session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
        }
        fs::write(&record, core_bad).unwrap();
        let result = if python_marks {
            let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
            (result.code, result.text)
        } else {
            python(
                tmp.path(),
                &["gate", state.to_str().unwrap(), record.to_str().unwrap()],
            )
        };
        assert_eq!(result.0, 2, "python_marks={python_marks}: {}", result.1);
        assert!(result.1.contains("fails core check"), "{}", result.1);
    }

    for python_marks in [true, false] {
        let (tmp, record, state) = setup(GOOD);
        if python_marks {
            assert_eq!(
                python(
                    tmp.path(),
                    &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
            let mut options = options(&state, &record, tmp.path());
            options.turns = 8;
            let result = session_gate::gate(&options).unwrap();
            assert_eq!(result.code, 2, "{}", result.text);
            assert!(result.text.contains("8 prompts in"));
        } else {
            session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
            let result = python(
                tmp.path(),
                &[
                    "gate",
                    state.to_str().unwrap(),
                    record.to_str().unwrap(),
                    "--turns",
                    "8",
                ],
            );
            assert_eq!(result.0, 2, "{}", result.1);
            assert!(result.1.contains("8 prompts in"));
        }
    }
}

#[test]
#[ignore = "requires an explicitly configured immutable Python oracle"]
fn mixed_marks_block_each_manual_ordinary_check_rule_family() {
    let cases = [
        (
            format!(
                "{GOOD}  p.bad: {{rule: {{op: add, args: [{{ref: p.ghost}}, {{num: '1'}}]}}}}\n"
            ),
            "p.bad: rule: unknown references: p.ghost",
        ),
        (
            format!("{GOOD}  p.bad: {{rule: {{op: add, of: [p.ready]}}}}\n"),
            "p.bad: rule: invalid expression fields",
        ),
        (
            format!(
                "{GOOD}  p.bad: {{v: 9, rule: {{op: add, args: [{{ref: p.ready}}, {{num: '1'}}]}}}}\n"
            ),
            "p.bad: a structured rule cannot also store v or quoted",
        ),
        (
            format!("{GOOD}  d.bad: {{rests_on: [], seen: {{}}, wrong_if: 'p.ready > 9'}}\n"),
            "d.bad: predicate reads p.ready, which it does not declare as a dependency",
        ),
        (
            format!(
                "{GOOD}  d.bad: {{rests_on: [p.ready], seen: {{p.ready: true}}, reopened_by: 'p.ready > 9'}}\n"
            ),
            "d.bad: reopened_by reads as a comparison (p.ready > 9)",
        ),
        (
            format!(
                "{GOOD}  d.bad: {{rests_on: [p.ready], seen: {{p.ready: true}}, wrong_if: {{op: eq, args: [{{ref: graph.moved}}, {{num: '0'}}]}}}}\n"
            ),
            "d.bad: predicate reads undeclared references: graph.moved",
        ),
    ];
    for (after, expected) in cases {
        for python_marks in [true, false] {
            let (tmp, record, state) = setup(GOOD);
            if python_marks {
                assert_eq!(
                    python(
                        tmp.path(),
                        &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                    )
                    .0,
                    0
                );
            } else {
                session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
            }
            fs::write(&record, &after).unwrap();
            let result = if python_marks {
                let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
                (result.code, result.text)
            } else {
                python(
                    tmp.path(),
                    &["gate", state.to_str().unwrap(), record.to_str().unwrap()],
                )
            };
            assert_eq!(
                result.0, 2,
                "python_marks={python_marks}, expected={expected}: {}",
                result.1
            );
            assert!(
                result.1.contains(expected),
                "python_marks={python_marks}, expected={expected}: {}",
                result.1
            );
        }
    }

    let inherited = "schema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.a: {v: 1}\n  p.b: {v: 2}\njudgments:\n  d.old: {rests_on: [p.a], seen: {p.a: 1}, wrong_if: 'p.b > 9'}\n";
    for python_marks in [true, false] {
        let (tmp, record, state) = setup(inherited);
        if python_marks {
            assert_eq!(
                python(
                    tmp.path(),
                    &["mark", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
            assert_eq!(
                session_gate::gate(&options(&state, &record, tmp.path()))
                    .unwrap()
                    .code,
                0
            );
        } else {
            session_gate::mark(&options(&state, &record, tmp.path())).unwrap();
            assert_eq!(
                python(
                    tmp.path(),
                    &["gate", state.to_str().unwrap(), record.to_str().unwrap()]
                )
                .0,
                0
            );
        }
        fs::write(&record, inherited.replace("d.old:", "d.new:")).unwrap();
        let result = if python_marks {
            let result = session_gate::gate(&options(&state, &record, tmp.path())).unwrap();
            (result.code, result.text)
        } else {
            python(
                tmp.path(),
                &["gate", state.to_str().unwrap(), record.to_str().unwrap()],
            )
        };
        assert_eq!(result.0, 2, "python_marks={python_marks}: {}", result.1);
        assert!(
            result.1.contains("d.new: predicate reads p.b"),
            "python_marks={python_marks}: {}",
            result.1
        );
    }
}
