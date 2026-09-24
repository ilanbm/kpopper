//! Immutable legacy receipt definitions in a lossless copy archive. Source bytes
//! are hash-checked before a cached decode can be reused; no legacy writer runs.
use crate::{
    history_contract::*, history_node_archive::Archive, history_node_publication::Snapshot,
    history_node_semantics::History, history_view::list, history_yaml as Y, require,
    value::TypedValue as V, Result,
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
};
pub(crate) const FORMAT: &str = "node-legacy-receipt/v1";
pub(crate) const CHECKPOINT: &str = "node-migration-checkpoint/v1";
pub(crate) const PREFIX: &str = "evidence/legacy/";

#[derive(Debug)]
pub(crate) struct Legacy {
    pub captured: crate::history_capture::Capture,
    manifests: BTreeMap<String, V>,
    receipts: BTreeMap<String, V>,
    orders: BTreeMap<String, V>,
    pub decoded_bytes: usize,
}
thread_local! { static CACHE: RefCell<BTreeMap<String,Weak<Legacy>>> = RefCell::new(BTreeMap::new()); }
pub(crate) fn kind(context: &V, kind: &str) -> bool {
    map(context)
        .ok()
        .and_then(|c| c.get("format"))
        .is_some_and(|v| string_is(v, kind))
}
pub(crate) fn load(raw: &[u8]) -> Result<Arc<Legacy>> {
    let digest = crate::identity::sha256(raw);
    if let Some(cached) = CACHE.with(|cache| cache.borrow().get(&digest).and_then(Weak::upgrade)) {
        return Ok(cached);
    }
    let archive = Archive::decode(raw)?;
    let meta = map(archive.metadata())?;
    let entry = text(field(meta, "entry")?)?;
    let layout = crate::history_capture::Layout::for_entry(entry)?;
    let files = archive.files();
    let mut core = BTreeMap::new();
    for (original, normalized) in [
        (entry, "entry.yaml"),
        (layout.authority.as_str(), "authority.yaml"),
    ] {
        core.insert(
            normalized.into(),
            files
                .get(original)
                .ok_or_else(|| error("node_legacy_archive_missing"))?
                .clone(),
        );
    }
    for (original, normalized) in [
        (&layout.commits, "commits"),
        (&layout.objects, "objects"),
        (&layout.cancellations, "cancellations"),
    ] {
        let prefix = format!("{original}/");
        for (path, bytes) in files {
            if let Some(tail) = path.strip_prefix(&prefix) {
                core.insert(format!("{normalized}/{tail}"), bytes.clone());
            }
        }
    }
    let captured = crate::history_bundle::capture(&core, None)?;
    let mut manifests = BTreeMap::new();
    let mut receipts = BTreeMap::new();
    for raw in captured.commits.values().chain(
        captured
            .inactive_generations
            .values()
            .flat_map(|g| g.commits.values()),
    ) {
        let hash = crate::identity::sha256(raw);
        let manifest = Y::decode_document(raw)?;
        let receipt = crate::history_transaction::validate_receipt(&map(&manifest)?["receipt"])?;
        manifests.insert(hash.clone(), manifest);
        receipts.insert(hash, receipt);
    }
    let mut orders = BTreeMap::new();
    for ((_, id), bytes) in &captured.object_bytes {
        let source =
            crate::history_node_source_order::without_saw(Y::decode_source_document(bytes)?)?;
        orders.insert(
            id.clone(),
            crate::history_node_source_order::encode(&source),
        );
    }
    let legacy = Arc::new(Legacy {
        captured,
        manifests,
        receipts,
        orders,
        decoded_bytes: files.values().map(Vec::len).sum(),
    });
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|_, v| v.strong_count() > 0);
        cache.insert(digest, Arc::downgrade(&legacy));
    });
    Ok(legacy)
}
fn definition<'a>(snapshot: &'a Snapshot, operation: &str) -> Result<(&'a Legacy, &'a V, &'a str)> {
    let tx = snapshot
        .transactions
        .get(operation)
        .ok_or_else(|| error("missing_commit"))?;
    let context = tx
        .context
        .as_ref()
        .ok_or_else(|| error("node_transaction_missing_context"))?;
    require(kind(context, FORMAT), "node_legacy_receipt_format")?;
    let c = crate::history_node_writer::context(context)?;
    require(
        c["header"] == V::Null
            && c["before"] == V::Null
            && c["after"] == V::Null
            && tx.evidence.is_empty(),
        "node_legacy_receipt_context",
    )?;
    let options = schema(&c["options"], &["original_manifest_sha256"], &[])?;
    let hash = text(&options["original_manifest_sha256"])?;
    require(
        hash.len() == 64 && crate::history_paths::object_id(hash),
        "node_legacy_receipt_context",
    )?;
    let legacy = snapshot
        .legacy
        .values()
        .find(|legacy| legacy.manifests.contains_key(hash))
        .ok_or_else(|| error("node_legacy_archive_missing"))?;
    let manifest = &legacy.manifests[hash];
    let m = map(manifest)?;
    let original = legacy
        .captured
        .commits
        .get(operation)
        .ok_or_else(|| error("node_legacy_inactive_operation"))?;
    require(
        string_is(&m["operation"], operation) && crate::identity::sha256(original) == hash,
        "node_legacy_operation_mismatch",
    )?;
    let authority = map(&snapshot.authority)?;
    let old = map(&legacy.captured.marker)?;
    require(
        authority["record_id"] == old["record_id"] && authority["generation"] == old["generation"],
        "node_legacy_authority_mismatch",
    )?;
    require(
        tx.parents.iter().cloned().collect::<BTreeSet<_>>()
            == map(&m["parents"])?.keys().cloned().collect(),
        "node_legacy_parent_mismatch",
    )?;
    Ok((legacy, manifest, hash))
}
pub(crate) fn receipt(snapshot: &Snapshot, operation: &str) -> Result<V> {
    let (legacy, _, hash) = definition(snapshot, operation)?;
    Ok(legacy.receipts[hash].clone())
}
fn inventory_of(manifest: &V) -> Result<BTreeSet<String>> {
    list(&map(manifest)?["objects"])?
        .iter()
        .map(|reference| text(&map(reference)?["id"]).map(str::to_owned))
        .collect()
}
/// Complete original legacy manifest inventory, including inherited references.
pub(crate) fn inventory(snapshot: &Snapshot, operation: &str) -> Result<BTreeSet<String>> {
    let (_, manifest, _) = definition(snapshot, operation)?;
    inventory_of(manifest)
}
/// Original inventory entries newly visible at this transaction's causal boundary.
/// Legacy manifests can repeat exact ancestor objects without creating them again.
pub(crate) fn introduced(snapshot: &Snapshot, operation: &str) -> Result<BTreeSet<String>> {
    let (legacy, manifest, _) = definition(snapshot, operation)?;
    introduced_from(legacy, manifest)
}
fn introduced_from(legacy: &Legacy, manifest: &V) -> Result<BTreeSet<String>> {
    let mut introduced = inventory_of(manifest)?;
    let mut pending = map(&map(manifest)?["parents"])?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while let Some(parent) = pending.pop() {
        if !visited.insert(parent.clone()) {
            continue;
        }
        require(visited.len() <= MAX_OBJECTS, "history_limit")?;
        let raw = legacy
            .captured
            .commits
            .get(&parent)
            .ok_or_else(|| error("node_legacy_parent_mismatch"))?;
        let ancestor = Y::decode_document(raw)?;
        for id in inventory_of(&ancestor)? {
            introduced.remove(&id);
        }
        pending.extend(map(&map(&ancestor)?["parents"])?.keys().cloned());
    }
    Ok(introduced)
}
pub(crate) fn member(snapshot: &Snapshot, transaction: &str, semantic: &str) -> Result<bool> {
    let (legacy, manifest, _) = definition(snapshot, transaction)?;
    for id in introduced_from(legacy, manifest)? {
        if legacy
            .captured
            .objects
            .get(&id)
            .is_some_and(|o| map(o).is_ok_and(|m| string_is(&m["op"], semantic)))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(crate) fn validate(
    snapshot: &Snapshot,
    history: &History,
    events: &BTreeMap<String, String>,
) -> Result<()> {
    for (op, tx) in &snapshot.transactions {
        let Some(context) = &tx.context else { continue };
        let actual = events
            .iter()
            .filter(|(_, event)| snapshot.operations.get(*event) == Some(op))
            .map(|(id, _)| id.clone())
            .collect::<BTreeSet<_>>();
        if kind(context, CHECKPOINT) {
            require(actual.is_empty(), "node_legacy_checkpoint_claim")?;
        }
        if !kind(context, FORMAT) {
            continue;
        }
        let (legacy, manifest, _) = definition(snapshot, op)?;
        let expected = introduced_from(legacy, manifest)?;
        require(actual == expected, "node_legacy_object_membership")?;
        for id in inventory_of(manifest)? {
            require(
                legacy.captured.objects.get(&id) == Some(&history.object(&id)?),
                "node_legacy_object_mismatch",
            )?;
            require(
                history.source_orders().get(&id).unwrap_or(&V::Null) == &legacy.orders[&id],
                "node_legacy_source_order_mismatch",
            )?;
        }
    }
    Ok(())
}
pub(crate) fn archived_object(snapshot: &Snapshot, subject: &str, id: &str) -> Result<V> {
    for legacy in snapshot.legacy.values() {
        for generation in legacy.captured.inactive_generations.values() {
            if let Some(object) = generation.objects.get(id) {
                require(
                    string_is(&map(object)?["subject"], subject),
                    "reference_mismatch",
                )?;
                return Ok(object.clone());
            }
        }
    }
    Err(error("missing_object"))
}
