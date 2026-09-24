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

#[test]
fn reconstructs_original_named_hypothesis_and_fold_fixture_receipts() {
    let mut count = 0;
    for raw in [
        include_str!("fixtures/history-hypothesis.json"),
        include_str!("fixtures/history-hypothesis-retained.json"),
        include_str!("fixtures/history-hypothesis-candidate.json"),
    ] {
        let data: serde_json::Value = serde_json::from_str(raw).unwrap();
        for case in data["cases"].as_array().unwrap() {
            let Some(receipt) = case.get("receipt") else {
                continue;
            };
            let receipt = V::from_tagged(receipt).unwrap();
            if case["name"] == "unrelated-physical" {
                assert!(Receipt::pack(&receipt).is_err());
                continue;
            }
            let packed =
                Receipt::pack(&receipt).unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            assert_eq!(packed.restore().unwrap(), receipt, "{}", case["name"]);
            count += 1;
        }
    }
    assert!(count >= 20, "only {count} hypothesis receipts");
}

#[test]
fn named_fold_receipt_accepts_dependency_reason_drops() {
    let source = receipt(1);
    let mut before = map(&source)["before"].clone();
    let mut after = map(&source)["after"].clone();
    let fold = V::from_json(&json!({
        "kind":"fold",
        "names":["trial"],
        "because":"explicit fold decision",
        "take":["trial"],
        "drops":{"p.00000":"kept by trial"},
        "assessment_version":1
    }))
    .unwrap();
    map_mut(&mut before).remove("authoring");
    map_mut(&mut after).remove("authoring");
    map_mut(&mut before).insert("hypothesis_authoring".into(), fold.clone());
    map_mut(&mut after).insert("hypothesis_authoring".into(), fold);
    let source =
        T::semantic_receipt("core/v1", &map(&source)["capabilities"], &before, &after).unwrap();
    let packed = Receipt::pack(&source).unwrap();
    assert_eq!(packed.restore().unwrap(), source);
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

#[test]
fn named_hypothesis_bodies_are_node_local_and_restore_exactly() {
    let source = receipt(2);
    let mut before = map(&source)["before"].clone();
    let mut after = map(&source)["after"].clone();
    for side in [&mut before, &mut after] {
        let report = map_mut(map_mut(side).get_mut("assessment").unwrap());
        report.insert(
            "scope".into(),
            V::from_json(&json!({"context":{"conflicts":{}},"hypotheses":{}})).unwrap(),
        );
        let scope = map_mut(report.get_mut("scope").unwrap());
        let doc_a = V::from_json(&json!({
            "known":{"p.00000":{"v":10}},
            "readings":{"p.00001":{"v":11}},
            "open":{"p.list":["typed",3]}
        }))
        .unwrap();
        let doc_b = V::from_json(&json!({"known":{"p.00000":{"v":20}}})).unwrap();
        scope.insert("hypotheses".into(), V::from_json(&json!({
            "first":{"kind":"named-history-hypothesis/v1","status":"inspected","document":doc_a.to_json().unwrap()},
            "second":{"kind":"named-history-hypothesis/v1","status":"inspected","document":doc_b.to_json().unwrap()}
        })).unwrap());
        map_mut(scope.get_mut("context").unwrap()).insert(
            "conflicts".into(),
            V::from_json(&json!({"p.00000":[["first",{"v":10}],["second",{"v":20}]]})).unwrap(),
        );
    }
    let source =
        T::semantic_receipt("core/v1", &map(&source)["capabilities"], &before, &after).unwrap();
    let packed = Receipt::pack(&source).unwrap();
    assert_eq!(packed.restore().unwrap(), source);
    let literal = &map(packed.after().context())["literal"];
    let assessment = &map(literal)["assessment"];
    let scope = &map(assessment)["scope"];
    assert!(map(&map(scope)["hypotheses"]).values().all(|h| {
        map(h)["document"].to_json().unwrap()["known"]
            .as_object()
            .unwrap()
            .is_empty()
    }));
    let node = map(packed.after().nodes().get("p.00000").unwrap());
    assert_eq!(map(&node["hypotheses"]).len(), 2);
    assert!(node.contains_key("conflicts"));
    let list_piece = map(packed.after().nodes().get("p.list").unwrap());
    assert_eq!(
        map(&map(&list_piece["hypotheses"])["first"])["open"],
        V::from_json(&json!(["typed", 3])).unwrap()
    );

    let mut retained = Nodes::new();
    retained
        .apply(&retained.prepare(packed.before()).unwrap())
        .unwrap();
    let mut next = map(&source)["after"].clone();
    let report = map_mut(map_mut(&mut next).get_mut("assessment").unwrap());
    let scope = map_mut(report.get_mut("scope").unwrap());
    for name in ["first", "second"] {
        let h = map_mut(
            map_mut(scope.get_mut("hypotheses").unwrap())
                .get_mut(name)
                .unwrap(),
        );
        let doc = map_mut(h.get_mut("document").unwrap());
        if let Some(known) = doc.get_mut("known") {
            map_mut(known).remove("p.00000");
        }
    }
    let next = T::semantic_receipt(
        "core/v1",
        &map(&source)["capabilities"],
        &map(&source)["before"],
        &next,
    )
    .unwrap();
    let packed_next = Receipt::pack(&next).unwrap();
    let changes = retained.prepare(packed_next.after()).unwrap();
    assert!(changes.iter().any(|change| change.subject == "p.00000"));
    retained.apply(&changes).unwrap();
    assert!(!map(retained.values().get("p.00000").unwrap()).contains_key("hypotheses"));
    assert_eq!(
        retained
            .side(packed_next.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        map(&next)["after"]
    );

    // A side with the component inactive must leave the retained prior node piece intact.
    let frozen = retained.values().clone();
    let mut inactive = map(&next)["after"].clone();
    let report = map_mut(map_mut(&mut inactive).get_mut("assessment").unwrap());
    let scope = map_mut(report.get_mut("scope").unwrap());
    scope.remove("hypotheses");
    let inactive = T::semantic_receipt(
        "core/v1",
        &map(&next)["capabilities"],
        &map(&next)["before"],
        &inactive,
    )
    .unwrap();
    let inactive = Receipt::pack(&inactive).unwrap();
    assert!(retained.prepare(inactive.after()).unwrap().is_empty());
    assert_eq!(retained.values(), &frozen);

    let mut tampered = packed.after().nodes().clone();
    map_mut(
        map_mut(tampered.get_mut("p.00000").unwrap())
            .get_mut("hypotheses")
            .unwrap(),
    )
    .remove("first");
    let tampered = Side::from_parts(packed.after().context().clone(), tampered).unwrap();
    assert!(
        Receipt::from_parts(packed.header().clone(), packed.before().clone(), tampered).is_err()
    );
    let mut tampered = packed.after().context().clone();
    let report = map_mut(
        map_mut(map_mut(&mut tampered).get_mut("literal").unwrap())
            .get_mut("assessment")
            .unwrap(),
    );
    let group = map_mut(
        map_mut(report.get_mut("scope").unwrap())
            .get_mut("hypotheses")
            .unwrap(),
    );
    map_mut(group.get_mut("second").unwrap()).insert(
        "document".into(),
        V::from_json(&json!({"unknown":{}})).unwrap(),
    );
    assert!(Side::from_parts(tampered, packed.after().nodes().clone()).is_err());
    let mut tampered = packed.after().context().clone();
    let report = map_mut(
        map_mut(map_mut(&mut tampered).get_mut("literal").unwrap())
            .get_mut("assessment")
            .unwrap(),
    );
    let group = map_mut(
        map_mut(report.get_mut("scope").unwrap())
            .get_mut("hypotheses")
            .unwrap(),
    );
    let document = map_mut(
        map_mut(group.get_mut("first").unwrap())
            .get_mut("document")
            .unwrap(),
    );
    map_mut(document.get_mut("open").unwrap())
        .insert("p.list".into(), V::from_json(&json!(["typed", 3])).unwrap());
    assert!(Side::from_parts(tampered, packed.after().nodes().clone()).is_err());
}

#[test]
fn proposal_world_is_partitioned_by_node_and_nested_graphs_refuse() {
    let source = receipt(100);
    let mut after = map(&source)["after"].clone();
    let mut proposal = after.clone();
    map_mut(&mut proposal).remove("authoring");
    map_mut(&mut after).insert("proposal".into(), proposal.clone());
    let source = T::semantic_receipt(
        "core/v1",
        &map(&source)["capabilities"],
        &map(&source)["before"],
        &after,
    )
    .unwrap();
    let packed = Receipt::pack(&source).unwrap();
    assert_eq!(packed.restore().unwrap(), source);
    let context = &map(packed.after().context())["proposal"];
    let literal = &map(context)["literal"];
    assert!(
        map(&map(literal)["document"])
            .get("known")
            .is_some_and(|v| map(v).is_empty())
    );
    assert!(
        map(&map(literal)["assessment"])["nodes"]
            .to_json()
            .unwrap()
            .as_object()
            .unwrap()
            .is_empty()
    );
    let mut nodes = Nodes::new();
    nodes
        .apply(&nodes.prepare(packed.before()).unwrap())
        .unwrap();
    nodes
        .apply(&nodes.prepare(packed.after()).unwrap())
        .unwrap();
    assert_eq!(
        nodes
            .side(packed.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        after
    );
    let mut forged = packed.after().context().clone();
    map_mut(
        map_mut(map_mut(&mut forged).get_mut("proposal").unwrap())
            .get_mut("literal")
            .unwrap(),
    )
    .insert("document".into(), map(&proposal)["document"].clone());
    assert!(Side::from_parts(forged, packed.after().nodes().clone()).is_err());
    let nested_proposal = proposal.clone();
    map_mut(&mut proposal).insert("proposal".into(), nested_proposal);
    map_mut(&mut after).insert("proposal".into(), proposal);
    let nested = T::semantic_receipt(
        "core/v1",
        &map(&source)["capabilities"],
        &map(&source)["before"],
        &after,
    )
    .unwrap();
    assert!(Receipt::pack(&nested).is_err());
}

#[test]
fn temporal_receipts_partition_snapshots_and_restore_exact_digests() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/temporal-authoring.json")).unwrap();
    let mut count = 0;
    let mut nested = false;
    for case in data["cases"].as_array().unwrap() {
        let Some(receipt) = case.get("receipt") else {
            continue;
        };
        // Historical batch versions below 6 are not admitted by the node writer.
        if case["family"] == "batch" && case["version"].as_u64().unwrap() < 6 {
            continue;
        }
        let receipt = V::from_tagged(receipt).unwrap();
        let packed = Receipt::pack(&receipt).unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
        assert_eq!(packed.restore().unwrap(), receipt, "{}", case["name"]);
        for side in [packed.before(), packed.after()] {
            assert!(!map(&map(side.context())["literal"]).contains_key("temporal_replay"));
            if let Some(temporal) = map(side.context()).get("temporal") {
                assert!(!map(temporal).contains_key("claims"));
                assert!(!map(&map(temporal)["snapshot"]).contains_key("nodes"));
                assert!(!map(&map(temporal)["snapshot"]).contains_key("document"));
            }
        }
        if map(&map(&receipt)["after"])
            .get("proposal")
            .is_some_and(|p| map(p).contains_key("temporal_replay"))
        {
            nested = true;
            let mut tampered = packed.after().context().clone();
            let proposal = map_mut(map_mut(&mut tampered).get_mut("proposal").unwrap());
            let temporal = map_mut(proposal.get_mut("temporal").unwrap());
            let V::Bool(reuse) = temporal["document_from_side"] else {
                panic!()
            };
            temporal.insert("document_from_side".into(), V::Bool(!reuse));
            assert!(Side::from_parts(tampered, packed.after().nodes().clone()).is_err());
            let context = map(packed.after().context());
            assert!(context.contains_key("temporal"));
            assert!(map(&context["proposal"]).contains_key("temporal"));
            assert!(packed.after().nodes().values().any(|node| {
                map(node)
                    .get("proposal")
                    .is_some_and(|p| map(p).contains_key("temporal"))
            }));
        }
        count += 1;
    }
    assert!(nested, "missing nested temporal proposal coverage");
    assert!(count >= 15, "only {count} receipts");
}

#[test]
fn temporal_clock_changes_do_not_rewrite_world_membership() {
    use kpop_native::reasoning_snapshot::{CaptureOptions, Snapshot};
    let mut document = V::from_json(&json!({"meta":{},"known":{}})).unwrap();
    for i in 0..100 {
        map_mut(map_mut(&mut document).get_mut("known").unwrap())
            .insert(format!("p.{i:03}"), V::from_json(&json!({"v":i})).unwrap());
    }
    let side = |doc: &V, day: &str| {
        let snapshot = Snapshot::from_data(
            doc,
            CaptureOptions {
                as_of: Some(V::Text(day.into())),
                ..Default::default()
            },
        )
        .unwrap();
        V::Map(BTreeMap::from([(
            "temporal_replay".into(),
            V::from_json(&json!({
                "version":1,"snapshot":snapshot.to_json().unwrap(),"claims":{}
            }))
            .unwrap(),
        )]))
    };
    let before = side(&document, "2026-09-24");
    let after = side(&document, "2026-09-25");
    let receipt =
        T::semantic_receipt("core/v1", &V::Map(BTreeMap::new()), &before, &after).unwrap();
    let packed = Receipt::pack(&receipt).unwrap();
    let mut changed_flag = packed.after().context().clone();
    map_mut(map_mut(&mut changed_flag).get_mut("temporal").unwrap())
        .insert("document_from_side".into(), V::Bool(true));
    assert!(Side::from_parts(changed_flag, packed.after().nodes().clone()).is_err());
    let mut nodes = Nodes::new();
    nodes
        .apply(&nodes.prepare(packed.before()).unwrap())
        .unwrap();
    assert!(nodes.prepare(packed.after()).unwrap().is_empty());
    assert_eq!(
        nodes
            .side(packed.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        after
    );
    map_mut(map_mut(&mut document).get_mut("known").unwrap())
        .insert("p.000".into(), V::from_json(&json!({"v":999})).unwrap());
    let changed = side(&document, "2026-09-25");
    let receipt =
        T::semantic_receipt("core/v1", &V::Map(BTreeMap::new()), &before, &changed).unwrap();
    let packed = Receipt::pack(&receipt).unwrap();
    let changes = nodes.prepare(packed.after()).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].subject, "p.000");
    // Rehashed snapshot metadata still cannot authorize mismatched node pieces.
    let mut context = packed.after().context().clone();
    let temporal = map_mut(map_mut(&mut context).get_mut("temporal").unwrap());
    map_mut(temporal.get_mut("snapshot").unwrap())
        .insert("as_of".into(), V::Text("2026-09-26".into()));
    assert!(Side::from_parts(context, packed.after().nodes().clone()).is_err());
}

#[test]
fn noncanonical_temporal_snapshot_bytes_refuse_instead_of_normalizing() {
    use kpop_native::reasoning_snapshot::{CaptureOptions, Snapshot};
    let snapshot = Snapshot::from_data(
        &V::from_json(&json!({"known":{"p.a":{"v":1}}})).unwrap(),
        CaptureOptions::default(),
    )
    .unwrap();
    let tagged: serde_json::Value = serde_json::from_str(&snapshot.to_json().unwrap()).unwrap();
    let raw = serde_json::to_string_pretty(&tagged).unwrap();
    assert!(Snapshot::from_json(raw.as_bytes()).is_ok());
    let side =
        V::from_json(&json!({"temporal_replay":{"version":1,"snapshot":raw,"claims":{}}})).unwrap();
    let receipt = T::semantic_receipt("core/v1", &V::Map(BTreeMap::new()), &side, &side).unwrap();
    assert_eq!(
        Receipt::pack(&receipt).unwrap_err().0,
        "node_temporal_snapshot_encoding"
    );
}

#[test]
fn first_temporal_snapshot_reuses_the_same_side_document() {
    use kpop_native::reasoning_snapshot::{CaptureOptions, Snapshot};
    let document = V::from_json(&json!({"known":{"p.a":{"v":1},"p.b":{"v":2}}})).unwrap();
    let snapshot = Snapshot::from_data(&document, CaptureOptions::default()).unwrap();
    let before = V::Map(BTreeMap::from([("document".into(), document.clone())]));
    let mut after = before.clone();
    map_mut(&mut after).insert(
        "temporal_replay".into(),
        V::from_json(&json!({
            "version":1,"snapshot":snapshot.to_json().unwrap(),"claims":{}
        }))
        .unwrap(),
    );
    let receipt =
        T::semantic_receipt("core/v1", &V::Map(BTreeMap::new()), &before, &after).unwrap();
    let packed = Receipt::pack(&receipt).unwrap();
    for mutation in ["flag", "missing_document"] {
        let mut context = packed.after().context().clone();
        if mutation == "flag" {
            map_mut(map_mut(&mut context).get_mut("temporal").unwrap())
                .insert("document_from_side".into(), V::Bool(false));
        } else {
            map_mut(map_mut(&mut context).get_mut("literal").unwrap()).remove("document");
        }
        assert!(
            Side::from_parts(context, packed.after().nodes().clone()).is_err(),
            "accepted {mutation}"
        );
    }
    let mut nodes = Nodes::new();
    nodes
        .apply(&nodes.prepare(packed.before()).unwrap())
        .unwrap();
    assert!(
        nodes.prepare(packed.after()).unwrap().is_empty(),
        "unchanged document copied into temporal node pieces"
    );
    assert_eq!(
        nodes
            .side(packed.after().context())
            .unwrap()
            .restore()
            .unwrap(),
        after
    );
}
