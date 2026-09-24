use serde_json::Value;
use std::{fs, path::Path, process::Command};

const RECORD: &str = "meta:\n  name: '<script>title</script>'\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n  s.note: {name: 'A & B', file: 'notes # %.txt'}\n  p.large: {v: 12345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890, from: s.note}\n  p.day: {v: \"2026-09-19\", from: s.note}\n";
fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    let resources = root.join(".test-runtime");
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    if ["GROUNDING.yaml", "PROVENANCE.yaml"].iter().any(|entry| {
        fs::read_to_string(root.join(entry))
            .is_ok_and(|record| !record.contains("profile: core/v1"))
    }) {
        let ordinary = resources.join("ordinary").join(&target);
        fs::create_dir_all(&ordinary).unwrap();
        let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                    .join(".cache/kpopper/lean")
                    .join(&target)
                    .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
            });
        let executable = if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        };
        for name in ["build.json", executable] {
            fs::copy(program.join(name), ordinary.join(name)).unwrap();
        }
    }
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .current_dir(root)
        .args(args)
        .env("KPOPPER_NATIVE_RESOURCES", resources)
        .env("KPOPPER_NATIVE_CACHE", root.join(".test-cache"))
        .env_remove("KPOPPER_READ_MODE")
        .env("XDG_STATE_HOME", root.join("private-state"))
        .output()
        .unwrap()
}

const ORDINARY: &str = "meta:\n  name: Ordinary <record>\nsources:\n  s.note: {name: 'Source & note', file: 'notes # %.html'}\nknown:\n  p.load: {v: 61, from: s.note}\n  p.enabled: {v: true}\njudgments:\n  d.work:\n    verdict: Keep the current format\n    rests_on: [p.load, p.enabled]\n    seen: {p.load: 44, p.enabled: true}\n    wrong_if: p.load > 80\n";

#[test]
fn ordinary_hub_builds_arranged_typed_interactive_page_and_verifies_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), ORDINARY).unwrap();
    fs::write(root.join("notes # %.html"), "source").unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/view.yaml"), "title: Current view\nsections:\n- title: Decision\n  pick: judgments\n  as: cards\n- title: Inputs\n  pick: p\n  as: table\n- title: Sources\n  pick: s\n  as: links\n").unwrap();
    let verified = ok(root, &["--frozen", "page", "--verify"]);
    assert!(
        String::from_utf8_lossy(&verified.stdout)
            .contains("4 elements, 3 entries, 1 judgments, 2 tabs, 0 problems")
    );
    assert!(!root.join("record.html").exists());
    ok(root, &["--frozen", "page", "--out", "site/page.html"]);
    let html = fs::read_to_string(root.join("site/page.html")).unwrap();
    assert!(html.contains("Ordinary &lt;record&gt;"));
    assert!(html.contains("data-tab=\"now\""));
    assert!(html.contains("class=\"card\""));
    assert!(html.contains("data-id=\"d.work\""));
    assert!(html.contains("data-id=\"p.enabled\">yes"));
    assert!(html.contains("../notes%20%23%20%25.html"));
    assert!(html.contains("window.__E="));
    assert!(html.contains("window.__J="));
    assert!(html.contains("<svg viewBox=\"0 0 960 700\""));
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        ORDINARY
    );
    assert!(
        !cli(root, &["--frozen", "page", "--out", "notes # %.html"])
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(root.join("notes # %.html")).unwrap(),
        "source"
    );
}

#[test]
fn ordinary_hub_uses_reader_flags_for_muted_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta: {name: Muted move}\nknown:\n  p.load: {v: 61}\n  p.flagcount: {rule: graph.flagged}\njudgments:\n  d.work:\n    verdict: Keep the current format\n    rests_on: [p.load]\n    seen: {p.load: 44}\n    wrong_if: p.load > 80\n",
    )
    .unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/view.yaml"),
        "title: Current view\nshape: {entries: 2, judgments: 1, flagged: 0, blocked: 0}\nsections:\n- {title: Inputs, why: input, pick: p, as: table}\n",
    )
    .unwrap();
    let verified = cli(root, &["--frozen", "page", "--verify"]);
    assert!(verified.status.success());
    let output = String::from_utf8(verified.stdout).unwrap();
    assert!(
        output.ends_with("4 elements, 3 entries, 1 judgments, 2 tabs, 0 problems\n"),
        "{output}"
    );
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(!html.contains("data-review=\"moved\""));
    assert!(!html.contains("Flagged outside the arrangement"));
}

