//! Read-only exact receipt partitioning over a disposable legacy history copy.
use kpop_native::{
    history_node_codec as C,
    history_node_receipt::{Nodes, Receipt},
    history_yaml as Y,
    value::TypedValue as V,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
fn map(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!("map") };
    m
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let base = Path::new(&args[1]);
    let mut receipts = 0;
    let mut contexts = 0;
    let mut headers = 0;
    let mut logical = 0;
    let mut pieces = BTreeMap::<String, BTreeSet<Vec<u8>>>::new();
    let mut files = fs::read_dir(base.join(".kpopper/history-commits"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect::<Vec<_>>();
    files.sort();
    let mut pending = BTreeMap::new();
    for path in files {
        let manifest = Y::decode_document(&fs::read(&path).unwrap()).unwrap();
        let V::Text(operation) = &map(&manifest)["operation"] else {
            panic!("operation")
        };
        pending.insert(operation.clone(), (path, manifest));
    }
    let mut worlds = BTreeMap::<String, (Nodes, BTreeMap<String, C::Version>)>::new();
    let mut node_patch_bytes = 0;
    let mut node_patch_events = 0;
    while !pending.is_empty() {
        let operation = pending
            .iter()
            .find(|(_, (_, m))| {
                map(&map(m)["parents"])
                    .keys()
                    .all(|p| worlds.contains_key(p))
            })
            .map(|(op, _)| op.clone())
            .expect("complete acyclic manifest closure");
        let (path, manifest) = pending.remove(&operation).unwrap();
        // The selected causal parent's retained state is an encoding base only.
        // Every receipt side is reconstructed exactly; sibling state is never inferred.
        let (mut retained, mut versions) = map(&map(&manifest)["parents"])
            .keys()
            .next()
            .map(|parent| worlds[parent].clone())
            .unwrap_or_default();
        let original = &map(&manifest)["receipt"];
        let receipt = Receipt::pack(original).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(receipt.restore().unwrap(), *original);
        receipts += 1;
        logical += original.canonical_bytes().unwrap().len();
        headers += receipt.header().canonical_bytes().unwrap().len();
        for (stage, side) in [("before", receipt.before()), ("after", receipt.after())] {
            contexts += side.context().canonical_bytes().unwrap().len();
            let changes = retained.prepare(side).unwrap();
            for change in &changes {
                let base = versions.get(&change.subject);
                let parents = base.map(|v| vec![v.id().into()]).unwrap_or_default();
                let event = C::Event::create(
                    &change.subject,
                    &format!("{operation}-{stage}"),
                    parents,
                    base,
                    change.after.clone(),
                )
                .unwrap();
                let raw = event.encode().unwrap();
                node_patch_bytes += raw.len();
                node_patch_events += 1;
                let decoded = C::Event::decode(&raw, &change.subject)
                    .unwrap()
                    .reconstruct(base)
                    .unwrap();
                assert_eq!(decoded.state(), change.after.as_ref());
                versions.insert(change.subject.clone(), decoded);
            }
            retained.apply(&changes).unwrap();
            let restored = retained.side(side.context()).unwrap();
            assert_eq!(restored.restore().unwrap(), side.restore().unwrap());
            for (id, piece) in side.nodes() {
                pieces
                    .entry(id.clone())
                    .or_default()
                    .insert(piece.canonical_bytes().unwrap());
            }
        }
        worlds.insert(operation, (retained, versions));
    }
    println!(
        "{}",
        serde_json::json!({
            "receipts_exactly_reconstructed":receipts,"original_typed_receipt_bytes":logical,
            "all_side_context_bytes":contexts,"header_bytes":headers,"node_component_patch_bytes":node_patch_bytes,"node_component_patch_events":node_patch_events,
            "distinct_node_pieces":pieces.values().map(BTreeSet::len).sum::<usize>(),
            "distinct_node_piece_bytes":pieces.values().flat_map(|s|s.iter()).map(Vec::len).sum::<usize>(),
            "coverage":"exact typed decomposition only; no publication, raw YAML archive, filesystem or full-writer reduction claim"
        })
    );
}
