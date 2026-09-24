use chrono::{Days, NaiveDate};
use clap::{Parser, ValueEnum};
use kpop_native::{
    history_authoring::Options,
    history_io_metrics::{self, Stats as FileIoStats},
    history_node_publication::Phase,
    history_node_writer as Writer,
    history_paths::Scheme,
    value::TypedValue as V,
};
use serde::Serialize;
use serde_json::json;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    error::Error,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
enum Workload {
    Repeated,
    Catalog,
}

#[derive(Parser)]
#[command(about = "Full node-history writer benchmark on a fresh disposable record")]
struct Args {
    #[arg(long)]
    output_directory: PathBuf,
    #[arg(long, value_enum)]
    workload: Workload,
    #[arg(long)]
    size: usize,
    #[arg(long)]
    max_seconds: Option<f64>,
}

struct CountingAllocator;

static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn payload_offset(align: usize) -> Option<usize> {
    let header = std::mem::size_of::<usize>();
    let remainder = header % align;
    header.checked_add(if remainder == 0 { 0 } else { align - remainder })
}

fn expanded_layout(layout: Layout, offset: usize) -> Option<Layout> {
    Layout::from_size_align(offset.checked_add(layout.size().max(1))?, layout.align()).ok()
}

fn update_peak(live: u64) {
    let mut peak = PEAK_LIVE_BYTES.load(Ordering::Relaxed);
    while live > peak {
        match PEAK_LIVE_BYTES.compare_exchange_weak(
            peak,
            live,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let Some(offset) = payload_offset(layout.align()) else {
            return std::ptr::null_mut();
        };
        let Some(expanded) = expanded_layout(layout, offset) else {
            return std::ptr::null_mut();
        };
        let base = unsafe { System.alloc(expanded) };
        if base.is_null() {
            return base;
        }
        let payload = unsafe { base.add(offset) };
        unsafe {
            payload
                .sub(std::mem::size_of::<usize>())
                .cast::<usize>()
                .write_unaligned(layout.size())
        };
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        let live =
            LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed) + layout.size() as u64;
        update_peak(live);
        payload
    }

    unsafe fn dealloc(&self, payload: *mut u8, layout: Layout) {
        let Some(offset) = payload_offset(layout.align()) else {
            return;
        };
        let requested = unsafe {
            payload
                .sub(std::mem::size_of::<usize>())
                .cast::<usize>()
                .read_unaligned()
        };
        let Some(expanded) = expanded_layout(layout, offset) else {
            return;
        };
        DEALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        DEALLOCATED_BYTES.fetch_add(requested as u64, Ordering::Relaxed);
        LIVE_BYTES.fetch_sub(requested as u64, Ordering::Relaxed);
        unsafe { System.dealloc(payload.sub(offset), expanded) };
    }

    unsafe fn realloc(&self, payload: *mut u8, old: Layout, new_size: usize) -> *mut u8 {
        REALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        let Ok(new_layout) = Layout::from_size_align(new_size, old.align()) else {
            return std::ptr::null_mut();
        };
        let Some(offset) = payload_offset(old.align()) else {
            return std::ptr::null_mut();
        };
        let Some(expanded) = expanded_layout(new_layout, offset) else {
            return std::ptr::null_mut();
        };
        let base = unsafe { System.alloc(expanded) };
        if base.is_null() {
            return base;
        }
        let new_payload = unsafe { base.add(offset) };
        unsafe {
            new_payload
                .sub(std::mem::size_of::<usize>())
                .cast::<usize>()
                .write_unaligned(new_size)
        };
        unsafe {
            std::ptr::copy_nonoverlapping(payload, new_payload, old.size().min(new_size));
        }
        let prior_size = unsafe {
            payload
                .sub(std::mem::size_of::<usize>())
                .cast::<usize>()
                .read_unaligned()
        };
        let Some(old_expanded) = expanded_layout(old, offset) else {
            unsafe { System.dealloc(base, expanded) };
            return std::ptr::null_mut();
        };
        ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        DEALLOCATED_BYTES.fetch_add(prior_size as u64, Ordering::Relaxed);
        let live = if new_size >= prior_size {
            LIVE_BYTES.fetch_add((new_size - prior_size) as u64, Ordering::Relaxed)
                + (new_size - prior_size) as u64
        } else {
            LIVE_BYTES.fetch_sub((prior_size - new_size) as u64, Ordering::Relaxed)
                - (prior_size - new_size) as u64
        };
        update_peak(live);
        unsafe { System.dealloc(payload.sub(offset), old_expanded) };
        new_payload
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
struct AllocatorInterval {
    allocation_calls: u64,
    reallocation_calls: u64,
    deallocation_calls: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
    live_bytes_before: u64,
    live_bytes_after: u64,
    peak_live_bytes: u64,
    peak_live_growth_bytes: u64,
}

fn reset_allocator_interval() -> u64 {
    let live = LIVE_BYTES.load(Ordering::Relaxed);
    ALLOCATION_CALLS.store(0, Ordering::Relaxed);
    REALLOCATION_CALLS.store(0, Ordering::Relaxed);
    DEALLOCATION_CALLS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    DEALLOCATED_BYTES.store(0, Ordering::Relaxed);
    PEAK_LIVE_BYTES.store(live, Ordering::Relaxed);
    live
}

fn allocator_snapshot(live_before: u64) -> AllocatorInterval {
    let live_after = LIVE_BYTES.load(Ordering::Relaxed);
    let peak_live_bytes = PEAK_LIVE_BYTES.load(Ordering::Relaxed);
    AllocatorInterval {
        allocation_calls: ALLOCATION_CALLS.load(Ordering::Relaxed),
        reallocation_calls: REALLOCATION_CALLS.load(Ordering::Relaxed),
        deallocation_calls: DEALLOCATION_CALLS.load(Ordering::Relaxed),
        allocated_bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        deallocated_bytes: DEALLOCATED_BYTES.load(Ordering::Relaxed),
        live_bytes_before: live_before,
        live_bytes_after: live_after,
        peak_live_bytes,
        peak_live_growth_bytes: peak_live_bytes.saturating_sub(live_before),
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct ByteCounts {
    logical_bytes: u64,
    allocated_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct StorageBytes {
    source: ByteCounts,
    history: ByteCounts,
    current: ByteCounts,
    total: ByteCounts,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct StorageGrowth {
    sample_operations: usize,
    source_logical_bytes: i64,
    source_allocated_bytes: i64,
    history_logical_bytes: i64,
    history_allocated_bytes: i64,
    current_logical_bytes: i64,
    current_allocated_bytes: i64,
    total_logical_bytes: i64,
    total_allocated_bytes: i64,
    average_total_logical_bytes_per_operation: f64,
    average_total_allocated_bytes_per_operation: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
struct BoundaryMaximum {
    observations: u64,
    maximum_logical_bytes: u64,
    maximum_allocated_bytes: u64,
    maximum_phase: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct OperationMeasurement {
    kind: String,
    operation: String,
    success: bool,
    latency_ms: f64,
    prepare_latency_ms: f64,
    publish_latency_ms: f64,
    error: Option<String>,
    file_io: FileIoStats,
    allocator: AllocatorInterval,
}

#[derive(Serialize)]
struct ResultDocument {
    status: &'static str,
    workload: Workload,
    requested_count: usize,
    completed_count: usize,
    size: usize,
    max_seconds: Option<f64>,
    elapsed_seconds: f64,
    error: Option<String>,
    error_phase: Option<String>,
    output_directory: String,
    record_directory: String,
    seed: Option<OperationMeasurement>,
    operations: Vec<OperationMeasurement>,
    storage: StorageBytes,
    last100_growth: Option<StorageGrowth>,
    maximum_temporary_journal: BoundaryMaximum,
    process_rss: RssSamples,
    allocator: AllocatorTotals,
    file_io: FileIoStats,
    measurement_caveats: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct RssSamples {
    start_bytes: Option<u64>,
    end_bytes: Option<u64>,
    peak_sampled_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct AllocatorTotals {
    allocation_calls: u64,
    reallocation_calls: u64,
    deallocation_calls: u64,
    allocated_bytes: u64,
    deallocated_bytes: u64,
    live_bytes_before_first_operation: Option<u64>,
    live_bytes_after_last_operation: Option<u64>,
    peak_live_bytes: Option<u64>,
    peak_live_growth_bytes: Option<u64>,
}

#[derive(Default)]
struct BoundaryTracker {
    maximum: BoundaryMaximum,
    maximum_phase: Option<String>,
}

fn phase_name(phase: Phase) -> String {
    match phase {
        Phase::Import(index) => format!("import:{index}"),
        Phase::Journal => "journal".into(),
        Phase::Append(index) => format!("append:{index}"),
        Phase::Evidence(index) => format!("evidence:{index}"),
        Phase::Commit => "commit".into(),
        Phase::View => "view".into(),
    }
}

fn temp_journal_bytes(record: &Path) -> std::io::Result<ByteCounts> {
    fn visit(directory: &Path, totals: &mut ByteCounts) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit(&entry.path(), totals)?;
            } else if file_type.is_file() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name == ".history-node-publication.json"
                    || name.starts_with(".history-")
                    || name.ends_with(".tmp")
                {
                    let metadata = entry.metadata()?;
                    totals.logical_bytes = totals.logical_bytes.saturating_add(metadata.len());
                    totals.allocated_bytes = totals
                        .allocated_bytes
                        .saturating_add(allocated_bytes(&metadata));
                }
            }
        }
        Ok(())
    }

    let mut totals = ByteCounts::default();
    let internal = record.join(".kpopper");
    if internal.exists() {
        visit(&internal, &mut totals)?;
    }
    Ok(totals)
}

fn allocated_bytes(metadata: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.blocks().saturating_mul(512)
    }
    #[cfg(not(unix))]
    {
        metadata.len()
    }
}

fn record_boundary(
    tracker: &mut BoundaryTracker,
    record: &Path,
    phase: Phase,
) -> Result<(), kpop_native::Error> {
    let bytes = temp_journal_bytes(record)
        .map_err(|error| kpop_native::Error(format!("benchmark_boundary_io: {error}")))?;
    tracker.maximum.observations = tracker.maximum.observations.saturating_add(1);
    if bytes.logical_bytes > tracker.maximum.maximum_logical_bytes
        || bytes.allocated_bytes > tracker.maximum.maximum_allocated_bytes
    {
        tracker.maximum.maximum_logical_bytes = tracker
            .maximum
            .maximum_logical_bytes
            .max(bytes.logical_bytes);
        tracker.maximum.maximum_allocated_bytes = tracker
            .maximum
            .maximum_allocated_bytes
            .max(bytes.allocated_bytes);
        tracker.maximum_phase = Some(phase_name(phase));
    }
    Ok(())
}

fn storage_bytes(record: &Path) -> std::io::Result<StorageBytes> {
    fn visit(record: &Path, directory: &Path, totals: &mut StorageBytes) -> std::io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                visit(record, &entry.path(), totals)?;
            } else if kind.is_file() {
                let metadata = entry.metadata()?;
                let logical_bytes = metadata.len();
                let allocated = allocated_bytes(&metadata);
                let path = entry.path();
                let relative = path.strip_prefix(record).unwrap_or(&path);
                let category = if relative == Path::new("GROUNDING.yaml") {
                    &mut totals.current
                } else if relative.starts_with(Path::new(".kpopper/history"))
                    || relative.starts_with(Path::new(".kpopper/history-commits"))
                {
                    &mut totals.history
                } else {
                    &mut totals.source
                };
                category.logical_bytes = category.logical_bytes.saturating_add(logical_bytes);
                category.allocated_bytes = category.allocated_bytes.saturating_add(allocated);
                totals.total.logical_bytes =
                    totals.total.logical_bytes.saturating_add(logical_bytes);
                totals.total.allocated_bytes =
                    totals.total.allocated_bytes.saturating_add(allocated);
            }
        }
        Ok(())
    }

    let mut totals = StorageBytes::default();
    visit(record, record, &mut totals)?;
    Ok(totals)
}

fn date_for(index: usize) -> Result<String, Box<dyn Error>> {
    let start = NaiveDate::from_ymd_opt(2000, 1, 1).ok_or("invalid benchmark base date")?;
    let date = start
        .checked_add_days(Days::new(index as u64))
        .ok_or("benchmark date overflow")?;
    Ok(date.format("%Y-%m-%d").to_string())
}

fn options(operation: &str, day: &str) -> Result<Options, Box<dyn Error>> {
    Ok(Options {
        operation: operation.into(),
        recorded_at: format!("{day}T12:00:00+00:00"),
        recording_day: day.into(),
        by: V::from_json(&json!("history-writer-benchmark"))?,
        strict: false,
        paths: Scheme::Hashed,
        receipt_version: None,
    })
}

fn action(workload: Workload, index: usize, day: &str) -> Result<V, Box<dyn Error>> {
    let value = match workload {
        Workload::Repeated => json!({
            "kind": "set",
            "id": "p.benchmark",
            "value": "fixed",
            "as_of": day,
        }),
        Workload::Catalog => json!({
            "kind": "add",
            "id": format!("p.{index:05}"),
            "body": {"v": "fixed"},
            "as_of": day,
        }),
    };
    Ok(V::from_json(&value)?)
}

fn run_operation(
    kind: &str,
    operation: &str,
    record: &Path,
    action: &V,
    options: &Options,
    boundary: &mut BoundaryTracker,
) -> OperationMeasurement {
    let live_before = reset_allocator_interval();
    history_io_metrics::reset();
    let started = Instant::now();
    let prepare_started = Instant::now();
    let prepared = Writer::prepare(record, action, options, None);
    let prepare_latency_ms = prepare_started.elapsed().as_secs_f64() * 1000.0;
    let mut publish_latency_ms = 0.0;
    let result = match prepared {
        Ok(prepared) => {
            let publish_started = Instant::now();
            let result = Writer::publish(record, &prepared, None, |phase| {
                record_boundary(boundary, record, phase)
            });
            publish_latency_ms = publish_started.elapsed().as_secs_f64() * 1000.0;
            result.map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    };
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    let file_io = history_io_metrics::snapshot();
    let allocator = allocator_snapshot(live_before);
    OperationMeasurement {
        kind: kind.into(),
        operation: operation.into(),
        success: result.is_ok(),
        latency_ms,
        prepare_latency_ms,
        publish_latency_ms,
        error: result.err(),
        file_io,
        allocator,
    }
}

fn add_stats(total: &mut FileIoStats, next: FileIoStats) {
    total.read_calls = total.read_calls.saturating_add(next.read_calls);
    total.read_bytes = total.read_bytes.saturating_add(next.read_bytes);
    total.write_calls = total.write_calls.saturating_add(next.write_calls);
    total.semantic_replays = total.semantic_replays.saturating_add(next.semantic_replays);
    total.write_bytes = total.write_bytes.saturating_add(next.write_bytes);
}

fn allocator_totals<'a>(
    measurements: impl Iterator<Item = &'a OperationMeasurement>,
) -> AllocatorTotals {
    let mut total = AllocatorTotals::default();
    for measurement in measurements {
        let stats = measurement.allocator;
        total.allocation_calls = total
            .allocation_calls
            .saturating_add(stats.allocation_calls);
        total.reallocation_calls = total
            .reallocation_calls
            .saturating_add(stats.reallocation_calls);
        total.deallocation_calls = total
            .deallocation_calls
            .saturating_add(stats.deallocation_calls);
        total.allocated_bytes = total.allocated_bytes.saturating_add(stats.allocated_bytes);
        total.deallocated_bytes = total
            .deallocated_bytes
            .saturating_add(stats.deallocated_bytes);
        total
            .live_bytes_before_first_operation
            .get_or_insert(stats.live_bytes_before);
        total.live_bytes_after_last_operation = Some(stats.live_bytes_after);
        total.peak_live_bytes = Some(
            total
                .peak_live_bytes
                .unwrap_or_default()
                .max(stats.peak_live_bytes),
        );
        total.peak_live_growth_bytes = Some(
            total
                .peak_live_growth_bytes
                .unwrap_or_default()
                .max(stats.peak_live_growth_bytes),
        );
    }
    total
}

fn storage_growth(before: StorageBytes, after: StorageBytes, operations: usize) -> StorageGrowth {
    fn delta(before: u64, after: u64) -> i64 {
        after as i64 - before as i64
    }
    let total_logical = delta(before.total.logical_bytes, after.total.logical_bytes);
    let total_allocated = delta(before.total.allocated_bytes, after.total.allocated_bytes);
    StorageGrowth {
        sample_operations: operations,
        source_logical_bytes: delta(before.source.logical_bytes, after.source.logical_bytes),
        source_allocated_bytes: delta(before.source.allocated_bytes, after.source.allocated_bytes),
        history_logical_bytes: delta(before.history.logical_bytes, after.history.logical_bytes),
        history_allocated_bytes: delta(
            before.history.allocated_bytes,
            after.history.allocated_bytes,
        ),
        current_logical_bytes: delta(before.current.logical_bytes, after.current.logical_bytes),
        current_allocated_bytes: delta(
            before.current.allocated_bytes,
            after.current.allocated_bytes,
        ),
        total_logical_bytes: total_logical,
        total_allocated_bytes: total_allocated,
        average_total_logical_bytes_per_operation: total_logical as f64 / operations as f64,
        average_total_allocated_bytes_per_operation: total_allocated as f64 / operations as f64,
    }
}

fn checkpoint(
    output: &Path,
    status: &str,
    args: &Args,
    completed: usize,
    last_operation: Option<&str>,
    error: Option<&str>,
    elapsed: Duration,
) -> std::io::Result<()> {
    let checkpoint = json!({
        "status": status,
        "workload": args.workload,
        "requested_count": args.size,
        "completed_count": completed,
        "last_operation": last_operation,
        "error": error,
        "elapsed_seconds": elapsed.as_secs_f64(),
    });
    let temporary = output.join("progress.json.tmp");
    let destination = output.join("progress.json");
    let mut file = File::create(&temporary)?;
    serde_json::to_writer(&mut file, &checkpoint).map_err(std::io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(temporary, destination)
}

fn process_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = fs::read_to_string("/proc/self/status").ok()?;
        let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
        return line
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024);
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        return String::from_utf8(output.stdout)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()?
            .checked_mul(1024);
    }
    #[allow(unreachable_code)]
    None
}

