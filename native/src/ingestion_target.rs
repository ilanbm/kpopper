//! Pure capture-time target binding for queued source reports.
use crate::{Result, history_contract::{error, map, text}, require, value::TypedValue as V};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

fn basic_type(value: &V) -> Option<&'static str> {
    match value {
        V::Bool(_) => Some("boolean"),
        V::Integer(_) => Some("integer"),
        V::Float(_) => Some("number"),
        V::Text(_) => Some("string"),
        V::Null => Some("null"),
        _ => None,
    }
}
fn body_hash(value: &V) -> Result<String> {
    let ordinary = crate::history_yaml::OrdinaryValue::from_typed(value);
    Ok(crate::identity::sha256(&crate::public_ordinary_readers::python_safe_dump_unicode(&ordinary)?))
}
fn entries(document: &V) -> Result<(BTreeMap<String, V>, BTreeMap<String, String>, String)> {
    let fields = crate::reasoning_fields::snapshot_fields(document)?;
    let deps = text(&fields["deps"])?.to_owned();
    let mut entries = BTreeMap::new();
    let mut homes = BTreeMap::new();
    for (collection, members) in crate::reasoning_fields::collections(document)? {
        for (id, body) in members {
            entries.insert(id.clone(), body);
            homes.insert(id, collection.clone());
        }
    }
    Ok((entries, homes, deps))
}
fn source_collection(entries: &BTreeMap<String, V>, homes: &BTreeMap<String, String>, deps: &str, source: &str) -> Result<String> {
    let body = entries.get(source).ok_or_else(|| error(&format!("{source} is not a recorded source; add the source before citing it")))?;
    let body = map(body).map_err(|_| error(&format!("{source} is not a recorded source; add the source before citing it")))?;
    let source_like = !["v", "quoted", "rule", deps].iter().any(|key| body.contains_key(*key))
        && ["asked", "file", "url", "of", "read"].iter().any(|key| body.get(*key).is_some_and(crate::history_view::truth));
    require(source_like, &format!("{source} is not a recorded source; add the source before citing it"))?;
    homes.get(source).cloned().ok_or_else(|| error("the recorded source must belong to one existing collection"))
}
fn target(entries: &BTreeMap<String, V>, homes: &BTreeMap<String, String>, deps: &str, id: &str, source_home: Option<&str>) -> Result<J> {
    let body = entries.get(id).ok_or_else(|| error(&format!("{id} is not an existing stored entry")))?;
    let body_map = map(body).map_err(|_| error(&format!("{id} is not an existing stored entry")))?;
    require(!body_map.contains_key(deps), &format!("{id} is a judgment and cannot be rewritten by ingestion"))?;
    require(!body_map.contains_key("rule"), &format!("{id} is worked out by a rule and cannot be rewritten"))?;
    let values = ["v", "quoted"].into_iter().filter_map(|field| body_map.get(field)).collect::<Vec<_>>();
    require(values.len() == 1, &format!("{id} does not have one stored scalar value"))?;
    let kind = basic_type(values[0]).ok_or_else(|| error(&format!("{id} is not a scalar reading")))?;
    let home = source_home.map(str::to_owned).or_else(|| body_map.get("from").and_then(|value| text(value).ok()).and_then(|source| homes.get(source)).cloned());
    Ok(json!({"id":id,"value":values[0].to_json()?,"type":kind,"body_sha256":body_hash(body)?,"source_collection":home,"source":body_map.get("from").map(V::to_json).transpose()?.unwrap_or(J::Null)}))
}

