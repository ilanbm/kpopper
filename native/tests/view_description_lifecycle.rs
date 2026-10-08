use kpop_native::view_descriptions::{
    DescriptionStyle, INDEX_SCHEMA, attach, build_index, build_index_with_style,
    refresh_for_groups, refresh_for_view, refresh_index,
};
use serde_json::{Value as J, json};
use std::collections::BTreeSet;

fn node(id: &str, title: &str) -> J {
    json!({
        "source_id":id,
        "body":["map",[["name",["text",title]],["stamp",["text",format!("stamp:{id}")]]]],
        "status":{"declared_conflict":false,"declared_gap":{"evaluation":"not_evaluated"}},
        "status_text":[],
        "dependencies":null,
        "uncertainty":[],
        "original_node":{"body":{"name":title,"stamp":format!("stamp:{id}")},"kind":"judgment","record_source":"record","states":[]}
    })
}

fn edge(from: &str, rel: &str, to: &str, tag: &str) -> J {
    json!({"source":{"from":from,"rel":rel,"to":to,"source_stamp":tag},
        "projected_from":from,"projected_to":to})
}

fn full() -> J {
    let nodes = vec![
        node("a", "alpha comet"),
        node("b", "alpha deadline"),
        node("c", "beta exception"),
        node("d", "beta support"),
        node("e", "gamma isolated"),
    ];
    let ids = ["a", "b", "c", "d", "e"];
    json!({
        "schema":"kpopper.canonical-graph-view/v1",
        "project":"lifecycle-fixture",
        "project_identity":["map",[["project",["text","lifecycle-fixture"]]]],
        "revision":"r1",
        "scope":"scope-all",
        "mode":"expand",
        "nodes":nodes,
        "groups":[],
        "links":[edge("a","rests_on","b","internal-alpha"),
            edge("b","from","c","cross-alpha-beta"),
            edge("c","rule_reads","d","internal-beta")],
        "coverage":{"source_ids":ids,"count":5},
        "navigation_membership":{
            "a":["/","/alpha"],"b":["/","/alpha"],
            "c":["/","/beta"],"d":["/","/beta"],"e":["/","/gamma"]
        },
        "rules":"fixture rules"
    })
}

fn changed_revision(mut view: J, revision: &str) -> J {
    view["revision"] = json!(revision);
    view
}

fn affected(result: &kpop_native::view_descriptions::RefreshResult) -> BTreeSet<String> {
    result.metrics.affected_group_ids.iter().cloned().collect()
}

