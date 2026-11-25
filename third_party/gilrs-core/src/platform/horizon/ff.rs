// Copyright 2016-2018 Mateusz Sieczko and other GilRs Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Rumble through libnx (`switch/services/hid.h`), addressed by the same npad slots as input.
//! Standard gilrs effects use fixed frequencies. HD envelopes use the same device writer but
//! have their own timer: the 50 ms gilrs mixer tick is too coarse for a 15 ms HD tick.

use super::gamepad::NPAD_IDS;
use std::sync::{Condvar, Mutex};
use std::thread::{self, Builder, JoinHandle};
use std::time::{Duration, Instant};

/// libnx `HidVibrationValue`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct VibrationValue {
    amp_low: f32,
    freq_low: f32,
    amp_high: f32,
    freq_high: f32,
}

extern "C" {
    fn hidGetNpadStyleSet(id: u32) -> u32;
    fn hidInitializeVibrationDevices(handles: *mut u32, total_handles: i32, id: u32, style: u32) -> u32;
    fn hidSendVibrationValues(handles: *const u32, values: *const VibrationValue, count: i32) -> u32;
}

/// libnx `HidNpadStyleTag`s and how many vibration handles each takes.
const STYLES: [(u32, i32); 5] = [
    (1 << 0, 2), // FullKey
    (1 << 1, 2), // Handheld
    (1 << 2, 2), // JoyDual
    (1 << 3, 1), // JoyLeft
    (1 << 4, 1), // JoyRight
];
/// The bands' frequencies libnx's own vibration example uses.
const LOW_HZ: f32 = 160.0;
const HIGH_HZ: f32 = 320.0;
const STILL: VibrationValue = VibrationValue {
    amp_low: 0.0,
    freq_low: LOW_HZ,
    amp_high: 0.0,
    freq_high: HIGH_HZ,
};

#[derive(Debug)]
pub struct Device {
    npad_id: u32,
    last: VibrationValue,
    /// Style and handle count for this slot's current devices.
    target: Option<(u32, i32)>,
    handles: [u32; 2],
}

impl Device {
    pub(super) const fn new(npad_id: u32) -> Self {
        Device {
            npad_id,
            last: STILL,
            target: None,
            handles: [0; 2],
        }
    }

    /// Sets magnitude for strong and weak ff motors. The values stay until the next call, which
    /// gilrs makes as effects start, change and stop.
    pub fn set_ff_state(&mut self, strong: u16, weak: u16, _min_duration: Duration) {
        self.send(VibrationValue {
            amp_low: f32::from(strong) / f32::from(u16::MAX),
            freq_low: LOW_HZ,
            amp_high: f32::from(weak) / f32::from(u16::MAX),
            freq_high: HIGH_HZ,
        });
    }

    fn send(&mut self, value: VibrationValue) {
        // In particular, gilrs can keep ticking after service shutdown. An idle device
        // must not even query HID. For silence, use the handles we actually energized.
        if self.last == value {
            return;
        }
        let previous = std::mem::replace(&mut self.last, value);
        if value == STILL {
            if let Some((_, count)) = self.target {
                self.write(STILL, count);
            }
            return;
        }
        // SAFETY: this Device was built with a configured npad id.
        let style_set = unsafe { hidGetNpadStyleSet(self.npad_id) };
        let target = STYLES.into_iter().find(|(style, _)| style_set & style != 0);
        if self.target != target {
            if let Some((_, count)) = self.target.take() {
                if previous != STILL {
                    self.write(STILL, count);
                }
            }
            if let Some((style, count)) = target {
                // SAFETY: up to two handles are written to the array.
                if unsafe {
                    hidInitializeVibrationDevices(self.handles.as_mut_ptr(), count, self.npad_id, style)
                } == 0
                {
                    self.target = target;
                }
            }
        }
        if let Some((_, count)) = self.target {
            self.write(value, count);
        }
    }

    fn write(&self, value: VibrationValue, count: i32) {
        let values = [value; 2];
        // SAFETY: one or two initialized handles and enough values for both.
        unsafe { hidSendVibrationValues(self.handles.as_ptr(), values.as_ptr(), count) };
    }
}

/// One step of an HD envelope: amplitude (0–1) and frequency in Hz for each band.
#[derive(Clone, Copy, Debug)]
pub struct HorizonRumbleStep {
    pub low: (f32, f32),
    pub high: (f32, f32),
    pub millis: u32,
}

