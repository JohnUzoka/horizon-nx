//! `HORIZON_TERRAIN_AB=MASK,MASK,...`: an in-run A/B of solid terrain variants. Every
//! `BLOCK` frames the next mask in the list applies, and each GPU frame time comes back
//! tagged with the mask its frame drew with, so every variant sees the same stretch of a
//! flight. Every `WINDOW` the log gets one line per mask: frames, median and mean GPU ms.
//!
//! Mask bits (terrain.wgsl's `terrain_ab`, `globals.filtering.w`):
//! - 1: `sample_nearest` takes an explicit level instead of the UV's gradients;
//! - 2: no `sample_nearest` (measurement only: the sprite at the UV, one level);
//! - 4: two solid passes, the opaque quads through `fs_opaque` first (`split_solid`);
//! - 8: no emission, HDR or fog after the cutout (measurement only);
//! - 16: Build 40's sprite sampling: a nearest sampler (the frame's terrain group swapped for
//!   `alt_group`) and the sprite at the UV's own gradients, no `sample_nearest`;
//! - 32: `fs_solid`'s opaque quads untextured, their vertex colour alone (measurement only);
//! - 64: the section vertex shaders' position, UV, layer and colour alone (measurement only).
use std::time::{Duration, Instant};

const BLOCK: u32 = 4;
const WINDOW: Duration = Duration::from_secs(2);
pub const TEX_LEVEL: u32 = 1;
pub const SPLIT: u32 = 4;
pub const NEAREST: u32 = 16;

pub struct TerrainAb {
    masks: Vec<u32>,
    frame: u32,
    window_start: Instant,
    times: Vec<(u32, f32)>,
    /// The terrain group with the nearest sampler, while `Renderer::group` holds the usual one
    /// (swapped while `swapped`); `None` until a frame asks for it and after a rebuild.
    pub alt_group: Option<wgpu::BindGroup>,
    pub swapped: bool,
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
            alt_group: None,
            swapped: false,
        })
    }
    /// Whether any mask asks for two solid passes, which needs meshes grouped by part.
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
