//! Compact authoring context. Claim bodies live in node streams, not transaction receipts.
use crate::{
    Result, history_authority as A, history_contract::*, history_node_publication as P,
    history_transaction as T, identity::sha256, require, value::TypedValue as V,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const FORMAT: &str = "node-ledger-authoring/v1";
const MAX_CONTEXT_BYTES: usize = 16 * 1024 * 1024;
fn s(value: &str) -> V {
    V::Text(value.into())
}
fn patch(kind: &str, value: Option<V>) -> V {
    let mut out = Map::from([("kind".into(), s(kind))]);
    if let Some(value) = value {
        out.insert("value".into(), value);
    }
    V::Map(out)
}
fn delta(before: &V, after: &V) -> Result<V> {
    if let (V::Map(old), V::Map(new)) = (before, after) {
        let mut edits = Map::new();
        for key in old.keys().chain(new.keys()).collect::<BTreeSet<_>>() {
            match (old.get(key), new.get(key)) {
                (Some(a), Some(b)) if a == b => {}
                (_, None) => {
                    edits.insert(key.clone(), patch("delete", None));
                }
                (Some(a), Some(b)) => {
                    edits.insert(key.clone(), delta(a, b)?);
                }
                (None, Some(b)) => {
                    edits.insert(key.clone(), patch("replace", Some(b.clone())));
                }
            }
        }
        let mapped = patch("map", Some(V::Map(edits)));
        let replaced = patch("replace", Some(after.clone()));
        if mapped.canonical_bytes()?.len() < replaced.canonical_bytes()?.len() {
            return Ok(mapped);
        }
    }
    Ok(patch("replace", Some(after.clone())))
}
fn apply(before: Option<&V>, change: &V, depth: usize) -> Result<Option<V>> {
    require(depth <= crate::value::MAX_DEPTH, "node_template_depth")?;
    let p = schema(change, &["kind"], &["value"])?;
    match text(&p["kind"])? {
        "delete" => {
            require(
                before.is_some() && !p.contains_key("value"),
                "node_template_patch",
            )?;
            Ok(None)
        }
        "replace" => Ok(Some(field(p, "value")?.clone())),
        "map" => {
            let mut result = map(before.ok_or_else(|| error("node_template_base"))?)?.clone();
            for (key, edit) in map(field(p, "value")?)? {
                match apply(result.get(key), edit, depth + 1)? {
                    Some(value) => {
                        result.insert(key.clone(), value);
                    }
                    None => {
                        result.remove(key);
                    }
                }
            }
            Ok(Some(V::Map(result)))
        }
        _ => Err(error("node_template_patch")),
    }
}
fn validate_delta(change: &V, depth: usize) -> Result<()> {
    require(depth <= crate::value::MAX_DEPTH, "node_template_depth")?;
    let p = schema(change, &["kind"], &["value"])?;
    match text(&p["kind"])? {
        "delete" => require(!p.contains_key("value"), "node_template_patch"),
        "replace" => {
            field(p, "value")?.validate()?;
            Ok(())
        }
        "map" => {
            for edit in map(field(p, "value")?)?.values() {
                validate_delta(edit, depth + 1)?;
            }
            Ok(())
        }
        _ => Err(error("node_template_patch")),
    }
}
fn heads(snapshot: &P::Snapshot) -> Vec<String> {
    let parents = snapshot
        .transactions
        .values()
        .flat_map(|t| t.parents.iter().cloned())
        .collect::<BTreeSet<_>>();
    snapshot
        .transactions
        .keys()
        .filter(|op| !parents.contains(*op))
        .cloned()
        .collect()
}
fn template(document: &V) -> Result<V> {
    let mut value = A::document_template(document)?;
    if let Some(meta) = map_mut(&mut value)?.get_mut("meta") {
        let meta = map_mut(meta)?;
        meta.remove("node_history");
        meta.remove("node_publication");
    }
    Ok(value)
}
fn map_mut(value: &mut V) -> Result<&mut Map> {
    if let V::Map(m) = value {
        Ok(m)
    } else {
        Err(error("invalid_schema"))
    }
}

pub(crate) fn is_context(value: &V) -> bool {
    map(value)
        .ok()
        .and_then(|m| m.get("format"))
        .is_some_and(|v| string_is(v, FORMAT))
}
pub(crate) fn validate(value: &V) -> Result<&Map> {
    let c = schema(
        value,
        &[
            "format", "options", "action", "archive", "evidence", "template", "audits", "result",
        ],
        &[],
    )?;
    require(string_is(&c["format"], FORMAT), "node_transaction_format")?;
    map(&c["options"])?;
    map(&c["action"])?;
    map(&c["archive"])?;
    for (path, hash) in map(&c["evidence"])? {
        A::relative_path(path)?;
        require(
            text(hash).is_ok_and(|h| h.len() == 64 && crate::history_paths::object_id(h)),
            "node_evidence_hash",
        )?;
    }
    let t = schema(&c["template"], &["base", "delta"], &[])?;
    if t["base"] != V::Null {
        token(&t["base"])?;
    } else {
        require(
            string_is(field(map(&t["delta"])?, "kind")?, "replace"),
            "node_template_patch",
        )?;
    }
    validate_delta(&t["delta"], 0)?;
    crate::history_authoring_audit::ReplayAudit::validate_compact(&c["audits"])?;
    schema(&c["result"], &[], &["notes", "diagnostics"])?;
    require(
        value.canonical_bytes()?.len() <= MAX_CONTEXT_BYTES,
        "node_transaction_limit",
    )?;
    Ok(c)
}
pub(crate) fn audits(context: &V) -> Result<&V> {
    Ok(&validate(context)?["audits"])
}

pub(crate) fn create(
    snapshot: &P::Snapshot,
    action: &V,
    options: V,
    archive: &V,
    evidence: &BTreeMap<String, Vec<u8>>,
    document: &V,
    diagnostic_receipt: &V,
) -> Result<V> {
    T::validate_receipt(diagnostic_receipt)?;
    let diagnostic_after = map(field(map(diagnostic_receipt)?, "after")?)?;
    let mut result = Map::new();
    if let Some(notes) = diagnostic_after
        .get("authoring")
        .map(map)
        .transpose()?
        .and_then(|m| m.get("notes"))
    {
        result.insert("notes".into(), notes.clone());
    }
    if let Some(diagnostics) = diagnostic_after
        .get("batch")
        .map(map)
        .transpose()?
        .and_then(|m| m.get("diagnostics"))
    {
        result.insert("diagnostics".into(), diagnostics.clone());
    }
    let after = template(document)?;
    let heads = heads(snapshot);
    let parent = heads
        .first()
        .filter(|parent| heads.len() == 1 && snapshot.transactions[*parent].context.is_some());
    let (base, change) = if let Some(parent) = parent {
        let prior = after_template(snapshot, parent)?;
        (s(parent), delta(&prior, &after)?)
    } else {
        (V::Null, patch("replace", Some(after)))
    };
    let context = V::Map(Map::from([
        ("format".into(), s(FORMAT)),
        ("options".into(), options),
        ("action".into(), action.clone()),
        ("archive".into(), archive.clone()),
        (
            "evidence".into(),
            V::Map(
                evidence
                    .iter()
                    .map(|(p, raw)| (p.clone(), s(&sha256(raw))))
                    .collect(),
            ),
        ),
        (
            "template".into(),
            V::Map(Map::from([("base".into(), base), ("delta".into(), change)])),
        ),
        (
            "audits".into(),
            crate::history_authoring_audit::ReplayAudit::compact(diagnostic_receipt)?,
        ),
        ("result".into(), V::Map(result)),
    ]));
    validate(&context)?;
    Ok(context)
}
fn legacy_template(snapshot: &P::Snapshot, operation: &str) -> Result<V> {
    let receipt = crate::history_node_writer::receipt(snapshot, operation)?;
    template(field(map(field(map(&receipt)?, "after")?)?, "document")?)
}
fn reconstruct(snapshot: &P::Snapshot, operation: &str, cache: &BTreeMap<String, V>) -> Result<V> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = operation.to_owned();
    let mut base = loop {
        if let Some(value) = cache.get(&current) {
            break value.clone();
        }
        require(
            seen.insert(current.clone()) && seen.len() <= 100_000,
            "node_template_cycle",
        )?;
        let tx = snapshot
            .transactions
            .get(&current)
            .ok_or_else(|| error("node_transaction_missing_parent"))?;
        let context = tx
            .context
            .as_ref()
            .ok_or_else(|| error("node_transaction_missing_context"))?;
        if !is_context(context) {
            break legacy_template(snapshot, &current)?;
        }
        let c = validate(context)?;
        let t = map(&c["template"])?;
        chain.push(current.clone());
        if t["base"] == V::Null {
            break V::Map(Map::new());
        }
        let parent = text(&t["base"])?;
        require(
            tx.parents.iter().any(|p| p == parent),
            "node_template_parent",
        )?;
        current = parent.to_owned();
    };
    for operation in chain.iter().rev() {
        let context = snapshot.transactions[operation].context.as_ref().unwrap();
        let t = map(&validate(context)?["template"])?;
        base = apply(Some(&base), &t["delta"], 0)?.ok_or_else(|| error("node_template_patch"))?;
        require(template(&base)? == base, "node_template_document")?;
    }
    Ok(base)
}
pub(crate) fn after_template(snapshot: &P::Snapshot, operation: &str) -> Result<V> {
    reconstruct(snapshot, operation, &BTreeMap::new())
}
/// Reconstruct each committed template once, in causal order.
pub(crate) fn templates(snapshot: &P::Snapshot) -> Result<BTreeMap<String, V>> {
    let mut remaining = snapshot
        .transactions
        .iter()
        .map(|(op, tx)| (op.clone(), tx.parents.len()))
        .collect::<BTreeMap<_, _>>();
    let mut children = BTreeMap::<String, Vec<String>>::new();
    for (op, tx) in &snapshot.transactions {
        for parent in &tx.parents {
            require(
                snapshot.transactions.contains_key(parent),
                "node_transaction_missing_parent",
            )?;
            children.entry(parent.clone()).or_default().push(op.clone());
        }
    }
    let mut ready = remaining
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(op, _)| op.clone())
        .collect::<BTreeSet<_>>();
    let mut out = BTreeMap::new();
    let mut processed = 0;
    while let Some(op) = ready.pop_first() {
        processed += 1;
        if snapshot.transactions[&op]
            .context
            .as_ref()
            .is_some_and(is_context)
        {
            let value = reconstruct(snapshot, &op, &out)?;
            out.insert(op.clone(), value);
        }
        if let Some(children) = children.get(&op) {
            for child in children {
                let count = remaining.get_mut(child).unwrap();
                *count -= 1;
                if *count == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }
    require(
        processed == snapshot.transactions.len(),
        "node_template_cycle",
    )?;
    Ok(out)
}
/// Compatibility projection for callers that only need receipt-shaped intent.
pub(crate) fn derive_receipt(snapshot: &P::Snapshot, operation: &str) -> Result<V> {
    let tx = snapshot
        .transactions
        .get(operation)
        .ok_or_else(|| error("node_transaction_missing_parent"))?;
    let c = validate(
        tx.context
            .as_ref()
            .ok_or_else(|| error("node_transaction_missing_context"))?,
    )?;
    let after = after_template(snapshot, operation)?;
    let before = if let Some(parent) = tx
        .parents
        .first()
        .filter(|p| snapshot.transactions[*p].context.is_some())
    {
        after_template(snapshot, parent)?
    } else {
        V::Map(Map::new())
    };
    let capabilities = crate::reasoning_fields::capabilities(&after, None)?;
    let profile = text(field(map(&capabilities)?, "profile")?)?;
    let intent = V::Map(Map::from([
        ("action".into(), c["action"].clone()),
        ("archive".into(), c["archive"].clone()),
    ]));
    let mut ids = BTreeSet::new();
    for versions in snapshot.versions.values() {
        for version in versions.values().filter(|v| v.operation() == operation) {
            let Some(state) = version.state() else {
                continue;
            };
            if crate::history_node_ledger::is_ledger(state) {
                for (object, _) in crate::history_node_ledger::unpack(state)? {
                    require(
                        crate::history_node_writer::operation_member(
                            snapshot,
                            operation,
                            text(field(map(&object)?, "op")?)?,
                        )?,
                        "node_receipt_operation",
                    )?;
                    ids.insert(text(field(map(&object)?, "id")?)?.to_owned());
                }
            } else {
                let payload = crate::history_node_frame::decode(state)?;
                if payload.is_semantic {
                    let header = map(field(
                        map(field(map(&payload.semantic)?, "context")?)?,
                        "header",
                    )?)?;
                    ids.insert(text(field(header, "id")?)?.to_owned());
                }
            }
        }
    }
    let mut authoring = Map::from([(
        "objects".into(),
        V::List(ids.into_iter().map(V::Text).collect()),
    )]);
    let result = map(&c["result"])?;
    if let Some(notes) = result.get("notes") {
        authoring.insert("notes".into(), notes.clone());
    }
    let mut after_fields = Map::from([
        ("document".into(), after),
        ("authoring".into(), V::Map(authoring)),
    ]);
    if let Some(diagnostics) = result.get("diagnostics") {
        after_fields.insert(
            "batch".into(),
            V::Map(Map::from([("diagnostics".into(), diagnostics.clone())])),
        );
    }
    T::semantic_receipt(
        profile,
        &capabilities,
        &V::Map(Map::from([
            ("document".into(), before),
            ("authoring".into(), intent),
        ])),
        &V::Map(after_fields),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_patch_preserves_updated_and_absent_values() {
        let before = V::Map(Map::from([
            (
                "meta".into(),
                V::Map(Map::from([("updated".into(), s("old"))])),
            ),
            ("removed".into(), V::Null),
        ]));
        let after = V::Map(Map::from([
            (
                "meta".into(),
                V::Map(Map::from([("updated".into(), s("new"))])),
            ),
            ("added".into(), V::Null),
        ]));
        let change = delta(&before, &after).unwrap();
        validate_delta(&change, 0).unwrap();
        assert_eq!(apply(Some(&before), &change, 0).unwrap(), Some(after));
    }
}
