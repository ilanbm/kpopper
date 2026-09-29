use kpop_native::view_selection::{allocate, allocate_with_anchors, discover};
use serde_json::{Value as J, json};
use std::collections::BTreeSet;

// Fixed marginal cost isolates ranking/allocation; view_selection tests cover
// the real tokenizer and compact representation's cost accounting.
const COST: usize = 17;
fn count(_: &str) -> usize {
    1
}

fn graph(ids: &[&str], edges: &[(&str, &str, &str)]) -> J {
    json!({
        "schema":"kpopper.canonical-graph-view/v1",
        "project":"anchors","project_identity":["text","anchors"],
        "revision":"r1","scope":"fixture","mode":"expand",
        "nodes":ids.iter().map(|id| json!({"source_id":id,"body":["text",id],
            "status":null,"status_text":[],"dependencies":null,"uncertainty":[]})).collect::<Vec<_>>(),
        "groups":[],
        "links":edges.iter().map(|(from,to,rel)|json!({"source":{"from":from,"to":to,"rel":rel}})).collect::<Vec<_>>(),
        "coverage":{"source_ids":ids,"count":ids.len()},
        "navigation_membership":ids.iter().enumerate().map(|(n,id)|(id.to_string(),json!([format!("/group/{n}")]))).collect::<serde_json::Map<_,_>>(),
        "rules":"rules","descriptions":{"generated_claims":false}
    })
}

fn hits(ids: &[&str]) -> Vec<J> {
    ids.iter()
        .map(|id| json!({"id":id,"match":"lexical"}))
        .collect()
}

#[test]
fn no_anchors_preserves_all_old_selection_and_receipt_fields() {
    let full = graph(
        &["claim", "basis", "other"],
        &[("claim", "basis", "rests_on")],
    );
    let hits = hits(&["other", "claim"]);
    for budget in [0, COST, 2 * COST, usize::MAX] {
        let explicit = vec!["node:claim".into()];
        let old = allocate(&full, &hits, &explicit, budget, count).unwrap();
        let new = allocate_with_anchors(&full, &hits, &explicit, &[], budget, count).unwrap();
        assert_eq!(old.ids, new.selection.ids);
        assert_eq!(old.mandatory, new.selection.mandatory);
        assert_eq!(old.candidates, new.selection.candidates);
        assert_eq!(old.costs, new.selection.costs);
        assert_eq!(old.matched, new.selection.matched);
        assert_eq!(
            old.receipt(&json!({}), "followup", budget),
            new.receipt(&json!({}), "followup", budget, 1)
        );
        assert!(new.anchor_trace.ranking.is_empty());
    }
}

#[test]
fn related_anchor_improves_ambiguous_followup_without_forcing_a_reread() {
    let mut full = graph(
        &["global", "weak", "previous", "detail"],
        &[("previous", "detail", "rests_on")],
    );
    full["nodes"][0]["body"] = json!(["text", "When does delivery arrive?"]);
    full["nodes"][1]["body"] = json!(["text", "delivery"]);
    let hits = discover(&full, "delivery arrive", |s| s.len()).unwrap();
    assert_eq!(hits[0]["id"], "global");
    let old = allocate(&full, &hits, &[], 3 * COST, count).unwrap();
    let got =
        allocate_with_anchors(&full, &hits, &[], &["previous".into()], 3 * COST, count).unwrap();
    assert_eq!(old.ids, ["global", "weak"]);
    assert_eq!(got.selection.ids, ["global", "previous", "detail"]);
    assert!(got.selection.mandatory.is_empty());
    assert_eq!(got.selection.matched, hits.len());
    let detail = got
        .anchor_trace
        .ranking
        .iter()
        .find(|row| row.id == "detail")
        .unwrap();
    assert_eq!(detail.anchor.as_deref(), Some("previous"));
    assert_eq!(detail.via.as_deref(), Some("previous"));
    assert_eq!(detail.relation.as_deref(), Some("rests_on"));
    assert_eq!(detail.depth, 1);
    assert_eq!(detail.unanchored_rank, None);
    assert!(got.selection.candidates.iter().any(|(id, _)| id == "weak"));
    let empty = allocate_with_anchors(&full, &hits, &[], &["previous".into()], 0, count).unwrap();
    assert!(empty.selection.ids.is_empty());
}

#[test]
fn misleading_anchor_cannot_displace_strong_global_match_or_its_qualification() {
    let full = graph(
        &["answer", "exception", "wrong", "weak"],
        &[("exception", "answer", "exception")],
    );
    let got = allocate_with_anchors(
        &full,
        &hits(&["answer", "weak"]),
        &[],
        &["wrong".into()],
        2 * COST,
        count,
    )
    .unwrap();
    assert_eq!(got.selection.ids, ["answer", "exception"]);
    assert!(got.selection.candidates.iter().any(|(id, _)| id == "weak"));
    assert!(got.selection.candidates.iter().any(|(id, _)| id == "wrong"));
    let receipt = got.receipt(&json!({}), "followup", 2 * COST, 4);
    assert_eq!(receipt["unread_candidates"], 2);
    assert_eq!(receipt["frontier"]["group:/"]["anchor"], 1);
    assert_eq!(receipt["frontier"]["group:/"]["global_search"], 1);
}

