//! Finite overlay façade; ordinary observations retain their full value domain.
use crate::{Result, value::TypedValue as V};
#[derive(Clone)]
pub(crate) struct Overlay<T = V> {
    pub pending: V,
    pub publication: V,
    pub contributions: Vec<V>,
    pub conflicts: V,
    pub target: Option<V>,
    pub target_snapshot: Option<T>,
    pub unavailable: Option<String>,
    pub history_contributions: V,
}
impl Overlay<crate::ordinary_value::Value> {
    pub(crate) fn try_finite(self) -> Result<Overlay> {
        Ok(Overlay {
            pending: self.pending,
            publication: self.publication,
            contributions: self.contributions,
            conflicts: self.conflicts,
            target: self.target,
            target_snapshot: self
                .target_snapshot
                .map(|v| v.finite_projection())
                .transpose()?,
            unavailable: self.unavailable,
            history_contributions: self.history_contributions,
        })
    }
}
