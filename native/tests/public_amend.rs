//! `answer` closes an open question with what answered it; `correct` rewrites an entry
//! only while nothing landed depends on it. Both run on ordinary and history records.
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const RECORD: &str = r#"meta:
  updated: '2026-09-20'
sources:
  src.inspection:
    name: The boiler inspection report
    file: inspection.pdf
    read: '2026-09-20'
known:
  m.boiler_age:
    name: Age of the boiler
    v: 14
    unit: years
    from: src.inspection
    at: p.2
open:
  q.second_boiler: Do we need a second boiler for the annex?
  q.annex_floor: Will the annex add a second floor?
judgments:
  d.one_boiler:
    name: One boiler is enough for now
    rests_on: [m.boiler_age]
    verdict: one boiler serves the annex until the boiler is 20 years old
    because: The inspection puts the boiler at {{m.boiler_age}} years
    wrong_if: "m.boiler_age >= 20"
    seen: {m.boiler_age: 14}
"#;

/// The runtime resources every command reads, assembled once: the reasoning archive and
/// the verified ordinary program, linked rather than copied where the volume allows.
fn resources() -> &'static Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let target = kpop_native::reasoning_runtime::target_name().unwrap();
        fs::create_dir_all(dir.path().join("reasoning")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/reasoning/native")
                .join(format!("{target}.kpopper-runtime")),
            dir.path().join("reasoning").join(format!("{target}.zip")),
        )
        .unwrap();
        let ordinary = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap())
                    .join(".cache/kpopper/lean")
                    .join(&target)
                    .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
            });
        let home = dir.path().join("ordinary").join(&target);
        fs::create_dir_all(&home).unwrap();
        for name in [
            "build.json",
            if cfg!(windows) {
                "epistemic-core.exe"
            } else {
                "epistemic-core"
            },
        ] {
            if fs::hard_link(ordinary.join(name), home.join(name)).is_err() {
                fs::copy(ordinary.join(name), home.join(name)).unwrap();
            }
        }
        dir
    })
    .path()
}
fn command(root: &Path, session: Option<&str>) -> Command {
    let private = root.join(".test-tmp");
    fs::create_dir_all(&private).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .current_dir(root)
        .env_remove("KPOPPER_AGENT_SESSION")
        .env_remove("CODEX_THREAD_ID")
        .env("TMPDIR", &private)
        .env("KPOPPER_NATIVE_RESOURCES", resources())
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"));
    if let Some(session) = session {
        command.env("KPOPPER_AGENT_SESSION", session);
    }
    command
}
fn run(root: &Path, args: &[&str]) -> Output {
    command(root, None).args(args).output().unwrap()
}
fn run_as(root: &Path, session: &str, args: &[&str]) -> Output {
    command(root, Some(session)).args(args).output().unwrap()
}
fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
fn success(output: Output) -> String {
    assert!(output.status.success(), "{}", text(&output));
    String::from_utf8(output.stdout).unwrap()
}
fn refused(output: Output, expect: &str) -> String {
    let said = text(&output);
    assert!(!output.status.success(), "unexpected success: {said}");
    assert!(said.contains(expect), "expected {expect:?} in: {said}");
    said
}
fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output));
}
fn commit(root: &Path) {
    git(root, &["add", "-A"]);
    git(
        root,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    );
}
fn repository() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "Fixture"]);
    git(&root, &["config", "user.email", "fixture@example.test"]);
    fs::write(root.join(".gitignore"), ".test-cache/\n.test-tmp/\n").unwrap();
    (temp, root)
}
fn ordinary() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("work");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    (temp, root)
}
fn record(root: &Path) -> String {
    fs::read_to_string(root.join("GROUNDING.yaml")).unwrap()
}
/// A history-backed record born through add, as a new record is.
fn born(root: &Path) {
    for args in [
        vec![
            "add",
            "src.inspection",
            "name=The boiler inspection report",
            "file=inspection.pdf",
            "read=2026-09-20",
        ],
        vec![
            "add",
            "m.boiler_age",
            "v=14",
            "unit=years",
            "name=Age of the boiler",
            "from=src.inspection",
            "at=p.2",
        ],
        vec![
            "add",
            "q.second_boiler",
            "Do we need a second boiler for the annex?",
        ],
        vec![
            "add",
            "d.one_boiler",
            "rests_on=[m.boiler_age]",
            "verdict=one boiler serves the annex until the boiler is 20 years old",
            "because=The inspection puts the boiler at {{m.boiler_age}} years",
            "wrong_if=m.boiler_age >= 20",
            "name=One boiler is enough for now",
        ],
    ] {
        success(run(root, &args));
    }
    assert!(record(root).contains("  history:"), "{}", record(root));
}

