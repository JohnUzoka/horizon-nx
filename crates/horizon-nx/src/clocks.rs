//! Optional application clocks, following oot3d Switch's Video performance settings
//! (606f98b3, 80431850, 5805f878). CPU uses a short clkrst/PCV session; GPU and RAM
//! share one official handheld APM configuration. The legacy diagnostic can also set docked APM.
//!
//! The applet pump owns changes. It applies them only in confirmed NoSuspend focus, restores
//! before we stop pumping messages, and never leaves a clock-service session open over sleep.
//! APM can reset the CPU asynchronously, so settle first, then ask for CPU once. A rate
//! changing under us outside an applet transition is somebody else's to manage, not ours to fight.

use crate::logging::write_log_line;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

const CPU: u8 = 1;
const GPU: u8 = 2;
const RAM: u8 = 4;
const DIAGNOSTIC: u8 = 8;
const CPU_STANDARD: u32 = 1_020_000_000;
const CPU_HIGH: u32 = 1_224_000_000;
const APPLET_TYPE_APPLICATION: i32 = 0;
const OPERATION_HANDHELD: i32 = 0;
const OPERATION_DOCKED: i32 = 1;
const MODE_HANDHELD: i32 = 0;
const MODE_DOCKED: i32 = 1;
const HOS_8: u32 = 8 << 16;
const HOS_12: u32 = 12 << 16;

const SETTLE: Duration = Duration::from_millis(500);
const MONITOR: Duration = Duration::from_secs(2);

/// switchbrew "PTM services", PerformanceConfiguration: the clocks behind each value.
const CONFIGURATIONS: [(u32, &str); 16] = [
    (0x0001_0000, "CPU 1020 / GPU 384 / memory 1600 MHz"),
    (0x0001_0001, "CPU 1020 / GPU 768 / memory 1600 MHz"),
    (0x0001_0002, "CPU 1224 / GPU 691.2 / memory 1600 MHz"),
    (0x0002_0000, "CPU 1020 / GPU 230.4 / memory 1600 MHz"),
    (0x0002_0001, "CPU 1020 / GPU 307.2 / memory 1600 MHz"),
    (0x0002_0002, "CPU 1224 / GPU 230.4 / memory 1600 MHz"),
    (0x0002_0003, "CPU 1020 / GPU 307.2 / memory 1331.2 MHz"),
    (0x0002_0004, "CPU 1020 / GPU 384 / memory 1331.2 MHz"),
    (0x0002_0005, "CPU 1020 / GPU 307.2 / memory 1065.6 MHz"),
    (0x0002_0006, "CPU 1020 / GPU 384 / memory 1065.6 MHz"),
    (0x9222_0007, "CPU 1020 / GPU 460.8 / memory 1600 MHz"),
    (0x9222_0008, "CPU 1020 / GPU 460.8 / memory 1331.2 MHz"),
    (0x9222_0009, "CPU 1785 / GPU 76.8 / memory 1600 MHz"),
    (0x9222_000A, "CPU 1785 / GPU 76.8 / memory 1331.2 MHz"),
    (0x9222_000B, "CPU 1020 / GPU 76.8 / memory 1600 MHz"),
    (0x9222_000C, "CPU 1020 / GPU 76.8 / memory 1331.2 MHz"),
];

/// libnx Service from `switch/sf/service.h`; `ClkrstSession` contains one.
#[repr(C)]
#[derive(Default)]
struct Service {
    session: u32,
    own_handle: u32,
    object_id: u32,
    pointer_buffer_size: u16,
}

const _: () = assert!(core::mem::size_of::<Service>() == 16);

#[derive(Clone, Copy)]
struct Override {
    prior: u32,
    requested: u32,
}

#[derive(Clone, Copy)]
struct ApmOverride {
    mode: i32,
    change: Override,
}

struct State {
    selected: u8,
    focused: bool,
    exiting: bool,
    cpu: Option<Override>,
    apm: Option<ApmOverride>,
    apm_due: Option<Instant>,
    cpu_due: Option<Instant>,
    monitor_due: Option<Instant>,
}

static PREFERENCES: AtomicU8 = AtomicU8::new(0);
static ENV_DIAGNOSTIC: AtomicU8 = AtomicU8::new(0);
static USED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<State> = Mutex::new(State {
    selected: 0,
    focused: false,
    exiting: false,
    cpu: None,
    apm: None,
    apm_due: None,
    cpu_due: None,
    monitor_due: None,
});