#[test]
fn ordinary_hub_selects_and_spills_blocked_judgments() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta: {name: Declared hole}\nschema: {deps: rests_on, snapshot: seen, predicate: wrong_if}\nknown:\n  p.load: {v: 61}\njudgments:\n  d.wait:\n    verdict: Hold until the survey lands\n    rests_on: [p.load, p.survey]\n    seen: {p.load: 61}\n    wrong_if: p.load > 80\n    blocked_on: the survey has not been run\n",
    )
    .unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    let view = root.join(".kpopper/view.yaml");
    fs::write(
        &view,
        "title: Holes\nsections:\n- {title: Declared holes, why: waiting, pick: blocked, as: cards}\n",
    )
    .unwrap();
    let verified = cli(root, &["--frozen", "page", "--verify"]);
    assert!(verified.status.success());
    let output = String::from_utf8(verified.stdout).unwrap();
    assert!(
        output.ends_with("2 elements, 1 entries, 1 judgments, 2 tabs, 0 problems\n"),
        "{output}"
    );
    ok(root, &["--frozen", "page", "--out", "selected.html"]);
    let selected = fs::read_to_string(root.join("selected.html")).unwrap();
    assert!(selected.contains("data-id=\"d.wait\""));
    assert!(!selected.contains("Flagged outside the arrangement"));

    fs::write(
        &view,
        "title: Inputs\nsections:\n- {title: Inputs, why: input, pick: p, as: table}\n",
    )
    .unwrap();
    ok(root, &["--frozen", "page", "--out", "spilled.html"]);
    let spilled = fs::read_to_string(root.join("spilled.html")).unwrap();
    assert!(spilled.contains("Flagged outside the arrangement <span class=\"n\">1</span>"));
    assert!(spilled.contains("data-id=\"d.wait\""));
}

#[test]
fn ordinary_hub_reports_stale_shape_as_note_without_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta: {name: R}\nsources:\n  s.ask: {asked: 'What now?', read: '2026-09-03'}\nknown:\n  p.a: {v: 1, from: s.ask}\n  p.b: {v: 2}\n  dates.d: {v: 2026-12-31}\njudgments:\n  d.x: {verdict: Go, rests_on: [p.a], seen: {p.a: 1}, wrong_if: 'p.a > 9'}\n",
    )
    .unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/view.yaml"),
        "shape: {entries: 3, judgments: 0, flagged: 7, blocked: 0}\nsections:\n- {title: In, why: w, pick: p, as: table}\n",
    )
    .unwrap();
    let verified = cli(root, &["--frozen", "page", "--verify"]);
    assert!(verified.status.success());
    let output = String::from_utf8(verified.stdout).unwrap();
    assert!(
        output.ends_with("5 elements, 4 entries, 1 judgments, 2 tabs, 0 problems\n"),
        "{output}"
    );
    assert!(output.contains("NOTE the brief recorded a different shape: entries: 3 -> 4; judgments: 0 -> 1; flagged: 7 -> 0\n"), "{output}");
    assert!(output.contains("NOTE no arrangement decision is recorded, so the brief is held against none - an arrangement is a judgment resting on the session sources a tab serves, with a sign over a count: add v.<slug> rests_on=[s.<...>, page.unserved] verdict=... wrong_if='page.unserved > 0'\n"), "{output}");
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    assert!(
        fs::read_to_string(root.join("page.html"))
            .unwrap()
            .contains("the brief recorded a different shape: entries: 3 -&gt; 4")
    );
}

