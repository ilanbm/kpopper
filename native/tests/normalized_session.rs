use serde_json::{Value as J, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn graph() -> J {
    json!({"nodes":{
    "p.value":{"kind":"known","states":[],"body":{"v":7}},
    "d.keep":{"kind":"judgment","states":[],"body":{"verdict":"keep","rests_on":["p.value"],"seen":{"p.value":7},"wrong_if":"p.value > 10"}}
},"topics":{"p.value":["known"],"d.keep":["judgments"]},"edges":[{"from":"d.keep","rel":"rests_on","to":"p.value"}]})
}
fn resources(root: &Path) -> PathBuf {
    if let Some(p) = std::env::var_os("KPOP_CONSOLIDATION_RESOURCES")
        .or_else(|| std::env::var_os("KPOPPER_NATIVE_RESOURCES"))
    {
        return p.into();
    }
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let program = std::env::var_os("KPOP_TEST_ORDINARY_PROGRAM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap())
                .join(".cache/kpopper/lean")
                .join(&target)
                .join(env!("KPOP_ORDINARY_SOURCE_SHA256"))
        });
    let r = root.join("resources");
    let o = r.join("ordinary").join(&target);
    fs::create_dir_all(&o).unwrap();
    for name in [
        "build.json",
        if cfg!(windows) {
            "epistemic-core.exe"
        } else {
            "epistemic-core"
        },
    ] {
        fs::copy(program.join(name), o.join(name)).unwrap();
    }
    fs::create_dir(r.join("reasoning")).unwrap();
    let name = format!("{target}.kpopper-runtime");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(&name),
        r.join("reasoning").join(name),
    )
    .unwrap();
    r
}
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("input.json"),
        serde_json::to_vec(&graph()).unwrap(),
    )
    .unwrap();
    let r = resources(t.path());
    (t, r)
}
fn run(root: &Path, r: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop"))
        .args(args)
        .args([
            "--normalized",
            "--no-settings",
            "--input",
            "input.json",
            "--project",
            "replay",
            "--state",
            "state",
        ])
        .current_dir(root)
        .env("KPOPPER_NATIVE_RESOURCES", r)
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .env("KPOPPER_PRIVATE_HOME", root.join("private"))
        .output()
        .unwrap()
}
fn ok(o: Output) -> String {
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout).unwrap()
}
fn revision(text: &str) -> String {
    regex::Regex::new(r"revision=([0-9a-f]{64})")
        .unwrap()
        .captures(text)
        .unwrap()[1]
        .to_owned()
}
#[test]
fn normalized_open_read_context_search_and_proposals_preserve_input() {
    let (t, r) = fixture();
    let root = t.path();
    let before = fs::read(root.join("input.json")).unwrap();
    let opening = ok(run(root, &r, &["session", "open"]));
    let rev = revision(&opening);
    let read = ok(run(
        root,
        &r,
        &[
            "session",
            "read",
            "--revision",
            &rev,
            "--ref",
            "node:p.value",
        ],
    ));
    assert!(read.contains("7"));
    let context = ok(run(root, &r, &["context", "d.keep", "--revision", &rev]));
    assert!(context.contains("p.value"));
    let found = ok(run(
        root,
        &r,
        &[
            "session",
            "search",
            "--revision",
            &rev,
            "--query",
            "value",
            "--search-mode",
            "lexical",
        ],
    ));
    assert!(found.contains("p.value"));
    ok(run(
        root,
        &r,
        &[
            "session",
            "propose",
            "--revision",
            &rev,
            "--kind",
            "observed",
            "--text",
            "A replay observation",
            "--basis",
            "node:p.value",
            "--revisit",
            "Recheck when the snapshot changes",
        ],
    ));
    assert_eq!(fs::read(root.join("input.json")).unwrap(), before);
    let mut changed = graph();
    changed["nodes"]["p.value"]["body"]["v"] = json!(8);
    fs::write(
        root.join("input.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    let stale = run(
        root,
        &r,
        &[
            "session",
            "read",
            "--revision",
            &rev,
            "--ref",
            "node:p.value",
        ],
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("reopen"));
    assert_ne!(revision(&ok(run(root, &r, &["session", "open"]))), rev);
}
#[test]
fn malformed_normalized_graphs_fail_without_panics_or_source_changes() {
    let mut cases = vec![];
    let mut g = graph();
    g["topics"].as_object_mut().unwrap().remove("p.value");
    cases.push(g);
    let mut g = graph();
    g["nodes"]["p.value"]["body"] = json!(7);
    cases.push(g);
    let mut g = graph();
    g["nodes"]["p.value"]["states"] = json!("checked");
    cases.push(g);
    let mut g = graph();
    let edge = g["edges"][0].clone();
    g["edges"].as_array_mut().unwrap().push(edge);
    cases.push(g);
    let mut g = graph();
    g["nodes"]["p.value"]["assessment_body"] = json!({"v":99});
    g["nodes"]["p.value"]["assessment_fields"] = json!({});
    cases.push(g);
    let mut g = graph();
    g["node_order"] = json!(["p.value", "p.value"]);
    cases.push(g);
    for g in cases {
        let (t, r) = fixture();
        let bytes = serde_json::to_vec(&g).unwrap();
        fs::write(t.path().join("input.json"), &bytes).unwrap();
        let out = run(t.path(), &r, &["session", "open"]);
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
        assert_eq!(fs::read(t.path().join("input.json")).unwrap(), bytes);
    }
}
#[test]
fn normalized_context_needs_no_prior_open_and_empty_graph_stays_addressable() {
    let (t, r) = fixture();
    assert!(ok(run(t.path(), &r, &["context", "p.value"])).contains("p.value"));
    fs::write(
        t.path().join("input.json"),
        b"{\"nodes\":{},\"topics\":{},\"edges\":[]}",
    )
    .unwrap();
    let rev = revision(&ok(run(t.path(), &r, &["session", "open"])));
    ok(run(
        t.path(),
        &r,
        &["session", "read", "--revision", &rev, "--ref", "/"],
    ));
}

#[test]
fn identical_graphs_in_different_files_have_distinct_revisions_and_hook_routes_keep_the_mode() {
    let (a, resources) = fixture();
    let (b, _) = fixture();
    let first = ok(run(a.path(), &resources, &["session", "open"]));
    let second = ok(run(b.path(), &resources, &["session", "open"]));
    assert_ne!(revision(&first), revision(&second));
    let hook = ok(run(
        a.path(),
        &resources,
        &["session", "hook-open", "--tokens", "2000"],
    ));
    assert!(hook.contains("--normalized"));
    let incompatible = run(
        a.path(),
        &resources,
        &["session", "open", "--assessment-profile", "core/v1"],
    );
    assert!(!incompatible.status.success());
    assert!(
        String::from_utf8_lossy(&incompatible.stderr)
            .contains("normalized input requires checked-reader/v1")
    );
}

#[test]
fn replay_recomputes_falsifiers_and_enforces_the_opening_budget() {
    let (t, r) = fixture();
    let mut data = graph();
    data["nodes"]["p.value"]["body"]["v"] = json!(20);
    data["scan"] = json!({"counts":{"falsifier_triggered":0}});
    fs::write(
        t.path().join("input.json"),
        serde_json::to_vec(&data).unwrap(),
    )
    .unwrap();
    let opened = ok(run(t.path(), &r, &["session", "open"]));
    assert!(opened.contains("triggered=1"), "{opened}");
    let small = run(t.path(), &r, &["session", "open", "--tokens", "64"]);
    assert!(!small.status.success());
    assert!(String::from_utf8_lossy(&small.stderr).contains("minimum complete opening"));
}
