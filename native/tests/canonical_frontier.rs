use kpop_native::canonical_view::{self, CanonicalViewRequest};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

struct Graph {
    nodes: BTreeMap<String, J>,
    groups: BTreeMap<String, BTreeSet<String>>,
    children: BTreeMap<String, BTreeSet<String>>,
    direct: BTreeMap<String, Vec<String>>,
    edges: Vec<J>,
}
impl Graph {
    fn new() -> Self {
        let mut graph = Self {
            nodes: BTreeMap::new(),
            groups: BTreeMap::new(),
            children: BTreeMap::new(),
            direct: BTreeMap::new(),
            edges: vec![
                json!({"from":"a.0.0","to":"b.1.1","rel":"qualifies","weight":null}),
                json!({"from":"a.0.0","to":"b.1.1","rel":7,"weight":1}),
                json!({"from":"a.0.0","to":"b.1.1","rel":"qualifies","weight":2}),
            ],
        };
        for branch in ["a", "b"] {
            for child in 0..3 {
                for n in 0..3 {
                    let id = format!("{branch}.{child}.{n}");
                    let paths = [
                        "/".to_owned(),
                        format!("/{branch}"),
                        format!("/{branch}/child{child}"),
                    ];
                    graph.nodes.insert(id.clone(),json!({"source_id":id,"body":["map",[["literal",["text",format!("exact {id}")]]]]}));
                    for path in &paths {
                        graph
                            .groups
                            .entry(path.clone())
                            .or_default()
                            .insert(id.clone());
                    }
                    for pair in paths.windows(2) {
                        graph
                            .children
                            .entry(pair[0].clone())
                            .or_default()
                            .insert(pair[1].clone());
                    }
                    graph.direct.entry(paths[2].clone()).or_default().push(id);
                }
            }
        }
        // A second navigation facet overlaps a primary branch. It must remain
        // a route without owning any source twice.
        graph.groups.insert(
            "/overlap".into(),
            BTreeSet::from(["a.0.0".into(), "b.1.1".into()]),
        );
        graph
            .children
            .entry("/".into())
            .or_default()
            .insert("/overlap".into());
        graph
    }
    fn build(
        &self,
        revision: &str,
        scope: &str,
        request: CanonicalViewRequest,
    ) -> kpop_native::Result<J> {
        canonical_view::build(
            "frontier",
            json!(["text", "identity"]),
            revision,
            scope,
            &self.nodes,
            &self.edges,
            &self.groups,
            &self.children,
            &self.direct,
            &BTreeMap::new(),
            &request,
        )
    }
    fn coarse(&self, focus: Vec<String>, expand: Vec<String>) -> J {
        self.build(
            "r1",
            "scope",
            CanonicalViewRequest {
                focus,
                expand,
                frontier_depth: Some(1),
            },
        )
        .unwrap()
    }
}
fn partition(packet: &J) -> Vec<String> {
    packet["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["source_id"].as_str().unwrap().to_owned())
        .chain(
            packet["groups"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|g| g["member_source_ids"].as_array().unwrap())
                .map(|id| id.as_str().unwrap().to_owned()),
        )
        .collect()
}
fn group_refs(packet: &J) -> BTreeSet<&str> {
    packet["dictionary"]
        .as_object()
        .unwrap()
        .values()
        .filter(|v| v["kind"] == "group")
        .map(|v| v["original"].as_str().unwrap())
        .collect()
}

#[test]
fn coarse_frontier_keeps_exact_coverage_and_hides_descendant_facets() {
    let graph = Graph::new();
    let ordinary = graph
        .build("r1", "scope", CanonicalViewRequest::default())
        .unwrap();
    let coarse = graph.coarse(vec![], vec![]);
    assert!(
        ordinary["groups"].as_array().unwrap().len() > coarse["groups"].as_array().unwrap().len()
    );
    assert!(ordinary.get("navigation_frontier").is_none());
    assert_eq!(ordinary["coverage"], coarse["coverage"]);
    let ids = partition(&coarse);
    assert_eq!(ids.len(), 18);
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 18);
    let compact = canonical_view::compact(&coarse, &[]).unwrap();
    let old = canonical_view::compact(&ordinary, &[]).unwrap();
    assert_eq!(
        compact["coverage"]["source_ids_sha256"],
        old["coverage"]["source_ids_sha256"]
    );
    assert_eq!(
        group_refs(&compact),
        BTreeSet::from(["group:/", "group:/a", "group:/b", "group:/overlap"])
    );
    assert!(compact.get("navigation_hidden_paths").is_none());
    let encoded = kpop_native::view_format::render(
        &compact,
        kpop_native::view_format::ViewFormat::CheckedTextTagged,
    )
    .unwrap();
    assert_eq!(
        kpop_native::view_format::decode_checked_text(&encoded).unwrap(),
        compact
    );
}

