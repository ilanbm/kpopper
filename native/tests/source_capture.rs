use kpop_native::{
    source_capture::{ReadMode, capture_source},
    value::TypedValue as V,
};
use std::path::PathBuf;

#[test]
fn source_snapshots_match_pinned_python() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/source-capture.json")).unwrap();
    let mut failures = vec![];
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for (path, raw) in case["files"].as_object().unwrap() {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw.as_str().unwrap()).unwrap();
        }
        let paths = case["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| PathBuf::from(p.as_str().unwrap()))
            .collect::<Vec<_>>();
        let mode = if case["mode"] == "live" {
            ReadMode::Live
        } else {
            ReadMode::Frozen
        };
        let result = capture_source(&paths, &root, mode, Some(V::Text("2026-09-19".into())));
        if let Some(expected) = case.get("snapshot") {
            match result {
                Ok(capture) => {
                    let expected = V::from_tagged(expected).unwrap();
                    let actual = capture.snapshot().unwrap().to_data();
                    if actual != expected {
                        let actual = actual.to_tagged().unwrap();
                        let expected = expected.to_tagged().unwrap();
                        failures.push(format!("{} differs", case["name"]));
                        eprintln!(
                            "{} tagged actual={} expected={}",
                            case["name"],
                            serde_json::to_string(&actual).unwrap(),
                            serde_json::to_string(&expected).unwrap()
                        );
                    }
                    assert_eq!(
                        capture.strict_document().unwrap(),
                        V::from_tagged(&case["document"]).unwrap()
                    );
                    capture.verify().unwrap();
                }
                Err(e) => failures.push(format!("{} refused: {e}", case["name"])),
            }
        } else if result.is_ok() {
            failures.push(format!("{} unexpectedly accepted", case["name"]));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
