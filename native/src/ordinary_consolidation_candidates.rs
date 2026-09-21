use super::{F, L, Map, R, V, get, named, py, s, short, text, truth};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};
fn norm(value: &V) -> String {
    py(value)
        .split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
include!("ordinary_candidate_algorithm.rs");
