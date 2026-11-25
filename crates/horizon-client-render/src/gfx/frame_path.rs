//! The frame's A/B switches (the benchmark's `ab:frame-path`, the `frame-path on|off` control
//! command): `HORIZON_FRAME_PATH`, read once at startup, lists what turning the switch on
//! changes, comma-separated; `HORIZON_FRAME_PATH_ON=1` starts with it on. The `no-*` items
//! leave a part of the frame out, to time it; the rest draw the same picture another way.

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) struct FramePath {
    /// `hand-pass`: the first-person hand in a pass of its own whose depth starts cleared,
    /// instead of a full-screen depth reset at the end of the world pass.
    pub hand_pass: bool,
    /// `cloud-shader`: clouds through `fs_cloud` instead of the entity shader's every branch.
    pub cloud_shader: bool,
    /// `sky-late`: the sky after the solid terrain, depth-tested at the far plane, so it
    /// shades only what the terrain left uncovered (`SkyLate`).
    pub sky_late: bool,
    pub no_sky: bool,
    pub no_celestial: bool,
    pub no_clouds: bool,
    pub no_screen: bool,
    pub no_hand: bool,
    pub no_hud: bool,
}

impl FramePath {
    pub fn parse(list: &str) -> Self {
        let mut p = Self::default();
        for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match item {
                "hand-pass" => p.hand_pass = true,
                "cloud-shader" => p.cloud_shader = true,
                "sky-late" => p.sky_late = true,
                "no-sky" => p.no_sky = true,
                "no-celestial" => p.no_celestial = true,
                "no-clouds" => p.no_clouds = true,
                "no-screen" => p.no_screen = true,
                "no-hand" => p.no_hand = true,
                "no-hud" => p.no_hud = true,
                other => log::warn!("HORIZON_FRAME_PATH: unknown item {other}"),
            }
        }
        p
    }

    /// What the switch turns on, and whether it starts on.
    pub fn from_env() -> (Self, bool) {
        let path = Self::parse(&std::env::var("HORIZON_FRAME_PATH").unwrap_or_default());
        let on = std::env::var("HORIZON_FRAME_PATH_ON").as_deref() == Ok("1");
        if path != Self::default() {
            log::info!("Horizon renderer: frame path {path:?}, {}", if on { "on" } else { "off" });
        }
        (path, on)
    }
}

/// `sky-late`'s pipelines: the sky's base and the sunrise fan with the depth test
/// (`GreaterEqual`) against the far plane the world pass clears to, drawn with a viewport
/// whose depth range is that plane alone; the celestial bodies' is `Celestial::added_late`.
pub(super) struct SkyLate {
    pub base: wgpu::RenderPipeline,
    pub sunrise: wgpu::RenderPipeline,
}
