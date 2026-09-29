//! Budget allocation over globally discovered candidates and declared evidence links.
//! Scores select reading order. They never establish absence or source truth.
use crate::{Error, Result};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[path = "view_anchors.rs"]
mod anchors;
pub use anchors::{AnchorRank, AnchorTrace, AnchoredSelection, allocate_with_anchors};

pub fn discover(full: &J, query: &str, count: impl Fn(&str) -> usize) -> Result<Vec<J>> {
    use crate::session_search::{SearchMode, SearchRequest};
    let rows = full["nodes"]
        .as_array()
        .ok_or_else(|| Error("selection needs captured nodes".into()))?;
    let nodes = rows
        .iter()
        .map(|node| {
            let id = node["source_id"]
                .as_str()
                .ok_or_else(|| Error("selection node lacks identity".into()))?;
            Ok((id.to_owned(), json!({"body":node["body"]})))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut hits = Vec::new();
    let mut cursor = None;
    loop {
        let response = crate::session_search::search_checked_session(
            full["project"].as_str().unwrap_or(""),
            full["revision"].as_str().unwrap_or(""),
            &nodes,
            &BTreeMap::new(),
            &SearchRequest {
                query: query.into(),
                ids: None,
                tokens: 65_536,
                limit: 32,
                branch: None,
                mode: SearchMode::Lexical,
                cursor: cursor.clone(),
            },
            &count,
            None,
        )
        .map_err(|error| Error(error.0))?;
        hits.extend(
            response.packet["hits"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        );
        let next = response.packet["next_cursor"].as_str().map(str::to_owned);
        if next.is_none() {
            break;
        }
        crate::require(next != cursor, "view search pagination made no progress")?;
        cursor = next;
    }
    Ok(hits)
}

#[derive(Clone, Debug)]
pub struct Selection {
    pub ids: Vec<String>,
    pub mandatory: BTreeSet<String>,
    pub candidates: Vec<(String, String)>,
    pub costs: BTreeMap<String, usize>,
    pub matched: usize,
}

/// Estimate the marginal v3 representation of one directly opened node. Shared
/// field schemas are already charged to the base view, so only this node row,
/// its exact identity entry, direct navigation routes and node-specific rules
/// are counted here. A one-node sample does not assume repeated assessments will
/// be pooled; the estimate is conservative and final packet sizing stays exact.
fn compact_node_cost(full: &J, id: &str, count: &impl Fn(&str) -> usize) -> Result<usize> {
    let node = full["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|node| node["source_id"].as_str() == Some(id))
        .ok_or_else(|| Error(format!("selection candidate is missing: {id}")))?;
    let routes = full["navigation_membership"]
        .get(id)
        .cloned()
        .unwrap_or_else(|| json!([]));
    let sample = json!({
        "schema":crate::canonical_view::SCHEMA,
        "project":full["project"],
        "project_identity":full["project_identity"],
        "revision":full["revision"],
        "scope":full["scope"],
        "mode":"focus",
        "nodes":[node],
        "groups":[],
        "links":[],
        "coverage":{"count":1,"source_ids":[id]},
        "navigation_membership":{(id):routes},
        "rules":full["rules"],
        "descriptions":full["descriptions"],
    });
    let compact = crate::canonical_view::compact(&sample, &[])?;

    let reference = compact["nodes"][0][0].as_str().unwrap();
    let node_row = compact["nodes"][0].clone();
    let dictionary_row = json!([reference, compact["dictionary"][reference]]);
    let navigation_row = compact["navigation_facets"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row[0].as_str() == Some(reference))
        .cloned()
        .unwrap_or(J::Null);
    let attribute_rules = compact["attribute_compaction"]["nodes"][reference].clone();
    let marginal = json!([node_row, dictionary_row, navigation_row, attribute_rules]);
    Ok(count(&serde_json::to_string(&marginal)?).saturating_add(16))
}

/// Keep global discovery separate from support traversal and view allocation.
/// `hits` retains the source reader's ranking, including literal/explicit IDs.
pub fn allocate(
    full: &J,
    hits: &[J],
    explicit: &[String],
    available: usize,
    count: impl Fn(&str) -> usize,
) -> Result<Selection> {
    let plan = selection_plan(full, hits, explicit)?;
    let candidates = ranked_candidates(&plan.seeds, &plan.supports);
    allocate_candidates(
        full,
        hits.len(),
        plan.mandatory,
        candidates,
        available,
        &count,
    )
}

type Supports = BTreeMap<String, Vec<(String, String)>>;

struct SelectionPlan {
    mandatory: BTreeSet<String>,
    seeds: Vec<(String, String)>,
    supports: Supports,
}

fn selection_plan(full: &J, hits: &[J], explicit: &[String]) -> Result<SelectionPlan> {
    let rows = full["nodes"]
        .as_array()
        .ok_or_else(|| Error("selection needs captured nodes".into()))?;
    let nodes = rows
        .iter()
        .map(|row| {
            let id = row["source_id"]
                .as_str()
                .ok_or_else(|| Error("selection node lacks identity".into()))?;
            Ok((id.to_owned(), row))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut mandatory = BTreeSet::new();
    for id in explicit {
        let original = crate::canonical_view::exact_node_id(&nodes, id)
            .ok_or_else(|| Error(format!("unknown focus node: {id}")))?;
        mandatory.insert(original);
    }
    let mut seeds = Vec::new();
    let mut seen = BTreeSet::new();
    for id in &mandatory {
        seen.insert(id.clone());
        seeds.push((id.clone(), "explicit".to_owned()));
    }
    for hit in hits {
        let Some(id) = hit["id"].as_str().filter(|id| nodes.contains_key(*id)) else {
            continue;
        };
        let exact = matches!(hit["match"].as_str(), Some("literal_id" | "explicit_id"));
        if exact {
            mandatory.insert(id.to_owned());
        }
        if seen.insert(id.to_owned()) {
            seeds.push((
                id.to_owned(),
                if exact { "exact_id" } else { "global_search" }.into(),
            ));
        }
    }
    let mut supports = Supports::new();
    for edge in full["links"].as_array().into_iter().flatten() {
        let source = &edge["source"];
        let (Some(from), Some(to), Some(rel)) = (
            source["from"].as_str(),
            source["to"].as_str(),
            source["rel"].as_str(),
        ) else {
            continue;
        };
        if nodes.contains_key(from) && nodes.contains_key(to) {
            supports
                .entry(from.into())
                .or_default()
                .push((to.into(), rel.into()));
            // Qualifications can point toward a claim. Surface these incident
            // relations without treating arbitrary reverse links as support.
            if [
                "contradicts",
                "refutes",
                "supersedes",
                "exception",
                "qualifies",
            ]
            .contains(&rel)
            {
                supports
                    .entry(to.into())
                    .or_default()
                    .push((from.into(), rel.into()));
            }
        }
    }
    for edges in supports.values_mut() {
        edges.sort();
        edges.dedup();
    }
    Ok(SelectionPlan {
        mandatory,
        seeds,
        supports,
    })
}

fn ranked_candidates(seeds: &[(String, String)], supports: &Supports) -> Vec<(String, String)> {
    let mut candidates = Vec::new();
    let mut ordered = BTreeSet::new();
    // A ranked seed is followed through its evidence chain before the next weak
    // lexical seed. No depth/ID count cutoff hides a required dependency.
    for (seed, reason) in seeds.iter().cloned() {
        let mut queue = VecDeque::from([(seed, reason)]);
        while let Some((id, why)) = queue.pop_front() {
            if !ordered.insert(id.clone()) {
                continue;
            }
            candidates.push((id.clone(), why));
            for (next, rel) in supports.get(&id).into_iter().flatten() {
                queue.push_back((next.clone(), format!("relation:{rel}")));
            }
        }
    }
    candidates
}

fn allocate_candidates(
    full: &J,
    matched: usize,
    mandatory: BTreeSet<String>,
    candidates: Vec<(String, String)>,
    available: usize,
    count: &impl Fn(&str) -> usize,
) -> Result<Selection> {
    let mut costs = BTreeMap::new();
    for (id, _) in &candidates {
        costs.insert(id.clone(), compact_node_cost(full, id, count)?);
    }
    let required: usize = mandatory
        .iter()
        .map(|id| costs.get(id).copied().unwrap_or(0))
        .sum();
    let mut used = required;
    let mut ids = mandatory.iter().cloned().collect::<Vec<_>>();
    for (id, _) in &candidates {
        if mandatory.contains(id) {
            continue;
        }
        let cost = costs[id];
        if used.saturating_add(cost) <= available {
            ids.push(id.clone());
            used += cost;
        }
    }
    Ok(Selection {
        ids,
        mandatory,
        candidates,
        costs,
        matched,
    })
}

impl Selection {
    pub fn receipt(&self, view: &J, query: &str, budget: usize) -> J {
        let mut selected = self.ids.iter().map(String::as_str).collect::<BTreeSet<_>>();
        selected.extend(
            view["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["source_id"].as_str()),
        );
        let mut owners = BTreeMap::new();
        for group in view["groups"].as_array().into_iter().flatten() {
            if let Some(handle) = group["group_id"].as_str() {
                for id in group["member_source_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(J::as_str)
                {
                    owners.insert(id, handle);
                }
            }
        }
        let mut frontier = BTreeMap::<String, BTreeMap<String, usize>>::new();
        let mut unread = 0;
        for (id, reason) in &self.candidates {
            if selected.contains(id.as_str()) {
                continue;
            }
            unread += 1;
            let handle = owners.get(id.as_str()).copied().unwrap_or("group:/");
            *frontier
                .entry(handle.into())
                .or_default()
                .entry(reason.clone())
                .or_default() += 1;
        }
        json!({"query":query,"policy":"ranked-evidence-budget/v2","working_budget_tokens":budget,
            "focus_ids":self.ids,"global_matches":self.matched,"candidate_count":self.candidates.len(),
            "unread_candidates":unread,"frontier":frontier,
            "scope":"Global lexical discovery plus declared evidence chains; weak scores and unmatched queries do not establish absence.",
            "next":"Read frontier group handles or continue global session search with alternate source-language terms. Repeat the current query and focus IDs when expanding an issued edge-set handle."})
    }
}