#[test]
fn answer_closes_a_question_where_it_stands() {
    let (_temp, root) = ordinary();
    let before = success(run(&root, &["open"]));
    assert!(before.contains("2 open questions"), "{before}");
    success(run(
        &root,
        &[
            "answer",
            "q.second_boiler",
            "d.one_boiler",
            "--why",
            "the inspection settles it",
        ],
    ));
    let file = record(&root);
    for line in [
        "  q.second_boiler:",
        "    question: \"Do we need a second boiler for the annex?\"",
        "    answered:",
        "      by: d.one_boiler",
        "      said: \"one boiler serves the annex until the boiler is 20 years old\"",
        "      because: \"the inspection settles it\"",
    ] {
        assert!(file.contains(line), "missing {line:?} in:\n{file}");
    }
    assert!(
        file.contains("  q.annex_floor: Will the annex add a second floor?"),
        "{file}"
    );
    let after = success(run(&root, &["open"]));
    assert!(after.contains("1 open questions"), "{after}");
    assert!(!after.contains("? q.second_boiler"), "{after}");
    assert!(after.contains("? q.annex_floor"), "{after}");
    let pulled = success(run(&root, &["pull", "q.second_boiler"]));
    assert!(pulled.contains("answered by d.one_boiler"), "{pulled}");
    success(run(&root, &["check"]));
}

#[test]
fn answering_keeps_a_question_s_own_fields() {
    let (_temp, root) = ordinary();
    let file = record(&root).replace(
        "  q.annex_floor: Will the annex add a second floor?\n",
        "  q.annex_floor:\n    name: Annex floor\n    v: Will the annex add a second floor?\n    from: src.inspection\n    at: p.9\n",
    );
    fs::write(root.join("GROUNDING.yaml"), file).unwrap();
    success(run(
        &root,
        &[
            "answer",
            "q.annex_floor",
            "d.one_boiler",
            "--why",
            "one floor keeps one boiler",
        ],
    ));
    let file = record(&root);
    assert!(file.contains("    at: p.9"), "{file}");
    assert!(file.contains("    from: src.inspection"), "{file}");
    assert!(
        file.contains("      because: \"one floor keeps one boiler\""),
        "{file}"
    );
    let pulled = success(run(&root, &["pull", "q.annex_floor"]));
    assert!(
        pulled.contains("Will the annex add a second floor? - answered by d.one_boiler"),
        "{pulled}"
    );
    refused(
        run(
            &root,
            &[
                "answer",
                "q.second_boiler",
                "--dropped",
                "no",
                "--why",
                "no",
            ],
        ),
        "--dropped carries the reason",
    );
}

#[test]
fn a_question_is_dropped_with_its_reason() {
    let (_temp, root) = ordinary();
    success(run(
        &root,
        &[
            "answer",
            "q.annex_floor",
            "--dropped",
            "the extension was cancelled",
        ],
    ));
    let file = record(&root);
    assert!(file.contains("    dropped:"), "{file}");
    assert!(
        file.contains("because: \"the extension was cancelled\""),
        "{file}"
    );
    assert!(!file.contains("answered:"), "{file}");
    let opened = success(run(&root, &["open"]));
    assert!(!opened.contains("? q.annex_floor"), "{opened}");
    let pulled = success(run(&root, &["pull", "q.annex_floor"]));
    assert!(pulled.contains("dropped"), "{pulled}");
}

#[test]
fn answer_refuses_what_it_cannot_close() {
    let (_temp, root) = ordinary();
    let untouched = record(&root);
    refused(
        run(&root, &["answer", "d.one_boiler", "m.boiler_age"]),
        "is not an open question",
    );
    refused(
        run(&root, &["answer", "q.second_boiler", "m.missing"]),
        "m.missing is not an entry",
    );
    refused(
        run(&root, &["answer", "q.second_boiler", "q.annex_floor"]),
        "is a question",
    );
    refused(run(&root, &["answer", "q.second_boiler"]), "--dropped");
    refused(
        run(&root, &["answer", "q.missing", "d.one_boiler"]),
        "q.missing is not an entry",
    );
    assert_eq!(record(&root), untouched);
    success(run(&root, &["answer", "q.second_boiler", "d.one_boiler"]));
    refused(
        run(&root, &["answer", "q.second_boiler", "m.boiler_age"]),
        "already answered",
    );
}

