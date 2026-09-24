use kpop_native::{
    history_node_receipt::{Receipt, Side},
    history_transaction as T,
    value::TypedValue as V,
};
use serde_json::json;
use std::collections::BTreeMap;
fn map(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}
fn map_mut(v: &mut V) -> &mut BTreeMap<String, V> {
    let V::Map(m) = v else { panic!() };
    m
}

#[test]
fn reconstructs_exact_original_core_receipt_digests_and_typed_documents() {
    let mut count = 0;
    for raw in [
        include_str!("fixtures/history-authoring.json"),
        include_str!("fixtures/history-authoring-candidate.json"),
    ] {
        let data: serde_json::Value = serde_json::from_str(raw).unwrap();
        for case in data["cases"].as_array().unwrap() {
            let Some(receipt) = case.get("receipt") else {
                continue;
            };
            let receipt = V::from_tagged(receipt).unwrap();
            let packed =
                Receipt::pack(&receipt).unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            assert_eq!(packed.restore().unwrap(), receipt, "{}", case["name"]);
            for side in [packed.before(), packed.after()] {
                let literal = &map(side.context())["literal"];
                if let Some(report) = map(literal).get("assessment") {
                    assert!(map(&map(report)["nodes"]).is_empty());
                    assert_eq!(map(report)["selection"], V::Null);
                }
                if let Some(baseline) = map(literal)
                    .get("authoring")
                    .and_then(|a| map(a).get("baseline"))
                {
                    for name in ["heads", "open_acts"] {
                        if let Some(v) = map(baseline).get(name) {
                            assert!(map(v).is_empty());
                        }
                    }
                }
            }
            count += 1;
        }
    }
    assert!(count >= 10, "only {count} receipts");
}

fn receipt(size: usize) -> V {
    let mut doc = serde_json::Map::new();
    let mut nodes = serde_json::Map::new();
    let mut heads = serde_json::Map::new();
    let mut acts = serde_json::Map::new();
    for i in 0..size {
        let id = format!("p.{i:05}");
        doc.insert(id.clone(), json!({"v":i}));
        nodes.insert(id.clone(), json!({"value":i,"snapshot_id":"a".repeat(64)}));
        heads.insert(id.clone(), json!(["b".repeat(64)]));
        acts.insert(id, json!([]));
    }
    let selection = nodes.keys().cloned().collect::<Vec<_>>();
    let side = V::from_json(&json!({"kind":"authored-computational-projection/v1",
        "document":{"meta":{"purpose":"Fixture"},"known":doc},
        "assessment":{"snapshot_id":"a".repeat(64),"nodes":nodes,"selection":selection},
        "authoring":{"baseline":{"heads":heads,"open_acts":acts,"record_id":"fixture"}}
    }))
    .unwrap();
    T::semantic_receipt("core/v1", &V::Map(BTreeMap::new()), &side, &side).unwrap()
}
#[test]
fn graph_growth_does_not_grow_record_context_and_missing_pieces_fail_digest_validation() {
    let small = Receipt::pack(&receipt(10)).unwrap();
    let large = Receipt::pack(&receipt(1000)).unwrap();
    assert_eq!(small.before().context(), large.before().context());
    assert_eq!(large.before().nodes().len(), 1000);
    let mut nodes = large.before().nodes().clone();
    nodes.remove("p.00000");
    let side = Side::from_parts(large.before().context().clone(), nodes).unwrap();
    assert!(Receipt::from_parts(large.header().clone(), side, large.after().clone()).is_err());
}
#[test]
fn mutated_context_or_node_evidence_cannot_satisfy_original_receipt() {
    let source = receipt(2);
    let packed = Receipt::pack(&source).unwrap();
    let mut nodes = packed.after().nodes().clone();
    let node = map_mut(nodes.get_mut("p.00000").unwrap());
    let V::List(pair) = node.get_mut("document").unwrap() else {
        panic!()
    };
    pair[1] = V::from_json(&json!({"v":99})).unwrap();
    let side = Side::from_parts(packed.after().context().clone(), nodes).unwrap();
    assert!(Receipt::from_parts(packed.header().clone(), packed.before().clone(), side).is_err());
    let mut unsupported = map(&source).clone();
    map_mut(unsupported.get_mut("before").unwrap())
        .insert("temporal_replay".into(), V::Map(BTreeMap::new()));
    let unsupported = T::semantic_receipt(
        "core/v1",
        &unsupported["capabilities"],
        &unsupported["before"],
        &unsupported["after"],
    )
    .unwrap();
    assert!(Receipt::pack(&unsupported).is_err());
}

#[test]
fn unsupported_nested_authoring_reports_are_not_retained_as_record_context() {
    let source = receipt(2);
    for name in ["objects", "steps", "actions", "graph"] {
        let mut before = map(&source)["before"].clone();
        map_mut(map_mut(&mut before).get_mut("authoring").unwrap()).insert(
            name.into(),
            map(&map(&source)["before"])["document"].clone(),
        );
        let changed = T::semantic_receipt(
            "core/v1",
            &map(&source)["capabilities"],
            &before,
            &map(&source)["after"],
        )
        .unwrap();
        assert!(Receipt::pack(&changed).is_err(), "retained {name}");
    }
    let packed = Receipt::pack(&source).unwrap();
    let mut context = packed.before().context().clone();
    let literal = map_mut(map_mut(&mut context).get_mut("literal").unwrap());
    literal.insert(
        "document".into(),
        map(&map(&source)["before"])["document"].clone(),
    );
    assert!(Side::from_parts(context, packed.before().nodes().clone()).is_err());
}
