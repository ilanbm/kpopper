//! Query-blind derived navigation aids. They never become source evidence.
use crate::{identity::sha256, value::TypedValue as V, Error, Result};
use serde::Serialize;
use serde_json::{json, Map, Value as J};
use std::collections::{BTreeMap, BTreeSet};

pub const GENERATOR: &str = "source-labels/v1";
pub const ROUTING_GENERATOR: &str = "routing-terms/v1";
pub const INDEX_SCHEMA: &str = "kpopper.derived-description-index/v2";
pub const DESCRIPTION_SCHEMA: &str = "kpopper.derived-description/v2";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptionStyle {
    SourceLabels,
    RoutingTerms,
}

impl DescriptionStyle {
    pub fn key(self) -> &'static str {
        match self {
            Self::SourceLabels => "source-labels",
            Self::RoutingTerms => "routing-terms",
        }
    }

    pub fn generator(self) -> &'static str {
        match self {
            Self::SourceLabels => GENERATOR,
            Self::RoutingTerms => ROUTING_GENERATOR,
        }
    }

    fn kind(self) -> &'static str {
        self.key()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DescriptionMetrics {
    /// Groups whose cached basis was valid, including groups rebound to a new revision.
    pub hits: usize,
    /// Requested groups whose cached description was missing, malformed or stale.
    pub misses: usize,
    /// Descriptions generated from current source material.
    pub generated: usize,
    /// Valid descriptions rebound after exact basis and scope checks.
    pub rebound: usize,
    /// Old groups removed from a full refreshed index.
    pub deleted: usize,
    /// Sorted group handles regenerated or removed by this refresh.
    pub affected_group_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RefreshResult {
    pub index: J,
    pub metrics: DescriptionMetrics,
}

#[derive(Debug)]
struct GraphIndex<'a> {
    revision: String,
    scope: J,
    project_identity: J,
    nodes: BTreeMap<String, &'a J>,
    members_by_path: BTreeMap<String, BTreeSet<String>>,
    edges: Vec<&'a J>,
    incident_edges: BTreeMap<String, Vec<usize>>,
}

#[derive(Clone, Debug)]
struct GroupSpec {
    group_id: String,
    member_ids: Vec<String>,
}

fn digest(value: &J) -> Result<String> {
    Ok(sha256(&serde_json::to_vec(value)?))
}

fn required_str<'a>(value: &'a J, field: &str) -> Result<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| Error(format!("invalid description view {field}")))
}

fn sorted_ids(value: &J) -> Result<Vec<String>> {
    let values = value
        .as_array()
        .ok_or_else(|| Error("invalid description members".into()))?;
    let mut ids = values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error("invalid description member identity".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    ids.sort();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(Error("duplicate description member identity".into()));
    }
    Ok(ids)
}