#[test]
fn a_moved_answer_is_flagged_for_a_person_and_stays_answered() {
    let (_temp, root) = ordinary();
    success(run(&root, &["answer", "q.second_boiler", "d.one_boiler"]));
    let file = record(&root).replace(
        "    verdict: one boiler serves the annex until the boiler is 20 years old",
        "    verdict: one boiler serves the annex until the boiler is 25 years old",
    );
    fs::write(root.join("GROUNDING.yaml"), file).unwrap();
    let opened = success(run(&root, &["open"]));
    assert!(
        opened.contains("q.second_boiler: its answer moved - d.one_boiler now says"),
        "{opened}"
    );
    let checked = success(run(&root, &["check"]));
    assert!(
        checked.contains("NOTE q.second_boiler: its answer moved"),
        "{checked}"
    );
    assert!(record(&root).contains("      by: d.one_boiler"));
    let gone = record(&root).replace("  d.one_boiler:", "  d.two_boilers:");
    fs::write(root.join("GROUNDING.yaml"), gone).unwrap();
    let opened = success(run(&root, &["open"]));
    assert!(
        opened.contains("its answer d.one_boiler is no longer an entry"),
        "{opened}"
    );
}

#[test]
fn correct_rewrites_what_no_commit_holds_and_flags_what_rests_on_it() {
    let (_temp, root) = repository();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    commit(&root);
    success(run(
        &root,
        &[
            "add",
            "m.annex_load",
            "v=40",
            "unit=kW",
            "name=Annex heat load",
            "from=src.inspection",
            "at=p.3",
        ],
    ));
    success(run(
        &root,
        &[
            "add",
            "d.annex_heating",
            "rests_on=[m.annex_load, m.boiler_age]",
            "verdict=the annex needs its own heating circuit",
            "because=the load is {{m.annex_load}} kW on a boiler {{m.boiler_age}} years old",
            "wrong_if=m.boiler_age >= 30",
            "name=Annex heating circuit",
        ],
    ));
    success(run(
        &root,
        &["correct", "m.annex_load", "v=45", "--why", "p.3 says 45"],
    ));
    let file = record(&root);
    assert!(file.contains("    v: 45"), "{file}");
    assert!(!file.contains("    v: 40"), "{file}");
    let checked = success(run(&root, &["check"]));
    assert!(checked.contains("MOVED d.annex_heating"), "{checked}");
    let untouched = record(&root);
    refused(
        run(&root, &["correct", "m.boiler_age", "v=15"]),
        "m.boiler_age is already in a commit",
    );
    assert_eq!(record(&root), untouched);
    commit(&root);
    refused(
        run(&root, &["correct", "m.annex_load", "v=46"]),
        "--hypothesis NAME",
    );
}

#[test]
fn correct_refuses_an_unlanded_entry_that_landed_work_rests_on() {
    let (_temp, root) = repository();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    commit(&root);
    success(run(
        &root,
        &[
            "add",
            "m.flue",
            "v=true",
            "name=Flue is clear",
            "from=src.inspection",
            "at=p.4",
        ],
    ));
    let file = record(&root).replace(
        "    rests_on: [m.boiler_age]",
        "    rests_on: [m.boiler_age, m.flue]",
    );
    fs::write(root.join("GROUNDING.yaml"), file).unwrap();
    refused(
        run(&root, &["correct", "m.flue", "v=false"]),
        "d.one_boiler rests on m.flue and is already in a commit",
    );
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    success(run(
        &root,
        &[
            "add",
            "m.flue",
            "v=true",
            "name=Flue is clear",
            "from=src.inspection",
            "at=p.4",
        ],
    ));
    success(run(&root, &["answer", "q.second_boiler", "m.flue"]));
    refused(
        run(&root, &["correct", "m.flue", "v=false"]),
        "q.second_boiler rests on m.flue and is already in a commit",
    );
}

#[test]
fn outside_git_only_the_session_that_wrote_it_corrects_it() {
    let (_temp, root) = ordinary();
    success(run_as(
        &root,
        "session-one",
        &[
            "add",
            "m.annex_load",
            "v=40",
            "unit=kW",
            "name=Annex heat load",
            "from=src.inspection",
            "at=p.3",
        ],
    ));
    refused(
        run(&root, &["correct", "m.annex_load", "v=45"]),
        "no session identity",
    );
    refused(
        run_as(&root, "session-two", &["correct", "m.annex_load", "v=45"]),
        "not written by this session",
    );
    success(run_as(
        &root,
        "session-one",
        &["correct", "m.annex_load", "v=45"],
    ));
    assert!(record(&root).contains("    v: 45"));
    refused(
        run_as(&root, "session-one", &["correct", "m.boiler_age", "v=15"]),
        "not written by this session",
    );
}