#[test]
fn ordinary_hub_preserves_every_declared_component_surface() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), "meta: {name: Components}\nsources:\n  s.docs: {name: Docs, url: 'https://example.com/a b'}\nknown:\n  p.one: {v: 1}\n  p.two: {v: 2}\n  q.three: {v: 3}\n  dates.deadline: {v: 2026-12-31}\njudgments:\n  d.choice: {verdict: Choose, rests_on: [p.one], seen: {p.one: 1}, wrong_if: 'p.one > 4'}\n").unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/view.yaml"), "groups:\n  Left: [p]\n  Right: [q]\ntabs:\n- title: Components\n  sections:\n  - {title: Table, pick: p, as: table}\n  - {title: Lines, pick: p, as: lines}\n  - {title: Cards, pick: judgments, as: cards}\n  - {title: Timeline, pick: dates, as: timeline}\n  - {title: Headline, pick: p, as: headline}\n  - {title: Grouped, pick: [p, q], as: grouped}\n  - {title: Fronts, pick: [p, q], as: fronts}\n  - {title: Alerts, pick: judgments, as: alerts}\n  - {title: Axis, pick: p, as: axis, text: 'p.one -> p.two'}\n  - {title: Links, pick: s, as: links}\n").unwrap();
    ok(root, &["--frozen", "page", "--out", "components.html"]);
    let html = fs::read_to_string(root.join("components.html")).unwrap();
    for component in [
        "table", "lines", "cards", "timeline", "headline", "grouped", "fronts", "alerts", "axis",
        "links",
    ] {
        assert!(
            html.contains(&format!("data-component=\"{component}\"")),
            "{component}"
        );
    }
    for class in [
        "<table>",
        "class=\"deps\"",
        "class=\"card\"",
        "class=\"tl\"",
        "class=\"heads\"",
        "class=\"grid\"",
        "class=\"alerts\"",
        "class=\"axis\"",
        "class=\"links\"",
    ] {
        assert!(html.contains(class), "{class}");
    }
    assert!(html.contains("href=\"https://example.com/a%20b\""));
}

#[test]
fn ordinary_hub_draws_arrangement_history_from_reader_semantics() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"),"meta: {name: Arrangement}\nsources:\n  s.request: {asked: 'What should this page show?', read: 2026-09-03}\nknown:\n  p.answer: {v: 2, from: s.request}\njudgments:\n  v.layout:\n    verdict: Keep this tab\n    rests_on: [s.request, page.unserved]\n    wrong_if: page.unserved > 0\n    born: 2026-09-03\n    request: s.request\n    seen: {s.request: 'read 2026-09-03', page.unserved: 0}\n").unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/view.yaml"),
        "title: Arrangement\nsections:\n- {title: Answer, pick: p, as: table}\n",
    )
    .unwrap();
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(html.contains("decided <span class=\"fx\" data-id=\"v.layout\">2026-09-03</span>"));
    assert!(html.contains("data-request=\"s.request\">What should this page show?</span>"));
}
const STRING_QUESTION: &str = "sources:\n  s.a: {name: A, read: \"2026-09-01\"}\nknown:\n  x.one: {v: 1, from: s.a}\nopen:\n  q.second: \"is a second boiler cheaper?\"\n";

#[test]
fn ordinary_hub_draws_a_string_open_question_as_its_value() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), STRING_QUESTION).unwrap();
    let verified = ok(root, &["--frozen", "experimental", "hub", "--verify"]);
    assert_eq!(
        String::from_utf8(verified.stdout).unwrap(),
        "3 elements, 3 entries, 0 judgments, 1 tab, 0 problems\n"
    );
    ok(
        root,
        &["--frozen", "experimental", "hub", "--out", "page.html"],
    );
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(html.contains("data-id=\"q.second\">is a second boiler cheaper?</span>"));
    assert!(html.contains("\"q.second\":{\"used\":[],\"v\":\"is a second boiler cheaper?\"}"));
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        STRING_QUESTION
    );
}

