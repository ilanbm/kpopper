//! Finite overlay façade; ordinary observations retain their full value domain.
use crate::{Result, value::TypedValue as V};
/// What comparing the record's holders found for one kind of reader: the ids held in more
/// than one version, and why the configured target could not be compared, when it could not.
#[derive(Clone)]
pub(crate) struct Comparison {
    pub conflicts: V,
    pub unavailable: Option<String>,
}
#[derive(Clone)]
pub(crate) struct Overlay<T = V> {
    pub pending: V,
    pub publication: V,
    pub contributions: Vec<V>,
    /// The comparison a core/v1 consumer reads, which includes a core/v1 target and a
    /// target kept by its history.
    pub core: Comparison,
    /// The comparison an ordinary reader reads. A target only a core/v1 consumer may read
    /// stays unverified here, and its entries contest nothing.
    pub ordinary: Comparison,
    pub target: Option<V>,
    pub target_snapshot: Option<T>,
    pub history_contributions: V,
}
impl Overlay<crate::ordinary_value::Value> {
    pub(crate) fn try_finite(self) -> Result<Overlay> {
        Ok(Overlay {
            pending: self.pending,
            publication: self.publication,
            contributions: self.contributions,
            core: self.core,
            ordinary: self.ordinary,
            target: self.target,
            target_snapshot: self
                .target_snapshot
                .map(|v| v.finite_projection())
                .transpose()?,
            history_contributions: self.history_contributions,
        })
    }
}