impl HorizonRumbleStep {
    fn value(self) -> VibrationValue {
        let amp_low = self.low.0.clamp(0.0, 1.0);
        let amp_high = self.high.0.clamp(0.0, 1.0);
        VibrationValue {
            amp_low,
            freq_low: if amp_low > 0.0 { self.low.1 } else { LOW_HZ },
            amp_high,
            freq_high: if amp_high > 0.0 { self.high.1 } else { HIGH_HZ },
        }
    }

    fn duration(self) -> Duration {
        Duration::from_millis(u64::from(self.millis))
    }
}

type Steps = [Option<HorizonRumbleStep>; 4];

struct Timeline {
    steps: Steps,
    index: usize,
    until: Option<Instant>,
}

impl Timeline {
    fn new(steps: Steps) -> Self {
        Self {
            steps,
            index: 0,
            until: None,
        }
    }

    fn at(&mut self, now: Instant) -> Option<(VibrationValue, Instant)> {
        let mut step = (*self.steps.get(self.index)?)?;
        let mut until = self.until.unwrap_or_else(|| now + step.duration());
        while now >= until {
            self.index += 1;
            step = (*self.steps.get(self.index)?)?;
            until += step.duration();
        }
        self.until = Some(until);
        Some((step.value(), until))
    }
}

#[derive(Default)]
enum Change {
    #[default]
    None,
    Play(Steps),
    Stop,
}

#[derive(Default)]
struct Request {
    change: Change,
    /// Forget old handles even if the disconnect and next play arrived together.
    disconnected: bool,
}

struct Slot {
    request: Request,
    generation: u64,
}

impl Slot {
    const fn new() -> Self {
        Self {
            request: Request {
                change: Change::None,
                disconnected: false,
            },
            generation: 0,
        }
    }

    fn play(&mut self, steps: Steps) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.request.change = Change::Play(steps);
        self.generation
    }

    fn stop(&mut self, generation: u64) -> bool {
        if self.generation != generation {
            return false;
        }
        self.request.change = Change::Stop;
        true
    }
}

struct Player {
    slots: [Slot; NPAD_IDS.len()],
    worker: Option<JoinHandle<()>>,
    shutdown: bool,
}

static PLAYER: Mutex<Player> = Mutex::new(Player {
    slots: [const { Slot::new() }; NPAD_IDS.len()],
    worker: None,
    shutdown: false,
});
static WAKE: Condvar = Condvar::new();

/// Owns a Switch HD envelope playing on one gilrs gamepad id. Dropping it stops that
/// envelope unless a newer effect has already replaced it on the same pad.
#[derive(Debug)]
pub struct HorizonRumble {
    slot: usize,
    generation: u64,
}

impl HorizonRumble {
    /// Play one to four steps, ending at the first `None`. Use the `GamepadId` from this
    /// controller's gilrs input. Do not drive the same pad with gilrs's standard mixer.
    /// The host must arrange for [`Self::shutdown`] to run before it closes HID.
    pub fn play(id: usize, steps: [Option<HorizonRumbleStep>; 4]) -> Option<Self> {
        if id >= NPAD_IDS.len() || steps[0].is_none() {
            return None;
        }
        let mut player = PLAYER.lock().unwrap_or_else(|e| e.into_inner());
        if player.shutdown {
            return None;
        }
        if player.worker.is_none() {
            match Builder::new()
                .name("HD rumble".into())
                .stack_size(64 * 1024)
                .spawn(run)
            {
                Ok(worker) => player.worker = Some(worker),
                Err(error) => {
                    warn!("Couldn't start Switch HD rumble: {error}");
                    return None;
                }
            }
        }
        let generation = player.slots[id].play(steps);
        WAKE.notify_one();
        Some(Self {
            slot: id,
            generation,
        })
    }

    /// No effect may carry over to the next controller connected to this gilrs slot.
    pub fn disconnected(id: usize) {
        let mut player = PLAYER.lock().unwrap_or_else(|e| e.into_inner());
        if player.worker.is_none() {
            return;
        }
        if let Some(slot) = player.slots.get_mut(id) {
            slot.request = Request {
                change: Change::Stop,
                disconnected: true,
            };
            WAKE.notify_one();
        }
    }

