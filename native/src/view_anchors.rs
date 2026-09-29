//! Optional question anchors over one captured full view. No read/ack state.
use super::{Selection, allocate, allocate_candidates, ranked_candidates, selection_plan};
use crate::{Error, Result};
use serde::Serialize;
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AnchorRank {
    pub id: String,
    /// Zero-based order before budget allocation; this is not a read receipt.
    pub rank: usize,
    pub unanchored_rank: Option<usize>,
    pub reason: String,
    /// The anchor that first reached this candidate, if any.
    pub anchor: Option<String>,
    pub via: Option<String>,
    pub relation: Option<String>,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AnchorTrace {
    pub policy: &'static str,
    pub anchors: Vec<String>,
    /// Full candidate ranking, including the unselected frontier. Callers must
    /// budget any rendered trace; a listed candidate is not evidence received.
    pub ranking: Vec<AnchorRank>,
}

#[derive(Clone, Debug)]
pub struct AnchoredSelection {
    pub selection: Selection,
    pub anchor_trace: AnchorTrace,
}

impl AnchoredSelection {
    /// Bounded model-facing explanation. `trace_rows` caps diagnostics only,
    /// never discovery/traversal. Final rendering must still fit the caller's
    /// exact token/byte budget, including this receipt and the view itself.
    /// Reads current `selection.ids` and directly visible nodes, so later budget
    /// pruning and bodies opened through group focus agree with the base receipt.
    pub fn receipt(&self, view: &J, query: &str, budget: usize, trace_rows: usize) -> J {
        let mut receipt = self.selection.receipt(view, query, budget);
        let trace = &self.anchor_trace;
        if trace.anchors.is_empty() {
            return receipt;
        }
        let mut selected = self
            .selection
            .ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        selected.extend(
            view["nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["source_id"].as_str()),
        );
        let anchors = trace
            .anchors
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let affected = trace
            .ranking
            .iter()
            .filter(|row| {
                (selected.contains(row.id.as_str()) || anchors.contains(row.id.as_str()))
                    && (row.anchor.is_some() || row.unanchored_rank != Some(row.rank))
            })
            .collect::<Vec<_>>();
        let shown = affected
            .iter()
            .take(trace_rows)
            .map(|row| {
                json!({
                    "id":row.id,"rank":row.rank,"unanchored_rank":row.unanchored_rank,
                    "reason":row.reason,"anchor":row.anchor,"via":row.via,
                    "relation":row.relation,"depth":row.depth,"selected":selected.contains(row.id.as_str()),
                })
            })
            .collect::<Vec<_>>();
        receipt["policy"] = json!(trace.policy);
        receipt["anchor_ranking"] = json!({
            "anchors":trace.anchors.iter().take(trace_rows).collect::<Vec<_>>(),
            "anchor_count":trace.anchors.len(),
            "omitted_anchor_ids":trace.anchors.len().saturating_sub(trace_rows),
            "ranking":shown,
            "affected_focus_or_anchor_count":affected.len(),
            "omitted_affected_rows":affected.len().saturating_sub(trace_rows),
            "diagnostic_candidate_count":trace.ranking.len(),
            "omitted_diagnostic_rows":trace.ranking.len().saturating_sub(shown.len()),
            "scope":"Question anchors affect optional ranking, not source truth or acknowledgment. Unselected candidates remain in the frontier."
        });
        receipt
    }
}

struct Step {
    id: String,
    reason: String,
    anchor: Option<String>,
    via: Option<String>,
    relation: Option<String>,
    depth: usize,
}

/// Allocate using exact original source IDs as optional question anchors.
///
/// Exact/explicit rereads and their evidence chains lead, followed by the top
/// global seed and its evidence chain. Then anchor and remaining global seeds
/// alternate. Traversal retains incoming qualifications across groups and has
/// no depth or ID cutoff. All globally discovered candidates remain eligible
/// and visible in the frontier, even when anchors reorder weak optional bodies.
/// Anchors are neither mandatory rereads, source truth, nor acknowledgment.
///
/// `full` and `hits` have the same contract as `allocate`: a full captured view
/// and global discovery over that view. IDs unavailable in that snapshot fail
/// explicitly; a snapshot alone cannot distinguish missing, retired and scoped
/// out nodes. Display aliases/handles are not resolved as anchor identities.
pub fn allocate_with_anchors(
    full: &J,
    hits: &[J],
    explicit: &[String],
    anchors: &[String],
    available: usize,
    count: impl Fn(&str) -> usize,
) -> Result<AnchoredSelection> {
    let mut trace = AnchorTrace {
        policy: "global-first-interleaved-anchors/v1",
        anchors: Vec::new(),
        ranking: Vec::new(),
    };
    if anchors.is_empty() {
        return Ok(AnchoredSelection {
            selection: allocate(full, hits, explicit, available, count)?,
            anchor_trace: trace,
        });
    }
    let plan = selection_plan(full, hits, explicit)?;
    let nodes = full["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["source_id"].as_str())
        .collect::<BTreeSet<_>>();
    let mut seen_anchors = BTreeSet::new();
    for id in anchors {
        if !nodes.contains(id.as_str()) {
            return Err(Error(format!(
                "anchor unavailable in captured revision/scope (missing, retired, or out of scope): {id}; use an original source ID"
            )));
        }
        if seen_anchors.insert(id) {
            trace.anchors.push(id.clone());
        }
    }
    let unanchored = ranked_candidates(&plan.seeds, &plan.supports)
        .into_iter()
        .enumerate()
        .map(|(rank, (id, _))| (id, rank))
        .collect::<BTreeMap<_, _>>();

    let mut ordered = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut visit =
        |id: &str, reason: &str, anchor: Option<&str>, ordered: &mut BTreeSet<String>| {
            let mut queue = VecDeque::from([Step {
                id: id.to_owned(),
                reason: reason.to_owned(),
                anchor: anchor.map(str::to_owned),
                via: None,
                relation: None,
                depth: 0,
            }]);
            while let Some(step) = queue.pop_front() {
                if !ordered.insert(step.id.clone()) {
                    continue;
                }
                trace.ranking.push(AnchorRank {
                    rank: candidates.len(),
                    unanchored_rank: unanchored.get(&step.id).copied(),
                    id: step.id.clone(),
                    reason: step.reason.clone(),
                    anchor: step.anchor.clone(),
                    via: step.via,
                    relation: step.relation,
                    depth: step.depth,
                });
                candidates.push((step.id.clone(), step.reason));
                for (next, relation) in plan.supports.get(&step.id).into_iter().flatten() {
                    queue.push_back(Step {
                        id: next.clone(),
                        reason: format!("relation:{relation}"),
                        anchor: step.anchor.clone(),
                        via: Some(step.id.clone()),
                        relation: Some(relation.clone()),
                        depth: step.depth + 1,
                    });
                }
            }
        };
    // Pick global seeds lazily: one reached by earlier evidence traversal must
    // not consume the global lead/alternation slot without new global discovery.
    // Mandatory still derives only from explicit requests and exact hits.
    for (id, reason) in plan
        .seeds
        .iter()
        .filter(|(id, _)| plan.mandatory.contains(id))
    {
        visit(id, reason, None, &mut ordered);
    }
    let mut global = plan
        .seeds
        .iter()
        .filter(|(id, _)| !plan.mandatory.contains(id));
    if let Some((id, reason)) = global.find(|(id, _)| !ordered.contains(id)) {
        visit(id, reason, None, &mut ordered);
    }
    for id in &trace.anchors {
        if ordered.contains(id) {
            continue;
        }
        visit(id, "anchor", Some(id), &mut ordered);
        if let Some((id, reason)) = global.find(|(id, _)| !ordered.contains(id)) {
            visit(id, reason, None, &mut ordered);
        }
    }
    for (id, reason) in global {
        visit(id, reason, None, &mut ordered);
    }
    Ok(AnchoredSelection {
        selection: allocate_candidates(
            full,
            hits.len(),
            plan.mandatory,
            candidates,
            available,
            &count,
        )?,
        anchor_trace: trace,
    })
}
