use kpop_native::view_descriptions::{
    DESCRIPTION_SCHEMA, DescriptionStyle, INDEX_SCHEMA, attach, build_for_groups, build_for_view,
    build_index, generate, generate_with_style, serve,
};
use serde_json::{Value as J, json};
fn view() -> J {
    json!({"schema":"kpopper.canonical-graph-view/v1","revision":"r1","scope":"allowed", "project_identity":{"project":"test"},
        "groups":[],"nodes":[{"source_id":"a","body":["map",[["value",["text","Reported deadline is Friday"]]]]}],
        "links":[],"coverage":{"source_ids":["a"],"count":1},"navigation_membership":{"a":["/","/schedule"]}})
}

#[test]
fn cached_member_count_cannot_disagree_with_its_current_basis() {
    let graph = view();
    let mut description = generate(&graph, "group:/schedule").unwrap();
    description["member_count"] = json!(99);
    assert_eq!(
        serve(&graph, "group:/schedule", &description).unwrap()["status"],
        "stale"
    );
}
#[test]
fn query_blind_description_is_derived_and_current() {
    let v = view();
    let d = generate(&v, "group:/schedule").unwrap();
    assert_eq!(d["schema"], DESCRIPTION_SCHEMA);
    assert_eq!(d["generator"], "source-labels/v1");
    assert_eq!(d["is_source_evidence"], false);
    assert!(d["text"].as_str().unwrap().contains("Reported deadline"));
    assert_eq!(generate(&v, "group:/schedule").unwrap(), d);
    assert_eq!(
        serve(&v, "group:/schedule", &d).unwrap()["status"],
        "current"
    );
}
#[test]
fn changes_in_sources_scope_revision_links_or_membership_hide_stale_text() {
    let v = view();
    let d = generate(&v, "group:/schedule").unwrap();
    for field in ["source", "scope", "revision", "links", "membership"] {
        let mut changed = v.clone();
        match field {
            "source" => {
                changed["nodes"][0]["body"] =
                    json!(["map", [["value", ["text", "Deadline withdrawn"]]]])
            }
            "scope" => changed["scope"] = json!("different"),
            "revision" => changed["revision"] = json!("r2"),
            "links" => {
                changed["links"] =
                    json!([{"source":{"from":"a","to":"external","kind":"requires"}}])
            }
            _ => {
                changed["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"source_id":"b","body":["null"]}));
                changed["coverage"]["count"] = json!(2);
                changed["coverage"]["source_ids"] = json!(["a", "b"]);
                changed["navigation_membership"]["b"] = json!(["/schedule"]);
            }
        }
        let got = serve(&changed, "group:/schedule", &d).unwrap();
        if field == "revision" {
            assert_eq!(got["status"], "rebound");
            assert_eq!(got["description"]["revision"], "r2");
            assert!(got.to_string().contains("Friday"));
        } else {
            assert_eq!(got["status"], "stale", "{field}");
            assert_eq!(got["text"], J::Null);
            assert!(!got.to_string().contains("Friday"));
        }
    }
}
#[test]
fn deleted_group_or_incomplete_source_cannot_supply_description() {
    let v = view();
    let d = generate(&v, "group:/schedule").unwrap();
    let mut deleted = v.clone();
    deleted["navigation_membership"] = json!({});
    assert!(serve(&deleted, "group:/schedule", &d).is_err());
    let mut folded = v.clone();
    folded["groups"] = json!([{"group_id":"group:/schedule"}]);
    assert!(generate(&folded, "group:/schedule").is_err());
    let mut missing = v.clone();
    missing["nodes"] = json!([]);
    assert!(generate(&missing, "group:/schedule").is_err());
}

#[test]
fn routing_style_is_distinct_query_blind_and_marked_derived() {
    let mut v = view();
    v["nodes"][0]["original_node"] = json!({"kind":"judgment","states":["uncertain"]});
    v["nodes"][0]["status"] = json!({"declared_conflict":true});
    let labels = generate(&v, "group:/schedule").unwrap();
    let routing =
        generate_with_style(&v, "group:/schedule", DescriptionStyle::RoutingTerms).unwrap();
    assert_eq!(routing["generator"], "routing-terms/v1");
    assert_eq!(routing["kind"], "routing-terms");
    assert_eq!(routing["is_source_evidence"], false);
    assert!(routing["text"].as_str().unwrap().contains("judgment"));
    assert!(routing["text"].as_str().unwrap().contains("conflict"));
    assert_ne!(routing["text"], labels["text"]);
    assert!(
        !routing["text"]
            .as_str()
            .unwrap()
            .contains("Reported deadline is Friday")
    );
}

#[test]
fn full_index_and_requested_group_index_attach_with_revision_rebind() {
    let v = view();
    let full = build_index(&v).unwrap();
    assert_eq!(full["schema"], INDEX_SCHEMA);
    assert!(full["descriptions"].get("group:/schedule").is_some());
    let partial = build_for_groups(
        &v,
        &["group:/schedule".into()],
        DescriptionStyle::SourceLabels,
    )
    .unwrap();
    let mut packet = v.clone();
    packet["nodes"] = json!([]);
    packet["groups"] = json!([{"group_id":"group:/schedule","path":"/schedule","member_source_ids":["a"],"label":"schedule"}]);
    attach(&mut packet, &v, &partial).unwrap();
    assert_eq!(packet["groups"][0]["description"]["status"], "current");

    let mut next = v.clone();
    next["revision"] = json!("r2");
    let mut next_packet = packet;
    next_packet["revision"] = json!("r2");
    let mut rebound_index = partial.clone();
    rebound_index["revision"] = json!("r1");
    attach(&mut next_packet, &next, &rebound_index).unwrap();
    assert_eq!(next_packet["groups"][0]["description"]["status"], "rebound");
    assert_eq!(
        next_packet["groups"][0]["description"]["description"]["revision"],
        "r2"
    );

    let mut incomplete = partial;
    incomplete["descriptions"] = json!({});
    let mut missing_group = next_packet.clone();
    assert!(attach(&mut missing_group, &next, &incomplete).is_err());
}

#[test]
fn residual_group_description_uses_residual_members_only() {
    let mut full = view();
    full["nodes"].as_array_mut().unwrap().push(json!({
        "source_id":"b",
        "body":["map",[["title",["text","Rare comet classification"]]]],
        "status":null,"status_text":[],"dependencies":null,"uncertainty":[]
    }));
    full["coverage"]["count"] = json!(2);
    full["coverage"]["source_ids"] = json!(["a", "b"]);
    full["navigation_membership"]["a"] = json!(["/schedule"]);
    full["navigation_membership"]["b"] = json!(["/schedule"]);
    let full_index = build_index(&full).unwrap();
    let mut selected = full.clone();
    selected["nodes"] = json!([]);
    selected["nodes"].as_array_mut().unwrap().extend(
        full["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node["source_id"] != "a")
            .cloned(),
    );
    selected["coverage"]["count"] = json!(2);
    selected["coverage"]["source_ids"] = json!(["a", "b"]);
    selected["groups"] = json!([{
        "group_id":"group:/schedule","path":"/schedule","label":"schedule",
        "member_source_ids":["a"]
    }]);
    let residual_index = build_for_view(&full, &selected, DescriptionStyle::SourceLabels).unwrap();
    let residual_text = residual_index["descriptions"]["group:/schedule"]["text"]
        .as_str()
        .unwrap();
    assert!(!residual_text.contains("Rare comet"));
    assert!(residual_text.contains("Reported deadline"));

    let mut packet = selected;
    let mut mismatched = packet.clone();
    assert!(attach(&mut mismatched, &full, &full_index).is_err());
    attach(&mut packet, &full, &residual_index).unwrap();
    assert!(
        !packet["groups"][0]["description"]["description"]["text"]
            .as_str()
            .unwrap()
            .contains("Rare comet")
    );
}
