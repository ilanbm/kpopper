use kpop_native::tokenizer::Encoding;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn fixture() -> tempfile::TempDir {
    fixture_counts(16, 24)
}

fn fixture_counts(group_count: usize, member_count: usize) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let mut record = String::from(
        "meta:\n  reasoning: {version: 1, profile: core/v1, requires: [arithmetic/v1]}\nknown:\n",
    );
    let mut groups = serde_json::Map::new();
    for group in 0..group_count {
        let mut ids = Vec::new();
        for n in 0..member_count {
            let id = format!("p.g{group}n{n}");
            let value = if group == 0 && n == 0 {
                "delivery handling charge is 12 units"
            } else {
                "unrelated background"
            };
            let relation = if group == 0 && n == 0 {
                ", from: p.g1n0"
            } else {
                ""
            };
            record.push_str(&format!("  {id}: {{v: '{value}'{relation}}}\n"));
            ids.push(id);
        }
        groups.insert(format!("topic-{group}"), json!(ids));
    }
    fs::write(root.path().join("GROUNDING.yaml"), record).unwrap();
    fs::write(
        root.path().join("profile.json"),
        json!({"groups":groups}).to_string(),
    )
    .unwrap();
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
            "frontier-cli",
            "--state",
            "state",
            "--assessment-profile",
            "core/v1",
            "--profile",
            "profile.json",
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
        .find_map(|line| line.strip_prefix("project=frontier-cli revision="))
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
fn orientation_coarsens_to_fit_without_dropping_the_matching_source_body() {
    let root = fixture();
    let revision = revision(root.path());
    let text = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--query",
            "delivery handling charge",
            "--tokens",
            "5000",
            "--max-view-bytes",
            "16000",
            "--view-format",
            "checked-text-tagged",
        ])
        .output()
        .unwrap());
    assert!(text.len() <= 16000);
    assert!(Encoding::O200kBase.count(&text) <= 5000);
    let packet = kpop_native::view_format::decode_checked_text(&text).unwrap();
    assert_eq!(packet["coverage"]["count"], 384);
    assert!(body(&packet, "p.g0n0").is_some());
    assert!(packet["navigation_frontier"].is_object());
    assert!(packet["groups"].as_array().unwrap().len() <= 16);
    let handle = packet["navigation_frontier"]["groups"][0]["expand"]
        .as_str()
        .unwrap();
    let children = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--expand",
            handle,
            "--tokens",
            "16000",
            "--max-view-bytes",
            "39000",
        ])
        .output()
        .unwrap());
    let children: Value = serde_json::from_str(&children).unwrap();
    assert_eq!(
        children["navigation_frontier"]["expanded_children"][0]["handle"],
        handle
    );
    let refused = cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--expand",
            "group:/",
            "--tokens",
            "5000",
            "--max-view-bytes",
            "16000",
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        refused.stdout.is_empty(),
        "explicit bodies cannot be hidden by coarse navigation"
    );
    let broad = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--tokens",
            "5000",
            "--max-view-bytes",
            "12000",
        ])
        .output()
        .unwrap());
    let broad: Value = serde_json::from_str(&broad).unwrap();
    assert!(broad["navigation_frontier"].is_object());
    assert!(!broad["links"].as_array().unwrap().is_empty());
    let edge = format!(
        "edgeset:{}:{}",
        broad["view_id"].as_str().unwrap(),
        broad["links"][0][4].as_str().unwrap()
    );
    let edges = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--tokens",
            "5000",
            "--max-view-bytes",
            "12000",
            "--expand",
            &edge,
        ])
        .output()
        .unwrap());
    let edges: Value = serde_json::from_str(&edges).unwrap();
    assert_eq!(edges["view_id"], broad["view_id"]);
    assert!(!edges["expanded_edges"].as_array().unwrap().is_empty());
    let hook = ok(cli(root.path(), "hook-view")
        .args(["--tokens", "5000", "--max-view-bytes", "12000"])
        .output()
        .unwrap());
    let route: Value = serde_json::from_str(
        hook.lines()
            .find_map(|line| line.strip_prefix("KPOPPER_CANONICAL_VIEW_ROUTE "))
            .unwrap(),
    )
    .unwrap();
    let inline = hook
        .lines()
        .find_map(|line| line.strip_prefix("KPOPPER_CANONICAL_GRAPH_VIEW "))
        .unwrap()
        .to_owned()
        + "\n";
    let packet: Value = serde_json::from_str(&inline).unwrap();
    assert!(packet["navigation_frontier"].is_object());
    let argv = route["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect::<Vec<_>>();
    let recovered = ok(Command::new(argv[0])
        .args(&argv[1..])
        .env("KPOPPER_NATIVE_RESOURCES", root.path().join("resources"))
        .env("KPOPPER_NATIVE_CACHE", root.path().join("cache"))
        .output()
        .unwrap());
    assert_eq!(recovered, inline);
    assert_eq!(
        kpop_native::identity::sha256(recovered.as_bytes()),
        route["view_sha256"]
    );
}

#[test]
fn edge_expansion_can_drop_descriptions_before_refusing_the_same_partition() {
    let root = fixture_counts(4, 8);
    let revision = revision(root.path());
    let detailed = ok(cli(root.path(), "view")
        .args(["--revision", &revision])
        .output()
        .unwrap());
    let packet: Value = serde_json::from_str(&detailed).unwrap();
    let edge = format!(
        "edgeset:{}:{}",
        packet["view_id"].as_str().unwrap(),
        packet["links"][0][4].as_str().unwrap()
    );
    let expanded = ok(cli(root.path(), "view")
        .args(["--revision", &revision, "--expand", &edge])
        .output()
        .unwrap());
    assert!(expanded.len() > detailed.len());
    let cap = detailed.len().to_string();
    let limited = ok(cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--expand",
            &edge,
            "--max-view-bytes",
            &cap,
        ])
        .output()
        .unwrap());
    assert!(limited.len() <= detailed.len());
    let limited: Value = serde_json::from_str(&limited).unwrap();
    assert_eq!(limited["view_id"], packet["view_id"]);
    assert_eq!(limited["navigation_detail"], "labels");
    assert!(!limited["expanded_edges"].as_array().unwrap().is_empty());
}

#[test]
fn explicit_refusals_keep_the_exact_recovery_guidance() {
    let root = fixture_counts(4, 8);
    let revision = revision(root.path());
    let refused = cli(root.path(), "view")
        .args(["--revision", &revision, "--id", "p.g0n0", "--tokens", "64"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("session read --ref"));
    let refused = cli(root.path(), "view")
        .args([
            "--revision",
            &revision,
            "--id",
            "p.g0n0",
            "--max-view-bytes",
            "64",
        ])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("narrower query or exact IDs"));
}
