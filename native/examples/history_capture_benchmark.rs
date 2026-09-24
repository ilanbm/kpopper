//! Read-only baseline capture measurement on a disposable copy.
use kpop_native::history_store::Store;
use serde_json::json;
use std::{path::Path, time::Instant};
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let entry = Path::new(&args[1]);
    let store = Store::new(entry).unwrap();
    let start = Instant::now();
    let capture = store.capture().unwrap();
    let capture_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    let rendered = store.render(&capture).unwrap();
    let render_ms = start.elapsed().as_secs_f64() * 1000.;
    println!(
        "{}",
        json!({"capture_ms":capture_ms,"render_ms":render_ms,"render_matches_entry":rendered==capture.entry_bytes,"commits":capture.commits.len(),"objects":capture.objects.len(),"manifest_bytes":capture.commits.values().map(Vec::len).sum::<usize>(),"object_bytes":capture.object_bytes.values().map(Vec::len).sum::<usize>(),"entry_bytes":capture.entry_bytes.len(),"inventory_observations":capture.inventory.len(),"coverage":"full existing capture plus render; parser and syscall counters not instrumented; no cold-cache assertion"})
    );
}
