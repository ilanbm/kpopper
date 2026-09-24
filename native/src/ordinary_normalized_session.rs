// Normalized input is authored graph data, not a retained or preverified assessment.
// Validate its structure and re-evaluate it with the same checked reader as records.
impl OrdinarySession {
    pub(crate) fn from_normalized(
        mut graph: J,
        source: &[u8],
        input_path: &std::path::Path,
        program: &crate::ordinary_runtime::Program,
        project: &str,
        profile: Option<&V>,
    ) -> Result<Self> {
        validate_normalized_graph(&graph)?;
        // These reader-owned fields bind the result to the actual input and program.
        graph["normalized_input"] =
            json!({"kind":"normalized/v1","sha256":sha256(source),"path":input_path});
        graph["project_context"] = json!(project);
        graph["origin"] = program.provenance()?;
        if graph.get("edges").is_none() {
            graph["edges"] = json!([]);
        }
        apply_profile(&mut graph, profile)?;
        let navigation = navigation_index(&graph)?;
        graph["navigation_routes"] = json!(navigation.groups);
        graph["navigation_leaf_routes"] = json!(navigation.leaves);
        let mut directory_keys = BTreeMap::new();
        // Strict JSON parsing above owns the values. The source graph retains
        // field order only for bounded directory/pagination presentation.
        let ordered = crate::history_yaml::decode_ordinary_source_value(source)?;
        for id in graph["nodes"].as_object().unwrap().keys() {
            if let Some(node) = ordered.get("nodes").and_then(|nodes| nodes.get(id)) {
                index_directory_keys(node, &format!("node:{id}"), &mut directory_keys);
            }
        }
        add_protocol_directory_keys(&mut directory_keys, &graph);
        let scan = program.scan(&graph, &OperationalBounds::default())?;
        let snapshot = sha256(canonical(&graph)?.as_bytes());
        let revision = sha256(canonical(&json!({"project":project,"graph":snapshot}))?.as_bytes());
        Ok(Self {
            project: project.into(),
            revision,
            graph,
            scan,
            groups: navigation.groups.clone(),
            navigation,
            proposals: BTreeMap::new(),
            directory_keys,
        })
    }
}

fn validate_normalized_graph(graph: &J) -> Result<()> {
    let nodes = graph
        .get("nodes")
        .and_then(J::as_object)
        .ok_or_else(|| Error("normalized input requires a nodes mapping".into()))?;
    let topics = graph
        .get("topics")
        .and_then(J::as_object)
        .ok_or_else(|| Error("normalized input requires a topics mapping".into()))?;
    require(
        nodes.keys().eq(topics.keys()),
        "topics must cover every node exactly once",
    )?;
    let unprintable = regex::Regex::new(r"[\p{C}\p{Z}]").unwrap();
    for (id, node) in nodes {
        require(
            !id.is_empty()
                && !id.contains(['#', ':', '/'])
                && !id
                    .chars()
                    .filter(|c| *c != ' ')
                    .any(|c| unprintable.is_match(&c.to_string())),
            "node IDs must be printable names without #, : or / reference delimiters",
        )?;
        let body = node
            .get("body")
            .and_then(J::as_object)
            .ok_or_else(|| Error("invalid node body".into()))?;
        require(
            node.get("kind").is_some_and(J::is_string)
                && node
                    .get("states")
                    .and_then(J::as_array)
                    .is_some_and(|s| s.iter().all(J::is_string)),
            "invalid node",
        )?;
        require(
            topics[id].as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .all(|p| p.as_str().is_some_and(|s| !s.is_empty()))
            }),
            "invalid topic path",
        )?;
        if let Some(assessment) = node.get("assessment_body") {
            let fields = node
                .get("assessment_fields")
                .and_then(J::as_object)
                .ok_or_else(|| Error("invalid assessment field mapping".into()))?;
            require(
                fields
                    .keys()
                    .all(|key| ["deps", "snapshot", "predicate"].contains(&key.as_str())),
                "invalid assessment field mapping",
            )?;
            let mut expected = body.clone();
            for (role, canonical) in [
                ("deps", "rests_on"),
                ("snapshot", "seen"),
                ("predicate", "wrong_if"),
            ] {
                if let Some(source) = fields.get(role).filter(|v| !v.is_null()) {
                    let source = source
                        .as_str()
                        .ok_or_else(|| Error("assessment source field must be a string".into()))?;
                    if !source.is_empty() {
                        expected.insert(
                            canonical.into(),
                            body.get(source).cloned().unwrap_or(J::Null),
                        );
                    }
                }
            }
            require(
                *assessment == J::Object(expected),
                "assessment body is stale or disagrees with the original fields",
            )?;
        }
    }
    if let Some(edges) = graph.get("edges") {
        let edges = edges
            .as_array()
            .ok_or_else(|| Error("invalid edges".into()))?;
        let mut seen = BTreeSet::new();
        for edge in edges {
            let edge = edge
                .as_object()
                .ok_or_else(|| Error("invalid edge".into()))?;
            require(
                edge.len() == 3
                    && ["from", "rel", "to"].iter().all(|k| {
                        edge.get(*k)
                            .and_then(J::as_str)
                            .is_some_and(|s| !s.is_empty())
                    }),
                "invalid edge",
            )?;
            require(
                nodes.contains_key(edge["from"].as_str().unwrap()),
                "invalid edge source",
            )?;
            require(seen.insert(serde_json::to_string(edge)?), "duplicate edge")?;
        }
    }
    if let Some(order) = graph.get("node_order") {
        let order = order
            .as_array()
            .ok_or_else(|| Error("invalid node_order".into()))?;
        let unique = order.iter().filter_map(J::as_str).collect::<BTreeSet<_>>();
        require(
            order.len() == nodes.len()
                && unique.len() == nodes.len()
                && nodes.keys().all(|id| unique.contains(id.as_str())),
            "node_order must be a permutation of node IDs",
        )?;
    }
    Ok(())
}
