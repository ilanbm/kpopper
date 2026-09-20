use super::*;
use serde_json::Value as J;
fn data() -> J {
    serde_json::from_str(include_str!("../fixtures/history-watch.json")).unwrap()
}
fn runtime(cache: &std::path::Path) -> Runtime {
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!(
            "{}.kpopper-runtime",
            crate::reasoning_runtime::target_name().unwrap()
        ));
    Runtime::open(&archive, cache, OperationalBounds::default()).unwrap()
}
#[test]
fn history_watch_pure_union_matches_final_python() {
    for case in data()["cases"].as_array().unwrap() {
        let records = V::from_tagged(&case["input"]).unwrap();
        let got = merged_snapshot(&records);
        if let Some(expected) = case.get("merged") {
            assert_eq!(
                got.unwrap_or_else(|e| panic!("{}: {e}", case["name"]))
                    .to_data(),
                V::from_tagged(expected).unwrap(),
                "{}",
                case["name"]
            );
        } else {
            assert!(got.is_err(), "{} accepted", case["name"]);
        }
    }
}
fn retain_actual_audit(value: &mut V, expected: &V) {
    match (value, expected) {
        (V::List(items), V::List(old)) => {
            for (item, old) in items.iter_mut().zip(old) {
                retain_actual_audit(item, old);
            }
        }
        (V::Map(m), V::Map(old)) => {
            if let Some(V::Map(implementation)) = m.get_mut("implementation")
                && implementation.contains_key("adapter_source_sha256")
            {
                assert_eq!(
                    implementation["adapter_source_sha256"],
                    s(env!("KPOP_REASONING_ADAPTER_SHA256"))
                );
                let old_impl = map(&old["implementation"]).unwrap();
                implementation.insert(
                    "adapter_source_sha256".into(),
                    old_impl["adapter_source_sha256"].clone(),
                );
                assert_eq!(*implementation, *old_impl);
                map_mut(m.get_mut("assurance").unwrap()).unwrap().insert(
                    "implementation".into(),
                    s(&digest(&V::Map(old_impl.clone())).unwrap()),
                );
            }
            for (key, item) in m.iter_mut() {
                if !["implementation", "assurance"].contains(&key.as_str())
                    && let Some(old) = old.get(key)
                {
                    retain_actual_audit(item, old);
                }
            }
            if ["fingerprint", "kind", "id", "reason"]
                .iter()
                .all(|key| m.contains_key(*key))
            {
                let raw = V::Map(
                    m.iter()
                        .filter(|(key, _)| *key != "fingerprint")
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                );
                m.insert("fingerprint".into(), s(&digest(&raw).unwrap()));
            }
        }
        _ => {}
    }
}
#[test]
fn history_watch_actual_comparison_preserves_scenarios_and_attention() {
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    let mut failures = vec![];
    for case in data()["cases"].as_array().unwrap() {
        let Some(expected) = case.get("output") else {
            continue;
        };
        let records = V::from_tagged(&case["input"]).unwrap();
        let expected = V::from_tagged(expected).unwrap();
        let mut got = compare(&records, Some(&runtime), OperationalBounds::default()).unwrap();
        retain_actual_audit(&mut got, &expected);
        if got != expected {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("actual.json"),
                serde_json::to_string(&got.to_tagged().unwrap()).unwrap(),
            )
            .unwrap();
            failures.push(format!(
                "{} mismatch: {}",
                case["name"],
                dir.keep().display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[test]
fn history_watch_binds_public_snapshot_to_independent_history_and_hypotheses() {
    let data = data();
    let case = data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c.get("merged").is_some())
        .unwrap();
    for kind in ["doc", "snapshot", "hypotheses", "clock", "checksum"] {
        let mut input = V::from_tagged(&case["input"]).unwrap();
        let r = map_mut(map_mut(&mut input).unwrap().get_mut("working").unwrap()).unwrap();
        match kind {
            "doc" => {
                r.insert("doc".into(), empty());
            }
            "snapshot" => {
                r.remove("snapshot");
            }
            "hypotheses" => {
                r.insert(
                    "hypotheses".into(),
                    V::List(vec![obj([
                        ("name", s("forged")),
                        ("doc", empty()),
                        ("head", empty()),
                    ])]),
                );
            }
            "clock" => {
                map_mut(r.get_mut("snapshot").unwrap())
                    .unwrap()
                    .insert("as_of".into(), s("2026-09-01"));
            }
            _ => {
                let evidence = map_mut(r.get_mut("history").unwrap()).unwrap();
                map_mut(evidence.get_mut("sha256").unwrap())
                    .unwrap()
                    .insert("entry.yaml".into(), s(&"0".repeat(64)));
            }
        }
        assert!(merged_snapshot(&input).is_err(), "{kind}");
        let result = compare(&input, None, OperationalBounds::default()).unwrap();
        assert_eq!(map(&result).unwrap()["state"], s("attention"));
        assert_eq!(map(&result).unwrap()["changed"], V::List(vec![]));
    }
}