#[test]
fn irrelevant_anchor_does_not_filter_global_discovery_or_infer_truth() {
    let mut full = graph(&["answer", "other", "unrelated", "sibling"], &[]);
    full["nodes"][2]["status"] = json!({"acceptance":"contested"});
    let before = full.clone();
    let hits = hits(&["answer", "other"]);
    let got =
        allocate_with_anchors(&full, &hits, &[], &["unrelated".into()], usize::MAX, count).unwrap();
    assert_eq!(got.selection.ids, ["answer", "unrelated", "other"]);
    assert_eq!(got.selection.matched, 2);
    assert!(got.selection.mandatory.is_empty());
    assert!(!got.selection.ids.iter().any(|id| id == "sibling"));
    assert_eq!(full, before);
}

#[test]
fn exact_rereads_and_their_evidence_precede_anchors_and_stay_mandatory() {
    let full = graph(
        &["exact", "basis", "explicit", "global", "anchor"],
        &[("exact", "basis", "rests_on")],
    );
    let hits = vec![
        json!({"id":"global","match":"lexical"}),
        json!({"id":"exact","match":"literal_id"}),
    ];
    let got = allocate_with_anchors(
        &full,
        &hits,
        &["node:explicit".into()],
        &["anchor".into()],
        0,
        count,
    )
    .unwrap();
    assert_eq!(got.selection.ids, ["exact", "explicit"]);
    assert_eq!(
        got.selection.mandatory,
        BTreeSet::from(["exact".into(), "explicit".into()])
    );
    assert_eq!(
        got.selection
            .candidates
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["explicit", "exact", "basis", "global", "anchor"]
    );
    let reread =
        allocate_with_anchors(&full, &[], &["anchor".into()], &["anchor".into()], 0, count)
            .unwrap();
    assert_eq!(reread.selection.ids, ["anchor"]);
}

#[test]
fn absent_retired_and_scoped_out_anchors_fail_visibly() {
    let mut full = graph(&["live", "retired", "scoped_out"], &[]);
    // The new captured revision/scope has no body for the old original IDs.
    full["nodes"].as_array_mut().unwrap().truncate(1);
    full["coverage"] = json!({"source_ids":["live"],"count":1});
    full["revision"] = json!("r2");
    for id in [
        "missing",
        "retired",
        "scoped_out",
        "node:live",
        "n0",
        "group:/",
    ] {
        let error = allocate_with_anchors(&full, &[], &[], &[id.into()], usize::MAX, count)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("anchor unavailable in captured revision/scope"),
            "{error}"
        );
        assert!(error.contains(id), "{error}");
    }
}

#[test]
fn original_ids_are_byte_exact_even_when_they_resemble_handles() {
    let full = graph(&["live", "node:live", "n0"], &[]);
    let got = allocate_with_anchors(
        &full,
        &[],
        &[],
        &["node:live".into(), "n0".into(), "node:live".into()],
        usize::MAX,
        count,
    )
    .unwrap();
    assert_eq!(got.selection.ids, ["node:live", "n0"]);
    assert_eq!(got.anchor_trace.anchors, ["node:live", "n0"]);
}

#[test]
fn deep_cross_group_qualifiers_and_cycles_have_no_id_or_depth_ceiling() {
    let names = (0..24).map(|n| format!("node.{n}")).collect::<Vec<_>>();
    let ids = names.iter().map(String::as_str).collect::<Vec<_>>();
    let mut edges = ids
        .windows(2)
        .map(|pair| (pair[0], pair[1], "rests_on"))
        .collect::<Vec<_>>();
    edges.push((ids[23], ids[0], "qualifies"));
    edges.push((ids[20], ids[2], "exception"));
    let full = graph(&ids, &edges);
    let got = allocate_with_anchors(&full, &[], &[], &[ids[0].into()], usize::MAX, count).unwrap();
    assert_eq!(got.selection.ids.len(), 24);
    assert_eq!(got.anchor_trace.ranking.len(), 24);
    assert!(got.anchor_trace.ranking.iter().any(|row| row.depth > 8));
    assert_eq!(got.selection.ids.iter().collect::<BTreeSet<_>>().len(), 24);
    assert!(got.selection.mandatory.is_empty());
}

#[test]
fn all_qualification_relations_are_followed_incoming_across_groups() {
    for relation in [
        "contradicts",
        "refutes",
        "supersedes",
        "exception",
        "qualifies",
    ] {
        let full = graph(
            &["anchor", "qualification", "unrelated"],
            &[
                ("qualification", "anchor", relation),
                ("unrelated", "anchor", "rests_on"),
            ],
        );
        let got =
            allocate_with_anchors(&full, &[], &[], &["anchor".into()], usize::MAX, count).unwrap();
        assert_eq!(got.selection.ids, ["anchor", "qualification"], "{relation}");
    }
}