impl<'a> GraphIndex<'a> {
    fn new(full: &'a J) -> Result<Self> {
        if full["schema"] != crate::canonical_view::SCHEMA
            || !full["groups"].as_array().is_some_and(Vec::is_empty)
        {
            return Err(Error(
                "description construction requires a fully expanded canonical v1 view".into(),
            ));
        }
        let revision = required_str(full, "revision")?.to_owned();
        let scope = full
            .get("scope")
            .cloned()
            .ok_or_else(|| Error("missing description scope".into()))?;
        if !scope.is_string() {
            return Err(Error("invalid description scope".into()));
        }
        let project_identity = full
            .get("project_identity")
            .cloned()
            .ok_or_else(|| Error("missing description project identity".into()))?;

        let rows = full["nodes"]
            .as_array()
            .ok_or_else(|| Error("invalid description nodes".into()))?;
        let mut nodes = BTreeMap::new();
        for node in rows {
            let id = required_str(node, "source_id")?.to_owned();
            if nodes.insert(id, node).is_some() {
                return Err(Error("duplicate description source identity".into()));
            }
            V::from_tagged(&node["body"])?;
        }
        if let Some(count) = full["coverage"]["count"].as_u64()
            && count as usize != nodes.len()
        {
            return Err(Error("incomplete description source coverage".into()));
        }
        if let Some(ids) = full["coverage"]["source_ids"].as_array() {
            let expected: BTreeSet<_> = ids.iter().filter_map(J::as_str).collect();
            if expected.len() != nodes.len()
                || !nodes.keys().all(|id| expected.contains(id.as_str()))
            {
                return Err(Error(
                    "description source identities disagree with coverage".into(),
                ));
            }
        }

        let navigation = full["navigation_membership"]
            .as_object()
            .ok_or_else(|| Error("missing navigation membership".into()))?;
        let mut members_by_path = BTreeMap::<String, BTreeSet<String>>::new();
        for (id, paths) in navigation {
            if !nodes.contains_key(id) {
                return Err(Error(
                    "navigation membership contains an unknown source".into(),
                ));
            }
            let paths = paths
                .as_array()
                .ok_or_else(|| Error("invalid navigation membership".into()))?;
            let mut unique_paths = BTreeSet::new();
            for path in paths {
                let path = path
                    .as_str()
                    .ok_or_else(|| Error("invalid navigation path".into()))?;
                if !unique_paths.insert(path.to_owned()) {
                    return Err(Error("duplicate navigation path".into()));
                }
                members_by_path
                    .entry(path.to_owned())
                    .or_default()
                    .insert(id.clone());
            }
        }

        let links = full["links"]
            .as_array()
            .ok_or_else(|| Error("invalid description links".into()))?;
        let mut edges = Vec::with_capacity(links.len());
        let mut incident_edges = BTreeMap::<String, Vec<usize>>::new();
        for link in links {
            let edge = link
                .get("source")
                .filter(|source| source.is_object())
                .ok_or_else(|| Error("invalid description source edge".into()))?;
            let edge_index = edges.len();
            edges.push(edge);
            let mut endpoints = BTreeSet::new();
            for endpoint in [edge.get("from"), edge.get("to")].into_iter().flatten() {
                if let Some(id) = endpoint.as_str().filter(|id| nodes.contains_key(*id)) {
                    endpoints.insert(id.to_owned());
                }
            }
            for id in endpoints {
                incident_edges.entry(id).or_default().push(edge_index);
            }
        }

        Ok(Self {
            revision,
            scope,
            project_identity,
            nodes,
            members_by_path,
            edges,
            incident_edges,
        })
    }

    fn full_group_specs(&self, requested: Option<&[String]>) -> Result<Vec<GroupSpec>> {
        let paths: BTreeSet<String> = if let Some(requested) = requested {
            let mut paths = BTreeSet::new();
            for group in requested {
                let path = group
                    .strip_prefix("group:")
                    .ok_or_else(|| Error(format!("invalid description group handle: {group}")))?;
                if !self.members_by_path.contains_key(path) {
                    return Err(Error(format!("unknown description group: {group}")));
                }
                paths.insert(path.to_owned());
            }
            paths
        } else {
            self.members_by_path.keys().cloned().collect()
        };
        Ok(paths
            .into_iter()
            .map(|path| GroupSpec {
                group_id: format!("group:{path}"),
                member_ids: self.members_by_path[&path].iter().cloned().collect(),
            })
            .collect())
    }

