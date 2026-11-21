//! Dependency closure for ordinary search rows. The canonical reader still captures
//! the source and rechecks files. An unsupported world uses the full projection.
use crate::{
    identity::sha256,
    ordinary_fields as fields, ordinary_language as language,
    ordinary_semantics::{self, Reader},
    ordinary_value::{Map, Value, map, text},
    search_index::Index,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Changes {
    pub dirty: BTreeSet<String>,
    pub needs_counts: bool,
}

pub(crate) fn changes(index: &mut Index, reader: &Reader<'_>) -> Option<Changes> {
    let document = map(&reader.document).ok()?;
    let roles = &reader.fields;
    let dependency = text(roles.get("deps")?).ok()?;
    let snapshot = roles
        .get("snapshot")
        .and_then(|v| text(v).ok())
        .unwrap_or("");
    let implementation = reader.program().map(|p| p.provenance()).transpose().ok()?;
    let mut meta = document.get("meta").cloned().unwrap_or(Value::Null);
    if let Value::Map(m) = &mut meta {
        // Publication dates change the corpus revision, but do not enter
        // ordinary entry values, rule evaluation or judgment field roles.
        m.remove("updated");
    }
    let world = sha256(
        &serde_json::to_vec(&(
            meta.try_typed().ok()?.digest().ok()?,
            document
                .get("schema")
                .map(Value::try_typed)
                .transpose()
                .ok()?
                .map(|v| v.digest())
                .transpose()
                .ok()?,
            Value::Map(roles.clone()).python_repr(),
            implementation,
        ))
        .ok()?,
    );
    let mut inputs = BTreeMap::new();
    let mut dependents = BTreeMap::<String, Vec<String>>::new();
    let mut reads_counts = BTreeSet::new();
    for (section, members) in fields::collections(&reader.document).ok()? {
        for (id, body) in members {
            if fields::BUILTINS.contains(&id.as_str()) || inputs.contains_key(&id) {
                return None;
            }
            let stamp = sha256(
                &serde_json::to_vec(&(
                    section.as_str(),
                    body.try_typed().ok()?.digest().ok()?,
                    body.python_repr(),
                ))
                .ok()?,
            );
            inputs.insert(id.clone(), stamp);
            let mut reads = BTreeSet::new();
            if let Ok(m) = map(&body) {
                if let Some(Value::Text(source)) = m.get("from") {
                    reads.insert(source.clone());
                }
                if let Some(deps) = m.get(dependency) {
                    let Value::List(list) = deps else { return None };
                    for item in list {
                        reads.insert(text(item).ok()?.to_owned());
                    }
                    if let Some(seen) = m.get(snapshot).filter(|v| **v != Value::Null) {
                        reads.extend(map(seen).ok()?.keys().cloned());
                    }
                    let pred = ordinary_semantics::predicate_of(&body, roles);
                    let refs = ordinary_semantics::predicate_refs(&pred);
                    if let Value::Map(_) = &pred {
                        language::legacy_expression(&pred, true).ok()?;
                        // The full scan uses the judgment's declaration. An
                        // incomplete declaration must keep its exact diagnostic.
                        if !refs.iter().all(|r| list.contains(&Value::Text(r.clone()))) {
                            return None;
                        }
                    } else if let Value::Text(s) = &pred
                        && let Some(parts) = ordinary_semantics::CMP.captures(s)
                    {
                        reads.insert(parts[3].trim().to_owned());
                    }
                    reads.extend(refs);
                }
                for name in ["rule", "v", "quoted"] {
                    match m.get(name) {
                        Some(v @ Value::Map(_)) if name == "rule" => {
                            language::legacy_rule(v).ok()?;
                            reads.extend(language::legacy_references(v));
                        }
                        Some(Value::Text(s)) if ordinary_semantics::EXPR.is_match(s) => {
                            reads.extend(
                                ordinary_semantics::ID
                                    .find_iter(s)
                                    .map(|m| m.as_str().to_owned()),
                            );
                        }
                        _ => {}
                    }
                }
            }
            for r in reads {
                if fields::BUILTINS.contains(&r.as_str()) {
                    reads_counts.insert(id.clone());
                }
                dependents.entry(r).or_default().push(id.clone());
            }
        }
    }
    let mut dirty = BTreeSet::new();
    for (id, key) in &inputs {
        if index.previous_world != world || index.previous.get(id).is_none_or(|(old, _)| old != key)
        {
            dirty.insert(id.clone());
        }
    }
    dirty.extend(
        index
            .previous
            .keys()
            .filter(|id| !inputs.contains_key(*id))
            .cloned(),
    );
    if !dirty.is_empty() {
        dirty.extend(reads_counts.iter().cloned());
    }
    let mut pending: Vec<_> = dirty.iter().cloned().collect();
    while let Some(id) = pending.pop() {
        for parent in dependents.get(&id).into_iter().flatten() {
            if dirty.insert(parent.clone()) {
                pending.push(parent.clone());
            }
        }
    }
    let needs_counts = reads_counts.iter().any(|id| dirty.contains(id));
    index.world = world;
    index.inputs = inputs;
    Some(Changes {
        dirty,
        needs_counts,
    })
}

/// Build exactly the expression inputs reachable through a chain of rule refs.
/// The Lean evaluator remains responsible for calculation, cycles and missing refs.
pub(crate) fn calculation_inputs(
    raw: &Map,
    ids: &BTreeSet<String>,
    seeds: Vec<String>,
) -> (Map, BTreeSet<String>) {
    let mut selected = Map::new();
    let mut present = BTreeSet::new();
    let mut pending = seeds;
    while let Some(id) = pending.pop() {
        if !ids.contains(&id) || !present.insert(id.clone()) {
            continue;
        }
        if let Some(body) = raw.get(&id) {
            if let Ok(m) = map(body)
                && let Some(rule @ Value::Map(_)) = m.get("rule")
            {
                pending.extend(language::legacy_references(rule));
            }
            selected.insert(id, body.clone());
        }
    }
    (selected, present)
}
