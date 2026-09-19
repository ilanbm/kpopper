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
                    let actual = capture.snapshot().to_data();
                    if actual != expected {
                        let a = actual.to_json().unwrap();
                        let b = expected.to_json().unwrap();
                        let differing = a
                            .as_object()
                            .unwrap()
                            .keys()
                            .filter(|k| a[*k] != b[*k])
                            .collect::<Vec<_>>();
                        failures.push(format!("{} differs: {:?}", case["name"], differing));
                        for key in differing {
                            if key != "snapshot_id" {
                                eprintln!(
                                    "{} {key}: actual={} expected={}",
                                    case["name"], a[key], b[key]
                                );
                            }
                        }
                    }
                    assert_eq!(
                        capture.document(),
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