    fn selected_group_specs(&self, selected: &J) -> Result<Vec<GroupSpec>> {
        if selected["schema"] != crate::canonical_view::SCHEMA
            || selected["revision"] != self.revision
            || selected["scope"] != self.scope
            || selected["project_identity"] != self.project_identity
        {
            return Err(Error(
                "description view revision/scope/project mismatch".into(),
            ));
        }
        let rows = selected["groups"]
            .as_array()
            .ok_or_else(|| Error("invalid selected description groups".into()))?;
        let mut specs = Vec::with_capacity(rows.len());
        let mut seen = BTreeSet::new();
        let mut partition = BTreeSet::new();
        let selected_nodes = selected["nodes"]
            .as_array()
            .ok_or_else(|| Error("invalid selected description nodes".into()))?;
        for node in selected_nodes {
            let id = required_str(node, "source_id")?.to_owned();
            let source = self
                .nodes
                .get(&id)
                .ok_or_else(|| Error("selected description node is outside full view".into()))?;
            if *source != node || !partition.insert(id) {
                return Err(Error(
                    "selected description nodes are not an exact partition".into(),
                ));
            }
        }
        for row in rows {
            let group_id = required_str(row, "group_id")?.to_owned();
            if !seen.insert(group_id.clone()) {
                return Err(Error("duplicate selected description group".into()));
            }
            let path = group_id
                .strip_prefix("group:")
                .ok_or_else(|| Error("invalid selected description group handle".into()))?;
            if row.get("path").and_then(J::as_str) != Some(path) {
                return Err(Error(
                    "selected description group handle/path mismatch".into(),
                ));
            }
            let member_ids = sorted_ids(&row["member_source_ids"])?;
            if member_ids.is_empty() {
                return Err(Error("empty selected description group".into()));
            }
            let full_members = self
                .members_by_path
                .get(path)
                .ok_or_else(|| Error(format!("unknown description group: {group_id}")))?;
            if member_ids.iter().any(|id| !full_members.contains(id)) {
                return Err(Error(
                    "selected description members exceed full group".into(),
                ));
            }
            if member_ids.iter().any(|id| !partition.insert(id.clone())) {
                return Err(Error(
                    "selected description groups overlap direct ownership".into(),
                ));
            }
            specs.push(GroupSpec {
                group_id,
                member_ids,
            });
        }
        let expected: BTreeSet<_> = self.nodes.keys().cloned().collect();
        if partition != expected
            || selected["coverage"]["count"].as_u64() != Some(expected.len() as u64)
        {
            return Err(Error(
                "selected description view is not a complete partition".into(),
            ));
        }
        if let Some(ids) = selected["coverage"]["source_ids"].as_array() {
            let coverage_ids: BTreeSet<_> = ids.iter().filter_map(J::as_str).collect();
            if coverage_ids.len() != expected.len()
                || !expected.iter().all(|id| coverage_ids.contains(id.as_str()))
            {
                return Err(Error(
                    "selected description identities disagree with coverage".into(),
                ));
            }
        }
        Ok(specs)
    }

