use kpop_native::view_selection::{allocate, discover};
use kpop_native::{reasoning_projection, tokenizer::Encoding, value::TypedValue as V};
use serde_json::{Value as J, json};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

fn full() -> J {
    let mut nodes = Vec::new();
    let mut links = Vec::new();
    let mut navigation_membership = serde_json::Map::new();
    for n in 0..14 {
        let id = format!("claim.{n}");
        nodes.push(
            json!({"source_id":id,"body":["map",[["name",["text",format!("source {n}")]]]],
            "status":null,"status_text":[],"dependencies":null,"uncertainty":[]}),
        );
        navigation_membership.insert(id.clone(), json!(["/claims"]));
        if n < 11 {
            links
                .push(json!({"source":{"from":id,"to":format!("claim.{}", n+1),"rel":"rests_on"}}));
        }
    }
    let mut source_ids = (0..14).map(|n| format!("claim.{n}")).collect::<Vec<_>>();
    source_ids.sort();
    json!({
        "schema":"kpopper.canonical-graph-view/v1",
        "project":"selection-fixture",
        "project_identity":["map",[["project",["text","selection-fixture"]]]],
        "revision":"r1","scope":"fixture","mode":"expand",
        "nodes":nodes,"groups":[],"links":links,
        "coverage":{"source_ids":source_ids,"count":14},
        "navigation_membership":navigation_membership,
        "rules":"rules","descriptions":{"generated_claims":false}
    })
}

#[test]
fn evidence_chain_can_exceed_eight_nodes_and_three_navigation_groups() {
    let graph = full();
    let hits = vec![
        json!({"id":"claim.0","match":"literal_id"}),
        json!({"id":"claim.13","match":"lexical"}),
    ];
    let got = allocate(&graph, &hits, &[], 20_000, |s| s.len()).unwrap();
    assert_eq!(got.ids.len(), 13);
    assert!(got.ids.contains(&"claim.11".into()));
    assert!(
        got.ids.contains(&"claim.13".into()),
        "global qualification is not dependent on graph connectivity"
    );
    assert!(!got.ids.contains(&"claim.12".into()));
}

#[test]
fn a_large_lexical_distraction_does_not_hide_smaller_later_candidates() {
    let mut graph = full();
    graph["nodes"][12]["body"] = json!(["text", "noise ".repeat(10_000)]);
    let hits = vec![
        json!({"id":"claim.12","match":"lexical"}),
        json!({"id":"claim.13","match":"lexical"}),
    ];
    let got = allocate(&graph, &hits, &[], 500, |s| s.len()).unwrap();
    assert_eq!(got.ids, ["claim.13"]);
    assert_eq!(
        got.candidates.len(),
        2,
        "unread candidate remains an explicit frontier"
    );
}

#[test]
fn exact_requested_evidence_is_never_silently_cropped_or_dropped() {
    let graph = full();
    let got = allocate(&graph, &[], &["node:claim.0".into()], 0, |s| s.len()).unwrap();
    assert_eq!(got.ids, ["claim.0"]);
    assert!(got.mandatory.contains("claim.0"));
    assert!(allocate(&graph, &[], &["missing".into()], 100, |s| s.len()).is_err());
}

#[test]
fn frontier_uses_folded_groups_instead_of_repeating_all_unread_ids() {
    let graph = full();
    let hits = vec![json!({"id":"claim.0","match":"lexical"})];
    let got = allocate(&graph, &hits, &[], 0, |s| s.len()).unwrap();
    let visible = json!({"nodes":[],"groups":[{"group_id":"group:/claims","member_source_ids":(0..14).map(|n| format!("claim.{n}")).collect::<Vec<_>>() }]});
    let receipt = got.receipt(&visible, "ראיות evidence", 16_000);
    assert_eq!(receipt["unread_candidates"], 12);
    assert_eq!(receipt["frontier"].as_object().unwrap().len(), 1);
    assert!(
        receipt["scope"]
            .as_str()
            .unwrap()
            .contains("do not establish absence")
    );
}

#[test]
fn incoming_exceptions_and_cycles_are_visible_and_terminate() {
    let mut graph = full();
    graph["links"] = json!([
        {"source":{"from":"claim.1","to":"claim.0","rel":"exception"}},
        {"source":{"from":"claim.0","to":"claim.2","rel":"rests_on"}},
        {"source":{"from":"claim.2","to":"claim.0","rel":"rests_on"}}
    ]);
    let got = allocate(
        &graph,
        &[json!({"id":"claim.0","match":"literal_id"})],
        &[],
        20_000,
        |s| s.len(),
    )
    .unwrap();
    assert_eq!(got.ids.len(), 3);
    assert!(got.ids.contains(&"claim.1".into()));
}

