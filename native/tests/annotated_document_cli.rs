use kpop_native::annotated_document as document;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args(["--workspace", root.to_str().unwrap()])
        .args(args)
        .env("PATH", "")
        .output()
        .unwrap()
}
fn json_output(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn portable_document_cli_build_inspect_refresh_and_input_protection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let authored = "<!doctype html><html><head><title>Count</title></head><body><p>Count: <span data-kpopper-claim=\"count\">80</span></p></body></html>";
    fs::write(root.join("authored.html"), authored).unwrap();
    fs::write(root.join("counts.json"), r#"{"count":80}"#).unwrap();
    let sources = json!({"counts":{"name":"Counts","path":"counts.json","format":"json"}});
    fs::write(root.join("manifest.json"), json!({"version":1,"title":"Count","language":"en","sources":sources,
        "claims":[{"id":"count","label":"Count","kind":"value","inputs":[{"source":"counts","pointer":"/count"}]}]}).to_string()).unwrap();
    let build = [
        "experimental",
        "annotated-doc",
        "build",
        "--html",
        "authored.html",
        "--manifest",
        "manifest.json",
        "--out",
        "copy.html",
    ];
    let output = json_output(run(root, &build));
    assert_eq!(output["checks"]["match"], 1);
    assert_eq!(output["source_files_changed"], false);
    let original = fs::read(root.join("copy.html")).unwrap();
    assert!(!run(root, &build).status.success());
    assert_eq!(fs::read(root.join("copy.html")).unwrap(), original);
    let inspected = json_output(run(root, &["document", "inspect", "copy.html"]));
    assert_eq!(inspected["checks"]["count"]["status"], "match");
    fs::write(root.join("counts.json"), r#"{"count":100}"#).unwrap();
    fs::write(root.join("sources.json"), sources.to_string()).unwrap();
    let refreshed = json_output(run(
        root,
        &[
            "experimental",
            "annotated-doc",
            "refresh",
            "copy.html",
            "--sources",
            "sources.json",
            "--out",
            "updated.html",
        ],
    ));
    assert_eq!(refreshed["proposals"], 1);
    assert_eq!(fs::read(root.join("copy.html")).unwrap(), original);
    let refused = run(
        root,
        &[
            "document",
            "build",
            "--html",
            "authored.html",
            "--manifest",
            "manifest.json",
            "--out",
            "counts.json",
            "--overwrite",
        ],
    );
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(root.join("counts.json")).unwrap(),
        r#"{"count":100}"#
    );
    assert_eq!(
        fs::read_to_string(root.join("authored.html")).unwrap(),
        authored
    );
    assert!(
        run(root, &["experimental", "annotated-doc", "guide"])
            .status
            .success()
    );
}

#[test]
fn core_document_uses_the_same_runtime_assessment_as_public_check() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let target = kpop_native::reasoning_runtime::target_name().unwrap();
    let resources = root.join("resources");
    fs::create_dir_all(resources.join("reasoning")).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/reasoning/native")
            .join(format!("{target}.kpopper-runtime")),
        resources.join("reasoning").join(format!("{target}.zip")),
    )
    .unwrap();
    fs::write(
        root.join("GROUNDING.yaml"),
        "meta:\n  reasoning: {version: 2, profile: core/v1, requires: [arithmetic/v1]}\nsources:\n  s.report: {name: Report}\nknown:\n  reading.base: {v: 6, from: s.report}\n  reading.total: {rule: {expr: 'reading.base * 7'}, from: s.report}\n",
    )
    .unwrap();
    fs::write(
        root.join("authored.html"),
        "<!doctype html><html><head><title>Total</title></head><body><p>Total: <span data-kpopper-claim=\"total\">42</span></p></body></html>",
    )
    .unwrap();
    fs::write(
        root.join("manifest.json"),
        json!({"version":1,"sources":{"record":{"name":"Record","path":"GROUNDING.yaml","format":"record","profile":"core/v1"}},"claims":[
            {"id":"total","label":"Total","kind":"value","inputs":[{"source":"record","pointer":"/reading.total/v"}]}
        ]})
        .to_string(),
    )
    .unwrap();
    let command = |args: &[&str], runtime: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kpop-native"));
        command
            .args(["--workspace", root.to_str().unwrap(), "--frozen"])
            .args(args)
            .env("KPOPPER_NATIVE_CACHE", root.join("cache"));
        if runtime {
            command.env("KPOPPER_NATIVE_RESOURCES", &resources);
        } else {
            command.env_remove("KPOPPER_NATIVE_RESOURCES");
        }
        command.output().unwrap()
    };
    let unavailable = command(
        &[
            "document",
            "build",
            "--html",
            "authored.html",
            "--manifest",
            "manifest.json",
            "--out",
            "unavailable.html",
        ],
        false,
    );
    assert!(
        unavailable.status.success(),
        "{}",
        String::from_utf8_lossy(&unavailable.stderr)
    );
    let unavailable =
        document::load_artifact(&fs::read_to_string(root.join("unavailable.html")).unwrap())
            .unwrap();
    let unavailable = &unavailable["sources"]["record"]["assessment"];
    let unavailable_node =
        kpop_native::value::TypedValue::from_tagged(&unavailable["nodes"]["reading.total"])
            .unwrap()
            .to_json()
            .unwrap();
    assert_eq!(
        unavailable_node["computation"]["status"],
        "operational_error"
    );

    let invalid = Command::new(env!("CARGO_BIN_EXE_kpop-native"))
        .args([
            "--workspace",
            root.to_str().unwrap(),
            "--frozen",
            "document",
            "build",
            "--html",
            "authored.html",
            "--manifest",
            "manifest.json",
            "--out",
            "invalid-runtime.html",
        ])
        .env(
            "KPOPPER_NATIVE_RESOURCES",
            root.join("invalid-runtime-resources"),
        )
        .env("KPOPPER_NATIVE_CACHE", root.join("cache"))
        .output()
        .unwrap();
    assert!(
        invalid.status.success(),
        "{}",
        String::from_utf8_lossy(&invalid.stderr)
    );
    let invalid =
        document::load_artifact(&fs::read_to_string(root.join("invalid-runtime.html")).unwrap())
            .unwrap();
    assert_eq!(invalid["sources"]["record"]["status"], "unavailable");
    assert!(invalid["sources"]["record"].get("assessment").is_none());

    let built = command(
        &[
            "document",
            "build",
            "--html",
            "authored.html",
            "--manifest",
            "manifest.json",
            "--out",
            "report.html",
        ],
        true,
    );
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let artifact =
        document::load_artifact(&fs::read_to_string(root.join("report.html")).unwrap()).unwrap();
    let assessment = &artifact["sources"]["record"]["assessment"];
    let embedded_node =
        kpop_native::value::TypedValue::from_tagged(&assessment["nodes"]["reading.total"])
            .unwrap()
            .to_json()
            .unwrap();
    assert_eq!(embedded_node["computation"]["status"], "ok");
    assert_eq!(assessment["snapshot_id"], unavailable["snapshot_id"]);
    assert_ne!(
        assessment["findings_revision"],
        unavailable["findings_revision"]
    );

    let checked = command(&["check"], true);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let checked = String::from_utf8(checked.stdout).unwrap();
    assert!(checked.contains(assessment["snapshot_id"].as_str().unwrap()));
    assert!(checked.contains(assessment["findings_revision"].as_str().unwrap()));
}
