use kpop_native::canonical_view::{self, CanonicalViewRequest};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

fn structural_packet() -> J {
    serde_json::from_str(include_str!("fixtures/compact-structural.json")).unwrap()
}

#[test]
fn allocated_original_ids_with_handle_prefixes_keep_their_identity() {
    let all = ["claim.0", "node:claim.0", "group:/bucket", "other"];
    let nodes = all
        .iter()
        .map(|id| ((*id).to_owned(), json!({"source_id":id,"body":["text",id]})))
        .collect::<BTreeMap<_, _>>();
    let members = |ids: &[&str]| {
        ids.iter()
            .map(|id| (*id).to_owned())
            .collect::<BTreeSet<_>>()
    };
    let groups = BTreeMap::from([
        ("/".into(), members(&all)),
        ("/bucket".into(), members(&["claim.0", "other"])),
        (
            "/prefixed".into(),
            members(&["node:claim.0", "group:/bucket"]),
        ),
    ]);
    let children = BTreeMap::from([(
        "/".into(),
        BTreeSet::from(["/bucket".into(), "/prefixed".into()]),
    )]);
    let build = |request: CanonicalViewRequest| {
        canonical_view::build(
            "prefixes",
            json!(["map", []]),
            "r1",
            "scope",
            &nodes,
            &[],
            &groups,
            &children,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &request,
        )
        .unwrap()
    };
    let full = build(CanonicalViewRequest {
        focus: vec![],
        expand: vec!["group:/".into()],
        frontier_depth: None,
    });
    for original in ["node:claim.0", "group:/bucket"] {
        let selected = kpop_native::view_selection::allocate(
            &full,
            &[json!({"id":original,"match":"literal_id"})],
            &[],
            20_000,
            |s| s.len(),
        )
        .unwrap();
        assert_eq!(selected.ids, [original]);
        let view = build(CanonicalViewRequest {
            focus: selected.ids,
            expand: vec![],
            frontier_depth: None,
        });
        let direct = view["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["source_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            direct,
            [original],
            "source ID must not become another node/group handle"
        );
    }
    let explicit_group = build(CanonicalViewRequest {
        focus: vec![],
        expand: vec!["group:/bucket".into()],
        frontier_depth: None,
    });
    assert_eq!(explicit_group["nodes"].as_array().unwrap().len(), 2);
}

