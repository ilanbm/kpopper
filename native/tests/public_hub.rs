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
            .join(format!("{target}.zip")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    if fs::read_to_string(root.join("GROUNDING.yaml"))
        .is_ok_and(|record| !record.contains("profile: core/v1"))
    {
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
        for name in ["build.json", "epistemic-core"] {
            fs::copy(program.join(name), ordinary.join(name)).unwrap();
        }
    }
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
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
    fs::write(root.join(".kpopper/view.yaml"), "title: Current view\nsections:\n- title: Decision\n  pick: judgments\n  as: cards\n- title: Inputs\n  pick: p\n  as: table\n").unwrap();
    let verified = ok(root, &["--frozen", "page", "--verify"]);
    assert!(
        String::from_utf8_lossy(&verified.stdout)
            .contains("4 elements, 3 entries, 1 judgments, 3 tabs, 0 problems")
    );
    assert!(!root.join("record.html").exists());
    ok(root, &["--frozen", "page", "--out", "site/page.html"]);
    let html = fs::read_to_string(root.join("site/page.html")).unwrap();
    assert!(html.contains("Ordinary &lt;record&gt;"));
    assert!(html.contains("data-tab=\"now\""));
    assert!(html.contains("class=\"card\""));
    assert!(html.contains("data-id=\"d.work\""));
    assert!(html.contains("data-id=\"p.enabled\">True"));
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
