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
