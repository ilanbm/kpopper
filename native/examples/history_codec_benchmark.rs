//! Representation experiment, not a production writer or durability benchmark.
use kpop_native::{history_node_codec as codec, history_yaml as yaml, value::TypedValue as V};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::Instant,
};
fn allocated(path: &Path) -> u64 {
    let metadata = fs::metadata(path).unwrap();
    #[cfg(unix)]
    {
        metadata.blocks() * 512
    }
    #[cfg(not(unix))]
    {
        metadata.len()
    }
}
fn quantile(values: &[f64], q: f64) -> f64 {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * q).round() as usize]
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let root = Path::new(&args[1]);
    let scenario = &args[2];
    let count: usize = args[3].parse().unwrap();
    assert!([100, 1000, 10000].contains(&count));
    assert!(["repeated", "catalog"].contains(&scenario.as_str()));
    assert!(!root.exists(), "use a new disposable directory");
    fs::create_dir_all(root.join("history")).unwrap();
    fs::create_dir(root.join("commits")).unwrap();
    let mut current = BTreeMap::new();
    let state =
        |n| V::from_json(&json!({"v":n,"source":"fixed source","text":"x".repeat(1024)})).unwrap();
    let initial = codec::Event::create("p.a", "create", vec![], None, Some(state(0))).unwrap();
    let mut version = initial.reconstruct(None).unwrap();
    let mut prior_tx = String::new();
    let mut added = Vec::new();
    let mut times = Vec::new();
    let mut history_bytes = 0;
    let mut manifest_bytes = 0;
    let mut frames = 0;
    let mut writes = 0;
    let start = Instant::now();
    for n in 1..=count {
        let operation = format!("change-{n:05}");
        let subject = if scenario == "catalog" {
            format!("p.{n:05}")
        } else {
            "p.a".into()
        };
        let now = Instant::now();
        let next = if scenario == "catalog" {
            codec::Event::create(&subject, &operation, vec![], None, Some(state(n))).unwrap()
        } else {
            codec::Event::create(
                &subject,
                &operation,
                vec![version.id().into()],
                Some(&version),
                Some(state(n)),
            )
            .unwrap()
        };
        let raw = next.encode().unwrap();
        let mut bytes = 0;
        if scenario == "repeated" {
            let path = root
                .join("history")
                .join(codec::subject_path(&subject).unwrap());
            let mut out = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            if n == 1 {
                let raw = initial.encode().unwrap();
                out.write_all(&raw).unwrap();
                bytes += raw.len();
                frames += 1;
                writes += 1;
            }
            out.write_all(&raw).unwrap();
            bytes += raw.len();
            frames += 1;
            writes += 1;
            version = next.reconstruct(Some(&version)).unwrap();
        }
        history_bytes += bytes;
        // Deliberately minimal candidate manifest: this is an envelope cost, not publication proof.
        let manifest = serde_json::to_vec(&json!({"format":"node-transaction-experiment/v1","operation":operation,"parents":if prior_tx.is_empty(){vec![]}else{vec![prior_tx.clone()]},"touched":[{"subject":subject,"event":next.id()}]})).unwrap();
        fs::write(
            root.join("commits").join(format!("{operation}.json")),
            &manifest,
        )
        .unwrap();
        writes += 1;
        manifest_bytes += manifest.len();
        bytes += manifest.len();
        prior_tx = kpop_native::identity::sha256(&manifest);
        // Current creation context is held here; no history file exists for catalog additions.
        current.insert(subject,json!({"v":n,"source":"fixed source","text":"x".repeat(1024),"creation":if scenario=="catalog"{{ let mut context = serde_json::from_slice::<serde_json::Value>(&raw).unwrap(); context.as_object_mut().unwrap().remove("delta"); context }}else{json!({"event":next.id()})}}));
        added.push(bytes as f64);
        times.push(now.elapsed().as_secs_f64() * 1000.);
    }
    let writer_ms = start.elapsed().as_secs_f64() * 1000.;
    let refresh = Instant::now();
    // JSON is a YAML subset; measure full-view refresh separately, without the single-value codec cap.
    let view = serde_json::to_vec_pretty(&json!({"known":current})).unwrap();
    fs::write(root.join("GROUNDING.yaml"), &view).unwrap();
    let current_refresh_ms = refresh.elapsed().as_secs_f64() * 1000.;
    let mut reads = 0;
    let mut decoded = 0;
    let audit = Instant::now();
    for file in fs::read_dir(root.join("history")).unwrap() {
        let raw = fs::read(file.unwrap().path()).unwrap();
        reads += 1;
        decoded += codec::decode_stream(&raw, "p.a").unwrap().len();
    }
    let audit_ms = audit.elapsed().as_secs_f64() * 1000.;
    let mut allocation = 0;
    let mut files = 0;
    for dir in ["history", "commits"] {
        for file in fs::read_dir(root.join(dir)).unwrap() {
            allocation += allocated(&file.unwrap().path());
            files += 1;
        }
    }
    let sample = codec::Event::create(
        "p.a",
        "encoding-sample",
        vec![version.id().into()],
        Some(&version),
        Some(state(count + 1)),
    )
    .unwrap();
    let json_frame = sample.encode().unwrap();
    let yaml_frame = yaml::encode_document(
        &V::from_json(&serde_json::from_slice::<serde_json::Value>(&json_frame).unwrap()).unwrap(),
    )
    .unwrap();
    println!("{}",serde_json::to_string(&json!({"scenario":scenario,"operations":count,"history_bytes":history_bytes,"manifest_bytes":manifest_bytes,"allocated_bytes":allocation,"files":files,"frames":frames,"writes":writes,"audit_file_reads":reads,"audit_decoded_frames":decoded,"writer_ms":writer_ms,"operation_p50_ms":quantile(&times,0.5),"operation_p95_ms":quantile(&times,0.95),"last100_average_added_bytes":added[added.len()-100..].iter().sum::<f64>()/100.,"audit_warm_ms":audit_ms,"current_view_bytes":view.len(),"current_view_allocated_bytes":allocated(&root.join("GROUNDING.yaml")),"current_view_refresh_ms":current_refresh_ms,"sample_json_frame_bytes":json_frame.len(),"sample_yaml_frame_bytes":yaml_frame.len(),"fsync":false,"coverage":"representation only; no production publication, receipt replay, syscall tracing or cold-cache claim"})).unwrap());
}