fn add_to_jsonl(log: &mut BufWriter<File>, sample: &OperationMeasurement) -> std::io::Result<()> {
    serde_json::to_writer(&mut *log, sample).map_err(std::io::Error::other)?;
    log.write_all(b"\n")
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    if ![100, 1000, 10000].contains(&args.size) {
        return Err("--size must be 100, 1000, or 10000".into());
    }
    if args
        .max_seconds
        .is_some_and(|seconds| !seconds.is_finite() || seconds < 0.0)
    {
        return Err("--max-seconds must be a finite non-negative number".into());
    }

    let start_rss = process_rss_bytes();
    let case_started = Instant::now();
    fs::create_dir_all(&args.output_directory)?;
    if fs::read_dir(&args.output_directory)?.next().is_some() {
        return Err("--output-directory must be empty".into());
    }
    let record = args.output_directory.join("node-history");
    fs::create_dir(&record)?;
    let progress_log = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(args.output_directory.join("progress.jsonl"))?;
    let mut progress_log = BufWriter::new(progress_log);
    let mut boundary = BoundaryTracker::default();
    let mut seed = None;
    let mut operations = Vec::with_capacity(args.size);
    let mut completed = 0usize;
    let mut status = "success";
    let mut error = None;
    let mut error_phase = None;
    let mut growth_baseline = None;
    let mut writer_io = FileIoStats::default();

    fs::create_dir(record.join(".kpopper"))?;
    fs::write(
        record.join(".kpopper/history.yaml"),
        "version: 3\nprofile: node-history/v1\nauthority: history\nrecord_id: history-writer-benchmark\ngeneration: 1\nrequires: [node-history/v1]\n",
    )?;
    fs::write(
        record.join("GROUNDING.yaml"),
        "meta:\n  purpose: History writer benchmark\nschema:\n  deps: rests_on\n  snapshot: seen\n  predicate: wrong_if\nknown: {}\n",
    )?;
    checkpoint(
        &args.output_directory,
        "running",
        &args,
        0,
        None,
        None,
        case_started.elapsed(),
    )?;

    if matches!(args.workload, Workload::Repeated)
        && !args
            .max_seconds
            .is_some_and(|seconds| case_started.elapsed().as_secs_f64() >= seconds)
    {
        let day = date_for(0)?;
        let seed_action = V::from_json(&json!({
            "kind": "add",
            "id": "p.benchmark",
            "body": {"v": "fixed"},
            "as_of": day,
        }))?;
        let seed_options = options("benchmark-seed", &day)?;
        let measurement = run_operation(
            "seed",
            "benchmark-seed",
            &record,
            &seed_action,
            &seed_options,
            &mut boundary,
        );
        add_stats(&mut writer_io, measurement.file_io);
        add_to_jsonl(&mut progress_log, &measurement)?;
        seed = Some(measurement);
        if !seed.as_ref().unwrap().success {
            status = "refused";
            error = seed.as_ref().unwrap().error.clone();
            error_phase = Some("seed".into());
        } else if args.size >= 100 {
            growth_baseline = Some(storage_bytes(&record)?);
        }
        progress_log.flush()?;
        progress_log.get_ref().sync_data()?;
        checkpoint(
            &args.output_directory,
            status,
            &args,
            0,
            Some("benchmark-seed"),
            error.as_deref(),
            case_started.elapsed(),
        )?;
    } else if matches!(args.workload, Workload::Repeated) {
        status = "time_limit";
    } else if args.size >= 100 {
        growth_baseline = Some(storage_bytes(&record)?);
    }

    while completed < args.size && status == "success" {
        if args
            .max_seconds
            .is_some_and(|seconds| case_started.elapsed().as_secs_f64() >= seconds)
        {
            status = "time_limit";
            break;
        }
        if args.size >= 100 && completed == args.size - 100 {
            growth_baseline = Some(storage_bytes(&record)?);
        }
        let operation_number = completed + 1;
        let day = date_for(operation_number)?;
        let operation = format!("benchmark-{operation_number:05}");
        let operation_action = action(args.workload, operation_number, &day)?;
        let operation_options = options(&operation, &day)?;
        let measurement = run_operation(
            "workload",
            &operation,
            &record,
            &operation_action,
            &operation_options,
            &mut boundary,
        );
        add_stats(&mut writer_io, measurement.file_io);
        let succeeded = measurement.success;
        if !succeeded {
            status = "refused";
            error = measurement.error.clone();
            error_phase = Some("writer".into());
        } else {
            completed += 1;
        }
        add_to_jsonl(&mut progress_log, &measurement)?;
        operations.push(measurement);

        if completed % 25 == 0 || status != "success" || completed == args.size {
            progress_log.flush()?;
            progress_log.get_ref().sync_data()?;
            checkpoint(
                &args.output_directory,
                status,
                &args,
                completed,
                Some(&operation),
                error.as_deref(),
                case_started.elapsed(),
            )?;
        }
    }

    progress_log.flush()?;
    progress_log.get_ref().sync_all()?;
    let storage = storage_bytes(&record)?;
    let last100_growth = if completed == args.size && completed >= 100 {
        growth_baseline.map(|baseline| storage_growth(baseline, storage, 100))
    } else {
        None
    };
    let end_rss = process_rss_bytes();
    let rss = RssSamples {
        start_bytes: start_rss,
        end_bytes: end_rss,
        peak_sampled_bytes: match (start_rss, end_rss) {
            (Some(start), Some(end)) => Some(start.max(end)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        },
    };
    let allocator = allocator_totals(seed.iter().chain(operations.iter()));
    let mut all_io = FileIoStats::default();
    add_stats(&mut all_io, writer_io);
    boundary.maximum.maximum_phase = boundary.maximum_phase.clone();
    let result = ResultDocument {
        status,
        workload: args.workload,
        requested_count: args.size,
        completed_count: completed,
        size: args.size,
        max_seconds: args.max_seconds,
        elapsed_seconds: case_started.elapsed().as_secs_f64(),
        error,
        error_phase,
        output_directory: args.output_directory.display().to_string(),
        record_directory: record.display().to_string(),
        seed,
        operations,
        storage,
        last100_growth,
        maximum_temporary_journal: boundary.maximum,
        process_rss: rss,
        allocator,
        file_io: all_io,
        measurement_caveats: vec![
            "Each requested workload operation calls the full writer prepare and publish APIs; repeated workload additionally performs one full-writer seed operation outside its requested count.",
            "A time_limit is the benchmark stop guard, not an intrinsic writer resource refusal; refused means the writer API returned an error.",
            "Source contains every record file except GROUNDING.yaml and files under .kpopper/history or .kpopper/history-commits; history includes streams and commit manifests.",
            "Allocated file bytes use Unix st_blocks multiplied by 512, or logical file length where unavailable; directory and metadata allocation are excluded.",
            "Temporary/journal maxima observe matching files only at publisher boundary callbacks; shorter-lived bytes between callbacks can be missed.",
            "RSS is sampled at the beginning and end only; peak_sampled_bytes is not a continuous process peak.",
            "Allocator counts cover intervals around writer calls and include allocations by the benchmark boundary callback; setup, progress serialization, and result encoding are excluded.",
            "Allocator byte and live/peak values count Rust global-allocator requested sizes; wrapper headers, allocator internals, and allocations bypassing GlobalAlloc are excluded.",
            "File I/O counts depend on parent-integrated library boundary hooks; they count Rust library byte boundaries, not OS syscalls or cache misses.",
            "File I/O counters exclude child runtime processes and direct directory/metadata scans; no cold-cache claim is made.",
            "No migration, external integrations, model inference, or live records are involved.",
        ],
    };
    let result_value = serde_json::to_value(&result)?;
    checkpoint(
        &args.output_directory,
        result.status,
        &args,
        completed,
        result
            .operations
            .last()
            .map(|sample| sample.operation.as_str()),
        result.error.as_deref(),
        case_started.elapsed(),
    )?;
    let result_path = args.output_directory.join("result.json");
    let mut output = File::create(&result_path)?;
    serde_json::to_writer_pretty(&mut output, &result_value)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    println!("{}", serde_json::to_string(&result_value)?);
    Ok(())
}