#[test]
fn focused_bodies_and_visible_overlapping_facets_remain_exact() {
    let graph = Graph::new();
    let coarse = graph.coarse(vec!["a.0.0".into()], vec![]);
    assert_eq!(coarse["nodes"][0], graph.nodes["a.0.0"]);
    let compact = canonical_view::compact(&coarse, &[]).unwrap();
    let refs = group_refs(&compact);
    assert!(refs.contains("group:/a/child0"));
    assert!(!refs.contains("group:/a/child1"));
    let route = compact["dictionary"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, v)| v["original"] == "group:/a/child0")
        .unwrap()
        .0;
    let count = compact["navigation_facets"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[0] == *route)
        .unwrap();
    assert_eq!(
        count[1], 3,
        "visible-node route count includes every member, not only visible bodies"
    );
    assert!(refs.contains("group:/overlap"));
}

#[test]
fn child_navigation_is_exact_revision_scoped_and_does_not_force_bodies() {
    let graph = Graph::new();
    let coarse = graph.coarse(vec![], vec![]);
    let summary = coarse["navigation_frontier"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["group"] == "group:/a")
        .unwrap();
    assert_eq!(summary["member_count"], 9);
    assert_eq!(summary["children_count"], 3);
    let handle = summary["expand"].as_str().unwrap().to_owned();
    let expanded = graph.coarse(vec![], vec![handle.clone()]);
    assert_eq!(expanded["nodes"], coarse["nodes"]);
    assert_eq!(expanded["coverage"], coarse["coverage"]);
    let details = &expanded["navigation_frontier"]["expanded_children"][0];
    assert_eq!(details["children"].as_array().unwrap().len(), 3);
    let first = &details["children"][0];
    assert_eq!(first["group"], "group:/a/child0");
    assert_eq!(first["member_count"], 3);
    let expected = kpop_native::identity::sha256(
        &serde_json::to_vec(&json!(["a.0.0", "a.0.1", "a.0.2"])).unwrap(),
    );
    assert_eq!(first["members_sha256"], expected);
    let compact = canonical_view::compact(&expanded, &[]).unwrap();
    assert!(group_refs(&compact).contains("group:/a/child1"));
    let mut members = compact.clone();
    canonical_view::add_membership_details(&expanded, &mut members, &["group:/a/child0".into()])
        .unwrap();
    let ids = members["dictionary"]
        .as_object()
        .unwrap()
        .values()
        .filter(|v| v["kind"] == "node")
        .map(|v| v["original"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids, BTreeSet::from(["a.0.0", "a.0.1", "a.0.2"]));
    let next = graph.coarse(vec![], vec![first["expand"].as_str().unwrap().into()]);
    assert_eq!(
        next["navigation_frontier"]["expanded_children"][0]["direct_ids"],
        json!(["a.0.0", "a.0.1", "a.0.2"])
    );
    for (revision, scope, handle) in [
        ("stale", "scope", handle.clone()),
        ("r1", "other", handle.clone()),
        (
            "r1",
            "scope",
            format!("children:{}:group:/unknown", "0".repeat(64)),
        ),
        ("r1", "scope", "children:group:/a".into()),
    ] {
        assert!(
            graph
                .build(
                    revision,
                    scope,
                    CanonicalViewRequest {
                        focus: vec![],
                        expand: vec![handle],
                        frontier_depth: Some(1)
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn explicit_expansion_preserves_whole_bodies_and_parallel_typed_edges() {
    let graph = Graph::new();
    let packet = graph.coarse(vec!["b.1.1".into()], vec!["group:/a/child0".into()]);
    for id in ["a.0.0", "a.0.1", "a.0.2", "b.1.1"] {
        assert!(
            packet["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| *node == graph.nodes[id])
        );
    }
    let compact = canonical_view::compact(&packet, &[]).unwrap();
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
    let mut edges = expanded["expanded_edges"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|r| r[1].as_array().unwrap())
        .cloned()
        .collect::<Vec<_>>();
    edges.sort_by_key(J::to_string);
    let mut expected = graph.edges.clone();
    expected.sort_by_key(J::to_string);
    assert_eq!(edges, expected);
    let other = graph.coarse(vec![], vec![]);
    assert!(canonical_view::compact(&other, &handles).is_err());
    let all = graph.coarse(vec![], vec!["group:/".into()]);
    assert_eq!(all["nodes"].as_array().unwrap().len(), 18);
    assert!(all["groups"].as_array().unwrap().is_empty());
}

#[test]
fn hidden_membership_is_explicitly_readable_without_restoring_all_facets() {
    let graph = Graph::new();
    let coarse = graph.coarse(vec![], vec![]);
    let mut compact = canonical_view::compact(&coarse, &[]).unwrap();
    assert!(!group_refs(&compact).contains("group:/a/child0"));
    canonical_view::add_membership_details(&coarse, &mut compact, &["group:/a/child0".into()])
        .unwrap();
    assert!(group_refs(&compact).contains("group:/a/child0"));
    assert!(!group_refs(&compact).contains("group:/a/child1"));
    assert_eq!(compact["coverage"]["count"], 18);
    assert!(compact["nodes"].as_array().unwrap().is_empty());
}

#[test]
fn child_navigation_has_no_fixed_depth_ceiling() {
    let mut graph = Graph::new();
    let ids = BTreeSet::from(["a.0.0".into(), "a.0.1".into()]);
    let mut path = String::from("/deep");
    graph
        .children
        .entry("/".into())
        .or_default()
        .insert(path.clone());
    graph.groups.insert(path.clone(), ids.clone());
    for n in 0..16 {
        let child = format!("{path}/level{n}");
        graph
            .children
            .entry(path.clone())
            .or_default()
            .insert(child.clone());
        graph.groups.insert(child.clone(), ids.clone());
        path = child;
    }
    graph
        .direct
        .insert(path.clone(), ids.iter().cloned().collect());
    let broad = graph.coarse(vec![], vec![]);
    let compact = canonical_view::compact(&broad, &[]).unwrap();
    assert!(group_refs(&compact).contains("group:/deep"));
    assert!(!group_refs(&compact).contains("group:/deep/level0"));
    let summary = broad["navigation_frontier"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["group"] == "group:/deep")
        .unwrap();
    assert_eq!(summary["owns_source_rows"], false);
    let mut handle = summary["expand"].as_str().unwrap().to_owned();
    for _ in 0..16 {
        let next = graph.coarse(vec![], vec![handle]);
        let details = &next["navigation_frontier"]["expanded_children"][0];
        assert_eq!(details["children"].as_array().unwrap().len(), 1);
        handle = details["children"][0]["expand"]
            .as_str()
            .unwrap()
            .to_owned();
    }
    let leaf = graph.coarse(vec![], vec![handle]);
    assert_eq!(
        leaf["navigation_frontier"]["expanded_children"][0]["direct_ids"],
        json!(ids)
    );
    // The deep route overlaps /a, so it remains a navigation facet independently
    // of which branch owns the source partition.
    let tree = graph
        .build(
            "r1",
            "scope",
            CanonicalViewRequest {
                focus: vec![],
                expand: vec!["group:/deep".into()],
                frontier_depth: Some(1),
            },
        )
        .unwrap();
    for id in &ids {
        assert!(
            tree["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["source_id"] == *id)
        );
    }
    let expanded = graph.coarse(vec![], vec![format!("group:{path}")]);
    for id in &ids {
        assert!(
            expanded["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["source_id"] == *id)
        );
    }
}

#[test]
fn explicit_overlapping_group_members_cannot_be_owned_by_folded_siblings() {
    let graph = Graph::new();
    for depth in [None, Some(1)] {
        let packet = graph
            .build(
                "r1",
                "scope",
                CanonicalViewRequest {
                    focus: vec![],
                    expand: vec!["group:/overlap".into()],
                    frontier_depth: depth,
                },
            )
            .unwrap();
        for id in ["a.0.0", "b.1.1"] {
            assert!(
                packet["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| *row == graph.nodes[id]),
                "missing explicit member {id} at {depth:?}"
            );
        }
        assert_eq!(partition(&packet).len(), 18);
        assert_eq!(partition(&packet).iter().collect::<BTreeSet<_>>().len(), 18);
    }
}

#[test]
fn mixed_bucket_direct_rows_fold_without_swallowing_named_children() {
    let mut graph = Graph::new();
    graph.nodes.insert(
        "loose".into(),
        json!({"source_id":"loose","body":["text","large irrelevant body".repeat(1000)]}),
    );
    graph.groups.get_mut("/").unwrap().insert("loose".into());
    let mut members = graph.groups["/a"].clone();
    members.insert("loose".into());
    graph.groups.insert("/@lex1".into(), members);
    graph.children.get_mut("/").unwrap().remove("/a");
    graph.children.get_mut("/").unwrap().insert("/@lex1".into());
    graph
        .children
        .insert("/@lex1".into(), BTreeSet::from(["/a".into()]));
    graph.direct.insert("/@lex1".into(), vec!["loose".into()]);
    let packet = graph.coarse(vec!["b.1.1".into()], vec![]);
    assert_eq!(
        packet["nodes"].as_array().unwrap().len(),
        1,
        "unrequested mixed-bucket body must remain folded"
    );
    assert_eq!(packet["nodes"][0], graph.nodes["b.1.1"]);
    let bucket = packet["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["path"] == "/@lex1")
        .unwrap();
    assert_eq!(bucket["member_source_ids"], json!(["loose"]));
    assert_eq!(partition(&packet).len(), 19);
    assert!(group_refs(&canonical_view::compact(&packet, &[]).unwrap()).contains("group:/a"));
}
