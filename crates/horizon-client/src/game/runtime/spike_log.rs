//! `HORIZON_SPIKE_LOG=MS`: a log line for every frame that took `MS` or more, with what the frame
//! spent its time in (the render thread's spans, wall time), and every 10 s a count of the slow
//! frames. For finding hitches on a console, where no capture can be read back live.
use horizon_client::spans::Span;
use std::cell::RefCell;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// How often the slow frames are counted up.
const WINDOW: Duration = Duration::from_secs(10);
/// Spans shorter than this are left out of a slow frame's line.
const SPAN_MS: f32 = 1.0;
/// The most spans a slow frame's line names.
const SPANS_NAMED: usize = 10;

/// The threshold in milliseconds, if `HORIZON_SPIKE_LOG` is set.
static THRESHOLD: LazyLock<Option<f32>> = LazyLock::new(|| {
    let ms = std::env::var("HORIZON_SPIKE_LOG").ok()?.trim().parse::<f32>().ok()?;
    log::info!("[Spike] logging frames of {ms} ms or more (HORIZON_SPIKE_LOG)");
    Some(ms)
});

pub(crate) fn threshold() -> Option<f32> {
    *THRESHOLD
}

/// What a frame did besides its spans.
pub(crate) struct Facts {
    /// From the start of the last frame to the start of this one.
    pub since_last_ms: f32,
    /// This frame's own time, from its start to after it was presented.
    pub frame_ms: f32,
    /// Of `frame_ms`, waiting for the GPU or a free image.
    pub wait_ms: f32,
    pub gpu_ms: Option<f32>,
    pub uploads: usize,
    pub streaming: usize,
}

struct State {
    frame: u64,
    threads: u32,
    window_start: Instant,
    window_frames: u32,
    /// Frames of 34, 50 and 100 ms or more (start to start) in the window.
    over: [u32; 3],
    max_ms: f32,
    window_threads: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub(crate) fn frame(facts: &Facts, spans: &[Span]) {
    let Some(limit) = threshold() else {
        return;
    };
    #[cfg(target_os = "horizon")]
    let threads = horizon_nx::threads_created();
    #[cfg(not(target_os = "horizon"))]
    let threads = 0u32;
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let s = state.get_or_insert_with(|| State {
            frame: 0,
            threads,
            window_start: Instant::now(),
            window_frames: 0,
            over: [0; 3],
            max_ms: 0.0,
            window_threads: threads,
        });
        s.frame += 1;
        let new_threads = threads.wrapping_sub(s.threads);
        s.threads = threads;
        let ms = facts.since_last_ms;
        s.window_frames += 1;
        s.max_ms = s.max_ms.max(ms);
        for (count, at) in s.over.iter_mut().zip([34.0, 50.0, 100.0]) {
            *count += u32::from(ms >= at);
        }
        if ms >= limit || facts.frame_ms >= limit {
            let mut named: Vec<&Span> = spans.iter().filter(|s| s.ms >= SPAN_MS && s.depth <= 2).collect();
            named.sort_by(|a, b| b.ms.total_cmp(&a.ms));
            named.truncate(SPANS_NAMED);
            let mut text = String::new();
            for span in named {
                use std::fmt::Write;
                let _ = write!(text, " {}{}:{:.1}", "/".repeat(span.depth.into()), span.name, span.ms);
            }
            log::info!(
                "[Spike] frame {}: {ms:.1} ms since the last; this frame {:.1} ms ({:.1} waiting), GPU {}; {new_threads} threads created; uploads {}, streaming {}; spans{text}",
                s.frame,
                facts.frame_ms,
                facts.wait_ms,
                facts.gpu_ms.map_or("-".to_owned(), |g| format!("{g:.1} ms")),
                facts.uploads,
                facts.streaming,
            );
        }
        if s.window_start.elapsed() >= WINDOW {
            log::info!(
                "[Spikes] last {:.1} s: {} frames, max {:.1} ms; 34 ms or more {}, 50 ms or more {}, 100 ms or more {}; {} threads created",
                s.window_start.elapsed().as_secs_f32(),
                s.window_frames,
                s.max_ms,
                s.over[0],
                s.over[1],
                s.over[2],
                threads.wrapping_sub(s.window_threads),
            );
            s.window_start = Instant::now();
            s.window_frames = 0;
            s.over = [0; 3];
            s.max_ms = 0.0;
            s.window_threads = threads;
        }
    });
}
