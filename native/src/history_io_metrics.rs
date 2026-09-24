use serde::Serialize;
use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Stats {
    pub read_calls: u64,
    pub read_bytes: u64,
    pub write_calls: u64,
    pub write_bytes: u64,
}

thread_local! {
    static COUNTERS: Cell<Stats> = const { Cell::new(Stats {
        read_calls: 0,
        read_bytes: 0,
        write_calls: 0,
        write_bytes: 0,
    }) };
}

pub fn reset() {
    COUNTERS.with(|counters| counters.set(Stats::default()));
}

pub fn snapshot() -> Stats {
    COUNTERS.with(Cell::get)
}

pub(crate) fn read(bytes: usize) {
    COUNTERS.with(|counters| {
        let mut stats = counters.get();
        stats.read_calls = stats.read_calls.saturating_add(1);
        stats.read_bytes = stats.read_bytes.saturating_add(bytes as u64);
        counters.set(stats);
    });
}

pub(crate) fn write(bytes: usize) {
    COUNTERS.with(|counters| {
        let mut stats = counters.get();
        stats.write_calls = stats.write_calls.saturating_add(1);
        stats.write_bytes = stats.write_bytes.saturating_add(bytes as u64);
        counters.set(stats);
    });
}