#[test]
fn all_anchors_interleave_with_global_seeds_without_a_fixed_eight_id_cap() {
    let names = (0..12).map(|n| format!("anchor.{n}")).collect::<Vec<_>>();
    let mut ids = names.iter().map(String::as_str).collect::<Vec<_>>();
    ids.extend(["g0", "g1", "g2"]);
    let full = graph(&ids, &[]);
    let got = allocate_with_anchors(
        &full,
        &hits(&["g0", "g1", "g2"]),
        &[],
        &names,
        usize::MAX,
        count,
    )
    .unwrap();
    assert_eq!(
        &got.selection.ids[..5],
        ["g0", "anchor.0", "g1", "anchor.1", "g2"]
    );
    assert_eq!(got.selection.ids.len(), 15);
    assert!(got.selection.ids.contains(&"anchor.11".into()));
    let receipt = got.receipt(&json!({}), "followup", 10_000, 2);
    assert_eq!(
        receipt["anchor_ranking"]["anchors"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        receipt["anchor_ranking"]["ranking"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(receipt["anchor_ranking"]["omitted_anchor_ids"], 10);
    assert_eq!(receipt["anchor_ranking"]["omitted_diagnostic_rows"], 13);
    let quiet = got.receipt(&json!({}), "followup", 10_000, 0);
    assert_eq!(quiet["anchor_ranking"]["ranking"], json!([]));
    assert_eq!(quiet["anchor_ranking"]["omitted_diagnostic_rows"], 15);
}

#[test]
fn trace_records_reordering_and_reflects_later_budget_pruning() {
    let full = graph(&["first", "weak", "anchored"], &[]);
    let mut got = allocate_with_anchors(
        &full,
        &hits(&["first", "weak", "anchored"]),
        &[],
        &["anchored".into()],
        usize::MAX,
        count,
    )
    .unwrap();
    let row = got
        .anchor_trace
        .ranking
        .iter()
        .find(|row| row.id == "anchored")
        .unwrap();
    assert_eq!((row.unanchored_rank, row.rank), (Some(2), 1));
    got.selection.ids.retain(|id| id != "anchored");
    let receipt = got.receipt(&json!({}), "followup", 10_000, 8);
    let row = receipt["anchor_ranking"]["ranking"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "anchored")
        .unwrap();
    assert_eq!(row["selected"], false);
    assert_eq!(receipt["unread_candidates"], 1);
}

#[test]
fn large_anchor_body_does_not_hide_smaller_global_candidates() {
    let mut full = graph(&["first", "large_anchor", "later"], &[]);
    full["nodes"][1]["body"] = json!(["text", "noise ".repeat(10_000)]);
    let got = allocate_with_anchors(
        &full,
        &hits(&["first", "later"]),
        &[],
        &["large_anchor".into()],
        1_000,
        |s| s.len(),
    )
    .unwrap();
    assert_eq!(got.selection.ids, ["first", "later"]);
    assert!(
        got.selection
            .candidates
            .iter()
            .any(|(id, _)| id == "large_anchor")
    );
}

#[test]
fn visited_global_seed_does_not_consume_the_global_lead_slot() {
    let full = graph(
        &["exact", "g1", "g2", "wrong", "w1", "w2"],
        &[
            ("exact", "g1", "rests_on"),
            ("wrong", "w1", "rests_on"),
            ("wrong", "w2", "rests_on"),
        ],
    );
    let hits = vec![
        json!({"id":"g1","match":"lexical"}),
        json!({"id":"exact","match":"literal_id"}),
        json!({"id":"g2","match":"lexical"}),
    ];
    let got = allocate_with_anchors(&full, &hits, &[], &["wrong".into()], 5 * COST, count).unwrap();
    assert_eq!(got.selection.ids, ["exact", "g1", "g2", "wrong", "w1"]);
}

#[test]
fn visited_global_seed_does_not_consume_an_alternation_slot() {
    let full = graph(
        &["g0", "g1", "g2", "a0", "a1", "detail"],
        &[("a0", "g1", "rests_on"), ("a1", "detail", "rests_on")],
    );
    let got = allocate_with_anchors(
        &full,
        &hits(&["g0", "g1", "g2"]),
        &[],
        &["a0".into(), "a1".into()],
        4 * COST,
        count,
    )
    .unwrap();
    assert_eq!(got.selection.ids, ["g0", "a0", "g1", "g2"]);
}

#[test]
fn trace_selected_flags_agree_with_bodies_visible_through_group_focus() {
    let full = graph(
        &["global", "weak", "anchor", "detail"],
        &[("anchor", "detail", "rests_on")],
    );
    let got = allocate_with_anchors(
        &full,
        &hits(&["global", "weak"]),
        &[],
        &["anchor".into()],
        COST,
        count,
    )
    .unwrap();
    assert_eq!(got.selection.ids, ["global"]);
    let visible = json!({"nodes":[{"source_id":"anchor"}, {"source_id":"detail"}]});
    let receipt = got.receipt(&visible, "followup", COST, 8);
    assert_eq!(receipt["unread_candidates"], 1);
    let rows = receipt["anchor_ranking"]["ranking"].as_array().unwrap();
    for id in ["anchor", "detail"] {
        let row = rows.iter().find(|row| row["id"] == id).unwrap();
        assert_eq!(row["selected"], true, "{id}");
    }
}
