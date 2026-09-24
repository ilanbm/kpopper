use kpop_native::{
    history_node_receipt::{Nodes, Receipt, Side},
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

#[test]
fn inactive_baseline_components_do_not_rewrite_unchanged_nodes() {
    let source = receipt(1000);
    let before = map(&source)["before"].clone();
    let mut after = before.clone();
    map_mut(&mut after).remove("authoring");
    let source =
        T::semantic_receipt("core/v1", &map(&source)["capabilities"], &before, &after).unwrap();
    let packed = Receipt::pack(&source).unwrap();
    let mut retained = Nodes::new();
    let first = retained.prepare(packed.before()).unwrap();
    assert_eq!(first.len(), 1000);
    retained.apply(&first).unwrap();
    assert!(retained.prepare(packed.after()).unwrap().is_empty());
    assert_eq!(
        retained
            .side(packed.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        after
    );
    assert_eq!(
        retained
            .side(packed.before().context())
            .unwrap()
            .restore()
            .unwrap(),
        before
    );

    let mut changed = after.clone();
    let doc = map_mut(map_mut(&mut changed).get_mut("document").unwrap());
    map_mut(doc.get_mut("known").unwrap())
        .insert("p.00000".into(), V::from_json(&json!({"v":99})).unwrap());
    let source =
        T::semantic_receipt("core/v1", &map(&source)["capabilities"], &before, &changed).unwrap();
    let changed = Receipt::pack(&source).unwrap();
    let updates = retained.prepare(changed.after()).unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].subject, "p.00000");
    retained.apply(&updates).unwrap();
    assert_eq!(
        retained
            .side(changed.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        map(&source)["after"]
    );
}

#[test]
fn node_deletion_stale_before_images_and_branch_preparation_are_exact() {
    let source = receipt(2);
    let packed = Receipt::pack(&source).unwrap();
    let mut base = Nodes::new();
    base.apply(&base.prepare(packed.before()).unwrap()).unwrap();
    let mut next = map(&source)["after"].clone();
    map_mut(&mut next).remove("authoring");
    let doc = map_mut(map_mut(&mut next).get_mut("document").unwrap());
    map_mut(doc.get_mut("known").unwrap()).remove("p.00000");
    let report = map_mut(map_mut(&mut next).get_mut("assessment").unwrap());
    map_mut(report.get_mut("nodes").unwrap()).remove("p.00000");
    report.insert(
        "selection".into(),
        V::from_json(&json!(["p.00001"])).unwrap(),
    );
    let next = T::semantic_receipt(
        "core/v1",
        &map(&source)["capabilities"],
        &map(&source)["before"],
        &next,
    )
    .unwrap();
    let packed_next = Receipt::pack(&next).unwrap();
    let changes = base.prepare(packed_next.after()).unwrap();
    assert_eq!(changes.len(), 1);
    let mut branch = base.clone();
    branch.apply(&changes).unwrap();
    assert_eq!(
        branch
            .side(packed_next.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        map(&next)["after"]
    );
    assert_eq!(
        base.side(packed.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        map(&source)["after"]
    );
    let frozen = branch.values().clone();
    assert!(branch.apply(&changes).is_err());
    assert_eq!(*branch.values(), frozen);
    // Reversing the exact images restores the previous branch; no ordering by time is involved.
    let reverse = changes
        .iter()
        .map(|c| kpop_native::history_node_receipt::Change {
            subject: c.subject.clone(),
            before: c.after.clone(),
            after: c.before.clone(),
        })
        .collect::<Vec<_>>();
    branch.apply(&reverse).unwrap();
    assert_eq!(branch.values(), base.values());
}
