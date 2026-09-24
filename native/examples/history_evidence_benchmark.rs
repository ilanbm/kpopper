//! Verify native node-evidence recipes against a read-only legacy receipt copy.
use kpop_native::{
    history_node_evidence::Evidence, history_transaction as T, history_yaml as Y,
    value::TypedValue as V,
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
fn map(v: &V) -> &BTreeMap<String, V> {
    let V::Map(m) = v else { panic!("map") };
    m
}
fn strings(v: &V) -> Option<Vec<String>> {
    let V::List(v) = v else { return None };
    v.iter()
        .map(|v| {
            if let V::Text(s) = v {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect()
}
fn declarations(v: &V, out: &mut BTreeSet<Vec<String>>) {
    match v {
        V::Map(m) => {
            for (key, value) in m {
                if ["rests_on", "depends_on", "deps"].contains(&key.as_str()) {
                    if let Some(mut ids) = strings(value) {
                        ids.sort();
                        ids.dedup();
                        out.insert(ids);
                    }
                }
                declarations(value, out);
            }
        }
        V::List(v) => {
            for value in v {
                declarations(value, out)
            }
        }
        _ => (),
    }
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let base = Path::new(&args[1]);
    let mut receipts = 0;
    let mut nodes = 0;
    let mut raw_bytes = 0;
    let mut normalized_bytes = 0;
    let mut restored = 0;
    let mut kinds = BTreeMap::<String, usize>::new();
    let mut versions = BTreeMap::<String, BTreeSet<Vec<u8>>>::new();
    for file in fs::read_dir(base.join(".kpopper/history-commits")).unwrap() {
        let path = file.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let manifest = Y::decode_document(&fs::read(path).unwrap()).unwrap();
        let receipt = &map(&manifest)["receipt"];
        T::validate_receipt(receipt).unwrap();
        receipts += 1;
        for side in ["before", "after"] {
            let evidence = map(&map(receipt)[side]);
            let Some(assessment) = evidence.get("assessment") else {
                continue;
            };
            let assessment = map(assessment);
            let V::Text(snapshot) = &assessment["snapshot_id"] else {
                panic!()
            };
            let mut declared = BTreeSet::new();
            if let Some(doc) = evidence.get("document") {
                declarations(doc, &mut declared);
            }
            for id in map(&assessment["nodes"]).keys() {
                declared.insert(vec![id.clone()]);
            }
            let declared = declared.into_iter().collect::<Vec<_>>();
            for (subject, node) in map(&assessment["nodes"]) {
                let packed = Evidence::pack(node, snapshot, &declared).unwrap();
                let bytes = packed.encode().unwrap();
                let restored_node = Evidence::decode(&bytes).unwrap().restore(snapshot).unwrap();
                assert_eq!(&restored_node, node);
                restored += 1;
                nodes += 1;
                raw_bytes += node.canonical_bytes().unwrap().len();
                normalized_bytes += bytes.len();
                versions
                    .entry(subject.clone())
                    .or_default()
                    .insert(bytes.clone());
                let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                for recipe in envelope["recipes"].as_array().unwrap() {
                    *kinds
                        .entry(recipe["recipe"]["kind"].as_str().unwrap().into())
                        .or_default() += 1;
                }
            }
        }
    }
    let distinct_bytes: usize = versions.values().flat_map(|v| v.iter()).map(Vec::len).sum();
    println!(
        "{}",
        json!({"receipts_validated":receipts,"node_occurrences":nodes,"exact_typed_node_reconstructions":restored,"raw_typed_occurrence_bytes":raw_bytes,"normalized_occurrence_bytes":normalized_bytes,"distinct_subject_payloads":versions.values().map(BTreeSet::len).sum::<usize>(),"distinct_subject_payload_bytes":distinct_bytes,"recipes":kinds,"coverage":"native recipe exactness and payload factorization only; no complete receipt/storage or reuse claim"})
    );
}