#[test]
fn ordinary_hub_without_a_brief_opens_on_the_record_tab() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let record = include_str!("../../examples/launch-party/GROUNDING.yaml");
    fs::write(root.join("GROUNDING.yaml"), record).unwrap();
    let verified = ok(root, &["--frozen", "experimental", "hub", "--verify"]);
    assert_eq!(
        String::from_utf8(verified.stdout).unwrap(),
        "3 elements, 2 entries, 1 judgments, 1 tab, 0 problems\n"
    );
    ok(
        root,
        &["--frozen", "experimental", "hub", "--out", "page.html"],
    );
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(!html.contains("data-tab=\"now\""));
    assert!(!html.contains("panel-now"));
    assert!(html.contains(
        "<div class=\"tabs\" role=\"tablist\"><button type=\"button\" data-tab=\"record\" aria-selected=\"true\">"
    ));
    assert!(html.contains("<section id=\"panel-record\">"));
    assert!(html.contains("<section id=\"panel-tree\" hidden>"));
}

#[test]
fn ordinary_hub_without_a_brief_says_what_needs_a_person_on_the_record_cards() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        "sources:\n  s.2026_09_20_ask: {asked: 'What should the page show?', read: '2026-09-20'}\nknown:\n  p.one: {v: 1, from: s.2026_09_20_ask}\njudgments:\n  v.layout:\n    verdict: Keep the page as it is\n    rests_on: [s.2026_09_20_ask, graph.entries]\n    seen: {s.2026_09_20_ask: 'read 2026-09-20', graph.entries: 2}\n    wrong_if: graph.entries > 0\n    born: '2026-09-20'\n  d.wait:\n    verdict: Hold for the survey\n    rests_on: [p.one]\n    seen: {p.one: 2}\n    wrong_if: p.one > 5\n    unverified: nobody has read the survey\n",
    )
    .unwrap();
    // Without a brief no arrangement is held against the page: the fired one is the
    // record's to report, and its card says so.
    let verified = ok(root, &["--frozen", "experimental", "hub", "--verify"]);
    assert_eq!(
        String::from_utf8(verified.stdout).unwrap(),
        "5 elements, 3 entries, 2 judgments, 1 tab, 0 problems\n"
    );
    ok(
        root,
        &["--frozen", "experimental", "hub", "--out", "page.html"],
    );
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(!html.contains("decided <span class=\"fx\" data-id=\"v.layout\">"));
    assert!(html.contains("data-id=\"v.layout\" dir=\"auto\">Keep the page as it is</div><div class=\"state\" data-warning=\"true\">its own condition for being wrong now holds</div>"));
    assert!(html.contains(
        "<div class=\"state\" data-warning=\"true\">unverified: nobody has read the survey</div>"
    ));
    assert_eq!(html.matches("class=\"state\"").count(), 2);
}

#[test]
fn ordinary_hub_verifies_the_page_fixture_and_its_string_open_question() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/page");
    for file in fs::read_dir(&fixture).unwrap() {
        let file = file.unwrap();
        fs::copy(file.path(), root.join(file.file_name())).unwrap();
    }
    let record = fs::read_to_string(root.join("PROVENANCE.yaml")).unwrap();
    assert!(record.contains(
        "  q.second_boiler: \"is a second boiler cheaper than glazing the north wall?\"\n"
    ));
    let verified = ok(root, &["--frozen", "experimental", "hub", "--verify"]);
    let stdout = String::from_utf8(verified.stdout).unwrap();
    assert_eq!(
        stdout,
        r#"NOTE a dump with a heading: What the quote buys
NOTE 1 entries carry no human name, so the page has to fall back to generic labels: q.second_boiler
NOTE 6 of 16 live dependencies are named in the prose that cites them; the rest are reachable only by hovering the judgment
NOTE coverage: 10 covered · spill 0 · 0 intents no tab serves · 0 recent in a row · drift 0.0 since 2026-09-03
NOTE tab 'The February night' serves s.2026_09_02_heating: picks 3 of 3 they recorded
NOTE tab 'The glazing quote' serves s.2026_09_03_glazing: picks 3 of 4 they recorded
NOTE no section picks: doc, q, s
17 elements, 14 entries, 3 judgments, 3 tabs, 0 problems
"#
    );
    ok(
        root,
        &["--frozen", "experimental", "hub", "--out", "page.html"],
    );
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(html.contains(
        "data-id=\"q.second_boiler\">is a second boiler cheaper than glazing the north wall?</span>"
    ));
    assert_eq!(
        fs::read_to_string(root.join("PROVENANCE.yaml")).unwrap(),
        record
    );
}