unsafe extern "C" {
    fn appletGetAppletType() -> i32;
    fn appletGetOperationMode() -> i32;
    fn appletGetPerformanceMode() -> i32;
    fn apmSetPerformanceConfiguration(mode: i32, configuration: u32) -> u32;
    fn apmGetPerformanceConfiguration(mode: i32, configuration: *mut u32) -> u32;
    fn appletGetCurrentPerformanceConfiguration(configuration: *mut u32) -> u32;
    fn hosversionGet() -> u32;
    fn hosversionIsAtmosphere() -> bool;
    fn pcvInitialize() -> u32;
    fn pcvExit();
    fn pcvGetClockRate(module: u32, hz: *mut u32) -> u32;
    fn pcvSetClockRate(module: u32, hz: u32) -> u32;
    fn clkrstInitialize() -> u32;
    fn clkrstExit();
    fn clkrstOpenSession(session: *mut Service, module: u32, access: u32) -> u32;
    fn clkrstCloseSession(session: *mut Service);
    fn clkrstGetClockRate(session: *mut Service, hz: *mut u32) -> u32;
    fn clkrstSetClockRate(session: *mut Service, hz: u32) -> u32;
    fn smGetServiceSession() -> *const Service;
    fn svcSendSyncRequest(handle: u32) -> u32;
}

fn describe(configuration: u32) -> String {
    let clocks = CONFIGURATIONS
        .iter()
        .find(|(value, _)| *value == configuration)
        .map_or("clocks unknown", |(_, clocks)| clocks);
    format!("0x{configuration:08x} ({clocks})")
}

fn is_application() -> bool {
    // SAFETY: a plain libnx call; libnx sets up the applet service before `main`.
    unsafe { appletGetAppletType() == APPLET_TYPE_APPLICATION }
}

/// One mode's configuration, as apm reports it.
fn configuration(mode: i32) -> String {
    let mut value = 0;
    // SAFETY: apm is set up for an Application (checked by the callers); `value` is written.
    let rc = unsafe { apmGetPerformanceConfiguration(mode, &mut value) };
    if rc == 0 {
        describe(value)
    } else {
        format!("unknown (apmGetPerformanceConfiguration: 0x{rc:x})")
    }
}

