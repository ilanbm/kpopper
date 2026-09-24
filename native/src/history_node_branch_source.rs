//! Pinned local Git observations for node history. Commit association records what
//! the local reader observed; it is not a Merkle inclusion or source-truth proof.
use crate::{
    Result, history_contract::*, history_node_capture::Capture, history_node_publication as P,
    require, value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct Observation {
    pub(crate) bundle: P::Bundle,
    binding: V,
}
impl Observation {
    pub(crate) fn from_git(
        bundle: P::Bundle,
        commit: &str,
        entry: &str,
        algorithm: &str,
    ) -> Result<Self> {
        let copy = bundle.reconstruct()?;
        let capture = Capture::read(copy.path())?;
        let parents = capture
            .snapshot
            .transactions
            .values()
            .flat_map(|tx| tx.parents.iter())
            .collect::<BTreeSet<_>>();
        let frontier = capture
            .snapshot
            .transactions
            .iter()
            .filter(|(op, _)| !parents.contains(op))
            .map(|(op, tx)| (op.clone(), V::Text(tx.digest.clone())))
            .collect();
        require(
            crate::history_source_ancestry::oid(commit)
                && commit.len() == if algorithm == "sha1" { 40 } else { 64 }
                && ["sha1", "sha256"].contains(&algorithm),
            "invalid_branch_source",
        )?;
        crate::history_branch::portable_path(entry)?;
        let binding = crate::history_authoring::obj([
            ("kind", V::Text("node-git-observation/v1".into())),
            ("commit", V::Text(commit.into())),
            ("entry", V::Text(entry.into())),
            ("object_format", V::Text(algorithm.into())),
            ("association", V::Text("local_git_capture".into())),
            ("frontier", V::Map(frontier)),
            (
                "bundle_sha256",
                V::Text(crate::identity::sha256(&bundle.encode()?)),
            ),
        ]);
        Ok(Self { bundle, binding })
    }
    pub fn binding(&self) -> &V {
        &self.binding
    }
    pub fn revision(&self) -> Result<String> {
        self.binding.digest()
    }
    pub fn capture(&self) -> Result<Capture> {
        let copy = self.bundle.reconstruct()?;
        Capture::read(copy.path())
    }
}

pub(crate) fn ordered(observations: &[Observation]) -> Result<Vec<&Observation>> {
    require(
        !observations.is_empty() && observations.len() <= 16,
        "invalid_branch_source_set",
    )?;
    let mut found = BTreeMap::new();
    let mut commits = BTreeSet::new();
    for observation in observations {
        require(
            commits.insert(map(observation.binding())?["commit"].digest()?),
            "duplicate_branch_source",
        )?;
        require(
            found.insert(observation.revision()?, observation).is_none(),
            "duplicate_branch_source",
        )?;
    }
    Ok(found.into_values().collect())
}