/// Bind the report to the exact target body and source collection seen at capture.
pub fn snapshot(document: &V, entry_bytes: &[u8], envelope: &J) -> Result<J> {
    let object = envelope.as_object().ok_or_else(|| error("capture envelope must be a JSON object"))?;
    let (entries, homes, deps) = entries(document)?;
    let explicit_source = object.get("source").and_then(J::as_str);
    let source_home = explicit_source.map(|source| source_collection(&entries, &homes, &deps, source)).transpose()?;
    let updates = object.get("updates").and_then(J::as_array);
    let additions = updates.is_some_and(|updates| updates.iter().any(|operation| operation["kind"] == "add"));
    if additions || explicit_source.is_some() {
        let expected = object.get("record_sha256").and_then(J::as_str).ok_or_else(|| error(if additions { "new entries require record_sha256 from the primary's prior open --json or search" } else { "an existing source citation requires record_sha256 from the primary's prior open --json or search" }))?;
        require(crate::identity::sha256(entry_bytes) == expected, "record changed since the primary read it; reread the premises before resubmitting")?;
    }
    let Some(updates) = updates else {
        return target(&entries, &homes, &deps, object.get("target").and_then(J::as_str).unwrap_or(""), source_home.as_deref());
    };
    let mut source_homes = BTreeSet::new();
    if let Some(home) = source_home { source_homes.insert(home); }
    let mut bodies = BTreeMap::new();
    for operation in updates {
        let id = operation["id"].as_str().unwrap_or("");
        if operation["kind"] == "set" {
            let old = target(&entries, &homes, &deps, id, source_homes.iter().next().map(String::as_str))?;
            let new_type = V::from_json(&operation["value"]).ok().and_then(|value| basic_type(&value));
            require(new_type == old["type"].as_str(), &format!("{id} has a different scalar type"))?;
            if let Some(home) = old["source_collection"].as_str() { source_homes.insert(home.into()); }
        } else {
            let body = V::from_json(&operation["body"])?;
            let body = map(&body)?;
            require(!body.contains_key("seen"), "judgment snapshots are computed by the writer")?;
            require(!["from", "at", "of", "src", "source"].iter().any(|key| body.contains_key(*key)), "batch citations use envelope.source and update.at, not citation fields inside body")?;
            let stored = ["v", "quoted"].iter().filter(|field| body.contains_key(**field)).count();
            let derived = usize::from(body.contains_key("rule"));
            let judgment = usize::from(body.contains_key(&deps));
            require(stored + derived + judgment == 1, "add one stored reading, rule, or judgment per entry")?;
            if let Some(home) = homes.get(id) { source_homes.insert(home.clone()); }
        }
        bodies.insert(id.into(), entries.get(id).cloned().unwrap_or(V::Null));
    }
    if source_homes.is_empty() {
        for (id, body) in &entries {
            if let Ok(body) = map(body)
                && !["v", "quoted", "rule", deps.as_str()].iter().any(|key| body.contains_key(*key))
                && ["asked", "file", "url", "read"].iter().any(|key| body.get(*key).is_some_and(crate::history_view::truth))
                && let Some(home) = homes.get(id)
            { source_homes.insert(home.clone()); }
        }
    }
    require(source_homes.len() == 1, "the batch needs one unambiguous existing source collection")?;
    Ok(json!({"body_sha256":body_hash(&V::Map(bodies))?,"source_collection":source_homes.into_iter().next().unwrap(),"type":"batch"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> (V, Vec<u8>) {
        let raw = b"sources:\n  s.old: {file: old.txt, read: 2026-09-01}\nknown:\n  p.a: {v: 1, from: s.old, at: line 1}\n  p.b: {v: two, from: s.old}\n".to_vec();
        (crate::history_yaml::decode_document(&raw).unwrap(), raw)
    }

    #[test]
    fn single_and_batch_snapshots_bind_exact_bodies() {
        let (document, raw) = document();
        let one = snapshot(&document, &raw, &json!({"source_quote":"one","target":"p.a","value":2,"date":"2026-09-02"})).unwrap();
        assert_eq!(one["id"], "p.a");
        assert_eq!(one["type"], "integer");
        assert_eq!(one["source_collection"], "sources");
        let batch = snapshot(&document, &raw, &json!({"source_quote":"two","date":"2026-09-02","record_sha256":crate::identity::sha256(&raw),"updates":[{"kind":"set","id":"p.a","value":2},{"kind":"add","id":"p.c","body":{"v":3}}]})).unwrap();
        assert_eq!(batch["type"], "batch");
        assert_eq!(batch["source_collection"], "sources");
    }

    #[test]
    fn capture_rejects_untrusted_source_role() {
        let (document, raw) = document();
        assert!(snapshot(&document, &raw, &json!({"source_quote":"bad","target":"p.a","value":2,"date":"2026-09-02","source":"p.b","record_sha256":crate::identity::sha256(&raw)})).unwrap_err().0.contains("not a recorded source"));
    }
}