#[test]
fn ordinary_hub_fails_a_section_only_when_its_selectors_pick_nothing_together() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta: {name: Sections}\nknown:\n  p.load: {v: 61}\n",
    )
    .unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    let verify = |view: &str| {
        fs::write(root.join(".kpopper/view.yaml"), view).unwrap();
        let verified = cli(root, &["--frozen", "experimental", "hub", "--verify"]);
        let lines = String::from_utf8(verified.stdout)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with("NOTE "))
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        (verified.status.code(), lines)
    };
    assert_eq!(
        verify("tabs:\n- title: Inputs\n  sections:\n  - {title: Mixed, pick: [p., scope.], as: table}\n  - {title: Gone, pick: [scope., name.], as: table}\n"),
        (
            Some(1),
            "FAIL section 'Gone' (tab 'Inputs') picks nothing - it is about something the record no longer holds\n1 elements, 1 entries, 0 judgments, 2 tabs, 1 problems\n".into()
        )
    );
    assert_eq!(
        verify("sections:\n- {title: Gone, pick: [scope.], as: table}\n- {title: Blank}\n"),
        (
            Some(1),
            "FAIL section 'Gone' picks nothing - it is about something the record no longer holds\nFAIL section 'Blank' picks nothing - it is about something the record no longer holds\n1 elements, 1 entries, 0 judgments, 2 tabs, 2 problems\n".into()
        )
    );
    assert_eq!(
        verify("sections:\n- {title: Mixed, pick: [p., scope.], as: table}\n"),
        (
            Some(0),
            "1 elements, 1 entries, 0 judgments, 2 tabs, 0 problems\n".into()
        )
    );
}

