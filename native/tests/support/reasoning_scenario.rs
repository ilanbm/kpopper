use super::*;
use serde_json::Value as J;
fn data() -> J {
    serde_json::from_str(include_str!("../fixtures/reasoning-scenario.json")).unwrap()
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
fn scenario_build_and_snapshot_replay_match_final_python() {
    let data = data();
    for case in data["cases"].as_array().unwrap() {
        let input = V::from_tagged(&case["input"]).unwrap();
        let input = map(&input).unwrap();
        let source = Snapshot::from_snapshot(&input["source"]);
        let shared = if input["shared"] == V::Null {
            Ok(None)
        } else {
            Snapshot::from_snapshot(&input["shared"]).map(Some)
        };
        let got = source.and_then(|source| {
            shared.and_then(|shared| build(&source, &input["selection"], shared.as_ref()))
        });
        if let Some(output) = case.get("output") {
            let got = got.unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            assert_eq!(
                got.to_data(),
                V::from_tagged(output).unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                Snapshot::from_json(got.to_json().unwrap().as_bytes())
                    .unwrap()
                    .to_data(),
                got.to_data()
            );
        } else {
            assert!(got.is_err(), "{} accepted", case["name"]);
        }
    }
    for case in data["forgeries"].as_array().unwrap() {
        assert!(
            Snapshot::from_snapshot(&V::from_tagged(&case["snapshot"]).unwrap()).is_err(),
            "{} accepted",
            case["name"]
        );
    }
}
#[test]
fn scenario_assessment_uses_lean_and_keeps_computational_dimensions() {
    let data = data();
    let cache = tempfile::tempdir().unwrap();
    let runtime = runtime(cache.path());
    for case in data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c.get("output").is_some())
    {
        let snapshot = Snapshot::from_snapshot(&V::from_tagged(&case["output"]).unwrap()).unwrap();
        let mut got = assess(&snapshot, Some(&runtime), OperationalBounds::default())
            .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        let expected = V::from_tagged(&case["assessment"]).unwrap();
        let old_heads = list(&map(&expected).unwrap()["heads"]).unwrap();
        let heads = map_mut(&mut got).unwrap().get_mut("heads").unwrap();
        if let V::List(heads) = heads {
            for (head, old) in heads.iter_mut().zip(old_heads) {
                let old = &map(old).unwrap()["computation"];
                let comp = map_mut(head).unwrap().get_mut("computation").unwrap();
                if *comp == V::Null {
                    continue;
                }
                let current = map_mut(comp).unwrap();
                let implementation = map_mut(current.get_mut("implementation").unwrap()).unwrap();
                assert_eq!(
                    implementation["adapter_source_sha256"],
                    s(env!("KPOP_REASONING_ADAPTER_SHA256"))
                );
                let old_impl = &map(old).unwrap()["implementation"];
                let mut comparable = V::Map(implementation.clone());
                map_mut(&mut comparable).unwrap().insert(
                    "adapter_source_sha256".into(),
                    map(old_impl).unwrap()["adapter_source_sha256"].clone(),
                );
                assert_eq!(comparable, *old_impl);
                current.insert("implementation".into(), old_impl.clone());
                map_mut(current.get_mut("assurance").unwrap())
                    .unwrap()
                    .insert("implementation".into(), s(&digest(old_impl).unwrap()));
            }
        }
        assert_eq!(got, expected, "{}", case["name"]);
        assert!(map(&got).unwrap().keys().all(|k| {
            [
                "findings",
                "heads",
                "collisions",
                "source_snapshot_id",
                "scenario_id",
                "snapshot",
            ]
            .contains(&k.as_str())
        }));
    }
}

#[test]
fn scenario_full_transport_limit_and_invalid_context_refuse() {
    let fixture = data();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c.get("output").is_some())
        .unwrap();
    let input = V::from_tagged(&case["input"]).unwrap();
    let input = map(&input).unwrap();
    let observed = Snapshot::from_snapshot(&input["source"]).unwrap();
    assert!(
        Snapshot::from_data(
            &map(observed.data()).unwrap()["document"],
            CaptureOptions {
                context: Some(obj([("read_mode", s("supplied")), ("scenario", empty())])),
                ..Default::default()
            }
        )
        .is_err()
    );
    let mut data = observed.to_data();
    let hyp = map_mut(map_mut(&mut data).unwrap().get_mut("hypotheses").unwrap()).unwrap();
    let body = map_mut(
        map_mut(
            map_mut(hyp.get_mut("future").unwrap())
                .unwrap()
                .get_mut("document")
                .unwrap(),
        )
        .unwrap()
        .get_mut("known")
        .unwrap(),
    )
    .unwrap();
    body.insert(
        "p.input".into(),
        obj([("v", s(&"x".repeat(MAX_REQUEST_BYTES / 3)))]),
    );
    let m = map(&data).unwrap();
    let large = Snapshot::from_data(
        &m["document"],
        CaptureOptions {
            hypotheses: Some(m["hypotheses"].clone()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        build(
            &large,
            &obj([("future", V::List(vec![s("p.input")]))]),
            None
        )
        .is_err()
    );
}
