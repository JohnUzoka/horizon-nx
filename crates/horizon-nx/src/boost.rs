//! O8: a CPU boost while the game loads. libnx's `appletSetCpuBoostMode(ApmCpuBoostMode_FastLoad)`
//! raises the CPU from 1020 to 1785 MHz and throttles the GPU to its minimum
//! (switch/services/apm.h). Loading on the Switch is CPU-bound: in log14 the two big loader threads
//! were on a core 88% and 90% of their lives, and with the boost (log16) resources loaded in 88 s
//! instead of 174 s. The GPU runs slower during FastLoad, so menus can feel less responsive.
//! Launch drops the CPU boost when the title becomes interactive. Opening a world restores
//! the boost until the world shows or the open is cancelled: through the server's start (the
//! benchmark's synchronous open too) and single player's loading screens (held until the spawn view
//! is in with `HORIZON_SPAWN_HOLD=1`). Gameplay never requests a sustained CPU overclock.
//! HOME/sleep temporarily releases the boost, and exit restores normal clocks.
//! `HORIZON_NX_CPU_BOOST=0` in env.txt keeps the normal clocks throughout, to compare, and
//! `HORIZON_NX_WORLD_BOOST=0` only while a world opens. Only an Application (title takeover) may
//! boost; an applet (album) logs why and loads at the normal clocks.
//! Explicit gameplay clock preferences take precedence over FastLoad.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::LazyLock;
use std::time::Instant;

use crate::logging::write_log_line;

/// libnx `ApmCpuBoostMode` (switch/services/apm.h): only these two are allowed with
/// `appletSetCpuBoostMode`.
const BOOST_NORMAL: u32 = 0;
const BOOST_FAST_LOAD: u32 = 1;
/// libnx `AppletType_Application` (switch/services/applet.h).
const APPLET_TYPE_APPLICATION: i32 = 0;

unsafe extern "C" {
    fn appletGetAppletType() -> i32;
    fn appletSetCpuBoostMode(mode: u32) -> u32;
}

/// The boost is on: set by `start_from_env` or `loading_world`, cleared by `end`.
static BOOSTED: AtomicBool = AtomicBool::new(false);
/// When the launch boost went on (forced by `start_from_env` as it does): the clock the rest of
/// this file's times count from.
static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);
/// What the boost is on for: the launch (ends at the world) or a world's
/// loading screens (end at the world, or once those screens are gone).
const FOR_LAUNCH: u8 = 1;
const FOR_WORLD: u8 = 2;
static REASON: AtomicU8 = AtomicU8::new(0);
/// The process may boost: an Application, and not `HORIZON_NX_CPU_BOOST=0`.
static ALLOWED: AtomicBool = AtomicBool::new(false);
/// `HORIZON_NX_WORLD_BOOST=0` opens worlds at the normal clocks.
static WORLD_BOOST: AtomicBool = AtomicBool::new(true);
/// When the boost last went on (ms after `STARTED`), for the line that ends it.
static SINCE_MS: AtomicU64 = AtomicU64::new(0);
/// Loop iterations since a loading screen was last drawn, counted while boosted for a world.
static ITERATIONS_WITHOUT_LOADING: AtomicU32 = AtomicU32::new(0);
/// That many iterations (about 2 s at 60 fps) without a loading screen: the open was cancelled.
const LOADING_GONE: u32 = 120;

/// Switches the boost on for the loading ahead, unless `HORIZON_NX_CPU_BOOST=0`. `init` calls it
/// once, after env.txt is applied.
pub(crate) fn start_from_env() {
    if std::env::var("HORIZON_NX_WORLD_BOOST").is_ok_and(|v| v.trim() == "0") {
        WORLD_BOOST.store(false, Ordering::Relaxed);
        write_log_line(format_args!(
            "[Boost] no CPU boost while a world opens (HORIZON_NX_WORLD_BOOST=0)\n"
        ));
    }
    if std::env::var("HORIZON_NX_CPU_BOOST").is_ok_and(|v| v.trim() == "0") {
        write_log_line(format_args!("[Boost] CPU boost off (HORIZON_NX_CPU_BOOST=0)\n"));
        return;
    }
    // SAFETY: plain libnx applet calls; libnx sets up the applet service before `main`.
    let kind = unsafe { appletGetAppletType() };
    if kind != APPLET_TYPE_APPLICATION {
        write_log_line(format_args!(
            "[Boost] no CPU boost: running as applet type {kind}, not an Application; loading at the normal clocks\n"
        ));
        return;
    }
    LazyLock::force(&STARTED);
    if begin(FOR_LAUNCH) {
        ALLOWED.store(true, Ordering::Relaxed);
    }
}