#[test]
fn arrangement_write_reads_the_page_of_a_record_with_a_string_open_question() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        "sources:\n  s.2026_09_20_ask: {asked: 'What should the page show?', read: '2026-09-20'}\nknown:\n  x.one: {v: 1, from: s.2026_09_20_ask}\nopen:\n  q.second: \"is a second boiler cheaper?\"\n",
    )
    .unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/view.yaml"),
        "tabs:\n- title: Answer\n  serves: [s.2026_09_20_ask]\n  sections:\n  - {title: Values, pick: x, as: table}\n",
    )
    .unwrap();
    ok(
        root,
        &[
            "add",
            "v.layout",
            "verdict=Keep the answer tab",
            "rests_on=[s.2026_09_20_ask, page.unserved]",
            "wrong_if=page.unserved > 0",
            "from=s.2026_09_20_ask",
            "--as-of",
            "2026-09-20",
        ],
    );
    let record = fs::read_to_string(root.join("GROUNDING.yaml")).unwrap();
    assert!(record.contains("  v.layout:\n"), "{record}");
    assert!(record.contains("  q.second: \"is a second boiler cheaper?\"\n"));
}
fn ok(root: &Path, args: &[&str]) -> std::process::Output {
    let result = cli(root, args);
    assert!(
        result.status.success(),
        "{args:?}: {} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}
fn envelope(html: &str) -> Value {
    serde_json::from_str(
        html.split("id=\"kpopper-page-assessment\">")
            .nth(1)
            .unwrap()
            .split("</script>")
            .next()
            .unwrap(),
    )
    .unwrap()
}
#[test]
fn hub_builds_captured_core_page_and_alias_preserves_exact_display_and_source_links() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    fs::write(root.join("notes # %.txt"), "source").unwrap();
    let out = "pages # % שלום/report.html";
    ok(root, &["--frozen", "experimental", "hub", "--out", out]);
    let html = fs::read_to_string(root.join(out)).unwrap();
    assert!(html.contains("&lt;script&gt;title&lt;/script&gt;"));
    assert!(html.contains("2026-09-19"));
    assert!(html.contains("123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890".get(..110).unwrap()));
    assert!(html.contains("../notes%20%23%20%25.txt"));
    let first = envelope(&html);
    ok(root, &["--frozen", "page", "--out", out]);
    assert_eq!(
        envelope(&fs::read_to_string(root.join(out)).unwrap()),
        first
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        RECORD
    );
    assert!(!root.join(".kpopper/build").exists());
}
#[test]
fn verify_is_read_only_and_default_output_ignores_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    let result = ok(root, &["experimental", "hub", "--verify"]);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("3 elements, 3 entries, 0 judgments, 1 tabs, 0 problems")
    );
    assert!(!root.join(".kpopper").exists());
    ok(root, &["page"]);
    assert!(root.join(".kpopper/build/page.html").is_file());
    assert_eq!(
        fs::read_to_string(root.join(".kpopper/build/.gitignore")).unwrap(),
        "*\n"
    );
}
#[test]
fn malformed_page_and_unsafe_destination_preserve_existing_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    fs::write(root.join("saved.html"), "retained").unwrap();
    fs::create_dir_all(root.join(".kpopper")).unwrap();
    fs::write(root.join(".kpopper/view.yaml"), "not: [valid").unwrap();
    let result = cli(root, &["page", "--out", "saved.html"]);
    assert!(!result.status.success());
    assert_eq!(
        fs::read_to_string(root.join("saved.html")).unwrap(),
        "retained"
    );
    fs::remove_file(root.join(".kpopper/view.yaml")).unwrap();
    assert!(
        !cli(root, &["page", "--out", "GROUNDING.yaml"])
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(root.join("GROUNDING.yaml")).unwrap(),
        RECORD
    );
}
#[cfg(unix)]
#[test]
fn output_parent_symlinks_keep_working_links_but_leaf_symlinks_are_refused() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), RECORD).unwrap();
    fs::create_dir_all(root.join("real/nested")).unwrap();
    symlink(root.join("real/nested"), root.join("alias")).unwrap();
    ok(root, &["page", "--out", "alias/page.html"]);
    let html = fs::read_to_string(root.join("real/nested/page.html")).unwrap();
    assert!(html.contains("../../notes%20%23%20%25.txt"));
    symlink(root.join("real/nested/page.html"), root.join("link.html")).unwrap();
    assert!(!cli(root, &["page", "--out", "link.html"]).status.success());
    assert_eq!(
        fs::read_to_string(root.join("real/nested/page.html")).unwrap(),
        html
    );
}

#[test]
fn hub_does_not_overwrite_a_linked_source_document() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for reference in [
        "file: 'source.html'",
        "file: 'missing/../source.html'",
        "file: 'source.html', url: 'https://example.com/report'",
    ] {
        fs::write(
            root.join("GROUNDING.yaml"),
            RECORD.replace("file: 'notes # %.txt'", reference),
        )
        .unwrap();
        fs::write(root.join("source.html"), "original source").unwrap();
        let result = cli(root, &["page", "--out", "source.html"]);
        assert!(!result.status.success(), "{reference}");
        assert_eq!(
            fs::read_to_string(root.join("source.html")).unwrap(),
            "original source",
            "{reference}"
        );
    }
}

#[test]
fn hub_protects_declared_files_in_record_and_also_containers() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for section in ["also", "record"] {
        fs::write(root.join("source.html"), "original source").unwrap();
        fs::write(root.join("GROUNDING.yaml"), format!("meta:\n  reasoning: {{version: 1, profile: core/v1, requires: [arithmetic/v1]}}\n{section}:\n  local: {{file: source.html}}\nknown:\n  p.a: {{v: 1}}\n")).unwrap();
        let result = cli(root, &["page", "--out", "source.html"]);
        assert!(
            !result.status.success(),
            "{section}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            fs::read_to_string(root.join("source.html")).unwrap(),
            "original source"
        );
    }
}

