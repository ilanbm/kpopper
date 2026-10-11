//! Experimental lossless graph view over an already captured checked session.
use serde_json::{json, Value as J};
use std::collections::{BTreeMap, BTreeSet};

#[path = "canonical_frontier.rs"]
mod frontier;

pub const SCHEMA: &str = "kpopper.canonical-graph-view/v1";
pub const COMPACT_SCHEMA: &str = "kpopper.canonical-graph-view/v3";

/// Original IDs take precedence over optional display-handle syntax. This keeps
/// an issued source ID byte-exact when it happens to begin with a reserved prefix.
pub(crate) fn exact_node_id<T>(nodes: &BTreeMap<String, T>, value: &str) -> Option<String> {
    if nodes.contains_key(value) { Some(value.to_owned()) }
    else { value.strip_prefix("node:").filter(|id| nodes.contains_key(*id)).map(str::to_owned) }
}

/// Reveal exact membership without requiring all member bodies to fit at once.
/// The node rows/coverage partition remain unchanged; additional dictionary IDs
/// are navigation references, not read claims. Scope comes only from `logical`.
pub fn add_membership_details(logical: &J, packet: &mut J, requested: &[String]) -> crate::Result<()> {
    use crate::Error;
    if requested.is_empty() { return Ok(()) }
    crate::require(logical["revision"] == packet["revision"] && logical["scope"] == packet["scope"],
        "membership revision/scope mismatch")?;
    let mut dictionary = packet["dictionary"].as_object().cloned().ok_or_else(|| Error("missing membership dictionary".into()))?;
    let mut references = BTreeMap::new();
    for (alias, value) in &dictionary {
        if value["kind"] == "node" && let Some(id) = value["original"].as_str() { references.insert(id.to_owned(), alias.clone()); }
    }
    let mut facets = BTreeMap::<String, BTreeSet<String>>::new();
    for row in packet["navigation_facets"]["nodes"].as_array().into_iter().flatten() {
        if let Some(id) = row[0].as_str() {
            facets.entry(id.into()).or_default().extend(row[1].as_array().into_iter().flatten().filter_map(J::as_str).map(str::to_owned));
        }
    }
    let mut sequence = 0usize;
    for handle in requested {
        let path = handle.strip_prefix("group:").ok_or_else(|| Error("membership requires group:/path".into()))?;
        let group_ref = if let Some((alias,_))=dictionary.iter().find(|(_, value)| value["kind"] == "group" && value["original"] == *handle) {
            alias.clone()
        } else {
            // A coarse frontier can hide this route's alias, never its exact
            // captured membership. Explicit membership requests restore it.
            let known=logical["navigation_membership"].as_object().into_iter().flat_map(|nav|nav.values())
                .any(|paths|paths.as_array().into_iter().flatten().any(|value|value.as_str()==Some(path)));
            crate::require(known,&format!("unknown membership group: {handle}"))?;
            while dictionary.contains_key(&format!("m{sequence}")) { sequence+=1; }
            let alias=format!("m{sequence}"); sequence+=1;
            dictionary.insert(alias.clone(),json!({"kind":"group","original":handle}));
            alias
        };
        let visible = logical["groups"].as_array().into_iter().flatten().find(|group| group["group_id"] == *handle);
        let members = if let Some(group) = visible {
            group["member_source_ids"].as_array().into_iter().flatten().filter_map(J::as_str).map(str::to_owned).collect::<BTreeSet<_>>()
        } else {
            logical["navigation_membership"].as_object().ok_or_else(|| Error("missing navigation membership".into()))?
                .iter().filter(|(_, paths)| paths.as_array().is_some_and(|p| p.iter().any(|v| v.as_str() == Some(path))))
                .map(|(id, _)| id.clone()).collect::<BTreeSet<_>>()
        };
        for id in members {
            let alias = if let Some(existing) = references.get(&id) { existing.clone() }
                else {
                    while dictionary.contains_key(&format!("m{sequence}")) { sequence += 1; }
                    let alias = format!("m{sequence}"); sequence += 1;
                    dictionary.insert(alias.clone(), json!({"kind":"node","original":id}));
                    references.insert(id, alias.clone()); alias
                };
            facets.entry(alias).or_default().insert(group_ref.clone());
        }
    }
    packet["dictionary"] = J::Object(dictionary);
    packet["navigation_facets"]["nodes"] = json!(facets.into_iter().map(|(id, groups)| json!([id,groups])).collect::<Vec<_>>());
    packet["membership_detail"] = json!({"groups":requested,"additional_bodies_read":false,
        "next":"Use dictionary node identities with --id to read exact bodies; listed membership alone is not evidence."});
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct CanonicalViewRequest {
    /// Explicit source IDs to expose directly while leaving other graph coverage intact.
    pub focus: Vec<String>,
    /// Addressable topic handles to expand. Repeating requests accumulates expansion.
    pub expand: Vec<String>,
    /// Optional presentation frontier used after ordinary orientation cannot fit.
    /// Exact focus/expansion overrides it; it never bounds evidence discovery.
    pub frontier_depth: Option<usize>,
}

/// Build a common graph packet. Every source node is represented exactly once in
/// `nodes` or `groups`; `navigation_membership` separately retains overlapping paths.
// `only_used_in_recursion` fires on the nested `visit` but honours only this
// enclosing item's level, so the allow cannot move onto `visit`.
#[allow(clippy::too_many_arguments, clippy::only_used_in_recursion)]
pub fn build(
    project: &str,
    project_identity: J,
    revision: &str,
    scope: &str,
    nodes: &BTreeMap<String, J>,
    links: &[J],
    groups: &BTreeMap<String, BTreeSet<String>>,
    children: &BTreeMap<String, BTreeSet<String>>,
    direct: &BTreeMap<String, Vec<String>>,
    leaves: &BTreeMap<String, String>,
    request: &CanonicalViewRequest,
) -> crate::Result<J> {
    crate::require(request.frontier_depth != Some(0), "navigation frontier depth must be positive")?;
    let tree = frontier::Tree { revision, scope, groups, children, direct };
    let child_details = request.expand.iter().filter(|id|id.starts_with("children:"))
        .map(|handle|tree.expand(handle)).collect::<crate::Result<Vec<_>>>()?;
    for id in &request.focus {
        let valid = exact_node_id(nodes, id).is_some()
            || groups.contains_key(id.strip_prefix("group:").unwrap_or(id));
        if !valid {
            return Err(crate::Error(format!("unknown canonical view handle: {id}")));
        }
    }
    let mut focus: BTreeSet<_> = request
        .focus
        .iter()
        .filter_map(|id| exact_node_id(nodes, id))
        .collect();
    let mut expanded: BTreeSet<_> = request
        .expand
        .iter()
        .filter(|id|!id.starts_with("children:"))
        .map(|id| id.strip_prefix("group:").unwrap_or(id).to_owned())
        .collect();
    for id in &request.focus {
        let key = id.strip_prefix("group:").unwrap_or(id);
        if exact_node_id(nodes, id).is_none() && groups.contains_key(key) {
            expanded.insert(key.to_owned());
        }
    }
    for id in &request.expand {
        if id.starts_with("children:") { continue; }
        let key = id.strip_prefix("group:").unwrap_or(id);
        if !groups.contains_key(key) {
            let message = if exact_node_id(nodes, id).is_some() { "expand requires a group handle" }
                else { "unknown canonical view handle" };
            return Err(crate::Error(format!("{message}: {id}")));
        }
    }
    // Resolve every explicit group to exact bodies before any sibling can own
    // them as folded coverage, including non-subset overlapping facets.
    for path in &expanded {
        focus.extend(groups.get(path).into_iter().flatten().cloned());
    }

    let mut visible = BTreeSet::new();
    let mut out_nodes = Vec::new();
    let mut out_groups = Vec::new();
    let mut frontier_paths = BTreeSet::new();
    #[allow(clippy::too_many_arguments)]
    fn visit(
        path: &str,
        nodes: &BTreeMap<String, J>,
        groups: &BTreeMap<String, BTreeSet<String>>,
        children: &BTreeMap<String, BTreeSet<String>>,
        direct: &BTreeMap<String, Vec<String>>,
        leaves: &BTreeMap<String, String>,
        focus: &BTreeSet<String>,
        expanded: &BTreeSet<String>,
        frontier_depth: Option<usize>,
        visible: &mut BTreeSet<String>,
        out_nodes: &mut Vec<J>,
        out_groups: &mut Vec<J>,
        frontier_paths: &mut BTreeSet<String>,
    ) {
        let ids = groups.get(path).cloned().unwrap_or_default();
        let original_member_count = ids.len();
        let force_open = expanded.contains(path) || expanded.iter().any(|target| {
            // Balancing buckets can contain named children outside their string
            // prefix. Membership is authoritative for keeping those routes open.
            target.starts_with(&format!("{}/", path.trim_end_matches('/')))
                || (frontier_depth.is_some() && groups.get(target).is_some_and(|members| members.is_subset(&ids)))
        });
        // Keep structural containers visible as navigation paths. Fold a terminal
        // branch, or a branch whose children are all terminal, so labels like
        // `proposal` and `report` remain visible instead of one generic `known`
        // group swallowing their useful distinction.
        let child_paths = children.get(path);
        let terminal_branch = child_paths.is_none_or(BTreeSet::is_empty)
            || child_paths.is_some_and(|paths| {
                paths.iter().all(|child| {
                    groups.get(child).is_some_and(|members| members.len() <= 1)
                        && children.get(child).is_none_or(BTreeSet::is_empty)
                })
            });
        let unowned: BTreeSet<_> = ids.into_iter().filter(|id| !visible.contains(id)).collect();
        let contains_named_children = child_paths.is_some_and(|paths| paths.iter()
            .any(|child|!child.starts_with(&format!("{}/",path.trim_end_matches('/')))));
        let coarse = frontier_depth.is_some_and(|depth| frontier::depth(path) >= depth)
            && !contains_named_children;
        if frontier_depth.is_some() && path!="/" && original_member_count>0
            && (terminal_branch || coarse) && !contains_named_children && !force_open {
            frontier_paths.insert(path.to_owned());
            // A completely overlapping facet still has a coarse navigation
            // boundary, even when another route owns all its source rows.
            if unowned.is_empty() { return; }
        }
        if path != "/"
            && (original_member_count > 1 || coarse)
            && !unowned.is_empty()
            && (terminal_branch || coarse)
            && !(frontier_depth.is_some() && contains_named_children)
            && !force_open
        {
            visible.extend(unowned.iter().cloned());
            out_groups.push(json!({"group_id":format!("group:{path}"),"path":path,"member_source_ids":unowned,"label":path.rsplit('/').next().unwrap_or(path)}));
            return;
        }
        if expanded.contains(path) {
            for id in groups.get(path).into_iter().flatten() {
                if visible.contains(id) {
                    continue;
                }
                if let Some(node) = nodes.get(id) {
                    out_nodes.push(node.clone());
                    visible.insert(id.clone());
                }
            }
            return;
        }
        let mut folded_direct=Vec::new();
        for id in direct.get(path).into_iter().flatten() {
            if visible.contains(id) {
                continue;
            }
            let Some(node) = nodes.get(id) else { continue };
            if frontier_depth.is_some() && path!="/" {
                folded_direct.push(id.clone());
            } else { out_nodes.push(node.clone()); }
            visible.insert(id.clone());
        }
        if !folded_direct.is_empty() {
            folded_direct.sort();
            out_groups.push(json!({"group_id":format!("group:{path}"),"path":path,
                "member_source_ids":folded_direct,"label":path.rsplit('/').next().unwrap_or(path)}));
        }
        for child in children.get(path).into_iter().flatten() {
            visit(
                child, nodes, groups, children, direct, leaves, focus, expanded, frontier_depth, visible,
                out_nodes, out_groups, frontier_paths,
            );
        }
    }
    // Focus materializes only the requested source rows. The navigation walk
    // below assigns any remaining siblings to their original folded path.
    for id in &focus {
        if let Some(node) = nodes.get(id) {
            out_nodes.push(node.clone());
            visible.insert(id.clone());
        }
    }
    visit(
        "/",
        nodes,
        groups,
        children,
        direct,
        leaves,
        &focus,
        &expanded,
        request.frontier_depth,
        &mut visible,
        &mut out_nodes,
        &mut out_groups,
        &mut frontier_paths,
    );
    let represented = out_nodes.len()
        + out_groups
            .iter()
            .map(|group| group["member_source_ids"].as_array().map_or(0, Vec::len))
            .sum::<usize>();
    crate::require(
        represented == visible.len()
            && visible.len() == nodes.len()
            && nodes.keys().all(|id| visible.contains(id)),
        "canonical view coverage is not a complete disjoint partition",
    )?;
    let mut coverage = visible.iter().cloned().collect::<Vec<_>>();
    coverage.sort();
    let mut navigation_membership = BTreeMap::<String, Vec<String>>::new();
    for (path, members) in groups {
        for id in members {
            navigation_membership
                .entry(id.clone())
                .or_default()
                .push(path.clone());
        }
    }
    for paths in navigation_membership.values_mut() {
        paths.sort();
    }
    let direct_owner: BTreeMap<String, String> = out_nodes
        .iter()
        .filter_map(|n| {
            Some((
                n["source_id"].as_str()?.to_owned(),
                n["source_id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let mut membership_owner = direct_owner;
    for group in &out_groups {
        for id in group["member_source_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(J::as_str)
        {
            membership_owner.insert(
                id.to_owned(),
                group["group_id"].as_str().unwrap().to_owned(),
            );
        }
    }
    let projected_links: Vec<J> = links.iter().map(|edge| {
        let from=edge["from"].as_str().unwrap_or(""); let to=edge["to"].as_str().unwrap_or("");
        json!({"source":edge,"projected_from":membership_owner.get(from).cloned().unwrap_or_else(||format!("external:{from}")),"projected_to":membership_owner.get(to).cloned().unwrap_or_else(||format!("external:{to}"))})
    }).collect();
    let mode = if !request.expand.is_empty() {
        "expand"
    } else if !request.focus.is_empty() {
        "focus"
    } else {
        "broad"
    };
    let mut packet =
        json!({"schema":SCHEMA,"project":project,"project_identity":project_identity,"revision":revision,"scope":scope,"mode":mode,
        "nodes":out_nodes,"groups":out_groups,"links":projected_links,"coverage":{"source_ids":coverage,"count":visible.len()},
        "navigation_membership":navigation_membership,"rules":crate::checked_session::CANONICAL_RULES,
        "descriptions":{"kind":"navigation_placeholder","generated_claims":false}});
    if request.frontier_depth.is_some() || !child_details.is_empty() {
        let folded = packet["groups"].as_array().unwrap();
        // Partial groups owning a mixed bucket's direct rows must not hide its
        // independently named children. Only fully collapsed paths hide routes.
        let hidden=tree.hidden_descendants(&frontier_paths);
        frontier_paths.extend(folded.iter().filter_map(|group|group["path"].as_str()).map(str::to_owned));
        let summaries = frontier_paths.iter().map(|path| {
            let owner=folded.iter().find(|group|group["path"].as_str()==Some(path));
            let members=owner.map(|group|group["member_source_ids"].clone()).unwrap_or_else(||json!(groups[path]));
            let mut summary=tree.summary(path,&members)?;
            summary["owns_source_rows"]=json!(owner.is_some());Ok(summary)
        }).collect::<crate::Result<Vec<_>>>()?;
        packet["navigation_hidden_paths"] = json!(hidden);
        packet["navigation_frontier"] = json!({"schema":"kpopper.navigation-frontier/v1",
            "revision":revision,"scope":scope,"depth":request.frontier_depth,
            "groups":summaries,"expanded_children":child_details,
            "meaning":"Navigation membership only; use issued children handles for the next level, group handles for exact bodies, or members:group:/path for exact IDs."});
    }
    Ok(packet)
}

/// Fold unrequested bodies at existing navigation routes. Explicit nodes and
/// group expansions retain their full bodies and every original edge is kept.
pub fn fold_unrequested(packet: &J, focus: &[String], expand: &[String]) -> crate::Result<J> {
    let mut result = packet.clone();
    let navigation = packet["navigation_membership"].as_object()
        .ok_or_else(|| crate::Error("view lacks exact navigation membership".into()))?;
    let all_ids = navigation.keys().map(|id|(id.clone(),())).collect::<BTreeMap<_,_>>();
    let mut protected = BTreeSet::new();
    let mut protected_groups = expand.iter().map(|s|s.strip_prefix("group:").unwrap_or(s).to_owned()).collect::<BTreeSet<_>>();
    for id in focus {
        if let Some(id)=exact_node_id(&all_ids,id) { protected.insert(id); }
        else { protected_groups.insert(id.strip_prefix("group:").unwrap_or(id).to_owned()); }
    }
    for (id,paths) in navigation {
        if paths.as_array().into_iter().flatten().filter_map(J::as_str).any(|p|protected_groups.contains(p)) {
            protected.insert(id.clone());
        }
    }
    let mut groups = packet["groups"].as_array().cloned().ok_or_else(||crate::Error("view lacks groups".into()))?;
    let mut retained = Vec::new();
    for node in packet["nodes"].as_array().into_iter().flatten() {
        let id=node["source_id"].as_str().ok_or_else(||crate::Error("view node lacks identity".into()))?;
        if protected.contains(id) { retained.push(node.clone()); continue; }
        let paths=navigation.get(id).and_then(J::as_array).into_iter().flatten()
            .filter_map(J::as_str).filter(|path|*path!="/").collect::<Vec<_>>();
        let existing=groups.iter().enumerate().filter(|(_,group)|paths.contains(&group["path"].as_str().unwrap_or("")))
            .max_by_key(|(_,group)|group["path"].as_str().unwrap_or("").len()).map(|(index,_)|index);
        let index=if let Some(index)=existing {index} else {
            let Some(path)=paths.iter().max_by_key(|path|path.len()) else {retained.push(node.clone());continue;};
            groups.push(json!({"group_id":format!("group:{path}"),"path":path,
                "label":path.rsplit('/').next().unwrap_or(path),"member_source_ids":[]}));
            groups.len()-1
        };
        groups[index]["member_source_ids"].as_array_mut().unwrap().push(json!(id));
    }
    let mut owners = retained.iter().filter_map(|node|node["source_id"].as_str().map(|id|(id.to_owned(),id.to_owned())))
        .collect::<BTreeMap<_,_>>();
    for group in &mut groups {
        let owner=group["group_id"].as_str().unwrap().to_owned();
        let members=group["member_source_ids"].as_array_mut().unwrap();
        members.sort_by(|a,b|a.as_str().cmp(&b.as_str()));
        for id in members.iter().filter_map(J::as_str) {
            crate::require(owners.insert(id.into(),owner.clone()).is_none(),"folded view has duplicate ownership")?;
        }
    }
    crate::require(owners.len()==all_ids.len() && all_ids.keys().all(|id|owners.contains_key(id)),"folded view lost source coverage")?;
    groups.sort_by(|a,b|a["group_id"].as_str().cmp(&b["group_id"].as_str()));
    result["nodes"]=json!(retained);result["groups"]=json!(groups);
    for link in result["links"].as_array_mut().into_iter().flatten() {
        for (source,projected) in [("from","projected_from"),("to","projected_to")] {
            let id=link["source"][source].as_str().unwrap_or("");
            link[projected]=json!(owners.get(id).cloned().unwrap_or_else(||format!("external:{id}")));
        }
    }
    Ok(result)
}

/// Compact a v1 packet with exact source types and selection-bound edge handles.
pub fn compact(packet: &J, requested_edge_sets: &[String]) -> crate::Result<J> {
    use crate::{identity::sha256, value::TypedValue as V, Error};

    fn required_str<'a>(value: &'a J, field: &str) -> crate::Result<&'a str> {
        value[field]
            .as_str()
            .ok_or_else(|| Error(format!("invalid compact view {field}")))
    }
    fn digest_json(value: &J) -> crate::Result<String> {
        Ok(sha256(&serde_json::to_vec(value)?))
    }
    fn sorted_unique_ids(value: &J, field: &str) -> crate::Result<Vec<String>> {
        let values = value[field]
            .as_array()
            .ok_or_else(|| Error(format!("invalid compact view {field}")))?;
        let mut ids = values
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| Error(format!("invalid compact view {field} member")))
            })
            .collect::<crate::Result<Vec<_>>>()?;
        ids.sort();
        ids.dedup();
        if ids.len() != values.len() {
            return Err(Error(format!("duplicate compact view {field} member")));
        }
        Ok(ids)
    }
    fn original_edges(packet: &J) -> crate::Result<Vec<J>> {
        packet["links"]
            .as_array()
            .ok_or_else(|| Error("invalid compact view links".into()))?
            .iter()
            .map(|link| {
                link.get("source")
                    .filter(|source| source.is_object())
                    .cloned()
                    .ok_or_else(|| Error("invalid compact view source edge".into()))
            })
            .collect()
    }

    if packet["schema"] != SCHEMA {
        return Err(Error(
            "compact projection requires canonical graph view v1".into(),
        ));
    }
    let project = required_str(packet, "project")?;
    let revision = required_str(packet, "revision")?;
    let scope = required_str(packet, "scope")?;
    let project_identity = packet
        .get("project_identity")
        .ok_or_else(|| Error("missing compact view project_identity".into()))?;
    V::from_tagged(project_identity)?;
    let nodes_in = packet["nodes"]
        .as_array()
        .ok_or_else(|| Error("invalid compact view nodes".into()))?;
    let groups_in = packet["groups"]
        .as_array()
        .ok_or_else(|| Error("invalid compact view groups".into()))?;
    let edges_in = original_edges(packet)?;

    let mut node_by_id = BTreeMap::<String, &J>::new();
    for node in nodes_in {
        let id = required_str(node, "source_id")?.to_owned();
        if node_by_id.insert(id, node).is_some() {
            return Err(Error("duplicate compact view source node".into()));
        }
        if !node.get("body").is_some_and(J::is_array) {
            return Err(Error("compact view node body is not a typed value".into()));
        }
        V::from_tagged(&node["body"])?;
    }

    let mut group_by_id = BTreeMap::<String, (&J, Vec<String>, String)>::new();
    for group in groups_in {
        let group_id = required_str(group, "group_id")?.to_owned();
        let path = required_str(group, "path")?;
        if group_id != format!("group:{path}") {
            return Err(Error("compact view group handle/path mismatch".into()));
        }
        let ids = sorted_unique_ids(group, "member_source_ids")?;
        if ids.is_empty() {
            return Err(Error("empty compact view group".into()));
        }
        let member_hash = digest_json(&json!(ids))?;
        if group_by_id
            .insert(group_id, (group, ids, member_hash))
            .is_some()
        {
            return Err(Error("duplicate compact view group handle".into()));
        }
    }

    // Build the disjoint ownership partition and the selection binding. An ID
    // may occur in several navigation facets, but it has exactly one row owner.
    let mut owner_by_id = BTreeMap::<String, String>::new();
    for id in node_by_id.keys() {
        owner_by_id.insert(id.clone(), format!("\0node:{id}"));
    }
    for (group_id, (_, ids, _)) in &group_by_id {
        for id in ids {
            if owner_by_id.contains_key(id) {
                return Err(Error(
                    "compact view ownership is not a complete disjoint partition".into(),
                ));
            }
            owner_by_id.insert(id.clone(), group_id.clone());
        }
    }
    if owner_by_id.len()
        != node_by_id.len()
            + group_by_id
                .values()
                .map(|(_, ids, _)| ids.len())
                .sum::<usize>()
        || owner_by_id.values().any(String::is_empty)
    {
        return Err(Error(
            "compact view ownership is not a complete disjoint partition".into(),
        ));
    }
    let mut all_ids: Vec<_> = owner_by_id.keys().cloned().collect();
    all_ids.sort();
    let navigation = packet["navigation_membership"]
        .as_object()
        .ok_or_else(|| Error("invalid compact navigation membership".into()))?;
    let navigation_ids: BTreeSet<_> = navigation.keys().cloned().collect();
    let coverage_ids: BTreeSet<_> = all_ids.iter().cloned().collect();
    if navigation_ids != coverage_ids {
        return Err(Error(
            "compact navigation identities disagree with coverage".into(),
        ));
    }
    for paths in navigation.values() {
        paths
            .as_array()
            .ok_or_else(|| Error("invalid compact navigation membership".into()))?
            .iter()
            .try_for_each(|path| {
                path.as_str()
                    .map(|_| ())
                    .ok_or_else(|| Error("invalid compact navigation path".into()))
            })?;
    }
    let selection_groups: Vec<_> = group_by_id
        .iter()
        .map(|(handle, (_, ids, hash))| json!([handle, ids.len(), hash]))
        .collect();
    let selection_sha256 = digest_json(&json!({
        "schema":"kpopper.canonical-selected-view/v1",
        "revision":revision,
        "scope":scope,
        "nodes":node_by_id.keys().collect::<Vec<_>>(),
        "groups":selection_groups,
    }))?;
    let view_id = digest_json(&json!({
        "schema":"kpopper.canonical-view-id/v1",
        "revision":revision,
        "scope":scope,
        "selection_sha256":selection_sha256,
        "nodes":node_by_id.iter().map(|(id, node)| json!([id, node])).collect::<Vec<_>>(),
        "edges":edges_in.clone(),
    }))?;

    // Exact handles are aliases only at the wire boundary. Source IDs and group
    // paths occur once in the dictionary, never in node/group rows.
    let mut dictionary = BTreeMap::<String, J>::new();
    let mut node_ref = BTreeMap::<String, String>::new();
    for (i, id) in node_by_id.keys().enumerate() {
        let reference = format!("n{i}");
        node_ref.insert(id.clone(), reference.clone());
        dictionary.insert(reference, json!({"kind":"node","original":id}));
    }
    let mut group_ref = BTreeMap::<String, String>::new();
    for (i, handle) in group_by_id.keys().enumerate() {
        let reference = format!("g{i}");
        group_ref.insert(handle.clone(), reference.clone());
        dictionary.insert(reference, json!({"kind":"group","original":handle}));
    }

    let mut node_rows = Vec::with_capacity(node_by_id.len());
    for (id, node) in &node_by_id {
        let mut attributes = node
            .as_object()
            .cloned()
            .ok_or_else(|| Error("invalid compact view node row".into()))?;
        attributes.remove("source_id");
        let body = attributes
            .remove("body")
            .ok_or_else(|| Error("missing compact view node body".into()))?;
        if let Some(original) = attributes.remove("original_node") {
            let mut metadata = original.clone();
            if let Some(object) = metadata.as_object_mut() {
                // Remove copied bodies only when the ordinary JSON value converts
                // to precisely the same typed body. Missing and null stay distinct.
                if let Some(source_body) = object.get("body")
                    && V::from_json(source_body)
                        .and_then(|value| value.to_tagged())
                        .ok()
                        .as_ref()
                        == Some(&body)
                {
                    object.remove("body");
                }
                if object.get("states").is_some()
                    && node.get("uncertainty").is_some()
                    && object.get("states") == node.get("uncertainty")
                {
                    object.remove("states");
                }
            }
            attributes.insert("record_metadata".into(), metadata);
        }
        node_rows.push(json!([node_ref[id], body, J::Object(attributes)]));
    }

    let mut group_rows = Vec::with_capacity(group_by_id.len());
    for (handle, (group, ids, _member_hash)) in &group_by_id {
        let label = group.get("label").cloned().unwrap_or(J::Null);
        let description_value = group.get("description").unwrap_or(&J::Null);
        let derived = description_value
            .get("description")
            .unwrap_or(description_value);
        let text = derived.get("text").and_then(J::as_str).unwrap_or("");
        const DESCRIPTION_LIMIT: usize = 144;
        let text_chars: Vec<char> = text.chars().collect();
        let truncated = text_chars.len() > DESCRIPTION_LIMIT;
        let text: String = text_chars.iter().take(DESCRIPTION_LIMIT).collect();
        let generator = derived.get("generator").cloned().unwrap_or(J::Null);
        let basis_hash = derived.get("basis_sha256").cloned().unwrap_or(J::Null);
        let excerpted = truncated || generator == "source-labels/v1";
        let description = json!([text, generator, basis_hash, excerpted]);
        group_rows.push(json!([group_ref[handle], ids.len(), label, description]));
    }

    let mut external_keys = BTreeSet::<String>::new();
    let mut projected = Vec::<(String, J, String, J)>::with_capacity(edges_in.len());
    for edge in &edges_in {
        let mut endpoint_ref = |name: &str| -> crate::Result<(String, Option<String>)> {
            match edge.get(name) {
                Some(J::String(id)) => {
                    if let Some(owner) = owner_by_id.get(id) {
                        let reference = if owner.starts_with('\0') {
                            node_ref[id].clone()
                        } else {
                            group_ref[owner].clone()
                        };
                        Ok((reference, None))
                    } else {
                        let key = serde_json::to_string(&json!([true, J::String(id.clone())]))?;
                        external_keys.insert(key.clone());
                        Ok((String::new(), Some(key)))
                    }
                }
                Some(value) => {
                    let key = serde_json::to_string(&json!([true, value]))?;
                    external_keys.insert(key.clone());
                    Ok((String::new(), Some(key)))
                }
                None => {
                    let key = "[false]".to_owned();
                    external_keys.insert(key.clone());
                    Ok((String::new(), Some(key)))
                }
            }
        };
        let (from_ref, from_external) = endpoint_ref("from")?;
        let (to_ref, to_external) = endpoint_ref("to")?;
        projected.push((
            from_ref,
            edge.get("rel")
                .cloned()
                .unwrap_or_else(|| json!({"missing":true})),
            to_ref,
            json!({"edge":edge,"from_external":from_external,"to_external":to_external}),
        ));
    }
    let mut external_ref = BTreeMap::<String, String>::new();
    for (i, key) in external_keys.iter().enumerate() {
        let reference = format!("x{i}");
        let decoded: J = serde_json::from_str(key)?;
        let (present, original) = if decoded
            .as_array()
            .is_some_and(|parts| parts.len() == 1 && parts[0] == false)
        {
            (false, J::Null)
        } else {
            let parts = decoded
                .as_array()
                .ok_or_else(|| Error("invalid external endpoint key".into()))?;
            (true, parts.get(1).cloned().unwrap_or(J::Null))
        };
        dictionary.insert(
            reference.clone(),
            json!({"kind":"external","original_present":present,"original":original}),
        );
        external_ref.insert(key.clone(), reference);
    }

    let mut aggregates = BTreeMap::<(String, String, String, String), Vec<J>>::new();
    for (mut from_ref, rel, mut to_ref, detail) in projected {
        let from_external = detail["from_external"].as_str();
        let to_external = detail["to_external"].as_str();
        if let Some(key) = from_external {
            from_ref = external_ref[key].clone();
        }
        if let Some(key) = to_external {
            to_ref = external_ref[key].clone();
        }
        let edge = detail["edge"].clone();
        let rel_key = format!(
            "{}:{}",
            edge.get("rel").is_some(),
            serde_json::to_string(&rel)?
        );
        aggregates
            .entry((from_ref, rel_key, to_ref, serde_json::to_string(&rel)?))
            .or_default()
            .push(edge);
    }

    let mut links = Vec::new();
    let mut handle_edges = BTreeMap::<String, Vec<J>>::new();
    for (edge_set_index, ((from_ref, _rel_key, to_ref, rel_json), exact_edges)) in
        aggregates.into_iter().enumerate()
    {
        let rel: J = serde_json::from_str(&rel_json)?;
        let alias = format!("e{edge_set_index}");
        if handle_edges
            .insert(alias.clone(), exact_edges.clone())
            .is_some()
        {
            return Err(Error("compact view edge-set alias collision".into()));
        }
        links.push(json!([from_ref, rel, to_ref, exact_edges.len(), alias]));
    }

    let handle_prefix = format!("edgeset:{view_id}:");
    let mut requested = BTreeMap::<String, String>::new();
    for handle in requested_edge_sets {
        let alias = handle
            .strip_prefix(&handle_prefix)
            .ok_or_else(|| Error(format!("unknown or stale edge_set handle: {handle}")))?;
        if !handle_edges.contains_key(alias) {
            return Err(Error(format!("unknown or stale edge_set handle: {handle}")));
        }
        requested.insert(alias.to_owned(), handle.clone());
    }
    let expanded_edges: Vec<J> = requested
        .iter()
        .map(|(alias, handle)| json!([handle, handle_edges[alias]]))
        .collect();

    // Navigation remains an independent overlapping facet. Folded views carry
    // only additional route counts plus dictionary paths; directly visible rows
    // carry their route refs. Expanding the whole packet recovers every per-ID route.
    let mut facet_members = BTreeMap::<String, BTreeSet<String>>::new();
    let mut node_facet_paths = BTreeMap::<String, BTreeSet<String>>::new();
    let hidden_paths = packet["navigation_hidden_paths"].as_array().into_iter().flatten()
        .filter_map(J::as_str).collect::<BTreeSet<_>>();
    let visible_paths = node_ref.keys().flat_map(|id| packet["navigation_membership"][id]
        .as_array().into_iter().flatten().filter_map(J::as_str)).collect::<BTreeSet<_>>();
    let mut revealed_paths = BTreeSet::new();
    for expanded in packet["navigation_frontier"]["expanded_children"].as_array().into_iter().flatten() {
        if let Some(path)=expanded["group"].as_str().and_then(|s|s.strip_prefix("group:")) { revealed_paths.insert(path); }
        for child in expanded["children"].as_array().into_iter().flatten() {
            if let Some(path)=child["group"].as_str().and_then(|s|s.strip_prefix("group:")) { revealed_paths.insert(path); }
        }
    }
    if let Some(nav) = packet["navigation_membership"].as_object() {
        for (id, paths) in nav {
            let path_values = paths
                .as_array()
                .ok_or_else(|| Error("invalid compact navigation membership".into()))?;
            for path in path_values {
                let path = path
                    .as_str()
                    .ok_or_else(|| Error("invalid compact navigation path".into()))?;
                if hidden_paths.contains(path) && !visible_paths.contains(path) && !revealed_paths.contains(path) {
                    continue;
                }
                let handle = if path.starts_with("group:") {
                    path.to_owned()
                } else {
                    format!("group:{path}")
                };
                facet_members
                    .entry(handle.clone())
                    .or_default()
                    .insert(id.clone());
                if node_ref.contains_key(id) {
                    node_facet_paths
                        .entry(id.clone())
                        .or_default()
                        .insert(handle);
                }
            }
        }
    }
    for (i, handle) in facet_members.keys().enumerate() {
        if !group_ref.contains_key(handle) {
            let reference = format!("f{i}");
            group_ref.insert(handle.clone(), reference.clone());
            dictionary.insert(reference, json!({"kind":"group","original":handle}));
        }
    }
    let mut visible_node_facets = Vec::new();
    for (id, handles) in node_facet_paths {
        let refs = handles
            .iter()
            .map(|handle| group_ref[handle].clone())
            .collect::<Vec<_>>();
        visible_node_facets.push(json!([node_ref[&id], refs]));
    }
    let mut navigation_groups = Vec::new();
    for (handle, members) in facet_members {
        if group_by_id.contains_key(&handle) {
            continue;
        }
        let reference = group_ref[&handle].clone();
        navigation_groups.push(json!([reference, members.len()]));
    }
    navigation_groups.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
    visible_node_facets.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));

    let direct_count = node_by_id.len();
    let folded_count = group_by_id
        .values()
        .map(|(_, ids, _)| ids.len())
        .sum::<usize>();
    let ids_hash = digest_json(&json!(all_ids))?;
    let mut coverage = json!({"count":all_ids.len(),"source_ids_sha256":ids_hash,"direct_count":direct_count,"folded_count":folded_count});
    let receipt = digest_json(&json!({
        "schema":"kpopper.canonical-coverage/v1",
        "revision":revision,
        "scope":scope,
        "view_id":view_id,
        "count":all_ids.len(),
        "source_ids_sha256":ids_hash,
        "direct_count":direct_count,
        "folded_count":folded_count,
    }))?;
    coverage["receipt"] = json!(receipt);
    if packet["coverage"]["count"].as_u64() != Some(all_ids.len() as u64) {
        return Err(Error("compact coverage disagrees with v1 packet".into()));
    }
    if let Some(expected) = packet["coverage"]["source_ids"].as_array() {
        let expected: BTreeSet<_> = expected.iter().filter_map(J::as_str).collect();
        if expected.len() != all_ids.len()
            || !all_ids.iter().all(|id| expected.contains(id.as_str()))
        {
            return Err(Error(
                "compact coverage identities disagree with v1 packet".into(),
            ));
        }
    }

    let mut out = json!({
        "schema":"kpopper.canonical-graph-view/v2",
        "project":project,
        "project_identity":project_identity,
        "revision":revision,
        "scope":scope,
        "mode":packet["mode"],
        "fields":{
            "node":["ref","body","attributes"],
            "group":["ref","count","label","description"],
            "description":{"kind":"navigation_only","row":["text","generator","basis_sha256","excerpted"]},
            "link":["from_ref","rel","to_ref","count","edge_set_ref"],
            "expanded_edge":["edge_set","original_edges"],
            "navigation_group":["ref","count"],
            "navigation_node":["node_ref","group_refs"]
        },
        "dictionary":dictionary,
        "nodes":node_rows,
        "groups":group_rows,
        "links":links,
        "expanded_edges":expanded_edges,
        "view_id":view_id,
        "edge_set_handle_template":format!("edgeset:{view_id}:{{edge_set_ref}}"),
        "group_expansion_route":"dictionary[group_ref].original (group:/path), with this revision and current focus",
        "coverage":coverage,
        "navigation_facets":{"groups":navigation_groups,"nodes":visible_node_facets},
    });

    // Keep packet-level fields whose meaning is not carried by a row. The v1
    // structural payload and redundant full membership/coverage lists are omitted.
    if let Some(object) = packet.as_object() {
        for (key, value) in object {
            if ![
                "schema",
                "project",
                "project_identity",
                "revision",
                "scope",
                "mode",
                "nodes",
                "groups",
                "links",
                "coverage",
                "navigation_membership",
                "navigation_hidden_paths",
            ]
            .contains(&key.as_str())
                && !out.as_object().unwrap().contains_key(key)
            {
                out[key] = value.clone();
            }
        }
    }
    crate::view_attributes::compact(&mut out)?;
    out["schema"] = json!(COMPACT_SCHEMA);
    Ok(out)
}