    fn basis(&self, group: &str, members: &[String], style: DescriptionStyle) -> Result<J> {
        let path = group
            .strip_prefix("group:")
            .ok_or_else(|| Error("invalid description group handle".into()))?;
        let full_members = self
            .members_by_path
            .get(path)
            .ok_or_else(|| Error("unknown description group".into()))?;
        if members.is_empty() || members.iter().any(|id| !full_members.contains(id)) {
            return Err(Error("invalid selected description membership".into()));
        }
        let nodes = members
            .iter()
            .map(|id| {
                self.nodes
                    .get(id)
                    .copied()
                    .cloned()
                    .ok_or_else(|| Error("missing description basis node".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut incident = BTreeSet::new();
        for id in members {
            if let Some(edge_indices) = self.incident_edges.get(id) {
                incident.extend(edge_indices.iter().copied());
            }
        }
        let edges: Vec<_> = incident
            .into_iter()
            .map(|index| self.edges[index].clone())
            .collect();
        let member_sha256 = digest(&json!(members))?;
        Ok(json!({
            "schema":"kpopper.derived-description-basis/v2",
            "group_id":group,
            "style":style.key(),
            "generator":style.generator(),
            "project_identity":self.project_identity,
            "scope":self.scope,
            "members":members,
            "member_count":members.len(),
            "member_sha256":member_sha256,
            "nodes":nodes,
            "incident_edges":edges,
        }))
    }
}

fn path_from_handle(group: &str) -> Result<&str> {
    group
        .strip_prefix("group:")
        .ok_or_else(|| Error("invalid description group handle".into()))
}

fn label_value(node: &J, key: &str) -> Option<String> {
    let value = V::from_tagged(&node["body"]).ok()?;
    let V::Map(fields) = value else { return None };
    match fields.get(key) {
        Some(V::Text(text)) if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    }
}

fn source_label(node: &J) -> Result<String> {
    V::from_tagged(&node["body"])?;
    let label = [
        "description",
        "title",
        "name",
        "summary",
        "v",
        "value",
        "text",
    ]
    .iter()
    .find_map(|key| label_value(node, key));
    let id = node["source_id"].as_str().unwrap_or("");
    match label {
        Some(label) => {
            let short: String = label.chars().take(180).collect();
            Ok(format!(
                "{id}: {}{}",
                short,
                if label.chars().count() > 180 {
                    "… [excerpt]"
                } else {
                    ""
                }
            ))
        }
        None => Ok(id.to_owned()),
    }
}

fn token_terms(text: &str) -> BTreeSet<String> {
    const STOP: &[&str] = &[
        "about", "after", "also", "and", "are", "because", "been", "being", "between", "but",
        "can", "claim", "claims", "for", "from", "have", "into", "its", "more", "not", "only",
        "our", "record", "should", "source", "that", "the", "their", "there", "these", "this",
        "those", "through", "was", "were", "what", "when", "where", "which", "with", "של", "את",
        "על", "עם", "הוא", "היא", "זה", "זו", "וכן",
    ];
    let normalized = text.to_lowercase();
    normalized
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| word.chars().count() > 2 && !STOP.contains(word))
        .map(str::to_owned)
        .collect()
}

fn field_text(node: &J, key: &str) -> Option<String> {
    label_value(node, key)
}

fn material_kind(node: &J) -> Option<String> {
    node["original_node"]["kind"]
        .as_str()
        .or_else(|| node["record_metadata"]["kind"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            ["kind", "type", "category"]
                .iter()
                .find_map(|key| field_text(node, key))
        })
}

fn uncertainty_tags(node: &J) -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    let status = &node["status"];
    if status["declared_conflict"] == true {
        tags.insert("conflict".to_owned());
    }
    if status["declared_gap"]["evaluation"]
        .as_str()
        .is_some_and(|value| value != "not_evaluated")
    {
        tags.insert("gap".to_owned());
    }
    if status["premise_changed"] == true {
        tags.insert("premise-changed".to_owned());
    }
    if node["uncertainty"]
        .as_array()
        .is_some_and(|states| !states.is_empty())
        || node["original_node"]["states"]
            .as_array()
            .is_some_and(|states| !states.is_empty())
    {
        tags.insert("uncertainty".to_owned());
    }
    tags
}

fn routing_text(nodes: &[J]) -> String {
    let mut kinds = BTreeSet::new();
    let mut states = BTreeSet::new();
    let mut member_terms = BTreeMap::<String, usize>::new();
    for node in nodes {
        if let Some(kind) = material_kind(node) {
            kinds.insert(kind);
        }
        states.extend(uncertainty_tags(node));
        let mut terms = BTreeSet::new();
        for field in [
            "title",
            "name",
            "summary",
            "description",
            "topic",
            "category",
            "type",
            "kind",
            "text",
            "value",
        ] {
            if let Some(text) = field_text(node, field) {
                terms.extend(token_terms(&text));
            }
        }
        for term in terms {
            *member_terms.entry(term).or_default() += 1;
        }
    }
    let member_count = nodes.len();
    let mut ranked: Vec<_> = member_terms.into_iter().collect();
    ranked.retain(|(_, frequency)| member_count <= 1 || *frequency < member_count);
    ranked.sort_by(|(left, left_df), (right, right_df)| {
        left_df.cmp(right_df).then_with(|| left.cmp(right))
    });
    let terms = ranked
        .into_iter()
        .take(8)
        .map(|(term, _)| term)
        .collect::<Vec<_>>();
    let mut cues = Vec::new();
    if !kinds.is_empty() {
        cues.push(format!(
            "material kind: {}",
            kinds.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if !states.is_empty() {
        cues.push(format!(
            "uncertainty: {}",
            states.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    if !terms.is_empty() {
        cues.push(format!("distinctive terms: {}", terms.join(", ")));
    }
    if cues.is_empty() {
        cues.push("no declared material kind, uncertainty flag or routing terms".into());
    }
    format!("Navigation only; derived routing cues: {}", cues.join("; "))
}

fn make_description(
    graph: &GraphIndex<'_>,
    group: &str,
    members: &[String],
    style: DescriptionStyle,
) -> Result<J> {
    let basis = graph.basis(group, members, style)?;
    let basis_sha256 = digest(&basis)?;
    let nodes = basis["nodes"]
        .as_array()
        .ok_or_else(|| Error("invalid description basis nodes".into()))?;
    let text = match style {
        DescriptionStyle::SourceLabels => {
            let labels = nodes.iter().map(source_label).collect::<Result<Vec<_>>>()?;
            format!(
                "Record claims in this group (navigation only): {}",
                labels.join("; ")
            )
        }
        DescriptionStyle::RoutingTerms => routing_text(nodes),
    };
    let description_sha256 = digest(&json!({
        "basis_sha256":basis_sha256,
        "member_count":members.len(),
        "style":style.key(),
        "generator":style.generator(),
        "kind":style.kind(),
        "text":text,
    }))?;
    Ok(json!({
        "schema":DESCRIPTION_SCHEMA,
        "group_id":group,
        "scope":graph.scope,
        "revision":graph.revision,
        "project_identity":graph.project_identity,
        "style":style.key(),
        "generator":style.generator(),
        "kind":style.kind(),
        "basis_sha256":basis_sha256,
        "description_sha256":description_sha256,
        "member_count":members.len(),
        "text":text,
        "is_source_evidence":false,
    }))
}

fn valid_cached_description(
    cached: &J,
    group: &str,
    scope: &J,
    identity: &J,
    basis_sha256: &str,
    style: DescriptionStyle,
    member_count: usize,
) -> bool {
    let content_hash = digest(&json!({
        "basis_sha256":cached["basis_sha256"],
        "member_count":cached["member_count"],
        "style":cached["style"],
        "generator":cached["generator"],
        "kind":cached["kind"],
        "text":cached["text"],
    }))
    .ok();
    cached["schema"] == DESCRIPTION_SCHEMA
        && cached["group_id"] == group
        && cached["scope"] == *scope
        && cached["project_identity"] == *identity
        && cached["style"] == style.key()
        && cached["generator"] == style.generator()
        && cached["kind"] == style.kind()
        && cached["basis_sha256"] == basis_sha256
        && cached["member_count"].as_u64() == Some(member_count as u64)
        && cached["text"].is_string()
        && content_hash
            .as_ref()
            .is_some_and(|hash| cached["description_sha256"] == *hash)
        && cached["is_source_evidence"] == false
        && cached["revision"].is_string()
}

fn index_header(
    graph: &GraphIndex<'_>,
    style: DescriptionStyle,
    descriptions: Map<String, J>,
) -> J {
    json!({
        "schema":INDEX_SCHEMA,
        "style":style.key(),
        "generator":style.generator(),
        "revision":graph.revision,
        "scope":graph.scope,
        "project_identity":graph.project_identity,
        "descriptions":descriptions,
    })
}

fn build_or_refresh_specs(
    graph: &GraphIndex<'_>,
    specs: &[GroupSpec],
    previous: Option<&J>,
    style: DescriptionStyle,
    delete_unrequested: bool,
) -> Result<RefreshResult> {
    let previous_descriptions = previous
        .filter(|index| {
            index["schema"] == INDEX_SCHEMA
                && index["style"] == style.key()
                && index["generator"] == style.generator()
                && index["revision"].is_string()
                && index.get("scope").is_some()
                && index.get("project_identity").is_some()
        })
        .and_then(|index| index["descriptions"].as_object());
    let mut descriptions = Map::new();
    let mut metrics = DescriptionMetrics::default();
    let mut requested = BTreeSet::new();
    for spec in specs {
        requested.insert(spec.group_id.clone());
        let basis = graph.basis(&spec.group_id, &spec.member_ids, style)?;
        let basis_sha256 = digest(&basis)?;
        let cached = previous_descriptions.and_then(|entries| entries.get(&spec.group_id));
        if let Some(cached) = cached.filter(|cached| {
            previous.is_some_and(|index| {
                cached["revision"] == index["revision"]
                    && cached["scope"] == index["scope"]
                    && cached["project_identity"] == index["project_identity"]
            }) && valid_cached_description(
                cached,
                &spec.group_id,
                &graph.scope,
                &graph.project_identity,
                &basis_sha256,
                style,
                spec.member_ids.len(),
            )
        }) {
            metrics.hits += 1;
            let mut rebound = cached.clone();
            if cached["revision"] != graph.revision {
                rebound["revision"] = json!(graph.revision);
                metrics.rebound += 1;
            }
            descriptions.insert(spec.group_id.clone(), rebound);
        } else {
            metrics.misses += 1;
            metrics.generated += 1;
            metrics.affected_group_ids.push(spec.group_id.clone());
            descriptions.insert(
                spec.group_id.clone(),
                make_description(graph, &spec.group_id, &spec.member_ids, style)?,
            );
        }
    }
    if delete_unrequested
        && let Some(old) = previous.and_then(|index| index["descriptions"].as_object())
    {
        for group in old.keys().filter(|group| !requested.contains(*group)) {
            metrics.deleted += 1;
            metrics.affected_group_ids.push(group.clone());
        }
    }
    metrics.affected_group_ids.sort();
    metrics.affected_group_ids.dedup();
    Ok(RefreshResult {
        index: index_header(graph, style, descriptions),
        metrics,
    })
}

/// Basis for a fully expanded source-label group. The global revision is
/// intentionally excluded; it is checked by the view service, not content reuse.
pub fn basis(view: &J, group: &str) -> Result<J> {
    basis_with_style(view, group, DescriptionStyle::SourceLabels)
}

pub fn basis_with_style(view: &J, group: &str, style: DescriptionStyle) -> Result<J> {
    let graph = GraphIndex::new(view)?;
    let path = path_from_handle(group)?;
    let members = graph
        .members_by_path
        .get(path)
        .ok_or_else(|| Error("unknown description group".into()))?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    graph.basis(group, &members, style)
}

/// The legacy generator remains the source-label control.
pub fn generate(view: &J, group: &str) -> Result<J> {
    generate_with_style(view, group, DescriptionStyle::SourceLabels)
}

pub fn generate_with_style(view: &J, group: &str, style: DescriptionStyle) -> Result<J> {
    let graph = GraphIndex::new(view)?;
    let path = path_from_handle(group)?;
    let members = graph
        .members_by_path
        .get(path)
        .ok_or_else(|| Error("unknown description group".into()))?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    make_description(&graph, group, &members, style)
}

fn stale(group: &str) -> J {
    json!({"status":"stale","group_id":group,"text":null,"is_source_evidence":false})
}

fn serve_members(
    graph: &GraphIndex<'_>,
    group: &str,
    member_ids: &[String],
    saved: &J,
) -> Result<J> {
    if saved["schema"] != DESCRIPTION_SCHEMA {
        // The v1 format is deliberately recognizable as stale, never upgraded by
        // interpreting old basis IDs or global-link hashes as the new basis.
        return Ok(stale(group));
    }
    let style = match saved["style"].as_str() {
        Some("source-labels") => DescriptionStyle::SourceLabels,
        Some("routing-terms") => DescriptionStyle::RoutingTerms,
        _ => return Ok(stale(group)),
    };
    let basis = graph.basis(group, member_ids, style)?;
    let basis_sha256 = digest(&basis)?;
    if !valid_cached_description(
        saved,
        group,
        &graph.scope,
        &graph.project_identity,
        &basis_sha256,
        style,
        member_ids.len(),
    ) {
        return Ok(stale(group));
    }
    let mut description = saved.clone();
    let status = if saved["revision"] == graph.revision {
        "current"
    } else {
        description["revision"] = json!(graph.revision);
        "rebound"
    };
    Ok(json!({"status":status,"description":description}))
}

/// Serve or safely rebind a cached full-group description. Stale values expose no text.
pub fn serve(view: &J, group: &str, saved: &J) -> Result<J> {
    let graph = GraphIndex::new(view)?;
    let path = path_from_handle(group)?;
    let member_ids = graph
        .members_by_path
        .get(path)
        .ok_or_else(|| Error("unknown description group".into()))?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    serve_members(&graph, group, &member_ids, saved)
}

pub fn build_index(full: &J) -> Result<J> {
    build_index_with_style(full, DescriptionStyle::SourceLabels)
}

pub fn build_index_with_style(full: &J, style: DescriptionStyle) -> Result<J> {
    let graph = GraphIndex::new(full)?;
    let specs = graph.full_group_specs(None)?;
    Ok(build_or_refresh_specs(&graph, &specs, None, style, false)?.index)
}

/// Build descriptions only for the full memberships of the requested handles.
pub fn build_for_groups(full: &J, groups: &[String], style: DescriptionStyle) -> Result<J> {
    let graph = GraphIndex::new(full)?;
    let specs = graph.full_group_specs(Some(groups))?;
    Ok(build_or_refresh_specs(&graph, &specs, None, style, false)?.index)
}

/// Build only the group rows visible in a selected v1 packet. Residual groups
/// use their exact displayed member set rather than the full navigation facet.
pub fn build_for_view(full: &J, selected: &J, style: DescriptionStyle) -> Result<J> {
    let graph = GraphIndex::new(full)?;
    let specs = graph.selected_group_specs(selected)?;
    Ok(build_or_refresh_specs(&graph, &specs, None, style, false)?.index)
}

/// Refresh a complete index, reusing entries whose exact content basis is unchanged.
pub fn refresh_index(full: &J, previous: &J, style: DescriptionStyle) -> Result<RefreshResult> {
    let graph = GraphIndex::new(full)?;
    let specs = graph.full_group_specs(None)?;
    build_or_refresh_specs(&graph, &specs, Some(previous), style, true)
}

/// Refresh only requested full-group entries. The result is a partial index
/// suitable for attaching to a packet that displays those groups.
pub fn refresh_for_groups(
    full: &J,
    previous: &J,
    groups: &[String],
    style: DescriptionStyle,
) -> Result<RefreshResult> {
    let graph = GraphIndex::new(full)?;
    let specs = graph.full_group_specs(Some(groups))?;
    build_or_refresh_specs(&graph, &specs, Some(previous), style, false)
}

/// Refresh only selected packet groups, using residual memberships when present.
pub fn refresh_for_view(
    full: &J,
    selected: &J,
    previous: &J,
    style: DescriptionStyle,
) -> Result<RefreshResult> {
    let graph = GraphIndex::new(full)?;
    let specs = graph.selected_group_specs(selected)?;
    build_or_refresh_specs(&graph, &specs, Some(previous), style, false)
}

/// Attach descriptions only when every displayed group has an exact current or
/// safely rebound basis. A full-group cache cannot describe a residual subset.
pub fn attach(packet: &mut J, full: &J, index: &J) -> Result<()> {
    let graph = GraphIndex::new(full)?;
    if packet["schema"] != crate::canonical_view::SCHEMA
        || packet["revision"] != graph.revision
        || packet["scope"] != graph.scope
        || packet["project_identity"] != graph.project_identity
    {
        return Err(Error(
            "description view revision/scope/project mismatch".into(),
        ));
    }
    if index["schema"] != INDEX_SCHEMA {
        let message = if index["schema"] == "kpopper.derived-description-index/v1" {
            "legacy derived description index is stale; rebuild v2 descriptions"
        } else {
            "invalid derived description index"
        };
        return Err(Error(message.into()));
    }
    if !index["revision"].is_string()
        || index["scope"] != graph.scope
        || index["project_identity"] != graph.project_identity
    {
        return Err(Error(
            "malformed or out-of-scope derived description index".into(),
        ));
    }
    let style = match index["style"].as_str() {
        Some("source-labels") => DescriptionStyle::SourceLabels,
        Some("routing-terms") => DescriptionStyle::RoutingTerms,
        _ => return Err(Error("invalid derived description style".into())),
    };
    if index["generator"] != style.generator() {
        return Err(Error("derived description generator/style mismatch".into()));
    }
    let entries = index["descriptions"]
        .as_object()
        .ok_or_else(|| Error("invalid derived description entries".into()))?;
    let specs = graph.selected_group_specs(packet)?;
    let groups = packet["groups"]
        .as_array_mut()
        .ok_or_else(|| Error("invalid view groups".into()))?;
    for (row, spec) in groups.iter_mut().zip(specs) {
        let saved = entries.get(&spec.group_id).ok_or_else(|| {
            Error(format!(
                "partial description index missing group {}",
                spec.group_id
            ))
        })?;
        let served = serve_members(&graph, &spec.group_id, &spec.member_ids, saved)?;
        if served["status"] == "stale" {
            return Err(Error(format!(
                "stale derived description for {}; refresh against displayed membership",
                spec.group_id
            )));
        }
        row["description"] = served;
    }
    packet["descriptions"] = json!({
        "kind":style.kind(),
        "generator":style.generator(),
        "generated_claims":false,
        "is_source_evidence":false
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_names_remain_distinct() {
        assert_ne!(GENERATOR, ROUTING_GENERATOR);
        assert_eq!(DescriptionStyle::SourceLabels.generator(), GENERATOR);
        assert_eq!(
            DescriptionStyle::RoutingTerms.generator(),
            ROUTING_GENERATOR
        );
    }
}
