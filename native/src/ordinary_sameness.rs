//! Declared-field identity candidates. This module never evaluates source truth
//! or writes a record; name overlap only ranks already justified candidates.
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
static WORDS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[\p{L}\p{N}]+").unwrap());
const STOP: &[&str] = &[
    "the", "and", "for", "its", "this", "that", "with", "from", "not", "are", "was", "were", "one",
    "what", "which", "when", "where", "how", "here", "into", "than", "then", "over", "under",
    "about", "after", "before", "does", "did", "has", "had",
];
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
                    if let Some(id) = id.text() {
                        if raw.contains_key(id) && !out.iter().any(|v| v == id) {
                            out.push(id.to_owned());
                        }
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
fn field(body: &V, key: &str, retired: Option<&BTreeMap<String, String>>) -> Option<String> {
    let V::Text(value) = get(body, key) else {
        return None;
    };
    if value.trim().is_empty() {
        return None;
    }
    let value = norm(&s(value));
    Some(
        retired
            .and_then(|m| m.get(&value))
            .cloned()
            .unwrap_or(value),
    )
}
fn rule(body: &V) -> Option<V> {
    match get(body, "rule") {
        v @ V::Map(_) => Some(v.clone()),
        V::Text(s) if !s.trim().is_empty() => Some(V::Text(s.clone())),
        _ => match get(body, "v") {
            V::Text(s)
                if crate::ordinary_reader::EXPR.is_match(s)
                    && crate::ordinary_reader::ID.is_match(s) =>
            {
                Some(V::Text(s.clone()))
            }
            _ => None,
        },
    }
}
fn canon_rule(value: &V, retired: &BTreeMap<String, String>) -> V {
    let input = if matches!(value, V::Map(_)) {
        value.clone()
    } else {
        V::Map(Map::from([("expr".into(), value.clone())]))
    };
    if let Ok(mut tree) = L::legacy_rule(&input) {
        fn rewrite(value: &mut V, retired: &BTreeMap<String, String>) {
            if let V::Map(m) = value {
                if let Some(V::Text(id)) = m.get_mut("ref") {
                    if let Some(new) = retired.get(id) {
                        *id = new.clone();
                    }
                }
                if let Some(V::List(a)) = m.get_mut("args") {
                    for child in a {
                        rewrite(child, retired);
                    }
                }
            }
        }
        rewrite(&mut tree, retired);
        tree
    } else {
        let rewritten = crate::ordinary_reader::ID
            .replace_all(&py(value), |capture: &regex::Captures| {
                retired
                    .get(&capture[0])
                    .cloned()
                    .unwrap_or_else(|| capture[0].into())
            })
            .to_string();
        s(&norm(&s(&rewritten)))
    }
}
fn premises(body: &V, deps: &str, raw: &Map) -> BTreeSet<String> {
    match get(body, deps) {
        V::List(a) => a
            .iter()
            .filter_map(|v| text(v).ok())
            .filter(|id| !raw.get(*id).is_some_and(|b| truth(get(b, "asked"))))
            .map(str::to_owned)
            .collect(),
        _ => BTreeSet::new(),
    }
}
fn tokens(value: &str) -> BTreeSet<String> {
    WORDS
        .find_iter(&value.to_lowercase())
        .map(|m| m.as_str().to_owned())
        .filter(|v| v.chars().count() > 2 && !STOP.contains(&v.as_str()))
        .collect()
}
fn overlap(left: &str, right: &str) -> f64 {
    let a = tokens(left);
    let b = tokens(right);
    if a.is_empty() || b.is_empty() {
        0.0
    } else {
        a.intersection(&b).count() as f64 / a.union(&b).count() as f64
    }
}
fn scalar_label(value: &str) -> String {
    static BARE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_.\-]*$").unwrap());
    if BARE.is_match(value)
        && crate::history_yaml::decode_ordinary_source_value(value.as_bytes())
            .is_ok_and(|v| v.projected() == s(value))
    {
        value.into()
    } else {
        format!(
            "\"{}\"",
            value
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        )
    }
}
fn verdict(body: &V) -> V {
    let value = match get(body, "verdict") {
        V::Null => get(body, "title"),
        v => v,
    };
    if truth(value) { value.clone() } else { s("") }
}
#[derive(Clone, Debug)]
pub(crate) struct Candidate {
    pub id: String,
    pub rank: u8,
    pub reasons: Vec<String>,
    pub score: f64,
}
impl Candidate {
    #[cfg(test)]
    pub(crate) fn json(&self) -> J {
        json!({"id":self.id,"rank":self.rank,"reasons":self.reasons,"score":self.score})
    }
}
pub(crate) fn near(
    subject: &str,
    body: &V,
    pool: &Map,
    deps: &str,
    raw: &Map,
    retired: &BTreeMap<String, String>,
    distinct: &BTreeSet<(String, String)>,
    limit: Option<usize>,
) -> Vec<Candidate> {
    let from = field(body, "from", Some(retired));
    let at = field(body, "at", None);
    let locations = ["url", "file"]
        .into_iter()
        .filter_map(|key| field(body, key, None).map(|v| (key, v)))
        .collect::<Vec<_>>();
    let rule = rule(body).map(|v| canon_rule(&v, retired));
    let prem = premises(body, deps, raw);
    let judgment = matches!(get(body, deps), V::List(_));
    let name = crate::reasoning_authoring::named(body);
    let mut out = vec![];
    for (id, other) in pool {
        if id == subject
            || !matches!(other, V::Map(_))
            || F::BUILTINS.contains(&id.as_str())
            || distinct.contains(&pair(subject, id))
        {
            continue;
        }
        let mut reasons = vec![];
        let mut rank = None;
        let other_from = field(other, "from", Some(retired));
        let other_at = field(other, "at", None);
        if from.is_some() && from == other_from {
            if at.is_some() && at == other_at {
                reasons.push(format!(
                    "same from and at ({}, {}) - certain",
                    from.as_ref().unwrap(),
                    scalar_label(at.as_ref().unwrap())
                ));
                rank = Some(0);
            } else {
                reasons.push(format!("same from ({})", from.as_ref().unwrap()));
                rank = Some(1);
            }
        }
        for (key, value) in &locations {
            if field(other, key, None).as_ref() == Some(value) {
                reasons.push(format!(
                    "same {key} ({}) - certain",
                    crate::public_ordinary_readers::short(&s(value), 60)
                ));
                rank = Some(0);
            }
        }
        if let (Some(rule), Some(other_rule)) = (&rule, self::rule(other)) {
            if canon_rule(&other_rule, retired) == *rule {
                reasons.push(format!(
                    "same rule ({})",
                    crate::public_ordinary_readers::short(&other_rule, 60)
                ));
                rank = Some(rank.unwrap_or(2).min(2));
            }
        }
        let other_prem = premises(other, deps, raw);
        if judgment
            && !prem.is_empty()
            && !other_prem.is_empty()
            && (prem.is_subset(&other_prem) || other_prem.is_subset(&prem))
        {
            let shared = prem
                .intersection(&other_prem)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            let same = crate::ordinary_reader::same_legacy(&verdict(body), &verdict(other));
            reasons.push(format!(
                "rests on {shared} too{}",
                if same {
                    ", with the same verdict - a duplicate"
                } else {
                    " - verdicts differ, a pair to judge"
                }
            ));
            rank = Some(rank.unwrap_or(3).min(3));
        }
        if !reasons.is_empty() {
            out.push(Candidate {
                id: id.clone(),
                rank: rank.unwrap(),
                reasons,
                score: overlap(&name, &crate::reasoning_authoring::named(other)),
            });
        }
    }
    out.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.id.cmp(&b.id))
    });
    if let Some(limit) = limit {
        out.truncate(limit);
    }
    out
}
fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.into(), b.into())
    } else {
        (b.into(), a.into())
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
/// Retain source order when duplicate retirement claims need the reference's first holder.
pub(crate) fn nearest_existing_from_sources(
    reader: &Reader<'_>,
    action: &V,
    sources: &Sources<'_>,
) -> Result<Notice> {
    let id = text(get(action, "id"))?;
    let base = bodies(reader.document());
    let mut raws = vec![(base.clone(), sources.base)];
    let mut live = base
        .keys()
        .filter(|id| !F::BUILTINS.contains(&id.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>();
    for (name, hyp) in &reader.hypotheses {
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
    let deps = text(&reader.fields()["deps"])?;
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
    if let Some(into) = retired.get(id) {
        if !reader.ids.contains(id) {
            return Ok(Notice {
                refusals: vec![format!(
                    "{id} was retired into {into} - write {into} instead; it carries also: [{id}]"
                )],
                ..Default::default()
            });
        }
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
    let pool = reader
        .raw()
        .iter()
        .filter(|(key, body)| {
            key.as_str() != id && reader.ids.contains(*key) && matches!(body, V::Map(_))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let candidates = near(
        id,
        body,
        &pool,
        deps,
        reader.raw(),
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