fn ids(packet: &J) -> Vec<String> {
    packet["coverage"]["source_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect()
}

fn original_edges(packet: &J) -> Vec<J> {
    packet["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| link["source"].clone())
        .collect()
}

fn group(handle: &str, members: &[String]) -> J {
    json!({
        "group_id":handle,
        "path":handle.strip_prefix("group:").unwrap(),
        "member_source_ids":members,
        "label":handle.rsplit('/').next().unwrap_or(handle),
        "description":{"status":"current","description":{
            "generator":"source-labels/v1",
            "basis_sha256":"a".repeat(64),
            "text":"Record claims in this group (navigation only): short derived excerpt.",
            "is_source_evidence":false
        }}
    })
}

fn folded_packet() -> J {
    let mut packet = structural_packet();
    let mut direct = Vec::new();
    let mut chain = Vec::new();
    let mut left = Vec::new();
    let mut right = Vec::new();
    for node in packet["nodes"].as_array().unwrap() {
        let id = node["source_id"].as_str().unwrap();
        if id.starts_with("chain.") {
            chain.push(id.to_owned());
        } else if id == "exception.left" {
            left.push(id.to_owned());
        } else if id == "exception.right" {
            right.push(id.to_owned());
        } else {
            direct.push(node.clone());
        }
    }
    packet["nodes"] = json!(direct);
    packet["groups"] = json!([
        group("group:/support/deep", &chain),
        group("group:/exceptions/left", &left),
        group("group:/exceptions/right", &right),
    ]);
    packet["mode"] = json!("broad");
    packet
}

#[test]
fn edge_handles_survive_navigation_text_detail_but_not_source_changes() {
    let original = folded_packet();
    let broad = canonical_view::compact(&original, &[]).unwrap();
    let handle = broad["edge_set_handle_template"]
        .as_str()
        .unwrap()
        .replace("{edge_set_ref}", broad["links"][0][4].as_str().unwrap());
    let mut labels = original.clone();
    for group in labels["groups"].as_array_mut().unwrap() {
        group.as_object_mut().unwrap().remove("description");
    }
    let expanded = canonical_view::compact(&labels, &[handle.clone()]).unwrap();
    assert_eq!(expanded["view_id"], broad["view_id"]);
    assert_eq!(expanded["expanded_edges"].as_array().unwrap().len(), 1);
    labels["revision"] = json!("changed-source");
    assert!(canonical_view::compact(&labels, &[handle]).is_err());
}

fn focused_packet() -> J {
    let mut packet = folded_packet();
    let full = structural_packet();
    let focused = full["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["source_id"] == "chain.04")
        .unwrap()
        .clone();
    packet["nodes"].as_array_mut().unwrap().push(focused);
    let members = packet["groups"][0]["member_source_ids"]
        .as_array_mut()
        .unwrap();
    members.retain(|id| id != "chain.04");
    packet["mode"] = json!("focus");
    packet
}

fn many_focus_packet() -> J {
    let mut packet = folded_packet();
    let full = structural_packet();
    let focus_ids = (0..7)
        .map(|i| format!("chain.{i:02}"))
        .chain(["exception.left".into(), "exception.right".into()])
        .collect::<BTreeSet<_>>();
    for id in &focus_ids {
        let node = full["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["source_id"].as_str() == Some(id.as_str()))
            .unwrap()
            .clone();
        packet["nodes"].as_array_mut().unwrap().push(node);
    }
    packet["groups"]
        .as_array_mut()
        .unwrap()
        .retain_mut(|group| {
            group["member_source_ids"]
                .as_array_mut()
                .unwrap()
                .retain(|id| !focus_ids.contains(id.as_str().unwrap()));
            !group["member_source_ids"].as_array().unwrap().is_empty()
        });
    packet["mode"] = json!("focus");
    packet
}

#[test]
fn compact_roundtrips_typed_edges_and_preserves_unique_metadata() {
    let packet = structural_packet();
    let compact = canonical_view::compact(&packet, &[]).unwrap();
    assert_eq!(compact["schema"], canonical_view::COMPACT_SCHEMA);
    assert_eq!(
        compact["fields"]["node"],
        json!(["ref", "body", "attributes"])
    );
    assert_eq!(compact["coverage"]["count"], 15);
    assert_eq!(compact["coverage"]["direct_count"], 15);
    assert_eq!(compact["coverage"]["folded_count"], 0);
    assert!(compact["coverage"]["source_ids_sha256"].as_str().is_some());
    assert!(compact["coverage"]["receipt"].as_str().is_some());
    assert!(
        compact["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.as_array().is_some_and(|row| row.len() == 3))
    );
    assert!(compact["groups"].as_array().unwrap().is_empty());

    let null_ref = compact["dictionary"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, entry)| entry["kind"] == "node" && entry["original"] == "isolate.null")
        .unwrap()
        .0
        .clone();
    let null_row = compact["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0] == null_ref)
        .unwrap();
    assert_eq!(null_row[1], json!(["null"]));
    assert_eq!(null_row[2]["uncertainty"], J::Null);
    assert_eq!(
        null_row[2]["record_metadata"]["unique_marker"],
        "keep-null-type"
    );
    assert!(null_row[2]["record_metadata"].get("body").is_none());
    assert!(null_row[2]["record_metadata"].get("states").is_none());

    let missing_ref = compact["dictionary"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, entry)| entry["kind"] == "node" && entry["original"] == "isolate.alpha")
        .unwrap()
        .0
        .clone();
    let missing_row = compact["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0] == missing_ref)
        .unwrap();
    assert!(missing_row[2]["record_metadata"].get("body").is_none());
    assert_eq!(
        missing_row[2]["record_metadata"]["unique_marker"],
        "keep-missing-body"
    );

    let handles = compact["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            format!(
                "edgeset:{}:{}",
                compact["view_id"].as_str().unwrap(),
                row[4].as_str().unwrap()
            )
        })
        .collect::<Vec<_>>();
    let expanded = canonical_view::compact(&packet, &handles).unwrap();
    let mut recovered = expanded["expanded_edges"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row[1].as_array().unwrap().iter().cloned())
        .map(|edge| serde_json::to_string(&edge).unwrap())
        .collect::<Vec<_>>();
    recovered.sort();
    let mut expected = original_edges(&packet)
        .iter()
        .map(|edge| serde_json::to_string(edge).unwrap())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(recovered, expected);
    assert_eq!(
        compact["links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[3].as_u64().unwrap())
            .sum::<u64>(),
        original_edges(&packet).len() as u64
    );
    let aggregate = compact["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[1] == "rests_on" && row[3].as_u64().unwrap() >= 2)
        .unwrap();
    let aggregate_handle = format!(
        "edgeset:{}:{}",
        compact["view_id"].as_str().unwrap(),
        aggregate[4].as_str().unwrap()
    );
    let roundtrip = canonical_view::compact(&packet, &[aggregate_handle]).unwrap();
    let restored = &roundtrip["expanded_edges"][0][1];
    assert!(
        restored
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["parallel"] == 1)
    );
    assert!(
        restored
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["parallel"] == 2)
    );
    assert!(restored.as_array().unwrap().iter().all(|edge| {
        edge.get("unknown_metadata")
            .is_none_or(|value| value == &json!({"x":null}))
    }));
}

#[test]
fn folded_projection_keeps_navigation_facets_separate_and_selection_bound() {
    let packet = folded_packet();
    let compact = canonical_view::compact(&packet, &[]).unwrap();
    assert_eq!(compact["coverage"]["count"], 15);
    assert_eq!(compact["coverage"]["direct_count"], 2);
    assert_eq!(compact["coverage"]["folded_count"], 13);
    assert_eq!(
        compact["coverage"]["direct_count"].as_u64().unwrap()
            + compact["coverage"]["folded_count"].as_u64().unwrap(),
        compact["coverage"]["count"].as_u64().unwrap()
    );
    assert_eq!(compact["groups"][0][1], 1);
    assert!(compact["nodes"].as_array().unwrap().iter().any(|row| {
        let entry = &compact["dictionary"][row[0].as_str().unwrap()];
        entry["original"] == "isolate.null"
    }));
    let facet = compact["navigation_facets"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            compact["dictionary"][row[0].as_str().unwrap()]["original"] == "group:/featured"
        })
        .unwrap();
    assert_eq!(facet[1], 2);
    assert_eq!(
        compact["navigation_facets"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(
        compact["navigation_facets"]["groups"]
            .as_array()
            .unwrap()
            .len()
            > 3
    );
    assert!(compact["coverage"].get("source_ids").is_none());
    assert!(compact["groups"].as_array().unwrap().iter().all(|row| {
        row[3][0].as_str().unwrap().chars().count() <= 144
            && row[3][1] == "source-labels/v1"
            && row[3][2].as_str().is_some()
            && row[3][3] == true
    }));

    let folded_handle = format!(
        "edgeset:{}:{}",
        compact["view_id"].as_str().unwrap(),
        compact["links"][0][4].as_str().unwrap()
    );
    let focus = focused_packet();
    let focused = canonical_view::compact(&focus, &[]).unwrap();
    assert_eq!(focused["coverage"]["direct_count"], 3);
    assert_eq!(focused["coverage"]["folded_count"], 12);
    assert!(
        focused["dictionary"]
            .as_object()
            .unwrap()
            .values()
            .any(|entry| { entry["kind"] == "node" && entry["original"] == "chain.04" })
    );
    assert!(
        focused["dictionary"]
            .as_object()
            .unwrap()
            .values()
            .any(|entry| {
                entry["kind"] == "group" && entry["original"] == "group:/support/deep"
            })
    );
    assert_ne!(compact["view_id"], focused["view_id"]);
    assert!(canonical_view::compact(&focus, &[folded_handle]).is_err());

    let old_edge = format!(
        "edgeset:{}:{}",
        compact["view_id"].as_str().unwrap(),
        compact["links"][0][4].as_str().unwrap()
    );
    let mut stale_revision = packet;
    stale_revision["revision"] = json!("r2-fixture");
    assert!(canonical_view::compact(&stale_revision, &[old_edge]).is_err());
    let mut changed_payload = folded_packet();
    changed_payload["nodes"][0]["body"] = json!(["text", "same revision, changed payload"]);
    let old_handle = format!(
        "edgeset:{}:{}",
        compact["view_id"].as_str().unwrap(),
        compact["links"][0][4].as_str().unwrap()
    );
    assert!(canonical_view::compact(&changed_payload, &[old_handle]).is_err());
    assert!(canonical_view::compact(&focus, &["unknown-edge-set".into()]).is_err());
}

#[test]
fn more_than_eight_focused_ids_keep_routes_across_multiple_groups() {
    let packet = many_focus_packet();
    let compact = canonical_view::compact(&packet, &[]).unwrap();
    assert_eq!(compact["coverage"]["count"], 15);
    assert_eq!(compact["coverage"]["direct_count"], 11);
    assert_eq!(compact["coverage"]["folded_count"], 4);
    assert_eq!(
        compact["navigation_facets"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        11
    );
    let route_refs = compact["navigation_facets"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row[1].as_array().unwrap())
        .filter_map(J::as_str)
        .collect::<BTreeSet<_>>();
    assert!(route_refs.len() > 3);
    assert!(
        compact["dictionary"]
            .as_object()
            .unwrap()
            .values()
            .any(|entry| { entry["kind"] == "group" && entry["original"] == "group:/featured" })
    );
}

#[test]
fn empty_selection_and_empty_graph_are_explicit_and_valid() {
    let packet = folded_packet();
    let empty_request = canonical_view::compact(&packet, &[]).unwrap();
    assert!(
        empty_request["expanded_edges"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let mut empty = structural_packet();
    empty["nodes"] = json!([]);
    empty["groups"] = json!([]);
    empty["links"] = json!([]);
    empty["navigation_membership"] = json!({});
    empty["coverage"] = json!({"count":0,"source_ids":[]});
    let compact = canonical_view::compact(&empty, &[]).unwrap();
    assert_eq!(compact["coverage"]["count"], 0);
    assert!(compact["nodes"].as_array().unwrap().is_empty());
    assert!(compact["groups"].as_array().unwrap().is_empty());
    assert!(compact["links"].as_array().unwrap().is_empty());
}

#[test]
fn focus_materializes_one_member_and_leaves_a_residual_group() {
    let packet = structural_packet();
    let node_map = packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| (node["source_id"].as_str().unwrap().to_owned(), node.clone()))
        .collect::<BTreeMap<_, _>>();
    let all: BTreeSet<_> = ids(&packet).into_iter().collect();
    let chain: BTreeSet<_> = all
        .iter()
        .filter(|id| id.starts_with("chain."))
        .cloned()
        .collect();
    let exceptions: BTreeSet<_> = all
        .iter()
        .filter(|id| id.starts_with("exception."))
        .cloned()
        .collect();
    let isolates: BTreeSet<_> = all
        .iter()
        .filter(|id| id.starts_with("isolate."))
        .cloned()
        .collect();
    let groups = BTreeMap::from([
        ("/".into(), all.clone()),
        ("/support".into(), chain),
        ("/exceptions".into(), exceptions),
        ("/isolates".into(), isolates),
    ]);
    let children = BTreeMap::from([(
        "/".into(),
        BTreeSet::from(["/support".into(), "/exceptions".into(), "/isolates".into()]),
    )]);
    let direct = BTreeMap::new();
    let links = original_edges(&packet);
    let broad = canonical_view::build(
        "fixture",
        packet["project_identity"].clone(),
        "r1-fixture",
        "fixture scope",
        &node_map,
        &links,
        &groups,
        &children,
        &direct,
        &BTreeMap::new(),
        &CanonicalViewRequest::default(),
    )
    .unwrap();
    let focused = canonical_view::build(
        "fixture",
        packet["project_identity"].clone(),
        "r1-fixture",
        "fixture scope",
        &node_map,
        &links,
        &groups,
        &children,
        &direct,
        &BTreeMap::new(),
        &CanonicalViewRequest {
            focus: vec!["chain.04".into()],
            expand: vec![],
            frontier_depth: None,
        },
    )
    .unwrap();
    assert_eq!(broad["coverage"]["count"], 15);
    assert_eq!(focused["coverage"], broad["coverage"]);
    assert_eq!(focused["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(focused["nodes"][0]["source_id"], "chain.04");
    let residual = focused["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["path"] == "/support")
        .unwrap();
    assert_eq!(residual["member_source_ids"].as_array().unwrap().len(), 10);
    assert!(
        !residual["member_source_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "chain.04")
    );

    let expanded = canonical_view::build(
        "fixture",
        packet["project_identity"].clone(),
        "r1-fixture",
        "fixture scope",
        &node_map,
        &links,
        &groups,
        &children,
        &direct,
        &BTreeMap::new(),
        &CanonicalViewRequest {
            focus: vec![],
            expand: vec!["group:/support".into()],
            frontier_depth: None,
        },
    )
    .unwrap();
    assert_eq!(expanded["nodes"].as_array().unwrap().len(), 11);
    assert!(
        !expanded["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["path"] == "/support")
    );
}

#[test]
fn unrequested_direct_bodies_fold_into_existing_exact_navigation_routes() {
    let packet = structural_packet();
    let id = packet["nodes"][0]["source_id"].as_str().unwrap().to_owned();
    let folded = canonical_view::fold_unrequested(&packet, std::slice::from_ref(&id), &[]).unwrap();
    let remaining = folded["nodes"].as_array().unwrap();
    assert!(remaining.len() < packet["nodes"].as_array().unwrap().len());
    assert_eq!(
        remaining.iter().find(|n| n["source_id"] == id).unwrap(),
        &packet["nodes"][0]
    );
    assert_eq!(folded["coverage"], packet["coverage"]);
    assert_eq!(
        folded["navigation_membership"],
        packet["navigation_membership"]
    );
    assert_eq!(original_edges(&folded), original_edges(&packet));
    let compact = canonical_view::compact(&folded, &[]).unwrap();
    assert_eq!(compact["coverage"]["count"], packet["coverage"]["count"]);
    for group in folded["groups"].as_array().unwrap() {
        let path = group["path"].as_str().unwrap();
        assert_ne!(path, "/");
        for member in group["member_source_ids"].as_array().unwrap() {
            assert!(
                packet["navigation_membership"][member.as_str().unwrap()]
                    .as_array()
                    .unwrap()
                    .contains(&json!(path))
            );
        }
    }
    let path = packet["navigation_membership"][&id]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(J::as_str)
        .find(|p| *p != "/")
        .unwrap();
    let expanded =
        canonical_view::fold_unrequested(&packet, &[], &[format!("group:{path}")]).unwrap();
    for node in packet["nodes"].as_array().unwrap() {
        let source = node["source_id"].as_str().unwrap();
        if packet["navigation_membership"][source]
            .as_array()
            .unwrap()
            .contains(&json!(path))
        {
            assert!(
                expanded["nodes"].as_array().unwrap().contains(node),
                "explicit expansion must retain {source}"
            );
        }
    }
}
