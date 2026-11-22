//! Switch six-axis input (libnx `switch/services/hid.h`). The game owns a sensor only while
//! gyro aiming is active. The started handle and its gravity filter have the same lifetime.

/// libnx `HidSixAxisSensorState`. A `HidVector` is three consecutive floats.
#[repr(C)]
struct State {
    delta_time: u64,
    sampling_number: u64,
    acceleration: [f32; 3],
    angular_velocity: [f32; 3],
    angle: [f32; 3],
    direction: [[f32; 3]; 3],
    attributes: u32,
    reserved: u32,
}

/// libnx `HidSixAxisSensorHandle` carries a packed word in a C union.
#[repr(C)]
#[derive(Clone, Copy)]
union Handle {
    bits: u32,
}

unsafe extern "C" {
    fn hidGetNpadStyleSet(id: u32) -> u32;
    fn hidGetSixAxisSensorHandles(handles: *mut Handle, count: i32, id: u32, style: u32) -> u32;
    fn hidStartSixAxisSensor(handle: Handle) -> u32;
    fn hidStopSixAxisSensor(handle: Handle) -> u32;
    fn hidGetSixAxisSensorStates(handle: Handle, states: *mut State, count: usize) -> usize;
}

/// libnx `HidNpadIdType` and `HidNpadStyleTag` values.
const PLAYER_1: u32 = 0;
const HANDHELD: u32 = 0x20;
const STYLE_FULL_KEY: u32 = 1 << 0;
const STYLE_HANDHELD: u32 = 1 << 1;
const STYLE_JOY_DUAL: u32 = 1 << 2;
const SENSOR_CONNECTED: u32 = 1;

/// As for rumble: handheld when attached, else player 1's Pro Controller or Joy-Con pair.
fn active_target() -> Option<(u32, u32)> {
    // SAFETY: both IDs are valid Npads. The game's controller backend has initialized HID.
    let handheld = unsafe { hidGetNpadStyleSet(HANDHELD) };
    if handheld & STYLE_HANDHELD != 0 {
        return Some((HANDHELD, STYLE_HANDHELD));
    }
    let player = unsafe { hidGetNpadStyleSet(PLAYER_1) };
    if player & STYLE_FULL_KEY != 0 {
        Some((PLAYER_1, STYLE_FULL_KEY))
    } else if player & STYLE_JOY_DUAL != 0 {
        Some((PLAYER_1, STYLE_JOY_DUAL))
    } else {
        None
    }
}

struct Sensor {
    target: (u32, u32),
    handle: Handle,
    up: Option<[f32; 3]>,
}

impl Sensor {
    fn start(target: (u32, u32)) -> Option<Self> {
        let (id, style) = target;
        // A pair has two sensors. Use the right Joy-Con, as for the right stick.
        let count = if style == STYLE_JOY_DUAL { 2 } else { 1 };
        let mut handles = [Handle { bits: 0 }; 2];
        // SAFETY: the buffer holds `count` handles and the Npad ID/style were checked above.
        if unsafe { hidGetSixAxisSensorHandles(handles.as_mut_ptr(), count, id, style) } != 0 {
            return None;
        }
        let handle = handles[count as usize - 1];
        // SAFETY: libnx just supplied the handle.
        if unsafe { hidStartSixAxisSensor(handle) } != 0 {
            return None;
        }
        Some(Self {
            target,
            handle,
            up: None,
        })
    }
}

impl Drop for Sensor {
    fn drop(&mut self) {
        // SAFETY: this sensor successfully started the handle and owns its stop.
        unsafe { hidStopSixAxisSensor(self.handle) };
    }
}

/// Motion in controller axes: right, up, and out of its face.
pub struct Motion {
    /// Turns per second around each axis.
    pub rotation: [f32; 3],
    /// Smoothed acceleration in g; at rest it points against gravity.
    pub up: [f32; 3],
}

/// A lazily started sensor for the Switch's default pad. Constructing an idle reader and
/// stopping or dropping one do not touch HID unless a sensor was successfully started.
#[derive(Default)]
pub struct MotionSensor(Option<Sensor>);

impl MotionSensor {
    pub fn is_running(&self) -> bool {
        self.0.is_some()
    }

    pub fn stop(&mut self) {
        self.0 = None;
    }

    /// Starts the sensor if necessary. A controller or style change forgets the old gravity
    /// estimate along with the old handle. Call only during eligible gameplay with HID ready.
    pub fn sample(&mut self, seconds: f32) -> Option<Motion> {
        let target = active_target();
        if self.0.as_ref().map(|sensor| sensor.target) != target {
            self.stop();
            self.0 = target.and_then(Sensor::start);
        }
        let sensor = self.0.as_mut()?;
        let mut state = std::mem::MaybeUninit::<State>::uninit();
        // SAFETY: there is room for one state. A nonzero count means libnx filled it.
        if unsafe { hidGetSixAxisSensorStates(sensor.handle, state.as_mut_ptr(), 1) } == 0 {
            return None;
        }
        let state = unsafe { state.assume_init() };
        if state.attributes & SENSOR_CONNECTED == 0 {
            sensor.up = None;
            return None;
        }
        let up = if let Some(up) = &mut sensor.up {
            // Smooth gravity so the controller's acceleration as it swings doesn't tip it.
            let weight = (seconds * 4.0).min(1.0);
            for (u, a) in up.iter_mut().zip(state.acceleration) {
                *u += (a - *u) * weight;
            }
            *up
        } else {
            sensor.up = Some(state.acceleration);
            state.acceleration
        };
        Some(Motion {
            rotation: state.angular_velocity,
            up,
        })
    }
}
