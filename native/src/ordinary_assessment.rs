//! Finite authoring facade over ordinary assessment findings.
pub use crate::ordinary_assessment_report::{
    assess, compact_json, from_capture, legacy_digest, legacy_json, load,
};
use crate::ordinary_findings as O;
use crate::{
    Result,
    ordinary_reader::{Reader, ordinary},
    value::TypedValue as V,
};
pub const PROFILE: &str = O::PROFILE;
pub const POLICY: &str = O::POLICY;
pub(crate) fn number(v: &V) -> Option<(num_bigint::BigInt, num_bigint::BigInt)> {
    O::number(&ordinary(v))
}
pub(crate) fn equal_value(a: &V, b: &V) -> bool {
    O::equal_value(&ordinary(a), &ordinary(b))
}
pub(crate) fn snapshot_value(reader: &Reader<'_>, dep: &str) -> Result<V> {
    O::snapshot_value(&reader.ordinary(), dep)?.finite_projection()
}
pub fn judgment_state(reader: &Reader<'_>, body: &V) -> Result<V> {
    O::judgment_state(&reader.ordinary(), &ordinary(body))?.finite_projection()
}
pub fn attention(state: &V, policy: &str) -> Result<V> {
    O::attention(&ordinary(state), policy)?.finite_projection()
}
pub fn attention_ordered(state: &V, policy: &str, order: Option<&[String]>) -> Result<V> {
    O::attention_ordered(&ordinary(state), policy, order)?.finite_projection()
}
pub fn selected_attention(
    report: &V,
    ids: Option<&[String]>,
    actions: Option<&[String]>,
) -> Result<V> {
    O::selected_attention(&ordinary(report), ids, actions)?.finite_projection()
}
