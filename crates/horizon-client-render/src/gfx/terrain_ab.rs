//! `HORIZON_TERRAIN_AB=MASK,MASK,...`: an in-run A/B of solid terrain variants. Every
//! `BLOCK` frames the next mask in the list applies, and each GPU frame time comes back
//! tagged with the mask its frame drew with, so every variant sees the same stretch of a
//! flight. Every `WINDOW` the log gets one line per mask: frames, median and mean GPU ms.
//!
//! Mask bits (terrain.wgsl's `terrain_ab`, `globals.filtering.w`):
//! - 1: `sample_nearest` at the UV's gradients (`textureSampleGrad`), not at their level;
//! - 2: no `sample_nearest` (measurement only: the sprite at the UV, one level);
//! - 4: solid terrain in one pass where it draws in two (`split_solid_terrain`), or the
//!   other way round;
//! - 8: no emission, HDR or fog after the cutout (measurement only);
//! - 16: Build 40's sprite sampling: a nearest sampler and the sprite at the UV's own
//!   gradients, no `sample_nearest`;
//! - 32: `fs_solid`'s opaque quads untextured, their vertex colour alone (measurement only);
//! - 64: the section vertex shaders' position, UV, layer and colour alone (measurement only);
//! - 128: with 16, the nearest sampler's mip filter linear (texels nearest, levels blended);
//!   alone, the usual sampler's mip filter nearest (`sample_nearest` on one level).
//!   Bits 16 and 128 swap the frame's terrain group for one with that sampler (`groups`).
use std::time::{Duration, Instant};

const BLOCK: u32 = 4;
const WINDOW: Duration = Duration::from_secs(2);
pub const SPLIT: u32 = 4;
pub const NEAREST: u32 = 16;
pub const MIP_FLIP: u32 = 128;

/// The sampler variant of a mask: 0 the usual one, 1 Build 40's nearest, 2 nearest with a
/// linear mip filter, 3 the usual one with a nearest mip filter.
pub fn sampler_variant(mask: u32) -> usize {
    match (mask & NEAREST != 0, mask & MIP_FLIP != 0) {
        (false, false) => 0,
        (true, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    }
}

pub struct TerrainAb {
    masks: Vec<u32>,
    frame: u32,
    window_start: Instant,
    times: Vec<(u32, f32)>,
    /// The terrain groups of the sampler variants (`sampler_variant`) not in
    /// `Renderer::group`, which holds variant `current`'s; built when a frame first asks.
    pub groups: [Option<wgpu::BindGroup>; 4],
    pub current: usize,
}

impl TerrainAb {
    pub fn from_env() -> Option<Self> {
        let list = std::env::var("HORIZON_TERRAIN_AB").ok()?;
        let masks: Vec<u32> = list
            .split(',')
            .filter_map(|m| m.trim().parse().ok())
            .collect();
        if masks.is_empty() {
            return None;
        }
        log::info!("Horizon: terrain A/B over masks {masks:?}, {BLOCK} frames each in turn");
        Some(Self {
            masks,
            frame: 0,
            window_start: Instant::now(),
            times: Vec::new(),
            groups: [None, None, None, None],
            current: 0,
        })
    }
    /// Whether any mask toggles the two solid passes, which need meshes grouped by part.
    pub fn splits(&self) -> bool {
        self.masks.iter().any(|m| m & SPLIT != 0)
    }
    /// The next frame's mask.
    pub fn next_frame(&mut self) -> u32 {
        let mask = self.masks[(self.frame / BLOCK) as usize % self.masks.len()];
        self.frame = self.frame.wrapping_add(1);
        mask
    }
    /// Takes the timer's tagged times; logs the window once it is over.
    pub fn collect(&mut self, tagged: &mut Vec<(u32, f32)>) {
        self.times.append(tagged);
        if self.window_start.elapsed() < WINDOW {
            return;
        }
        self.window_start = Instant::now();
        let mut line = String::from("Horizon: terrain A/B window:");
        let mut masks = self.masks.clone();
        masks.sort_unstable();
        masks.dedup();
        for mask in masks {
            let mut ms: Vec<f32> = (self.times.iter())
                .filter(|(m, _)| *m == mask)
                .map(|(_, t)| *t)
                .collect();
            if ms.is_empty() {
                continue;
            }
            ms.sort_by(f32::total_cmp);
            let mean = ms.iter().sum::<f32>() / ms.len() as f32;
            line += &format!(
                " mask {mask}: {} frames, median {:.2}, mean {:.2};",
                ms.len(),
                ms[ms.len() / 2],
                mean
            );
        }
        self.times.clear();
        log::info!("{line}");
    }
}
