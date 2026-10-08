use kpop_native::{tokenizer::Encoding, value::TypedValue};
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn fixture(large: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let mut record = String::from(
        "meta:\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n",
    );
    for (id, word) in [
        ("p.anchor", "orchard"),
        ("p.global", "needle"),
        ("p.other", "unrelated"),
    ] {
        record.push_str(&format!(
            "  {id}: {{v: '{}'}}\n",
            word.repeat(if large { 350 } else { 1 })
        ));
    }
    if large {
        for index in 0..10 {
            record.push_str(&format!(
                "  p.filler{index}: {{v: '{}'}}\n",
                "background ".repeat(400)
            ));
        }
    }
    fs::write(root.path().join("GROUNDING.yaml"), record).unwrap();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    fs::create_dir_all(root.path().join("resources/reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        root.path()
            .join("resources/reasoning")
            .join(format!("{target}.zip")),
    )
    .unwrap();
    root
}

fn cli(root: &Path, operation: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kpop"));
    command
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "--frozen",
            "session",
            operation,
            "--no-settings",
            "--input",
            "GROUNDING.yaml",
            "--project",
            "anchor-cli",
            "--state",
            "state",
            "--assessment-profile",
            "core/v1",
        ])
        .env("KPOPPER_NATIVE_RESOURCES", root.join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"));
    command
}

fn ok(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn revision(root: &Path) -> String {
    let text = ok(cli(root, "open")
        .args(["--tokens", "16000"])
        .output()
        .unwrap());
    text.lines()
        .find_map(|line| line.strip_prefix("project=anchor-cli revision="))
        .unwrap()
        .to_owned()
}

fn body<'a>(packet: &'a Value, id: &str) -> Option<&'a Value> {
    packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| packet["dictionary"][row[0].as_str().unwrap()]["original"] == id)
        .map(|row| &row[1])
}

#[test]
fn unavailable_anchors_refuse_even_on_empty_query_and_complete_view_fast_paths() {
    let root = fixture(false);
    let revision = revision(root.path());
    for query in ["", "orchard"] {
        for anchor in ["p.missing", "n0"] {
            let output = cli(root.path(), "view")
                .args([
                    "--revision",
                    &revision,
                    "--query",
                    query,
                    "--tokens",
                    "16000",
                    "--anchor",
                    anchor,
                ])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("anchor unavailable"));
        }
    }
    let output = cli(root.path(), "open")
        .args(["--anchor", "p.anchor"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("only supported by session view"));
    let output = cli(root.path(), "enable")
        .args(["--anchor", "p.anchor"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("only supported by session view"));
}

#[test]
fn anchor_only_and_explicit_reread_preserve_bodies_and_budgeted_trace() {
    let root = fixture(true);
    let revision = revision(root.path());
    for explicit in [false, true] {
        let mut command = cli(root.path(), "view");
        command.args([
            "--revision",
            &revision,
            "--anchor",
            "p.anchor",
            "--tokens",
            "3500",
            "--max-view-bytes",
            "20000",
            "--view-format",
            "checked-text-tagged",
        ]);
        if explicit {
            command.args(["--id", "p.global"]);
        }
        let text = ok(command.output().unwrap());
        assert!(text.len() <= 20000);
        assert!(Encoding::O200kBase.count(&text) <= 3500);
        let packet = kpop_native::view_format::decode_checked_text(&text).unwrap();
        let received_anchor = body(&packet, "p.anchor");
        if !explicit {
            assert!(
                received_anchor.is_some(),
                "anchor-only navigation must select the fitting anchor"
            );
        }
        if let Some(value) = received_anchor {
            let anchor_body = TypedValue::from_tagged(value).unwrap().to_json().unwrap();
            assert_eq!(anchor_body["v"], "orchard".repeat(350));
        }
        if explicit {
            assert!(body(&packet, "p.global").is_some());
        }
        assert_eq!(packet["selection"]["anchor_ranking"]["anchor_count"], 1);
        assert_eq!(
            packet["selection"]["anchor_ranking"]["anchors"][0],
            "p.anchor"
        );
        let traced_anchor = packet["selection"]["anchor_ranking"]["ranking"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "p.anchor")
            .unwrap();
        assert_eq!(traced_anchor["selected"], received_anchor.is_some());
        assert!(
            packet["selection"]["anchor_ranking"]["ranking"]
                .as_array()
                .unwrap()
                .len()
                <= 24
        );
    }
}

#[test]
fn full_fit_records_anchor_input_and_no_anchor_keeps_existing_selection_policy() {
    let root = fixture(false);
    let revision = revision(root.path());
    let text = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--query",
            "orchard",
            "--tokens",
            "16000",
        ])
        .output()
        .unwrap());
    let plain: Value = serde_json::from_str(&text).unwrap();
    assert!(plain["selection"].get("anchor_ranking").is_none());
    let text = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--query",
            "orchard",
            "--tokens",
            "16000",
            "--anchor",
            "p.anchor",
        ])
        .output()
        .unwrap());
    let anchored: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(plain["nodes"], anchored["nodes"]);
    assert_eq!(anchored["selection"]["anchor_ranking"]["anchor_count"], 1);
    assert_eq!(anchored["selection"]["scope"], plain["selection"]["scope"]);
    assert_eq!(anchored["selection"]["unread_candidates"], 0);
    assert!(anchored["selection"].get("next").is_none());
    assert!(
        anchored["selection"]["anchor_ranking"]["ranking"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| !row["unanchored_rank"].is_null())
    );
    assert_eq!(
        anchored["selection"]["policy"],
        "complete-view-fits-budget/v2"
    );
}

