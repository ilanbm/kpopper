//! Optimistic report preconditions. The writer verifies these against its locked
//! authoritative capture; the search index is never a source of write authority.
use crate::{
    Result,
    history_contract::{error, map, text},
    identity::sha256,
    reasoning_fields as fields, require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn links(body: &V, roles: &BTreeMap<String, V>, pending: &mut Vec<String>) -> Result<()> {
    let V::Map(m) = body else { return Ok(()) };
    if let Some(V::Text(source)) = m.get("from") {
        pending.push(source.clone());
    }
    if let Some(deps) = m.get(text(&roles["deps"])?) {
        let V::List(items) = deps else {
            return Err(error(
                "scoped report needs a declared dependency list; use record_sha256",
            ));
        };
        for item in items {
            pending.push(text(item)?.to_owned());
        }
    }
    if let Some(seen) = roles
        .get("snapshot")
        .and_then(|f| text(f).ok())
        .and_then(|f| m.get(f))
        .and_then(|v| map(v).ok())
    {
        pending.extend(seen.keys().cloned());
    }
    for key in [
        Some("rule"),
        roles.get("predicate").and_then(|v| text(v).ok()),
        Some("v"),
        Some("quoted"),
    ]
    .into_iter()
    .flatten()
    {
        match m.get(key) {
            Some(v @ V::Map(_)) if key != "v" && key != "quoted" => {
                let converted = crate::reasoning_language::legacy_expression(v, false)
                    .or_else(|_| crate::reasoning_language::legacy_expression(v, true));
                require(
                    converted.is_ok(),
                    "unsupported expression in scoped report; use record_sha256",
                )?;
                pending.extend(crate::reasoning_language::legacy_references(v));
            }
            Some(V::Text(s)) if crate::ordinary_semantics::EXPR.is_match(s) => {
                pending.extend(
                    crate::ordinary_semantics::ID
                        .find_iter(s)
                        .map(|m| m.as_str().to_owned()),
                );
                if let Some(parts) = crate::ordinary_semantics::CMP.captures(s) {
                    pending.push(parts[3].trim().to_owned());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn for_report(document: &V, report: &J) -> Result<J> {
    let doc = map(document)?;
    let roles = fields::snapshot_fields(document)?;
    let collections = fields::collections(document)?;
    let mut entries = BTreeMap::new();
    for (name, members) in &collections {
        for (id, body) in members {
            require(
                entries.insert(id.as_str(), (name, body)).is_none(),
                "ambiguous entry identity; use record_sha256",
            )?;
        }
    }
    let mut pending = Vec::new();
    if let Some(source) = report["source"].as_str() {
        pending.push(source.into());
    }
    if let Some(ids) = report.get("read_ids") {
        let ids = ids
            .as_array()
            .ok_or_else(|| error("read_ids must be an array of record IDs"))?;
        require(ids.len() <= 256, "too many report premises")?;
        for id in ids {
            pending.push(
                id.as_str()
                    .filter(|v| !v.is_empty() && v.len() <= 1024)
                    .ok_or_else(|| error("invalid report premise ID"))?
                    .to_owned(),
            );
        }
    }
    if let Some(ops) = report["updates"].as_array() {
        for op in ops {
            pending.push(
                op["id"]
                    .as_str()
                    .ok_or_else(|| error("invalid report target"))?
                    .into(),
            );
            if let Some(body) = op.get("body") {
                links(&V::from_json(body)?, &roles, &mut pending)?;
            }
        }
    } else if let Some(target) = report["target"].as_str() {
        pending.push(target.to_owned());
    }
    let mut bodies = BTreeMap::new();
    while let Some(id) = pending.pop() {
        if bodies.contains_key(&id) {
            continue;
        }
        require(
            !fields::BUILTINS.contains(&id.as_str()),
            "a graph-wide premise requires record_sha256",
        )?;
        require(
            bodies.len() < 4096,
            "too many transitive report premises; use record_sha256",
        )?;
        let value = if let Some((collection, body)) = entries.get(id.as_str()) {
            links(body, &roles, &mut pending)?;
            json!([collection, body.digest()?])
        } else {
            J::Null
        };
        bodies.insert(id, value);
    }
    let mut meta = doc.get("meta").cloned().unwrap_or(V::Null);
    let core = map(&meta)
        .ok()
        .and_then(|m| m.get("reasoning"))
        .and_then(|v| map(v).ok())
        .and_then(|m| m.get("profile"))
        .is_some_and(|v| text(v).ok() == Some("core/v1"));
    if !core && let V::Map(m) = &mut meta {
        m.remove("updated");
    }
    let runtime = crate::public_workspace::runtime_for_document(document)?;
    let implementation = runtime
        .as_ref()
        .map(|r| -> Result<J> {
            Ok(json!({
                "core":r.implementation,
                "ordinary":r.ordinary_program().map(|p|p.provenance()).transpose()?
            }))
        })
        .transpose()?;
    let shape = sha256(&serde_json::to_vec(&(
        meta.digest()?,
        doc.get("schema").map(V::digest).transpose()?,
        V::Map(roles).digest()?,
        implementation,
    ))?);
    Ok(json!({"schema":shape,"entries":bodies}))
}

pub(crate) fn record_key(record: &Path) -> Result<String> {
    Ok(sha256(
        crate::source_inventory::name(&crate::project_modes::resolved(record)?)?.as_bytes(),
    ))
}

pub(crate) fn verify(document: &V, raw: &J) -> Result<()> {
    let actual = for_report(document, raw)?;
    let expected = &raw["write_basis"];
    require(
        expected["schema"] == actual["schema"],
        "write conflict: schema, policy or computation runtime changed; reread the report premises",
    )?;
    if expected["entries"] != actual["entries"] {
        let before = expected["entries"]
            .as_object()
            .ok_or_else(|| error("invalid scoped report basis"))?;
        let after = actual["entries"].as_object().unwrap();
        let changed: BTreeSet<_> = before
            .keys()
            .chain(after.keys())
            .filter(|id| before.get(*id) != after.get(*id))
            .collect();
        return Err(error(&format!(
            "write conflict: {} changed; reread the listed report premises",
            changed
                .into_iter()
                .take(8)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}
