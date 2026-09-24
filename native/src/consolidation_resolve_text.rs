//! A three-way merge of complete entries, preserving the chosen source blocks verbatim.
//! Metadata and entry bodies are atomic: no field-level synthesis of a new claim.
use crate::{
    Result,
    history_contract::{error, map},
    history_yaml as Y, require,
    value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Block {
    key: String,
    raw: String,
}
struct Parts {
    prefix: String,
    blocks: Vec<Block>,
}

fn split(raw: &str, expected: &BTreeMap<String, V>, root: bool) -> Result<Parts> {
    let pattern = if root {
        r"^([A-Za-z_][A-Za-z0-9_]*):(?:\s|$)"
    } else {
        r"^([ ]+)([A-Za-z_][A-Za-z0-9_.-]*):(?:\s|$)"
    };
    let re = regex::Regex::new(pattern).unwrap();
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();
    let mut offset = 0;
    let mut starts = Vec::new();
    let mut indent = None;
    for line in lines {
        if let Some(m) = re.captures(line) {
            if !root {
                let width = m[1].len();
                if indent.is_none() {
                    indent = Some(width);
                }
                if indent != Some(width) {
                    offset += line.len();
                    continue;
                }
            }
            starts.push((m[if root { 1 } else { 2 }].to_owned(), offset));
        }
        offset += line.len();
    }
    let keys: BTreeSet<_> = starts.iter().map(|(k, _)| k.clone()).collect();
    require(
        starts.len() == expected.len() && keys == expected.keys().cloned().collect(),
        "resolve_unsupported_layout: use block collections and plain entry IDs; record left untouched",
    )?;
    let prefix = raw[..starts.first().map_or(raw.len(), |(_, at)| *at)].to_owned();
    let blocks = starts
        .iter()
        .enumerate()
        .map(|(i, (key, start))| Block {
            key: key.clone(),
            raw: raw[*start..starts.get(i + 1).map_or(raw.len(), |(_, at)| *at)].into(),
        })
        .collect();
    Ok(Parts { prefix, blocks })
}

fn choose<'a>(
    base: Option<&'a str>,
    ours: Option<&'a str>,
    theirs: Option<&'a str>,
    label: &str,
) -> Result<Option<&'a str>> {
    if ours == theirs {
        Ok(ours)
    } else if ours == base {
        Ok(theirs)
    } else if theirs == base {
        Ok(ours)
    } else {
        Err(error(&format!(
            "resolve_contested: {label} changed on both sides; resolve this choice explicitly"
        )))
    }
}
fn block<'a>(p: &'a Parts, key: &str) -> Option<&'a str> {
    p.blocks
        .iter()
        .find(|b| b.key == key)
        .map(|b| b.raw.as_str())
}
fn order<'a>(ours: &'a Parts, theirs: &'a Parts) -> Vec<&'a str> {
    let mut seen = BTreeSet::new();
    ours.blocks
        .iter()
        .chain(&theirs.blocks)
        .filter_map(|b| seen.insert(b.key.as_str()).then_some(b.key.as_str()))
        .collect()
}
fn members(raw: Option<&str>, value: Option<&V>) -> Result<Parts> {
    match (raw, value) {
        (Some(raw), Some(V::Map(m))) => split(raw, m, false),
        (None, None) => Ok(Parts {
            prefix: String::new(),
            blocks: vec![],
        }),
        _ => Err(error(
            "resolve_unsupported_layout: changed collection is not a block mapping",
        )),
    }
}

pub(super) fn merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> Result<Vec<u8>> {
    let [b, o, t] =
        [base, ours, theirs].map(|v| std::str::from_utf8(v).map_err(|_| error("invalid_utf8")));
    let (b, o, t) = (b?, o?, t?);
    let docs = [
        Y::decode_document(base)?,
        Y::decode_document(ours)?,
        Y::decode_document(theirs)?,
    ];
    for doc in &docs {
        let m = map(doc)?;
        require(
            !m.contains_key("record") && !m.contains_key("also"),
            "resolve_pointer_record_unsupported: reconcile the complete pointer record explicitly",
        )?;
        require(
            !m.get("meta").is_some_and(|v| {
                map(v).is_ok_and(|m| {
                    ["history", "node_history", "node_publication"]
                        .iter()
                        .any(|k| m.contains_key(*k))
                })
            }),
            "resolve_history_required: generated records must be rebuilt through their supported history store",
        )?;
        crate::reasoning_snapshot::entries(doc)?;
    }
    let maps = [map(&docs[0])?, map(&docs[1])?, map(&docs[2])?];
    let parts = [
        split(b, maps[0], true)?,
        split(o, maps[1], true)?,
        split(t, maps[2], true)?,
    ];
    let mut out = choose(
        Some(&parts[0].prefix),
        Some(&parts[1].prefix),
        Some(&parts[2].prefix),
        "record preamble",
    )?
    .unwrap()
    .to_owned();
    let mut expected = BTreeMap::new();
    for key in order(&parts[1], &parts[2]) {
        let raws = [
            block(&parts[0], key),
            block(&parts[1], key),
            block(&parts[2], key),
        ];
        match choose(raws[0], raws[1], raws[2], key) {
            Ok(Some(raw)) => {
                let side = if Some(raw) == raws[1] { 1 } else { 2 };
                out.push_str(raw);
                expected.insert(key.into(), maps[side][key].clone());
            }
            Ok(None) => {}
            Err(e) => {
                require(!["meta", "schema", "hypothesis"].contains(&key), &e.0)?;
                // A whole-collection delete/edit is a choice, not an empty collection.
                require(raws[1].is_some() && raws[2].is_some(), &e.0)?;
                let bs = members(raws[0], maps[0].get(key))?;
                let os = members(raws[1], maps[1].get(key))?;
                let ts = members(raws[2], maps[2].get(key))?;
                let prefix = if raws[0].is_none() && os.prefix == ts.prefix {
                    &os.prefix
                } else {
                    choose(
                        Some(&bs.prefix),
                        Some(&os.prefix),
                        Some(&ts.prefix),
                        &format!("{key} header"),
                    )?
                    .unwrap()
                };
                out.push_str(prefix);
                let mut merged = BTreeMap::new();
                for id in order(&os, &ts) {
                    if let Some(raw) = choose(block(&bs, id), block(&os, id), block(&ts, id), id)? {
                        let side = if Some(raw) == block(&os, id) { 1 } else { 2 };
                        merged.insert(id.into(), map(&maps[side][key])?[id].clone());
                        out.push_str(raw);
                    }
                }
                // Empty block mappings read as null; keep a valid explicit mapping.
                if merged.is_empty() {
                    return Err(error(
                        "resolve_empty_collection: review the collection deletion explicitly",
                    ));
                }
                expected.insert(key.into(), V::Map(merged));
            }
        }
    }
    let merged = Y::decode_document(out.as_bytes())?;
    require(
        merged == V::Map(expected),
        "resolve_layout_mismatch: no files changed",
    )?;
    crate::reasoning_snapshot::entries(&merged)?;
    Ok(out.into_bytes())
}