#[test]
fn anchor_does_not_add_a_token_cap_to_unbounded_explicit_reread() {
    let root = fixture(false);
    let mut record = fs::read_to_string(root.path().join("GROUNDING.yaml")).unwrap();
    record.push_str(&format!(
        "  p.huge: {{v: '{}'}}\n",
        "evidence ".repeat(17000)
    ));
    fs::write(root.path().join("GROUNDING.yaml"), record).unwrap();
    let revision = revision(root.path());
    let text = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--id",
            "p.huge",
            "--anchor",
            "p.anchor",
        ])
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&text).unwrap();
    assert!(body(&packet, "p.huge").is_some());
    assert!(Encoding::O200kBase.count(&text) > 16000);
    let output = cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--id",
            "p.huge",
            "--anchor",
            "p.anchor",
            "--max-view-bytes",
            "50000",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("minimal anchor receipt"));
    assert!(error.contains("increase --max-view-bytes"));
    assert!(!error.contains("increase --tokens"));
}

#[test]
fn optional_trace_shrinks_before_mandatory_evidence_is_refused() {
    let root = fixture(false);
    let mut record = String::from(
        "meta:\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n",
    );
    record.push_str(&format!("  p.big: {{v: '{}'}}\n", "orchard ".repeat(300)));
    record.push_str("  p.other: {v: unrelated}\n");
    for i in 0..6 {
        record.push_str(&format!(
            "  p.f{i}: {{v: '{}'}}\n",
            "background ".repeat(200)
        ));
    }
    fs::write(root.path().join("GROUNDING.yaml"), record).unwrap();
    let revision = revision(root.path());
    // Paths and revision hashes change reference-token counts across platforms.
    // Tighten from the actual output until optional diagnostics must shrink;
    // every successful step must retain the complete explicitly requested body.
    let mut budget = 4096;
    for _ in 0..16 {
        let text = ok(cli(root.path(), "view")
            .args([
                "--revision",
                &revision,
                "--id",
                "p.big",
                "--query",
                "orchard",
                "--anchor",
                "p.other",
                "--tokens",
                &budget.to_string(),
            ])
            .output()
            .unwrap());
        let used = Encoding::O200kBase.count(&text);
        assert!(used <= budget);
        let packet: Value = serde_json::from_str(&text).unwrap();
        let received = TypedValue::from_tagged(body(&packet, "p.big").unwrap())
            .unwrap()
            .to_json()
            .unwrap();
        assert_eq!(received["v"], "orchard ".repeat(300));
        if packet["selection"]["anchor_ranking"]["omitted_affected_rows"]
            .as_u64()
            .unwrap()
            > 0
        {
            return;
        }
        budget = used - 1;
    }
    panic!("optional anchor diagnostics did not shrink before mandatory evidence");
}
