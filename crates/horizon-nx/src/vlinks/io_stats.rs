//! What the SD card costs the game: how many opens, `stat`s, listings and reads went through the
//! SD card's device (`devoptab.rs`), and how long they took, so a log tells waiting on the card
//! from work on the CPU. [`report`] gives what happened since its last call.

use core::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Instant;

/// One kind of operation: how many, how long in all (ns), and a third number (failures, bytes).
pub(crate) struct Counter {
    count: AtomicU64,
    nanos: AtomicU64,
    extra: AtomicU64,
}

impl Counter {
    const fn new() -> Self {
        Counter {
            count: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
            extra: AtomicU64::new(0),
        }
    }

    /// Counts one operation that started at `since`, adding `extra`.
    pub(crate) fn add(&self, since: Instant, extra: u64) {
        self.count.fetch_add(1, Relaxed);
        self.nanos
            .fetch_add(since.elapsed().as_nanos() as u64, Relaxed);
        if extra != 0 {
            self.extra.fetch_add(extra, Relaxed);
        }
    }

    /// Adds time and `extra` without counting an operation (a listing's next entry).
    pub(crate) fn add_time(&self, since: Instant, extra: u64) {
        self.nanos
            .fetch_add(since.elapsed().as_nanos() as u64, Relaxed);
        self.extra.fetch_add(extra, Relaxed);
    }

    fn get(&self) -> [u64; 3] {
        [
            self.count.load(Relaxed),
            self.nanos.load(Relaxed),
            self.extra.load(Relaxed),
        ]
    }
}

/// Opens (extra: failed ones, mostly files that aren't there).
pub(crate) static OPEN: Counter = Counter::new();
/// `stat` and `lstat` (extra: failed ones).
pub(crate) static STAT: Counter = Counter::new();
/// Folder listings opened (extra: entries read).
pub(crate) static LIST: Counter = Counter::new();
/// Reads (extra: bytes).
pub(crate) static READ: Counter = Counter::new();
/// Closes.
pub(crate) static CLOSE: Counter = Counter::new();
/// Writes (extra: bytes).
pub(crate) static WRITE: Counter = Counter::new();
/// Links' manifests looked for, the first time a path goes through a folder (extra: found).
pub(crate) static PROBE: Counter = Counter::new();

const ALL: [&Counter; 7] = [&OPEN, &STAT, &LIST, &READ, &CLOSE, &WRITE, &PROBE];

/// The totals at the last [`report`], and when it was.
static LAST: Mutex<Option<(Instant, [[u64; 3]; 7])>> = Mutex::new(None);

fn ms(nanos: u64) -> f64 {
    nanos as f64 / 1e6
}

/// One line on the SD card since the last call (since boot for the first): counts and the time
/// the calls took, added over all threads (so it can exceed the wall time when threads wait on
/// the card together). `None` when nothing happened.
pub fn report() -> Option<String> {
    let now: [[u64; 3]; 7] = ALL.map(Counter::get);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let (since, before) = last.unwrap_or((crate::vlinks::io_stats::boot(), [[0; 3]; 7]));
    *last = Some((Instant::now(), now));
    let d: Vec<[u64; 3]> = now
        .iter()
        .zip(before)
        .map(|(n, b)| [n[0] - b[0], n[1] - b[1], n[2] - b[2]])
        .collect();
    if d.iter().all(|c| c[0] == 0) {
        return None;
    }
    let [open, stat, list, read, close, write, probe] = [d[0], d[1], d[2], d[3], d[4], d[5], d[6]];
    let waited = d.iter().map(|c| c[1]).sum::<u64>();
    Some(format!(
        "[SD] over {:.1} s: {:.1} s in SD card calls; {} opens ({} missing) {:.0} ms, {} closes {:.0} ms, {} stats ({} missing) {:.0} ms, {} listings ({} entries) {:.0} ms, {} reads of {:.1} MiB {:.0} ms ({:.1} MiB/s while reading), {} writes of {:.1} MiB {:.0} ms, {} link manifests looked for ({} found) {:.0} ms",
        since.elapsed().as_secs_f64(),
        ms(waited) / 1000.0,
        open[0],
        open[2],
        ms(open[1]),
        close[0],
        ms(close[1]),
        stat[0],
        stat[2],
        ms(stat[1]),
        list[0],
        list[2],
        ms(list[1]),
        read[0],
        read[2] as f64 / 1048576.0,
        ms(read[1]),
        if read[1] > 0 {
            read[2] as f64 / 1048576.0 / (read[1] as f64 / 1e9)
        } else {
            0.0
        },
        write[0],
        write[2] as f64 / 1048576.0,
        ms(write[1]),
        probe[0],
        probe[2],
        ms(probe[1]),
    ))
}

static BOOT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// When counting started (`install`).
pub(crate) fn boot() -> Instant {
    *BOOT.get_or_init(Instant::now)
}
