use kpop_native::{
    canonical_view::CanonicalViewRequest, checked_session::CheckedSession,
    reasoning_context::CapturedAssessment, value::TypedValue as V,
};
use serde_json::{Value as J, json};

fn session() -> CheckedSession {
    let corpus: J = serde_json::from_str(include_str!("fixtures/reasoning-context.json")).unwrap();
    let data = corpus["contexts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "all-None")
        .unwrap();
    let context = CapturedAssessment::from_data(&data["serialized"]).unwrap();
    CheckedSession::new(
        context,
        V::from_json(
            &json!({"version":1,"project":"fixture","input_path":"/portable/GROUNDING.yaml"}),
        )
        .unwrap(),
        None,
    )
    .unwrap()
}

#[test]
fn broad_focus_expand_share_lossless_graph_and_revision() {
    let checked = session();
    let example: J = serde_json::from_str(include_str!("fixtures/canonical-view-example.json")).unwrap();
    let broad = checked
        .canonical_view(checked.revision(), &CanonicalViewRequest::default())
        .unwrap();
    assert_eq!(broad["schema"], "kpopper.canonical-graph-view/v1");
    assert_eq!(
        broad["project_identity"],
        checked.project_identity().to_tagged().unwrap()
    );
    assert_eq!(broad["revision"], checked.revision());
    assert!(broad["scope"].as_str().is_some());
    assert!(
        broad["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["body"].is_array() && n["body"][0] == "map")
    );
    let flattened = broad["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["source_id"].as_str().unwrap().to_owned())
        .chain(broad["groups"].as_array().unwrap().iter().flat_map(|g| {
            g["member_source_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| id.as_str().unwrap().to_owned())
        }))
        .collect::<Vec<_>>();
    let unique = flattened
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(flattened.len(), 4);
    assert_eq!(
        unique.len(),
        flattened.len(),
        "coverage is a partition, not just a set"
    );

    let focused = checked
        .canonical_view(
            checked.revision(),
            &CanonicalViewRequest {
                focus: example["focus"]["ids"].as_array().unwrap().iter().map(|id| id.as_str().unwrap().to_owned()).collect(),
                expand: vec![],
                frontier_depth: None,
            },
        )
        .unwrap();
    assert_eq!(focused["coverage"], broad["coverage"]);
    assert_eq!(focused["revision"], broad["revision"]);
    assert!(
        focused["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["source_id"] == "p.a")
    );
    assert!(
        broad["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"]["from"] != e["projected_from"]
                || e["source"]["to"] != e["projected_to"])
    );

    let group = broad["groups"].as_array().unwrap().iter().find(|g| g["member_source_ids"].as_array().unwrap().iter().any(|id| id == "p.a")).unwrap()["group_id"].as_str().unwrap().to_owned();
    let expanded = checked
        .canonical_view(
            checked.revision(),
            &CanonicalViewRequest {
                focus: vec![],
                expand: vec![group.clone()],
                frontier_depth: None,
            },
        )
        .unwrap();
    assert_eq!(expanded["coverage"], broad["coverage"]);
    assert_eq!(expanded["revision"], broad["revision"]);
    assert!(
        !expanded["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["group_id"] == group),
        "expanded group itself is materialized"
    );
    assert!(
        expanded["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["source_id"] == "p.a")
    );
    let group_focus = checked
        .canonical_view(
            checked.revision(),
            &CanonicalViewRequest {
                focus: vec![group],
                expand: vec![],
                frontier_depth: None,
            },
        )
        .unwrap();
    assert!(
        group_focus["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["source_id"] == "p.a")
    );
}

#[test]
fn stale_revision_unknown_ids_and_wrong_expand_kind_fail_explicitly() {
    let checked = session();
    assert!(
        checked
            .canonical_view("stale", &CanonicalViewRequest::default())
            .unwrap_err()
            .0
            .contains("unknown core session revision")
    );
    assert!(
        checked
            .canonical_view(
                checked.revision(),
                &CanonicalViewRequest {
                    focus: vec!["missing.id".into()],
                    expand: vec![],
                    frontier_depth: None,
                }
            )
            .unwrap_err()
            .0
            .contains("unknown canonical view handle")
    );
    assert!(
        checked
            .canonical_view(
                checked.revision(),
                &CanonicalViewRequest {
                    focus: vec![],
                    expand: vec!["node:p.a".into()],
                    frontier_depth: None,
                }
            )
            .unwrap_err()
            .0
            .contains("expand requires a group handle")
    );
}


#[test]
fn broad_view_keeps_proposal_and_report_navigation_branches_visible() {
    use kpop_native::canonical_view::build;
    use std::collections::{BTreeMap, BTreeSet};
    let ids = ["known.proposal.pilot", "known.proposal.direction", "known.report.baseline", "known.report.criteria", "known.source_limits", "source.hist"];
    let nodes = ids.iter().map(|id| ((*id).to_owned(), json!({"source_id":id,"body":["map",[]]}))).collect::<BTreeMap<_,_>>();
    let members = |subset: &[&str]| subset.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
    let groups = BTreeMap::from([
        ("/".into(), members(&ids)),
        ("/known".into(), members(&ids[..5])),
        ("/known/proposal".into(), members(&ids[..2])),
        ("/known/proposal/pilot".into(), members(&ids[..1])),
        ("/known/proposal/direction".into(), members(&ids[1..2])),
        ("/known/report".into(), members(&ids[2..4])),
        ("/known/report/baseline".into(), members(&ids[2..3])),
        ("/known/report/criteria".into(), members(&ids[3..4])),
        ("/known/source_limits".into(), members(&ids[4..5])),
        ("/source".into(), members(&ids[5..])),
        ("/source/hist".into(), members(&ids[5..])),
    ]);
    let children = BTreeMap::from([
        ("/".into(), BTreeSet::from(["/known".into(), "/source".into()])),
        ("/known".into(), BTreeSet::from(["/known/proposal".into(), "/known/report".into(), "/known/source_limits".into()])),
        ("/known/proposal".into(), BTreeSet::from(["/known/proposal/pilot".into(), "/known/proposal/direction".into()])),
        ("/known/report".into(), BTreeSet::from(["/known/report/baseline".into(), "/known/report/criteria".into()])),
        ("/source".into(), BTreeSet::from(["/source/hist".into()])),
    ]);
    let direct = BTreeMap::from([
        ("/known/source_limits".into(), vec!["known.source_limits".into()]),
        ("/source/hist".into(), vec!["source.hist".into()]),
    ]);
    let leaves = BTreeMap::new();
    let packet = build("fixture", json!({}), "r1", "scope", &nodes, &[], &groups, &children, &direct, &leaves, &CanonicalViewRequest::default()).unwrap();
    let paths = packet["groups"].as_array().unwrap().iter().map(|g| g["path"].as_str().unwrap()).collect::<BTreeSet<_>>();
    assert!(paths.contains("/known/proposal"));
    assert!(paths.contains("/known/report"));
    assert!(!paths.contains("/known"), "generic parent must not swallow the useful categories");
    assert!(packet["nodes"].as_array().unwrap().iter().any(|n| n["source_id"] == "known.source_limits"));
    assert!(packet["nodes"].as_array().unwrap().iter().any(|n| n["source_id"] == "source.hist"));
}

#[test]
fn question_focus_reuses_bound_search_and_keeps_unmatched_coverage() {
    let checked=session();
    let ids=checked.canonical_query_focus(checked.revision(),"p.a").unwrap();
    assert!(ids.contains(&"p.a".to_string()));
    let packet=checked.canonical_view(checked.revision(),&CanonicalViewRequest {focus:ids,expand:vec![], frontier_depth:None}).unwrap();
    assert_eq!(packet["coverage"]["count"],4);
    assert!(checked.canonical_query_focus("stale","p.a").is_err());
    assert!(checked.canonical_query_focus(checked.revision(),"zxqv_nonexistent_topic").unwrap().is_empty());
}

#[test]
fn nested_handle_prefixes_are_not_silent_no_ops() {
    let checked=session();
    for request in [CanonicalViewRequest{focus:vec!["node:group:/readings".into()],expand:vec![], frontier_depth:None},
        CanonicalViewRequest{focus:vec![],expand:vec!["node:group:/readings".into()], frontier_depth:None}] {
        assert!(checked.canonical_view(checked.revision(),&request).is_err());
    }
}