#[test]
fn correct_keeps_what_an_entry_is_and_what_the_tool_writes() {
    let (_temp, root) = repository();
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta:\n  updated: '2026-09-20'\n",
    )
    .unwrap();
    commit(&root);
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    let untouched = record(&root);
    refused(
        run(
            &root,
            &["correct", "m.boiler_age", "rests_on=[src.inspection]"],
        ),
        "is not a judgment",
    );
    refused(
        run(&root, &["correct", "d.one_boiler", "--unset", "rests_on"]),
        "rests on something",
    );
    refused(
        run(
            &root,
            &["correct", "d.one_boiler", "seen={m.boiler_age: 15}"],
        ),
        "written by this tool",
    );
    refused(
        run(&root, &["correct", "m.boiler_age", "v=14"]),
        "nothing to correct",
    );
    refused(
        run(&root, &["correct", "m.missing", "v=1"]),
        "m.missing is not an entry",
    );
    assert_eq!(record(&root), untouched);
    success(run(
        &root,
        &["correct", "q.annex_floor", "Will the annex = two floors?"],
    ));
    assert!(
        record(&root).contains("Will the annex = two floors?"),
        "{}",
        record(&root)
    );
    success(run(
        &root,
        &["correct", "d.one_boiler", "wrong_if=m.boiler_age >= 25"],
    ));
    let file = record(&root);
    assert!(file.contains("m.boiler_age >= 25"), "{file}");
    assert!(
        file.contains("seen: {m.boiler_age: 14}") || file.contains("m.boiler_age: 14"),
        "{file}"
    );
}

#[test]
fn correct_leaves_a_hypothesis_to_the_fold() {
    let (_temp, root) = ordinary();
    success(run_as(
        &root,
        "session-one",
        &[
            "add",
            "m.annex_load",
            "v=40",
            "unit=kW",
            "name=Annex heat load",
            "from=src.inspection",
            "at=p.3",
        ],
    ));
    fs::create_dir_all(root.join(".kpopper/hypotheses")).unwrap();
    fs::write(
        root.join(".kpopper/hypotheses/annex_load.yaml"),
        "meta:\n  born: '2026-09-21'\nknown:\n  m.annex_load:\n    v: 48\n    unit: kW\n    from: src.inspection\n    at: p.3\n    of: '2026-09-21'\n",
    )
    .unwrap();
    let untouched = record(&root);
    refused(
        run_as(&root, "session-one", &["correct", "m.annex_load", "v=45"]),
        "a decision for the fold",
    );
    assert_eq!(record(&root), untouched);
}

#[test]
fn history_records_mark_the_corrected_version_and_pin_the_answer() {
    let (_temp, root) = repository();
    born(&root);
    success(run(
        &root,
        &[
            "answer",
            "q.second_boiler",
            "d.one_boiler",
            "--why",
            "the rating settles it",
        ],
    ));
    let pins = walk(&root.join(".kpopper"))
        .into_iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .any(|body| {
            body.contains("act: accept") && body.contains("read:") && body.contains("d.one_boiler")
        });
    assert!(pins, "no accept act pins the answer's version");
    let pulled = success(run(&root, &["--json", "pull", "q.second_boiler"]));
    assert!(pulled.contains("answered"), "{pulled}");
    success(run(
        &root,
        &["correct", "m.boiler_age", "v=15", "--why", "p.2 says 15"],
    ));
    let status = success(run(&root, &["history", "status", "--json"]));
    assert!(status.contains("m.boiler_age"), "{status}");
    let marks = success(run(&root, &["--json", "pull", "m.boiler_age"]));
    assert!(marks.contains("corrected"), "{marks}");
    let opened = success(run(&root, &["check"]));
    assert!(
        opened.contains("0 problems") || opened.contains("problems"),
        "{opened}"
    );
    commit(&root);
    refused(
        run(&root, &["correct", "m.boiler_age", "v=16"]),
        "already in a commit",
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut found = vec![];
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walk(&path));
            } else {
                found.push(path);
            }
        }
    }
    found
}