const COUNTED_ARRANGEMENT: &str = "meta: {name: Page counts}\nsources:\n  s.request: {name: Request, asked: Show the current reading, read: 2026-09-03}\nknown:\n  p.answer: {name: Answer, v: 2, from: s.request}\n  page.unserved: {name: Unserved, v: 0}\njudgments:\n  v.layout:\n    verdict: Keep the reading together\n    rests_on: [s.request, page.unserved]\n    seen: {s.request: 'read 2026-09-03', page.unserved: 0}\n    wrong_if: page.unserved > 0\n    born: 2026-09-03\n";

#[test]
fn ordinary_hub_draws_flags_after_page_counts_for_cards_selectors_spill_and_alerts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), COUNTED_ARRANGEMENT).unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    for (pick, renderer) in [
        ("falsified", "cards"),
        ("flagged", "alerts"),
        ("p", "table"),
    ] {
        fs::write(root.join(".kpopper/view.yaml"), format!("title: Current reading\nsections:\n- {{title: Review, why: Current state, pick: {pick}, as: {renderer}}}\n")).unwrap();
        let verified = cli(root, &["--frozen", "page", "--verify"]);
        let output = String::from_utf8(verified.stdout).unwrap();
        assert_eq!(verified.status.code(), Some(1), "{output}");
        assert!(
            output.contains(
                "FAIL v.layout: wrong_if holds (page.unserved > 0) - decided by the page"
            ),
            "{output}"
        );
        assert!(!output.contains("picks nothing"), "{output}");
        ok(root, &["--frozen", "page", "--out", "page.html"]);
        let html = fs::read_to_string(root.join("page.html")).unwrap();
        let now = html
            .split("<section id=\"panel-now\"")
            .nth(1)
            .unwrap()
            .split("<section id=\"panel-record\"")
            .next()
            .unwrap();
        assert!(now.contains("data-id=\"v.layout\""), "{now}");
        assert!(
            now.contains("its own condition for being wrong now holds"),
            "{now}"
        );
        assert_eq!(now.contains("class=\"spill\""), pick == "p");
    }
}

#[test]
fn ordinary_hub_record_groups_names_references_and_heading_match_python() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), "meta: {name: Record, scope: Current scope, updated: 2026-09-24, prefixes: {p: parameter, s: source}}\nsources:\n  s.reading: {name: Reading}\nknown:\n  p.one: {name: First, v: 100}\n  p.two: {name: Second, v: 200}\njudgments:\n  d.choice:\n    verdict: Keep p.one\n    because: '{{p.two}} supports p.one'\n    rests_on: [p.one, p.two]\n    seen: {p.one: 100, p.two: 200}\n    wrong_if: p.one > 150\n").unwrap();
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    for expected in [
        "<nav class=\"ns\" dir=\"ltr\"><a href=\"#g-p\" title=\"p.\">parameter (2)</a><a href=\"#g-s\" title=\"s.\">source (1)</a></nav>",
        "<h2 id=\"g-p\" title=\"p.\">parameter</h2>",
        "<p class=\"scope\" dir=\"auto\">Current scope</p>",
        "3 entries and 1 judgments. Last updated 2026-09-24",
        "Keep <span class=\"fx in\" data-id=\"p.one\">First</span>",
        "<div class=\"bc\" dir=\"auto\"><span class=\"fx in\" data-id=\"p.two\">200</span> supports <span class=\"fx in\" data-id=\"p.one\">First</span></div>",
        "Generated from the record - nothing here was typed twice.",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[cfg(unix)]
#[test]
fn hub_output_is_readable_by_other_accounts_when_created_and_replaced() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), ORDINARY).unwrap();
    let output = root.join("page.html");
    for _ in 0..2 {
        ok(root, &["--frozen", "page", "--out", "page.html"]);
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o644
        );
        fs::set_permissions(&output, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[test]
fn ordinary_hub_notes_distinguish_written_and_resolved_reasoning_lengths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let written = "Long words ".repeat(45);
    let quote = "א".repeat(450);
    fs::write(root.join("GROUNDING.yaml"), format!("meta: {{name: Reasoning}}\nknown:\n  p.quote: {{name: Quote, v: '{quote}'}}\n  p.load: {{name: Load, v: 1}}\njudgments:\n  d.cut:\n    verdict: Keep\n    because: '{written}'\n    rests_on: [p.load]\n    seen: {{p.load: 1}}\n    wrong_if: p.load > 3\n  d.resolved:\n    verdict: Keep\n    because: '{{{{p.quote}}}}'\n    rests_on: [p.quote, p.load]\n    seen: {{p.quote: '{quote}', p.load: 1}}\n    wrong_if: p.load > 3\n")).unwrap();
    let output = String::from_utf8(ok(root, &["--frozen", "page", "--verify"]).stdout).unwrap();
    assert!(output.contains("NOTE 1 reasoning longer than the 400 characters a card carries; each is drawn to its last whole word and marked. Longest first: d.cut (495)\n"), "{output}");
    assert!(output.contains("NOTE 1 reasoning within 400 characters as written and past them once their references resolve; what each names is long, so the card is drawn whole. Longest first: d.resolved (450)\n"), "{output}");
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(!html.contains(&format!("<div class=\"bc\" dir=\"auto\">{written}")));
    assert!(html.contains(&format!("data-id=\"p.quote\">{quote}</span></div>")));
    assert!(html.contains("words…</div>"));
}

