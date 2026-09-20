//! Finite authoring facade over the ordinary read domain.
use crate::ordinary_domain_counts as O;
use crate::{
    Result,
    ordinary_reader::{Reader, ordinary},
    value::TypedValue as V,
};
use std::collections::BTreeSet;
pub(crate) fn same_rule(a: &V, b: &V) -> bool {
    O::same_rule(&ordinary(a), &ordinary(b))
}
pub(crate) fn legacy_rule(old: &V, current: &V) -> Option<V> {
    O::legacy_rule(&ordinary(old), &ordinary(current))
        .map(|v| v.finite_projection().expect("finite rule"))
}
pub(crate) fn flags(reader: &Reader<'_>, body: &V) -> Result<BTreeSet<&'static str>> {
    O::flags(&reader.ordinary(), &ordinary(body))
}
