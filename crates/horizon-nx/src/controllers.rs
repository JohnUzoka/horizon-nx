//! The system controller applet used to connect, pair and order controllers for splitscreen.

/// libnx `HidLaControllerSupportArg` (libnx converts it on firmware below 8.0.0).
#[repr(C)]
struct Arg {
    player_count_min: i8,
    player_count_max: i8,
    enable_take_over_connection: u8,
    enable_left_justify: u8,
    enable_permit_joy_dual: u8,
    enable_single_mode: u8,
    enable_identification_color: u8,
    identification_color: [[u8; 4]; 8],
    enable_explain_text: u8,
    explain_text: [[u8; 0x81]; 8],
}

type ResultInfo = [u32; 2];

const _: () = {
    assert!(std::mem::size_of::<Arg>() == 0x430);
    assert!(std::mem::offset_of!(Arg, explain_text) == 0x28);
};

unsafe extern "C" {
    fn hidLaCreateControllerSupportArg(arg: *mut Arg);
    fn hidLaShowControllerSupport(result: *mut ResultInfo, arg: *const Arg) -> u32;
    fn hidSetNpadJoyHoldType(hold: u32) -> u32;
}

/// libnx `HidNpadJoyHoldType_Horizontal`.
const HOLD_HORIZONTAL: u32 = 1;

/// Shows the system controller applet and returns when it closes. The usual HID reader still
/// handles the configured pads when the applet is unavailable (e.g. in an emulator).
pub fn show() {
    let mut arg = std::mem::MaybeUninit::<Arg>::uninit();
    // SAFETY: libnx fully initializes its argument struct. A null result pointer is supported;
    // joining depends on the pad events, not the number of controllers reported by the applet.
    let rc = unsafe {
        hidLaCreateControllerSupportArg(arg.as_mut_ptr());
        let mut arg = arg.assume_init();
        arg.player_count_min = 1;
        // Set the hold type before launching: with libnx's default vertical hold type the applet
        // returned LibAppletBadExit on hardware, while horizontal let the applet open normally.
        match hidSetNpadJoyHoldType(HOLD_HORIZONTAL) {
            0 => hidLaShowControllerSupport(std::ptr::null_mut(), &arg),
            rc => rc,
        }
    };
    if rc != 0 {
        log::warn!("Horizon: the controller applet did not open or was cancelled (0x{rc:x})");
    }
}
