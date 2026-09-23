//! Declared-field identity candidates. This module never evaluates source truth
//! or writes a record; name overlap only ranks already justified candidates.
use crate::ordinary_reader as R;
use crate::public_ordinary_readers::short;
use crate::reasoning_authoring::named;
use crate::{
    Result,
    history_contract::{Map, map, text},
    history_view::truth,
    history_yaml::OrdinaryValue as O,
    ordinary_reader::Reader,
    reasoning_fields as F, reasoning_language as L,
    value::TypedValue as V,
};
#[cfg(test)]
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
static IDISH: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z0-9_]+)*$").unwrap());
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn get<'a>(value: &'a V, key: &str) -> &'a V {
    match value {
        V::Map(m) => m.get(key).unwrap_or(&V::Null),
        _ => &V::Null,
    }
}
fn py(value: &V) -> String {
    crate::source_text::ordinary_python_str(value)
}
fn norm(value: &V) -> String {
    py(value)
        .split(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn strings(value: &V) -> Vec<String> {
    match value {
        V::Text(s) => vec![s.clone()],
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok().map(str::to_owned))
            .collect(),
        _ => vec![],
    }
}
fn ids(value: &V) -> Vec<String> {
    let value = if truth(value) {
        py(value)
    } else {
        String::new()
    };
    value
        .split(|c: char| c == ',' || c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}
fn bodies(document: &V) -> Map {
    map(document)
        .into_iter()
        .flat_map(|m| m.iter())
        .filter(|(k, _)| !["meta", "schema", "record", "also"].contains(&k.as_str()))
        .filter_map(|(_, v)| map(v).ok())
        .flat_map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())))
        .collect()
}
fn ordered_ids(document: Option<&O>, raw: &Map) -> Vec<String> {
    let mut out = vec![];
    if let Some(O::Map(collections)) = document {
        for (name, value) in collections {
            if name
                .text()
                .is_some_and(|k| ["meta", "schema", "record", "also", "hypothesis"].contains(&k))
            {
                continue;
            }
            if let O::Map(m) = value {
                for (id, _) in m {
                    if let Some(id) = id.text()
                        && raw.contains_key(id)
                        && !out.iter().any(|v| v == id)
                    {
                        out.push(id.to_owned());
                    }
                }
            }
        }
    }
    for id in raw.keys() {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}
include!("ordinary_candidate_algorithm.rs");
impl Candidate {
    #[cfg(test)]
    pub(crate) fn json(&self) -> J {
        json!({"id":self.id,"rank":self.rank,"reasons":self.reasons,"score":self.score})
    }
}

#[derive(Default)]
pub(crate) struct Sources<'a> {
    pub base: Option<&'a O>,
    pub hypotheses: BTreeMap<String, &'a O>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Notice {
    pub refusals: Vec<String>,
    #[cfg(test)]
    pub candidates: Vec<Candidate>,
    pub text: String,
}
/// What the note reads of a record: its base document, the readable hypothesis
/// layers beside it, the dependency field, and every entry's body by id.
pub(crate) struct Inputs<'a> {
    pub document: &'a V,
    pub hypotheses: &'a Map,
    pub deps: &'a str,
    pub ids: &'a BTreeSet<String>,
    pub raw: &'a Map,
}
/// Retain source order when duplicate retirement claims need the reference's first holder.
pub(crate) fn nearest_existing_from_sources(
    reader: &Reader<'_>,
    action: &V,
    sources: &Sources<'_>,
) -> Result<Notice> {
    nearest_existing(
        &Inputs {
            document: reader.document(),
            hypotheses: &reader.hypotheses,
            deps: text(&reader.fields()["deps"])?,
            ids: &reader.ids,
            raw: reader.raw(),
        },
        action,
        sources,
    )
}
/// The same note over any record's inputs; `sources` orders retirement claims as above.
pub(crate) fn nearest_existing(
    record: &Inputs<'_>,
    action: &V,
    sources: &Sources<'_>,
) -> Result<Notice> {
    let id = text(get(action, "id"))?;
    let base = bodies(record.document);
    let mut raws = vec![(base.clone(), sources.base)];
    let mut live = base
        .keys()
        .filter(|id| !F::BUILTINS.contains(&id.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    for (name, hyp) in record.hypotheses {
        if truth(get(hyp, "error")) {
            continue;
        }
        let doc = if get(hyp, "doc") != &V::Null {
            get(hyp, "doc")
        } else {
            get(hyp, "document")
        };
        let raw = if let V::Map(raw) = get(hyp, "raw") {
            raw.clone()
        } else {
            bodies(doc)
        };
        live.extend(if let V::List(a) = get(hyp, "ids") {
            a.iter()
                .filter_map(|v| text(v).ok().map(str::to_owned))
                .collect::<Vec<_>>()
        } else {
            F::collections(doc)?
                .values()
                .flat_map(|m| m.keys().cloned())
                .collect()
        });
        raws.push((raw, sources.hypotheses.get(name).copied()));
    }
    let deps = record.deps;
    let mut retired = BTreeMap::new();
    if deps != "also" {
        for (raw, source) in &raws {
            for holder in ordered_ids(*source, raw) {
                for alias in strings(get(&raw[&holder], "also")) {
                    if IDISH.is_match(&alias) && !live.contains(&alias) && alias != holder {
                        retired.entry(alias).or_insert(holder.clone());
                    }
                }
            }
        }
    }
    if let Some(into) = retired.get(id)
        && !record.ids.contains(id)
    {
        return Ok(Notice {
            refusals: vec![format!(
                "{id} was retired into {into} - write {into} instead; it carries also: [{id}]"
            )],
            ..Default::default()
        });
    }
    if get(action, "kind") != &s("add") || !matches!(get(action, "body"), V::Map(_)) {
        return Ok(Notice::default());
    }
    let body = get(action, "body");
    let mut distinct = BTreeSet::new();
    for (raw, _) in &raws {
        for (id, body) in raw {
            if truth(get(body, "distinct_from")) {
                for other in ids(get(body, "distinct_from")) {
                    if other != *id {
                        distinct.insert(pair(id, &other));
                    }
                }
            }
        }
    }
    for other in ids(get(body, "distinct_from")) {
        distinct.insert(pair(id, &other));
    }
    let pool = record
        .raw
        .iter()
        .filter(|(key, body)| {
            key.as_str() != id && record.ids.contains(*key) && matches!(body, V::Map(_))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let candidates = near(
        id,
        body,
        &pool,
        deps,
        record.raw,
        &retired,
        &distinct,
        Some(3),
    );
    let mut text = String::new();
    if !candidates.is_empty() {
        text.push_str("nearest existing:\n");
        for candidate in &candidates {
            text.push_str(&format!(
                "  {}: {}\n",
                candidate.id,
                candidate.reasons.join("; ")
            ));
        }
        text.push_str(&format!("  one subject: same <id> {id} folds it in · two: distinct {id} <id> \"why\" keeps them apart\n"));
    }
    Ok(Notice {
        refusals: vec![],
        #[cfg(test)]
        candidates,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_notices_match_immutable_python_over_captured_worlds() {
        let cases: J =
            serde_json::from_str(include_str!("../tests/fixtures/ordinary-nearest.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let source = crate::history_yaml::decode_ordinary_source_value(
                case["document"].as_str().unwrap().as_bytes(),
            )
            .unwrap();
            let mut hyps = Map::new();
            let mut hyp_sources = BTreeMap::new();
            for (name, raw) in case["hypotheses"].as_object().unwrap() {
                let source = crate::history_yaml::decode_ordinary_source_value(
                    raw.as_str().unwrap().as_bytes(),
                )
                .unwrap();
                let document = source.projected();
                hyps.insert(
                    name.clone(),
                    V::Map(Map::from([
                        ("doc".into(), document.clone()),
                        ("document".into(), document),
                        ("error".into(), V::Null),
                    ])),
                );
                hyp_sources.insert(name.clone(), source);
            }
            let reader = Reader::new(&source.projected(), None)
                .unwrap()
                .with_layers(hyps, BTreeSet::new())
                .unwrap();
            let action = V::from_json(&case["action"]).unwrap();
            let sources = Sources {
                base: Some(&source),
                hypotheses: hyp_sources.iter().map(|(k, v)| (k.clone(), v)).collect(),
            };
            let result = nearest_existing_from_sources(&reader, &action, &sources).unwrap();
            let actual = json!({"refusals":result.refusals,"notice":result.text,"candidates":result.candidates.iter().map(Candidate::json).collect::<Vec<_>>()});
            assert_eq!(actual, case["expected"], "{}", case["name"]);
        }
    }
}