fn elapsed_ms() -> u64 {
    u64::try_from(STARTED.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn begin(reason: u8) -> bool {
    if crate::clocks::preferences_requested() || !crate::clocks::prepare_fastload() {
        return false;
    }
    // SAFETY: appletInitialize has registered an application with the applet service.
    let rc = unsafe { appletSetCpuBoostMode(BOOST_FAST_LOAD) };
    if rc != 0 {
        write_log_line(format_args!(
            "[Boost] FastLoad failed: 0x{rc:x}; keeping normal clocks\n"
        ));
        return false;
    }
    SINCE_MS.store(elapsed_ms(), Ordering::Relaxed);
    REASON.store(reason, Ordering::Relaxed);
    BOOSTED.store(true, Ordering::SeqCst);
    write_log_line(format_args!("[Boost] CPU boost on until world entry or cancelled load\n"));
    true
}

pub(crate) fn release_for_clock_preferences() -> bool {
    end("explicit Switch clock preferences")
}

pub(crate) fn fastload_active() -> bool {
    BOOSTED.load(Ordering::Relaxed)
}

/// Restore normal GPU clocks for the interactive title. Repeated frames are cheap once off.
pub fn title_ready() {
    end("the title is interactive");
}

/// The game draws a world's loading screen (opening a world, its server starting, "Loading
/// terrain"), once a frame. Opening a world is CPU-bound too (log22: 170 s at the normal clocks),
/// so the boost goes back on for it, and off at the world (`horizon_nx_entered_world`) or once
/// these screens are gone for [`LOADING_GONE`] loop iterations (a cancelled open). Input doesn't end
/// it: a button still held from picking the world would, and the boost would flicker. A launch
/// boost still on (the world picked on the startup title) carries on as this one.
pub fn loading_world() {
    if !WORLD_BOOST.load(Ordering::Relaxed) {
        end("world loading boost disabled");
        return;
    }
    if !ALLOWED.load(Ordering::Relaxed) {
        return;
    }
    if crate::clocks::preferences_requested() {
        return;
    }
    ITERATIONS_WITHOUT_LOADING.store(0, Ordering::Relaxed);
    if BOOSTED.load(Ordering::Relaxed) {
        REASON.store(FOR_WORLD, Ordering::Relaxed);
        return;
    }
    if !begin(FOR_WORLD) {
        WORLD_BOOST.store(false, Ordering::Relaxed);
    }
}

/// Detect a cancelled world open. Ordinary title input does not change the launch clocks.
pub(crate) fn tick_loading() {
    if !BOOSTED.load(Ordering::Relaxed) {
        return;
    }
    if REASON.load(Ordering::Relaxed) == FOR_WORLD {
        let iterations = ITERATIONS_WITHOUT_LOADING.fetch_add(1, Ordering::Relaxed) + 1;
        if iterations >= LOADING_GONE {
            end("the world's loading screens are gone");
        }
    }
}

/// Release FastLoad while HOME or sleep owns the display. Only a load that was actually
/// boosted is eligible to resume when focus returns.
pub(crate) fn pause_loading() -> Option<u8> {
    if !BOOSTED.load(Ordering::Relaxed) {
        return None;
    }
    let reason = REASON.load(Ordering::Relaxed);
    end("application in background").then_some(reason)
}

pub(crate) fn resume_loading(reason: Option<u8>) {
    if let Some(reason) = reason {
        begin(reason);
    }
}

pub(crate) fn on_exit() {
    end("application exiting");
}

/// The winit backend calls this whenever the game grabs the cursor; the first call means the player
/// is in a world, so the clocks go back to normal and the GPU runs at full speed for play. Every
/// later call returns at once. A world's loading screen drawn in the last two loop iterations
/// keeps the boost: the game grabs the cursor once its player exists, while single player's
/// loading screen stays up until the spawn view is in ([`world_entered`] ends it there).
#[unsafe(no_mangle)]
pub extern "C" fn horizon_nx_entered_world() {
    if REASON.load(Ordering::Relaxed) == FOR_WORLD
        && ITERATIONS_WITHOUT_LOADING.load(Ordering::Relaxed) < 2
    {
        return;
    }
    end("the player is in a world");
}

/// The game shows the world: its loading screens are done.
pub fn world_entered() {
    end("the player is in a world");
}

/// Ends the boost (once) and says why.
fn end(why: &str) -> bool {
    if !BOOSTED.swap(false, Ordering::SeqCst) {
        return true;
    }
    // SAFETY: as in `start_from_env`.
    let rc = unsafe { appletSetCpuBoostMode(BOOST_NORMAL) };
    if rc != 0 {
        BOOSTED.store(true, Ordering::SeqCst);
    }
    let secs = elapsed_ms().saturating_sub(SINCE_MS.load(Ordering::Relaxed)) as f64 / 1000.0;
    write_log_line(format_args!(
        "[Boost] CPU boost off: {why}, {secs:.1} s after it went on (result 0x{rc:x})\n"
    ));
    rc == 0
}
