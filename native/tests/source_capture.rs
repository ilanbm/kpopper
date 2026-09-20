use kpop_native::{
    source_capture::{ReadMode, capture_source},
    value::TypedValue as V,
};
use std::path::PathBuf;

#[cfg(windows)]
fn platform_expected(mut value: V, case: &serde_json::Value) -> V {
    if case["name"] != "hypotheses" {
        return value;
    }
    // The corpus was generated on POSIX. Python 3.13+ ntpath.isabs leaves this
    // exact synthetic, rooted-without-drive head string unchanged on Windows.
    // No source bytes, other metadata or runtime identities are normalized.
    kpop_native::reasoning_snapshot::Snapshot::from_snapshot(&value).unwrap();
    let V::Map(top) = &mut value else { panic!("snapshot map") };
    let V::Map(hypotheses) = top.get_mut("hypotheses").unwrap() else { panic!("hypotheses") };
    let V::Map(idea) = hypotheses.get_mut("idea").unwrap() else { panic!("idea") };
    let V::Map(head) = idea.get_mut("head").unwrap() else { panic!("head") };
    assert_eq!(head["claim"], V::Text("external:12a7a06e1a109026c0f33a1e/head".into()));
    head.insert("claim".into(), V::Text("/authored/head".into()));
    let preimage = V::Map(top.iter()
        .filter(|(key, _)| !["snapshot_id", "authored_revision"].contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone())).collect());
    top.insert("snapshot_id".into(), V::Text(preimage.digest().unwrap()));
    kpop_native::reasoning_snapshot::Snapshot::from_snapshot(&value).unwrap();
    value
}

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
                    #[cfg(windows)]
                    let expected = platform_expected(expected, case);
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