fn duplicate_display_graph(count: usize, repeated: &str) -> J {
    let quote = format!("{repeated}\nsource quote");
    let value_text = reasoning_projection::render_value(&V::Map(BTreeMap::from([
        ("type".into(), V::Text("text".into())),
        ("value".into(), V::Text(quote.clone())),
    ])))
    .unwrap();
    let status = json!({
        "acceptance":"accepted",
        "computation":{"status":"ok","truth":true,"value_text":value_text},
        "basis":"not_applicable",
        "falsifier":{"status":"not_applicable","holds":null},
        "contention":"none_detected",
        "integrity":"assessed",
        "coverage":"complete",
        "coverage_included":true,
        "assurance":"computed",
        "recorded_evidence_kinds":[],
        "support":{"status":"clear","states":[repeated]}
    });
    let status_text =
        reasoning_projection::render_projected_status(&V::from_json(&status).unwrap()).unwrap();
    let mut nodes = Vec::new();
    let mut navigation = serde_json::Map::new();
    for index in 0..count {
        let id = format!("source.{index}");
        let body = json!([
            "map",
            [
                ["name", ["text", format!("candidate {index}")]],
                ["quoted", ["text", quote]]
            ]
        ]);
        nodes.push(
            json!({"source_id":id,"body":body,"status":status,"status_text":status_text,
            "dependencies":null,"uncertainty":[]}),
        );
        navigation.insert(id, json!(["/candidates"]));
    }
    let ids = (0..count)
        .map(|index| format!("source.{index}"))
        .collect::<Vec<_>>();
    json!({
        "schema":"kpopper.canonical-graph-view/v1",
        "project":"selection-cost-fixture",
        "project_identity":["map",[["project",["text","selection-cost-fixture"]]]],
        "revision":"r1","scope":"fixture","mode":"expand",
        "nodes":nodes,"groups":[],"links":[],
        "coverage":{"source_ids":ids,"count":count},
        "navigation_membership":navigation
    })
}

fn v3_selection(full: &J, visible: &[String]) -> J {
    let visible: BTreeSet<_> = visible.iter().cloned().collect();
    let mut packet = full.clone();
    packet["nodes"] = json!(
        full["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| visible.contains(row["source_id"].as_str().unwrap()))
            .cloned()
            .collect::<Vec<_>>()
    );
    let residual = full["coverage"]["source_ids"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(J::as_str)
        .filter(|id| !visible.contains(*id))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    packet["groups"] = if residual.is_empty() {
        json!([])
    } else {
        json!([{"group_id":"group:/claims","path":"/claims","label":"claims","member_source_ids":residual}])
    };
    packet["mode"] = json!(if visible.is_empty() { "broad" } else { "focus" });
    kpop_native::canonical_view::compact(&packet, &[]).unwrap()
}

#[test]
fn allocation_budgets_against_v3_rows_without_recharging_removed_display_text() {
    let graph = duplicate_display_graph(4, &"long repeated assessment ".repeat(32));
    let hits = (0..4)
        .map(|index| json!({"id":format!("source.{index}"),"match":"lexical"}))
        .collect::<Vec<_>>();
    let count = |text: &str| Encoding::O200kBase.count(text);
    let all = allocate(&graph, &hits, &[], usize::MAX, count).unwrap();

    let mut budget = 0usize;
    let mut legacy_cost = 0usize;
    for index in 0..3 {
        let id = format!("source.{index}");
        budget += all.costs[&id];
        let row = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["source_id"] == id)
            .unwrap();
        legacy_cost += count(&serde_json::to_string(row).unwrap()).saturating_add(16);
        assert!(all.costs[&id] < count(&serde_json::to_string(row).unwrap()));
    }
    assert!(
        legacy_cost > budget,
        "old duplicated status_text would price fewer nodes"
    );

    let selected = allocate(&graph, &hits, &[], budget, count).unwrap();
    assert_eq!(selected.ids, ["source.0", "source.1", "source.2"]);
    assert!(
        selected
            .ids
            .iter()
            .map(|id| selected.costs[id])
            .sum::<usize>()
            <= budget
    );
    for id in &selected.ids {
        assert!(
            graph["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["source_id"].as_str() == Some(id.as_str()))
        );
    }
    let base_tokens = count(&(serde_json::to_string(&v3_selection(&graph, &[])).unwrap() + "\n"));
    let final_v3 = v3_selection(&graph, &selected.ids);
    let final_tokens = count(&(serde_json::to_string(&final_v3).unwrap() + "\n"));
    assert!(final_tokens <= base_tokens + budget);
    let rendered_ids = final_v3["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            final_v3["dictionary"][row[0].as_str().unwrap()]["original"]
                .as_str()
                .unwrap()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        rendered_ids,
        selected.ids.iter().map(String::as_str).collect()
    );
}

#[test]
fn global_discovery_keeps_exact_ids_hebrew_and_declared_aliases() {
    let graph = json!({"project":"language","revision":"r1","nodes":[
        {"source_id":"rule.delivery","body":["map",[["name",["text","Shipping delay / עיכוב משלוח"]],["aliases",["text","dispatch postponement"]]]]},
        {"source_id":"rule.delivery2","body":["text","Unrelated inventory"]}
    ]});
    for query in ["עיכוב משלוח", "dispatch postponement", "rule.delivery"] {
        let hits = discover(&graph, query, |s| s.len()).unwrap();
        assert_eq!(hits[0]["id"], "rule.delivery", "{query}");
    }
    let hits = discover(&graph, "rule.delivery", |s| s.len()).unwrap();
    assert_eq!(hits[0]["match"], "literal_id");
    assert!(
        discover(&graph, "unrepresented_query_zxq", |s| s.len())
            .unwrap()
            .is_empty()
    );
}
