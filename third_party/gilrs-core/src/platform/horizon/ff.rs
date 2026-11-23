// Copyright 2016-2018 Mateusz Sieczko and other GilRs Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

//! Rumble through libnx's vibration API (`switch/services/hid.h`): the strong motor drives the
//! low band and the weak one the high band, on both vibration devices of the addressed npad.

use std::time::Duration;

/// libnx `HidVibrationValue`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
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

/// libnx `HidNpadStyleTag`s with two vibration devices.
const STYLE_FULL_KEY: u32 = 1 << 0;
const STYLE_HANDHELD: u32 = 1 << 1;
const STYLE_JOY_DUAL: u32 = 1 << 2;
/// The bands' frequencies libnx's own vibration example uses.
const LOW_HZ: f32 = 160.0;
const HIGH_HZ: f32 = 320.0;

#[derive(Debug)]
pub struct Device {
    npad_id: u32,
    last: (u16, u16),
    /// The npad id and style the handles were made for, and the handles.
    target: Option<(u32, u32)>,
    handles: [u32; 2],
}

impl Device {
    pub(super) fn new(npad_id: u32) -> Self {
        Device {
            npad_id,
            last: (0, 0),
            target: None,
            handles: [0; 2],
        }
    }

    /// Sets magnitude for strong and weak ff motors. The values stay until the next call, which
    /// gilrs makes as effects start, change and stop.
    pub fn set_ff_state(&mut self, strong: u16, weak: u16, _min_duration: Duration) {
        // gilrs calls this on every tick, even when still. Don't query HID while idle: the
        // worker can outlive service shutdown, and a query on closed HID aborts the app.
        if self.last == (strong, weak) {
            return;
        }
        self.last = (strong, weak);
        let Some(target) = target(self.npad_id) else {
            self.target = None;
            return;
        };
        if self.target != Some(target) {
            let (id, style) = target;
            // SAFETY: libnx writes two handles into the array it is given.
            if unsafe { hidInitializeVibrationDevices(self.handles.as_mut_ptr(), 2, id, style) } != 0 {
                self.target = None;
                return;
            }
            self.target = Some(target);
        }
        let value = VibrationValue {
            amp_low: f32::from(strong) / f32::from(u16::MAX),
            freq_low: LOW_HZ,
            amp_high: f32::from(weak) / f32::from(u16::MAX),
            freq_high: HIGH_HZ,
        };
        let values = [value; 2];
        // SAFETY: two initialized handles and two values.
        unsafe { hidSendVibrationValues(self.handles.as_ptr(), values.as_ptr(), 2) };
    }
}

/// A full controller's vibration style. A single Joy-Con has no two-motor output.
fn target(id: u32) -> Option<(u32, u32)> {
    // SAFETY: the id is a configured libnx npad id.
    let styles = unsafe { hidGetNpadStyleSet(id) };
    [STYLE_FULL_KEY, STYLE_HANDHELD, STYLE_JOY_DUAL]
        .into_iter()
        .find(|style| styles & style != 0)
        .map(|style| (id, style))
}
