//! Encode already normalized receipt evidence into node-local experimental streams.
//! Input is a private experiment artifact, not a supported migration or publication format.
use kpop_native::{history_node_codec as codec, value::TypedValue as V};
use serde_json::{Value, json};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let rows: Vec<Value> = serde_json::from_slice(&fs::read(&args[1]).unwrap()).unwrap();
    let root = Path::new(&args[2]);
    assert!(!root.exists());
    fs::create_dir_all(root).unwrap();
    let mut latest = BTreeMap::<String, codec::Version>::new();
    let mut origins = BTreeMap::<String, Vec<u8>>::new();
    let mut bytes = 0;
    let mut events = 0;
    let mut references = 0;
    for row in &rows {
        let op = row["operation"].as_str().unwrap();
        let mut nodes = row["nodes"].as_object().unwrap().clone();
        // Keep record-wide context separately; this candidate is measured, not approved as bounded.
        nodes.insert("@record-context".into(), row["context"].clone());
        let mut touched = Vec::new();
        for (subject, payload) in nodes {
            let state = V::from_json(&payload).unwrap();
            if latest
                .get(&subject)
                .is_some_and(|v| v.state() == Some(&state))
            {
                continue;
            }
            let base = latest.get(&subject);
            let event = codec::Event::create(
                &subject,
                op,
                base.map(|v| vec![v.id().into()]).unwrap_or_default(),
                base,
                Some(state),
            )
            .unwrap();
            let version = event.reconstruct(base).unwrap();
            let raw = event.encode().unwrap();
            if base.is_some() {
                let file = root.join(codec::subject_path(&subject).unwrap());
                let mut out = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(file)
                    .unwrap();
                if let Some(first) = origins.remove(&subject) {
                    out.write_all(&first).unwrap();
                    bytes += first.len();
                    events += 1;
                }
                out.write_all(&raw).unwrap();
                bytes += raw.len();
                events += 1;
            } else {
                origins.insert(subject.clone(), raw);
            }
            touched.push(json!({"subject":subject,"event":event.id()}));
            latest.insert(subject, version);
        }
        let manifest = serde_json::to_vec(&json!({"operation":op,"touched":touched})).unwrap();
        references += manifest.len();
        fs::write(root.join(format!("{op}.json")), manifest).unwrap();
    }
    let current = serde_json::to_vec(
        &latest
            .iter()
            .map(|(subject, v)| (subject, v.state().unwrap().to_tagged().unwrap()))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap();
    fs::write(root.join("current-typed.json"), &current).unwrap();
    let mut reconstructed = 0;
    for subject in latest.keys() {
        let path = root.join(codec::subject_path(subject).unwrap());
        if path.exists() {
            let all = codec::decode_stream(&fs::read(path).unwrap(), subject).unwrap();
            assert_eq!(all[latest[subject].id()].state(), latest[subject].state());
            reconstructed += all.len();
        }
    }
    #[cfg(unix)]
    let allocated = fs::read_dir(root)
        .unwrap()
        .map(|x| x.unwrap().metadata().unwrap().blocks() * 512)
        .sum::<u64>();
    #[cfg(not(unix))]
    let allocated = 0;
    println!(
        "{}",
        json!({"history_frame_bytes":bytes,"transaction_reference_bytes":references,"current_typed_bytes":current.len(),"allocated_bytes_including_current":allocated,"frames":events,"reconstructed_frames":reconstructed,"lazy_unchanged_nodes":origins.len(),"history_streams":latest.len()-origins.len(),"history_and_reference_bytes":bytes+references,"coverage":"representation only; record context still not proven bounded; no original raw archive, transaction integrity or fsync cost included"})
    );
}