#[test]
fn ordinary_hub_keeps_movement_and_container_source_order_on_the_record() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.reading: {name: Reading, v: 20}\n  p.limit: {name: Limit, v: 100}\n  p.details: {name: Details, v: {z: 1, a: [2, 3]}}\njudgments:\n  d.keep:\n    verdict: 'Keep {{p.reading}}'\n    rests_on: [p.reading, p.limit]\n    seen: {p.reading: 10, p.limit: 100}\n    wrong_if: p.limit > 200\n").unwrap();
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    for expected in [
        "class=\"card moved\" data-judgment=\"d.keep\" data-review=\"moved\"",
        "class=\"fx in mv\" data-id=\"p.reading\" title=\"was 10 when this was reviewed\">20</span>",
        "<div class=\"mvd\" dir=\"auto\">Moved since this was reviewed: Reading 10 &rarr; 20</div>",
        "data-id=\"p.details\">{&#x27;z&#x27;: 1, &#x27;a&#x27;: [2, 3]}</span>",
    ] {
        assert!(html.contains(expected), "missing {expected}");
    }
}

#[test]
fn ordinary_hub_reports_undecidable_page_counts_after_counting() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(
        root.join("GROUNDING.yaml"),
        COUNTED_ARRANGEMENT
            .replace("page.unserved", "page.drift")
            .replace("    born: 2026-09-03\n", ""),
    )
    .unwrap();
    fs::create_dir(root.join(".kpopper")).unwrap();
    fs::write(
        root.join(".kpopper/view.yaml"),
        "sections:\n- {title: Undecided, why: Unavailable counts, pick: unknown, as: cards}\n",
    )
    .unwrap();
    let verified = String::from_utf8(ok(root, &["--frozen", "page", "--verify"]).stdout).unwrap();
    assert!(!verified.contains("picks nothing"), "{verified}");
    assert!(verified.contains("drift -"), "{verified}");
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(html.contains("its condition cannot currently be evaluated"));
}

#[test]
fn ordinary_hub_float_references_use_ten_significant_digits() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::write(root.join("GROUNDING.yaml"), "known:\n  p.precise: {name: Precise, v: 1.23456789123}\njudgments:\n  d.keep:\n    verdict: 'Keep {{p.precise}}'\n    rests_on: [p.precise]\n    seen: {p.precise: 1.23456789123}\n    wrong_if: p.precise > 2\n").unwrap();
    ok(root, &["--frozen", "page", "--out", "page.html"]);
    let html = fs::read_to_string(root.join("page.html")).unwrap();
    assert!(html.contains("Keep <span class=\"fx in\" data-id=\"p.precise\">1.234567891</span>"));
}