fn set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn no_op_and_revision_only_refresh_reuse_and_rebind_without_regeneration() {
    let initial = full();
    let index = build_index(&initial).unwrap();
    assert_eq!(index["schema"], INDEX_SCHEMA);
    assert_eq!(index["descriptions"].as_object().unwrap().len(), 4);

    let no_op = refresh_index(&initial, &index, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(no_op.metrics.hits, 4);
    assert_eq!(no_op.metrics.misses, 0);
    assert_eq!(no_op.metrics.generated, 0);
    assert_eq!(no_op.metrics.rebound, 0);
    assert!(no_op.metrics.affected_group_ids.is_empty());

    let next = changed_revision(initial.clone(), "r2");
    let rebound = refresh_index(&next, &index, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(rebound.metrics.hits, 4);
    assert_eq!(rebound.metrics.generated, 0);
    assert_eq!(rebound.metrics.rebound, 4);
    assert!(rebound.metrics.affected_group_ids.is_empty());
    assert!(
        rebound.index["descriptions"]
            .as_object()
            .unwrap()
            .values()
            .all(|description| description["revision"] == "r2")
    );

    let requested = refresh_for_groups(
        &initial,
        &index,
        &["group:/alpha".into()],
        DescriptionStyle::SourceLabels,
    )
    .unwrap();
    assert_eq!(requested.metrics.hits, 1);
    assert_eq!(requested.metrics.generated, 0);
    assert_eq!(
        requested.index["descriptions"].as_object().unwrap().len(),
        1
    );
    assert!(
        requested.index["descriptions"]
            .get("group:/alpha")
            .is_some()
    );
}

#[test]
fn body_edits_regenerate_only_membership_ancestors_and_not_unrelated_groups() {
    let before = full();
    let cache = build_index(&before).unwrap();
    let mut edited = before.clone();
    edited["revision"] = json!("r2");
    edited["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["source_id"] == "c")
        .unwrap()["body"] = json!([
        "map",
        [
            ["name", ["text", "beta changed"]],
            ["stamp", ["text", "stamp:c"]]
        ]
    ]);
    edited["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["source_id"] == "c")
        .unwrap()["original_node"]["body"]["name"] = json!("beta changed");
    let update = refresh_index(&edited, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(affected(&update), set(&["group:/", "group:/beta"]));
    assert_eq!(update.metrics.generated, 2);
    assert_eq!(update.metrics.rebound, 2);
    assert_eq!(update.metrics.hits, 2);
}

#[test]
fn add_and_delete_update_exact_membership_and_incident_edge_groups() {
    let initial = full();
    let cache = build_index(&initial).unwrap();

    let mut added = initial.clone();
    added["revision"] = json!("r2");
    added["nodes"]
        .as_array_mut()
        .unwrap()
        .push(node("f", "alpha proposal"));
    added["coverage"]["count"] = json!(6);
    added["coverage"]["source_ids"]
        .as_array_mut()
        .unwrap()
        .push(json!("f"));
    added["navigation_membership"]["f"] = json!(["/", "/alpha"]);
    let add_update = refresh_index(&added, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(affected(&add_update), set(&["group:/", "group:/alpha"]));
    assert_eq!(add_update.metrics.generated, 2);
    assert_eq!(add_update.metrics.rebound, 2);
    assert_eq!(add_update.metrics.hits, 2);

    let mut deleted = initial.clone();
    deleted["revision"] = json!("r2");
    deleted["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["source_id"] != "c");
    deleted["coverage"]["count"] = json!(4);
    deleted["coverage"]["source_ids"] = json!(["a", "b", "d", "e"]);
    deleted["navigation_membership"]
        .as_object_mut()
        .unwrap()
        .remove("c");
    deleted["navigation_membership"]["d"] = json!(["/", "/beta"]);
    deleted["links"]
        .as_array_mut()
        .unwrap()
        .retain(|link| link["source"]["from"] != "c" && link["source"]["to"] != "c");
    let delete_update = refresh_index(&deleted, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(
        affected(&delete_update),
        set(&["group:/", "group:/alpha", "group:/beta"])
    );
    assert_eq!(delete_update.metrics.generated, 3);
    assert_eq!(delete_update.metrics.deleted, 0);
    assert_eq!(delete_update.metrics.rebound, 1);
}

#[test]
fn group_rename_membership_moves_and_scope_narrowing_have_exact_affected_sets() {
    let initial = full();
    let cache = build_index(&initial).unwrap();

    let mut renamed = initial.clone();
    renamed["revision"] = json!("r2");
    renamed["navigation_membership"]["c"] = json!(["/", "/delta"]);
    renamed["navigation_membership"]["d"] = json!(["/", "/delta"]);
    let rename_update = refresh_index(&renamed, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(
        affected(&rename_update),
        set(&["group:/beta", "group:/delta"])
    );
    assert_eq!(rename_update.metrics.generated, 1);
    assert_eq!(rename_update.metrics.deleted, 1);
    assert_eq!(rename_update.metrics.rebound, 3);

    let mut moved = initial.clone();
    moved["revision"] = json!("r2");
    moved["navigation_membership"]["b"] = json!(["/", "/beta"]);
    let move_update = refresh_index(&moved, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(
        affected(&move_update),
        set(&["group:/alpha", "group:/beta"])
    );
    assert_eq!(move_update.metrics.generated, 2);
    assert_eq!(move_update.metrics.rebound, 2);

    let mut narrow = initial.clone();
    narrow["revision"] = json!("r2");
    narrow["scope"] = json!("scope-alpha-only");
    narrow["nodes"] = json!([initial["nodes"][0].clone(), initial["nodes"][1].clone()]);
    narrow["coverage"] = json!({"count":2,"source_ids":["a","b"]});
    narrow["navigation_membership"] = json!({"a":["/","/alpha"],"b":["/","/alpha"]});
    narrow["links"] = json!([initial["links"][0].clone()]);
    let scope_update = refresh_index(&narrow, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(
        affected(&scope_update),
        set(&["group:/", "group:/alpha", "group:/beta", "group:/gamma"])
    );
    assert_eq!(scope_update.metrics.generated, 2);
    assert_eq!(scope_update.metrics.deleted, 2);
    assert_eq!(scope_update.metrics.rebound, 0);
}

#[test]
fn internal_and_crossing_edge_changes_invalidate_only_incident_groups_and_ancestors() {
    let initial = full();
    let cache = build_index(&initial).unwrap();

    let mut internal = initial.clone();
    internal["revision"] = json!("r2");
    internal["links"][0]["source"]["rel"] = json!("contradicts");
    let internal_update = refresh_index(&internal, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(
        affected(&internal_update),
        set(&["group:/", "group:/alpha"])
    );
    assert_eq!(internal_update.metrics.generated, 2);
    assert_eq!(internal_update.metrics.rebound, 2);

    let mut crossing = initial.clone();
    crossing["revision"] = json!("r2");
    crossing["links"][1]["source"]["rel"] = json!("rule_reads");
    let crossing_update = refresh_index(&crossing, &cache, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(
        affected(&crossing_update),
        set(&["group:/", "group:/alpha", "group:/beta"])
    );
    assert_eq!(crossing_update.metrics.generated, 3);
    assert_eq!(crossing_update.metrics.rebound, 1);
}

#[test]
fn partial_corrupt_and_legacy_caches_never_supply_old_text() {
    let full = full();
    let cache = build_index(&full).unwrap();

    let mut partial = cache.clone();
    partial["descriptions"]
        .as_object_mut()
        .unwrap()
        .remove("group:/beta");
    let repaired = refresh_index(&full, &partial, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(repaired.metrics.generated, 1);
    assert_eq!(affected(&repaired), set(&["group:/beta"]));

    let mut corrupt = cache.clone();
    corrupt["descriptions"]["group:/alpha"]["text"] = json!("stale injected text");
    assert_eq!(
        kpop_native::view_descriptions::serve(
            &full,
            "group:/alpha",
            &corrupt["descriptions"]["group:/alpha"]
        )
        .unwrap()["status"],
        "stale"
    );
    let repaired = refresh_index(&full, &corrupt, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(repaired.metrics.generated, 1);
    assert!(
        !repaired.index["descriptions"]["group:/alpha"]["text"]
            .as_str()
            .unwrap()
            .contains("stale injected")
    );

    let legacy = json!({"schema":"kpopper.derived-description-index/v1","generator":"source-labels/v1",
        "descriptions":{"group:/alpha":{"schema":"kpopper.derived-description/v1","text":"legacy stale text"}}});
    let refreshed = refresh_index(&full, &legacy, DescriptionStyle::SourceLabels).unwrap();
    assert_eq!(refreshed.metrics.generated, 4);
    assert!(refreshed.metrics.affected_group_ids.len() == 4);
    let mut packet = full.clone();
    packet["groups"] =
        json!([{"group_id":"group:/alpha","path":"/alpha","member_source_ids":["a","b"]}]);
    assert!(attach(&mut packet, &full, &legacy).is_err());
}

#[test]
fn selected_residual_membership_forces_regeneration_in_requested_only_refresh() {
    let full = full();
    let cache = build_index_with_style(&full, DescriptionStyle::RoutingTerms).unwrap();
    let mut selected = full.clone();
    selected["groups"] = json!([{"group_id":"group:/alpha","path":"/alpha","label":"alpha",
        "member_source_ids":["b"]}]);
    selected["nodes"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["source_id"] != "b");
    let residual =
        refresh_for_view(&full, &selected, &cache, DescriptionStyle::RoutingTerms).unwrap();
    assert_eq!(residual.metrics.generated, 1);
    assert_eq!(residual.metrics.misses, 1);
    assert_eq!(affected(&residual), set(&["group:/alpha"]));
    let text = residual.index["descriptions"]["group:/alpha"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("deadline"));
    assert!(!text.contains("comet"));
}
