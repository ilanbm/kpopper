use kpop_native::{
    history_migration::{Options, Plan, replay_from_copy, restore_from_copy},
    source_capture::ReadMode,
};
use std::{collections::BTreeMap, fs, path::Path};
fn fixture(root: &Path, files: &serde_json::Value) {
    for (name, raw) in files.as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw.as_str().unwrap()).unwrap();
    }
}
fn options(mode: ReadMode) -> Options {
    Options {
        operation: "import-fixture".into(),
        recorded_at: "2026-09-19T12:00:00+00:00".into(),
        record_id: Some("record-import-fixture".into()),
        read_mode: mode,
        route: false,
        as_of: None,
    }
}
#[test]
fn imports_match_final_python_and_restore_exact_originals() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-migration.json")).unwrap();
    let mut failures = vec![];
    for case in corpus["cases"].as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fixture(&source, &case["files"]);
        let mode = if case["mode"] == "live" {
            ReadMode::Live
        } else {
            ReadMode::Frozen
        };
        let prepared = Plan::prepare(
            &source.join(case["entry"].as_str().unwrap()),
            &source,
            options(mode),
            None,
        );
        if case.get("refused").is_some() {
            assert!(prepared.is_err(), "{}", case["name"]);
            continue;
        }
        let plan = match prepared {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{} prepare: {e}", case["name"]));
                continue;
            }
        };
        let expected = case["output"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(p, v)| (p.clone(), v.as_str().unwrap().as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>();
        let different = expected
            .keys()
            .chain(plan.files().keys())
            .filter(|p| expected.get(*p) != plan.files().get(*p))
            .collect::<std::collections::BTreeSet<_>>();
        if !different.is_empty() {
            failures.push(format!("{} differs: {:?}", case["name"], different));
            let diagnostic = std::env::temp_dir().join("kpop-native-migration-diff");
            fs::create_dir_all(&diagnostic).unwrap();
            for p in different {
                let name = p.replace('/', "_");
                if let Some(raw) = plan.files().get(p) {
                    fs::write(diagnostic.join(format!("actual-{name}")), raw).unwrap();
                }
                if let Some(raw) = expected.get(p) {
                    fs::write(diagnostic.join(format!("expected-{name}")), raw).unwrap();
                }
            }
            continue;
        }
        let destination = root.join("copy");
        if case["manifest"][1]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v[0] == "complete" && v[1] == serde_json::json!(["bool", false]))
        {
            assert!(plan.publish(&destination).is_err());
            continue;
        }
        if let Err(e) = plan.publish(&destination) {
            failures.push(format!("{} publish: {e}", case["name"]));
            continue;
        }
        match replay_from_copy(&destination) {
            Ok(s) => assert_eq!(s.snapshot_id(), plan.candidate().snapshot_id()),
            Err(e) => {
                failures.push(format!("{} replay: {e}", case["name"]));
                continue;
            }
        }
        let restored = root.join("restored");
        if let Err(e) = restore_from_copy(&destination, &restored) {
            failures.push(format!("{} restore: {e}", case["name"]));
            continue;
        }
        for (name, raw) in case["files"].as_object().unwrap() {
            assert_eq!(
                fs::read(restored.join(name)).unwrap(),
                raw.as_str().unwrap().as_bytes(),
                "{} {name}",
                case["name"]
            );
        }
        plan.verify_source().unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
fn simple_plan(root: &Path) -> Plan {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("GROUNDING.yaml"), b"known: {p.x: {v: 1}}\n").unwrap();
    Plan::prepare(
        &root.join("GROUNDING.yaml"),
        root,
        options(ReadMode::Frozen),
        None,
    )
    .unwrap()
}
#[test]
fn nested_external_topology_restores_after_source_is_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    let project = source.join("project");
    let shared = source.join("shared");
    fs::create_dir_all(shared.join("parts")).unwrap();
    fs::create_dir(&project).unwrap();
    let originals = BTreeMap::from([
        (
            "project/GROUNDING.yaml",
            b"# entry\r\nrecord: ../shared/member.yaml\r\n".as_slice(),
        ),
        (
            "shared/member.yaml",
            b"# member\nrecord: parts/values.yaml\nknown: {p.first: {v: 1}}\n".as_slice(),
        ),
        (
            "shared/parts/values.yaml",
            b"known: {p.second: {v: 2}}\n".as_slice(),
        ),
    ]);
    for (name, raw) in &originals {
        fs::write(source.join(name), raw).unwrap();
    }
    fs::write(shared.join("private.yaml"), b"private: true\n").unwrap();
    let plan = Plan::prepare(
        &project.join("GROUNDING.yaml"),
        &project,
        options(ReadMode::Frozen),
        None,
    )
    .unwrap();
    assert!(!plan.files().keys().any(|p| p.contains("private")));
    let copy = root.join("copy");
    plan.publish(&copy).unwrap();
    fs::remove_dir_all(&source).unwrap();
    assert!(plan.verify_source().is_err());
    let replay = replay_from_copy(&copy).unwrap();
    assert_eq!(replay.snapshot_id(), plan.candidate().snapshot_id());
    let restored = root.join("restored");
    restore_from_copy(&copy, &restored).unwrap();
    for (name, raw) in originals {
        assert_eq!(fs::read(restored.join(name)).unwrap(), raw);
    }
}
#[test]
fn source_bytes_membership_evidence_and_layout_changes_refuse() {
    for change in [
        "entry",
        "hypothesis",
        "sidecar",
        "alternate",
        "evidence",
        "external",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("GROUNDING.yaml"),
            b"known: {p.x: {v: 1}}\nsources: {s.local: {file: evidence.txt}}\n",
        )
        .unwrap();
        fs::write(source.join("evidence.txt"), b"original").unwrap();
        let plan = Plan::prepare(
            &source.join("GROUNDING.yaml"),
            &source,
            options(ReadMode::Frozen),
            None,
        )
        .unwrap();
        let (path, raw) = match change {
            "entry" => ("GROUNDING.yaml", b"known: {p.x: {v: 2}}\n".as_slice()),
            "hypothesis" => (
                ".kpopper/hypotheses/late.yaml",
                b"known: {p.x: {v: 3}}\n".as_slice(),
            ),
            "sidecar" => (".kpopper/view.yaml", b"sections: []\n".as_slice()),
            "alternate" => ("PROVENANCE.replaced.yaml", b"{}\n".as_slice()),
            "evidence" => ("evidence.txt", b"changed".as_slice()),
            _ => ("GROUNDING.yaml", b"record: ../outside.yaml\n".as_slice()),
        };
        let path = source.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, raw).unwrap();
        let dest = root.join("refused");
        assert!(plan.publish(&dest).is_err(), "{change}");
        assert!(!dest.exists());
    }
}
#[test]
fn sealed_copy_refuses_tampering_and_forged_original_allowlist() {
    use kpop_native::{identity::sha256, value::TypedValue as V};
    for change in [
        "bytes",
        "delete",
        "extra",
        "mapping",
        "topology",
        "whitespace",
        "duplicate",
        "candidate",
        "authority",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let plan = simple_plan(&root.join("source"));
        let copy = root.join("copy");
        plan.publish(&copy).unwrap();
        let receipt_path = copy.join(".kpopper-history-migration/receipt.json");
        let mut receipt = plan.manifest().clone();
        match change {
            "bytes" => fs::write(copy.join("GROUNDING.yaml"), b"known: {}\n").unwrap(),
            "delete" => {
                fs::remove_file(copy.join(".kpopper-history-migration/original.json")).unwrap()
            }
            "extra" => fs::write(copy.join("extra"), b"unknown").unwrap(),
            "mapping" => {
                let path = ".kpopper-history-migration/originals/injected.txt";
                let bytes = b"injected";
                fs::write(copy.join(path), bytes).unwrap();
                let V::Map(m) = &mut receipt else {
                    unreachable!()
                };
                let V::Map(originals) = m.get_mut("originals").unwrap() else {
                    unreachable!()
                };
                originals.insert(
                    "injected.txt".into(),
                    V::from_json(&serde_json::json!({"path":path,"sha256":sha256(bytes)})).unwrap(),
                );
                let V::Map(dest) = m.get_mut("destination").unwrap() else {
                    unreachable!()
                };
                dest.insert(path.into(), V::Text(sha256(bytes)));
                fs::write(&receipt_path, receipt.canonical_bytes().unwrap()).unwrap();
            }
            "topology" => {
                let V::Map(m) = &mut receipt else {
                    unreachable!()
                };
                m.insert("topology".into(),V::from_json(&serde_json::json!({"version":1,"entry":"redirect/GROUNDING.yaml","originals":{"GROUNDING.yaml":"redirect/GROUNDING.yaml"}})).unwrap());
                fs::write(&receipt_path, receipt.canonical_bytes().unwrap()).unwrap();
            }
            "whitespace" => {
                let mut bytes = fs::read(&receipt_path).unwrap();
                bytes.push(b'\n');
                fs::write(&receipt_path, bytes).unwrap();
            }
            "duplicate" => {
                let bytes = fs::read_to_string(&receipt_path)
                    .unwrap()
                    .replace("[\"map\",[", "[\"map\",[[\"complete\",[\"bool\",true]],");
                fs::write(&receipt_path, bytes).unwrap();
            }
            "candidate" => fs::write(
                copy.join(".kpopper-history-migration/candidate.json"),
                b"[]",
            )
            .unwrap(),
            _ => fs::write(copy.join(".kpopper/history.yaml"), b"{}\n").unwrap(),
        }
        assert!(replay_from_copy(&copy).is_err(), "{change}");
        let dest = root.join("restore");
        assert!(restore_from_copy(&copy, &dest).is_err(), "{change}");
        assert!(!dest.exists());
    }
}
#[test]
fn pending_observation_replays_without_original_repository() {
    use kpop_native::value::TypedValue as V;
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-migration.json")).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fixture(&root, &corpus["live_copy"]["files"]);
    let expected = V::from_tagged(&corpus["live_copy"]["candidate"]).unwrap();
    let replay = replay_from_copy(&root).unwrap();
    assert_eq!(replay.to_data(), expected);
}
#[cfg(unix)]
#[test]
fn directory_symlinks_and_absolute_inverse_refuse() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let plan = simple_plan(&root.join("source"));
    let copy = root.join("copy");
    plan.publish(&copy).unwrap();
    fs::create_dir(root.join("empty")).unwrap();
    symlink(root.join("empty"), copy.join("link")).unwrap();
    assert!(replay_from_copy(&copy).is_err());
    assert!(restore_from_copy(&copy, &root.join("restored")).is_err());
    let source = root.join("absolute");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("member.yaml"), b"known: {p.x: {v: 1}}\n").unwrap();
    fs::write(
        source.join("GROUNDING.yaml"),
        format!("record: {}\n", source.join("member.yaml").display()),
    )
    .unwrap();
    let plan = Plan::prepare(
        &source.join("GROUNDING.yaml"),
        &source,
        options(ReadMode::Frozen),
        None,
    )
    .unwrap();
    let copy = root.join("absolute-copy");
    plan.publish(&copy).unwrap();
    assert!(restore_from_copy(&copy, &root.join("absolute-restore")).is_err());
    replay_from_copy(&copy).unwrap();
}
#[cfg(unix)]
#[test]
fn evidence_control_paths_and_replaced_history_symlinks_refuse() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("GROUNDING.yaml"),
        b"sources: {s.bad: {file: \"evidence\\n.txt\"}}\n",
    )
    .unwrap();
    fs::write(source.join("evidence\n.txt"), b"bytes").unwrap();
    assert!(
        Plan::prepare(
            &source.join("GROUNDING.yaml"),
            &source,
            options(ReadMode::Frozen),
            None
        )
        .is_err()
    );
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/history-migration.json")).unwrap();
    let case = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"].as_str().unwrap().contains("-inactive-"))
        .unwrap();
    let source = root.join("history-source");
    fs::create_dir(&source).unwrap();
    fixture(&source, &case["files"]);
    let plan = Plan::prepare(
        &source.join("GROUNDING.yaml"),
        &source,
        options(ReadMode::Frozen),
        None,
    )
    .unwrap();
    let path = case["files"]
        .as_object()
        .unwrap()
        .keys()
        .find(|p| p.starts_with(".kpopper/history/"))
        .unwrap();
    let outside = root.join("same-bytes.yaml");
    fs::copy(source.join(path), &outside).unwrap();
    fs::remove_file(source.join(path)).unwrap();
    symlink(&outside, source.join(path)).unwrap();
    assert!(plan.verify_source().is_err());
    assert!(plan.publish(&root.join("refused")).is_err());
}