    /// Stop every HD envelope and join the timer before the host shuts down libnx HID.
    pub fn shutdown() {
        let mut player = PLAYER.lock().unwrap_or_else(|e| e.into_inner());
        player.shutdown = true;
        let worker = player.worker.take();
        drop(player);
        if let Some(worker) = worker {
            WAKE.notify_one();
            // libnx can run exit hooks on an aborting worker. Never join ourselves.
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for HorizonRumble {
    fn drop(&mut self) {
        let mut player = PLAYER.lock().unwrap_or_else(|e| e.into_inner());
        if !player.shutdown && player.slots[self.slot].stop(self.generation) {
            WAKE.notify_one();
        }
    }
}

fn run() {
    // The timer alone owns the handles. In particular never hold PLAYER during libnx calls:
    // a fatal libnx error can invoke the application's shutdown hook on this very thread.
    let mut devices = NPAD_IDS.map(Device::new);
    let mut timelines = [const { None }; NPAD_IDS.len()];
    let mut deadline: Option<Instant> = None;
    loop {
        let mut player = PLAYER.lock().unwrap_or_else(|e| e.into_inner());
        while !player.shutdown
            && player
                .slots
                .iter()
                .all(|slot| matches!(slot.request.change, Change::None))
        {
            player = if let Some(deadline) = deadline {
                let delay = deadline.saturating_duration_since(Instant::now());
                if delay.is_zero() {
                    break;
                }
                WAKE.wait_timeout(player, delay)
                    .unwrap_or_else(|e| e.into_inner())
                    .0
            } else {
                WAKE.wait(player).unwrap_or_else(|e| e.into_inner())
            };
        }
        if player.shutdown {
            drop(player);
            for device in &mut devices {
                device.send(STILL);
            }
            return;
        }
        let requests: [_; NPAD_IDS.len()] =
            std::array::from_fn(|id| std::mem::take(&mut player.slots[id].request));
        drop(player);

        let now = Instant::now();
        deadline = None;
        for ((request, device), timeline) in requests.into_iter().zip(&mut devices).zip(&mut timelines)
        {
            if request.disconnected {
                device.send(STILL);
                device.target = None;
            }
            match request.change {
                Change::Play(steps) => *timeline = Some(Timeline::new(steps)),
                Change::Stop => *timeline = None,
                Change::None => {}
            }
            if let Some((value, until)) = timeline.as_mut().and_then(|t| t.at(now)) {
                device.send(value);
                deadline = Some(deadline.map_or(until, |next| next.min(until)));
            } else {
                *timeline = None;
                device.send(STILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_advances_on_absolute_deadlines() {
        let step = HorizonRumbleStep {
            low: (0.3, 100.0),
            high: (0.4, 200.0),
            millis: 15,
        };
        let tail = HorizonRumbleStep { low: (0.1, 90.0), ..step };
        let mut timeline = Timeline::new([Some(step), Some(tail), None, None]);
        let start = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(timeline.at(start), Some((step.value(), start + ms(15))));
        assert_eq!(
            timeline.at(start + ms(20)),
            Some((tail.value(), start + ms(30)))
        );
        assert_eq!(timeline.at(start + ms(30)), None);
    }

    #[test]
    fn stopping_one_envelope_does_not_erase_a_replacement_or_another_slot() {
        let steps = [
            Some(HorizonRumbleStep {
                low: (0.3, 100.0),
                high: (0.4, 200.0),
                millis: 15,
            }),
            None,
            None,
            None,
        ];
        let mut slots = [Slot::new(), Slot::new()];
        let old = slots[0].play(steps);
        slots[1].play(steps);
        let new = slots[0].play(steps);
        assert!(!slots[0].stop(old));
        assert!(matches!(slots[0].request.change, Change::Play(_)));
        assert!(slots[0].stop(new));
        assert!(matches!(slots[0].request.change, Change::Stop));
        assert!(matches!(slots[1].request.change, Change::Play(_)));
    }

    #[test]
    fn unpowered_bands_have_canonical_frequencies() {
        let step = HorizonRumbleStep {
            low: (0.0, 100.0),
            high: (0.0, 200.0),
            millis: 15,
        };
        assert_eq!(step.value(), STILL);
    }
}