/// Request the three independent Switch video preferences. The next applet pump applies them.
pub fn set_preferences(cpu: bool, gpu: bool, memory: bool) {
    let selected = (u8::from(cpu) * CPU) | (u8::from(gpu) * GPU) | (u8::from(memory) * RAM);
    PREFERENCES.store(selected, Ordering::Relaxed);
    if selected != 0 {
        USED.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn preferences_requested() -> bool {
    PREFERENCES.load(Ordering::Relaxed) != 0
}

pub(crate) fn start_from_env() {
    log_clocks("at launch");
    let Ok(value) = std::env::var("HORIZON_NX_GPU_CLOCKS") else {
        return;
    };
    let diagnostic = match value.trim() {
        "system" => return,
        "fast" => DIAGNOSTIC | GPU,
        "max" => DIAGNOSTIC | GPU | RAM,
        other => {
            write_log_line(format_args!(
                "[Clocks] HORIZON_NX_GPU_CLOCKS={other:?} isn't fast, max or system; leaving clocks alone"
            ));
            return;
        }
    };
    ENV_DIAGNOSTIC.store(diagnostic, Ordering::Relaxed);
    USED.store(true, Ordering::Relaxed);
}

pub(crate) fn needs_focus_wait() -> bool {
    preferences_requested() || ENV_DIAGNOSTIC.load(Ordering::Relaxed) != 0
}

/// FastLoad owns APM while loading. Any previously requested clock must be restored first.
pub(crate) fn prepare_fastload() -> bool {
    if !USED.load(Ordering::Relaxed) {
        return true;
    }
    let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    state.selected = 0;
    state.restore();
    !state.exiting && !state.owns_clocks()
}

fn rc(result: u32) -> Result<(), u32> {
    if result == 0 { Ok(()) } else { Err(result) }
}

/// Atmosphère's read-only SM HasService extension (65100). Do not use GetService as a presence
/// check: SM can defer that command indefinitely for a service that isn't registered.
fn registered(name: [u8; 8]) -> Option<bool> {
    // SAFETY: SM is initialized by libnx before main. This is our thread's AArch64 HIPC buffer.
    // These are descriptor-free requests; reject responses with descriptors or malformed status.
    unsafe {
        let sm = smGetServiceSession().as_ref()?;
        if sm.session == 0 || sm.object_id != 0 {
            return None;
        }
        let tls: *mut u32;
        core::arch::asm!("mrs {}, tpidrro_el0", out(reg) tls, options(nomem, nostack));
        let (first, last) = (
            u32::from_le_bytes(name[..4].try_into().ok()?),
            u32::from_le_bytes(name[4..].try_into().ok()?),
        );
        let tipc = hosversionGet() >= HOS_12;
        if tipc {
            // HIPC header, then the eight-byte service name. TIPC adds 16 to the command ID.
            let request = [65100 + 16, 2, first, last];
            core::ptr::copy_nonoverlapping(request.as_ptr(), tls, request.len());
        } else {
            // HIPC request 4, 10 data words (including CMIF alignment padding).
            let request = [4, 10, 0, 0, 0x4943_4653, 0, 65100, 0, first, last, 0, 0];
            core::ptr::copy_nonoverlapping(request.as_ptr(), tls, request.len());
        }
        rc(svcSendSyncRequest(sm.session)).ok()?;
        let (header0, header1) = (tls.read(), tls.add(1).read());
        if header0 >> 16 != 0 || header1 >> 10 != 0 {
            return None;
        }
        let (result, answer) = if tipc && header1 >= 2 {
            (tls.add(2).read(), tls.add(3).read())
        } else if !tipc && header1 >= 7 && tls.add(4).read() == 0x4f43_4653 {
            (tls.add(6).read(), tls.add(8).read())
        } else {
            return None;
        };
        if result != 0 {
            return None;
        }
        match answer & 0xff {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

/// Guard against missing services and emulators before issuing any clock writes.
fn supported() -> bool {
    if !is_application() || !unsafe { hosversionIsAtmosphere() } {
        write_log_line(format_args!(
            "[Clocks] optional clocks require an Atmosphère application with title takeover"
        ));
        return false;
    }
    match registered(*b"sys:clk\0") {
        Some(false) => true,
        Some(true) => {
            write_log_line(format_args!("[Clocks] sys-clk is registered; leaving clocks to it"));
            false
        }
        None => {
            write_log_line(format_args!("[Clocks] could not check for sys-clk; leaving clocks alone"));
            false
        }
    }
}

/// A single read and optional compare-and-set, with a short-lived CPU session. The guard check
/// happens in the same session as the write. No raw clock modifications use an inferred rate.
fn cpu_clock(change: Option<Override>) -> Result<u32, u32> {
    // SAFETY: libnx initializes the given service/session; output fields are valid Rust storage.
    unsafe {
        let mut hz = 0;
        let result = if hosversionGet() >= HOS_8 {
            rc(clkrstInitialize())?;
            let mut session = Service::default();
            let result = rc(clkrstOpenSession(&mut session, 0x4000_0001, 3)).and_then(|()| {
                let result = rc(clkrstGetClockRate(&mut session, &mut hz)).and_then(|()| {
                    if let Some(change) = change.filter(|change| hz == change.prior) {
                        rc(clkrstSetClockRate(&mut session, change.requested))?;
                    }
                    Ok(())
                });
                clkrstCloseSession(&mut session);
                result
            });
            clkrstExit();
            result
        } else {
            rc(pcvInitialize())?;
            let result = rc(pcvGetClockRate(0, &mut hz)).and_then(|()| {
                if let Some(change) = change.filter(|change| hz == change.prior) {
                    rc(pcvSetClockRate(0, change.requested))?;
                }
                Ok(())
            });
            pcvExit();
            result
        };
        result.map(|()| hz)
    }
}

fn apm_configuration(mode: i32) -> Result<u32, u32> {
    let mut value = 0;
    rc(unsafe { apmGetPerformanceConfiguration(mode, &mut value) })?;
    Ok(value)
}

fn set_apm_configuration(mode: i32, value: u32) -> Result<(), u32> {
    rc(unsafe { apmSetPerformanceConfiguration(mode, value) })
}

/// Only use configurations where an unchecked control retains the prior domain's rate.
fn handheld_target(prior: u32, gpu: bool, memory: bool) -> Option<u32> {
    let (prior_gpu, prior_memory) = match prior {
        0x0002_0000 => (230, 1600),
        0x0002_0001 => (307, 1600),
        0x0002_0003 => (307, 1331),
        0x0002_0004 => (384, 1331),
        0x0002_0005 => (307, 1065),
        0x0002_0006 => (384, 1065),
        0x9222_0007 => (460, 1600),
        0x9222_0008 => (460, 1331),
        _ => return None,
    };
    Some(match (if gpu { 460 } else { prior_gpu }, if memory { 1600 } else { prior_memory }) {
        (230, 1600) => 0x0002_0000,
        (307, 1600) => 0x0002_0001,
        (307, 1331) => 0x0002_0003,
        (384, 1331) => 0x0002_0004,
        (307, 1065) => 0x0002_0005,
        (384, 1065) => 0x0002_0006,
        (460, 1600) => 0x9222_0007,
        (460, 1331) => 0x9222_0008,
        _ => return None,
    })
}

impl State {
    fn apply_apm(&mut self) -> Result<(), u32> {
        let mode = match unsafe { appletGetOperationMode() } {
            OPERATION_HANDHELD => MODE_HANDHELD,
            OPERATION_DOCKED if self.selected & DIAGNOSTIC != 0 => MODE_DOCKED,
            _ => return Ok(()),
        };
        let mut prior = apm_configuration(mode)?;
        if prior == 0 {
            // No per-mode request yet. Preserve the real configuration, not a guessed default.
            rc(unsafe { appletGetCurrentPerformanceConfiguration(&mut prior) })?;
        }
        let target = if mode == MODE_DOCKED {
            matches!(prior, 0x0001_0000 | 0x0001_0001).then_some(0x0001_0001)
        } else {
            handheld_target(prior, self.selected & GPU != 0, self.selected & RAM != 0)
        };
        let Some(requested) = target else {
            write_log_line(format_args!(
                "[Clocks] no compatible APM configuration for mode {mode} baseline 0x{prior:08x}"
            ));
            return Ok(());
        };
        if prior != requested {
            set_apm_configuration(mode, requested)?;
            self.apm = Some(ApmOverride { mode, change: Override { prior, requested } });
            write_log_line(format_args!(
                "[Clocks] mode {mode} configuration 0x{prior:08x} -> 0x{requested:08x}"
            ));
        }
        Ok(())
    }

    fn apply_cpu(&mut self) -> Result<(), u32> {
        let service = if unsafe { hosversionGet() } >= HOS_8 { *b"clkrst\0\0" } else { *b"pcv\0\0\0\0\0" };
        if registered(service) != Some(true) {
            write_log_line(format_args!("[Clocks] CPU clock service is unavailable"));
            return Ok(());
        }
        let change = Override { prior: CPU_STANDARD, requested: CPU_HIGH };
        match cpu_clock(Some(change))? {
            CPU_STANDARD => {
                self.cpu = Some(change);
                // Read back once, so an emulator/no-op cannot be mistaken for a successful change.
                if cpu_clock(None)? == CPU_HIGH {
                    write_log_line(format_args!("[Clocks] CPU {CPU_STANDARD} -> {CPU_HIGH} Hz"));
                } else {
                    self.cpu = None;
                    write_log_line(format_args!("[Clocks] CPU clock request did not take effect"));
                }
            }
            rate => write_log_line(format_args!("[Clocks] CPU already at {rate} Hz; left alone")),
        }
        Ok(())
    }

    fn restore(&mut self) {
        self.apm_due = None;
        self.cpu_due = None;
        self.monitor_due = None;
        if let Some(change) = self.cpu {
            let result = cpu_clock(Some(Override { prior: change.requested, requested: change.prior }));
            match result {
                Ok(current) => {
                    self.cpu = None;
                    if current == change.requested {
                        write_log_line(format_args!("[Clocks] CPU restored to {} Hz", change.prior));
                    }
                }
                Err(error) => write_log_line(format_args!("[Clocks] CPU restore failed: 0x{error:x}")),
            }
        }
        if let Some(ApmOverride { mode, change }) = self.apm {
            let result = apm_configuration(mode).and_then(|current| {
                if current == change.requested {
                    set_apm_configuration(mode, change.prior)?;
                    write_log_line(format_args!(
                        "[Clocks] mode {mode} configuration restored to 0x{:08x}", change.prior
                    ));
                }
                Ok(())
            });
            match result {
                Ok(()) => self.apm = None,
                Err(error) => write_log_line(format_args!("[Clocks] APM restore failed: 0x{error:x}")),
            }
        }
    }

    fn owns_clocks(&self) -> bool {
        self.cpu.is_some() || self.apm.is_some()
    }
}

/// Once per applet pump and immediately on focus loss; `focused` includes confirmed NoSuspend.
pub(crate) fn update(focused: bool, mode_changed: bool) {
    if !USED.load(Ordering::Relaxed) {
        return;
    }
    let preferred = PREFERENCES.load(Ordering::Relaxed);
    let selected = if preferred != 0 {
        preferred
    } else if crate::boost::fastload_active() {
        0
    } else {
        ENV_DIAGNOSTIC.load(Ordering::Relaxed)
    };
    let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    if state.exiting {
        return;
    }
    let changed = state.selected != selected || state.focused != focused || mode_changed;
    if changed {
        state.restore();
        state.selected = selected;
        state.focused = focused;
        if selected == 0 || !focused || state.owns_clocks() || !supported() {
            return;
        }
        if preferred != 0 && !crate::boost::release_for_clock_preferences() {
            return;
        }
        state.apm_due = Some(Instant::now() + SETTLE);
    } else if focused && selected != 0 {
        let now = Instant::now();
        if state.apm_due.is_some_and(|due| due <= now) {
            state.apm_due = None;
            if !supported() {
                return;
            }
            if selected & (GPU | RAM) != 0 && let Err(error) = state.apply_apm() {
                write_log_line(format_args!("[Clocks] APM configuration unavailable: 0x{error:x}"));
            }
            if selected & CPU != 0 {
                state.cpu_due = Some(if state.apm.is_some() { now + SETTLE } else { now });
            }
            if state.owns_clocks() {
                state.monitor_due = Some(now + MONITOR);
            }
        }
        if state.cpu_due.is_some_and(|due| due <= now) {
            state.cpu_due = None;
            if supported() {
                if let Err(error) = state.apply_cpu() {
                    write_log_line(format_args!("[Clocks] CPU clock unavailable: 0x{error:x}"));
                }
            } else {
                state.restore();
            }
            if state.owns_clocks() {
                state.monitor_due = Some(now + MONITOR);
            }
        }
        if state.monitor_due.is_some_and(|due| due <= now) {
            if !supported() {
                state.restore();
                return;
            }
            if let Some(change) = state.cpu
                && let Ok(current) = cpu_clock(None)
                && current != change.requested
            {
                state.cpu = None;
                write_log_line(format_args!("[Clocks] CPU changed to {current} Hz; not reapplying"));
            }
            state.monitor_due = state.owns_clocks().then(|| now + MONITOR);
        }
    }
}

pub(crate) fn pause() -> bool {
    if !USED.load(Ordering::Relaxed) {
        return true;
    }
    let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    state.focused = false;
    state.restore();
    !state.owns_clocks()
}

pub(crate) fn on_exit() -> bool {
    let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    state.exiting = true;
    state.restore();
    !state.owns_clocks()
}

/// Logs both APM configurations, which do not include direct CPU overrides.
pub(crate) fn log_clocks(when: &str) {
    if !is_application() {
        return;
    }
    // SAFETY: plain libnx getters of the applet state it keeps.
    let (operation, performance) =
        unsafe { (appletGetOperationMode(), appletGetPerformanceMode()) };
    let mode = if operation == OPERATION_DOCKED { "docked" } else { "handheld" };
    write_log_line(format_args!(
        "[Clocks] {when}: {mode} (performance mode {performance}); handheld configuration {}, docked {}\n",
        configuration(MODE_HANDHELD),
        configuration(MODE_DOCKED),
    ));
}

#[cfg(test)]
mod tests {
    use super::handheld_target;

    #[test]
    fn handheld_stock_controls_are_independent() {
        let stock = 0x0002_0003;
        assert_eq!(handheld_target(stock, false, false), Some(stock));
        assert_eq!(handheld_target(stock, true, false), Some(0x9222_0008));
        assert_eq!(handheld_target(stock, false, true), Some(0x0002_0001));
        assert_eq!(handheld_target(stock, true, true), Some(0x9222_0007));
    }

    #[test]
    fn handheld_clock_requests_never_replace_an_unselected_rate() {
        assert_eq!(handheld_target(0x0002_0001, true, false), Some(0x9222_0007));
        assert_eq!(handheld_target(0x9222_0008, false, true), Some(0x9222_0007));
        assert_eq!(handheld_target(0x0002_0005, true, false), None);
        assert_eq!(handheld_target(0x0002_0004, false, true), None);
        assert_eq!(handheld_target(0x0001_0001, true, true), None);
    }
}
