//! Finite source-document façade for canonical admission and existing writers.
//! Complete ordinary reads use ordinary_document; unsupported source atoms are
//! refused here before a canonical consumer can observe substituted data.
use crate::{
    Result, history_yaml::OrdinaryValue as S, source_inventory::Inventory, value::TypedValue as V,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
pub(crate) struct Document {
    pub source: S,
    pub hypotheses: V,
    pub origins: BTreeMap<String, BTreeMap<String, PathBuf>>,
    pub members: Vec<PathBuf>,
    pub history: Option<crate::history_capture::Capture>,
    pub history_projection: Option<V>,
    pub history_view: Option<V>,
    pub overlay: Option<crate::source_overlay::Overlay>,
}
pub(crate) fn members(entries: &[PathBuf], inventory: &mut Inventory) -> Result<Vec<PathBuf>> {
    crate::ordinary_document::members(entries, inventory)
}
pub(crate) fn load(
    paths: &[PathBuf],
    inventory: &mut Inventory,
    allow_missing: bool,
) -> Result<Document> {
    crate::ordinary_document::load(paths, inventory, allow_missing)?.try_finite()
}

pub(crate) fn merge(
    target: &mut S,
    incoming: &S,
    path: &Path,
    origins: &mut BTreeMap<String, BTreeMap<String, PathBuf>>,
) -> Result<()> {
    let mut target_full = crate::ordinary_source::Source::from_finite(target);
    let incoming_full = crate::ordinary_source::Source::from_finite(incoming);
    crate::ordinary_document::merge(&mut target_full, &incoming_full, path, origins)?;
    *target = target_full.try_finite()?;
    Ok(())
}
