//! Structured observations from the same union that produced the preview report.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewDecision {
    pub hypothesis: String,
    pub allowed: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewJudgment {
    pub predicate_references: Vec<String>,
    pub page_references: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewPageBound {
    pub id: String,
    pub pages: Vec<String>,
    pub readings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreviewFacts {
    pub base_ids: BTreeSet<String>,
    pub contested: BTreeMap<String, Vec<String>>,
    pub refused: BTreeMap<String, PreviewDecision>,
    pub reversals: BTreeMap<String, PreviewDecision>,
    pub untaken: BTreeMap<String, PreviewDecision>,
    pub untakeable: BTreeSet<String>,
    pub drops_needed: BTreeMap<String, Vec<String>>,
    pub moved: Vec<String>,
    pub falsified: Vec<String>,
    pub holes: Vec<String>,
    pub head_falsified: BTreeMap<String, String>,
    /// Empty when contention stopped the union before there was a candidate.
    pub judgments: BTreeMap<String, PreviewJudgment>,
    /// The union's red state. Dropped dependencies also require a fold choice,
    /// while movement alone is informational; these remain separate facts.
    pub red: bool,
}

impl PreviewFacts {
    /// A page predicate that also reads a tree-differing id cannot be decided by
    /// page evidence from the record. Consumers must keep this a refused case.
    pub fn page_bound(&self, changed: &BTreeSet<String>) -> Vec<PreviewPageBound> {
        self.judgments
            .iter()
            .filter_map(|(id, judgment)| {
                let readings = judgment
                    .predicate_references
                    .iter()
                    .filter(|name| changed.contains(*name))
                    .cloned()
                    .collect::<Vec<_>>();
                (!judgment.page_references.is_empty() && !readings.is_empty()).then(|| {
                    PreviewPageBound {
                        id: id.clone(),
                        pages: judgment.page_references.clone(),
                        readings,
                    }
                })
            })
            .collect()
    }
}

/// Optional bytes from the caller's captured brief. Preview reads no files and
/// evaluates page evidence only against its base, never synthetic tree readings.
#[derive(Clone, Copy, Debug, Default)]
pub struct PreviewEvidence<'a> {
    pub brief: Option<&'a [u8]>,
}
