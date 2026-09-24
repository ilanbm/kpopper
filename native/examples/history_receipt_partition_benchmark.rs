//! Read-only exact receipt partitioning over a disposable legacy history copy.
use kpop_native::{history_node_receipt::Receipt, history_yaml as Y, value::TypedValue as V};
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
    for path in files {
        let manifest = Y::decode_document(&fs::read(&path).unwrap()).unwrap();
        let original = &map(&manifest)["receipt"];
        let receipt = Receipt::pack(original).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(receipt.restore().unwrap(), *original);
        receipts += 1;
        logical += original.canonical_bytes().unwrap().len();
        headers += receipt.header().canonical_bytes().unwrap().len();
        for side in [receipt.before(), receipt.after()] {
            contexts += side.context().canonical_bytes().unwrap().len();
            for (id, piece) in side.nodes() {
                pieces
                    .entry(id.clone())
                    .or_default()
                    .insert(piece.canonical_bytes().unwrap());
            }
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "receipts_exactly_reconstructed":receipts,"original_typed_receipt_bytes":logical,
            "all_side_context_bytes":contexts,"header_bytes":headers,
            "distinct_node_pieces":pieces.values().map(BTreeSet::len).sum::<usize>(),
            "distinct_node_piece_bytes":pieces.values().flat_map(|s|s.iter()).map(Vec::len).sum::<usize>(),
            "coverage":"exact typed decomposition only; no publication, raw YAML archive, filesystem or full-writer reduction claim"
        })
    );
}
