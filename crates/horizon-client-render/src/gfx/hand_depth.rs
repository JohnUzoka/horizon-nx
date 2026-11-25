//! How the first-person hand gets the cleared depth it draws on (GameRenderer.renderItemInHand
//! clears depth first). Drawn at the end of the world pass (`hand_in_world`), the clear is a
//! triangle writing the far plane: over the whole target it is a full-screen depth fill each
//! frame, ~0.9 MB of depth writes at 720p on the Switch's ROPs. Only the hand reads that depth,
//! so unless a later pass reads the world's depth (a post chain), it is enough to reset the
//! pixels the hand can cover: the same image, a fraction of the fill.
//!
//! The hand also covers a share of the screen (a tenth or so with an item) whose world pixels
//! are shaded only to be drawn over. Before the terrain, `cover` writes the nearest depth where
//! the hand will replace the pixel (its alpha is 1 there, through the hand's alpha blend) and
//! none of its pixels blends over it, so the world's depth test rejects what lies there; the
//! reset then clears it for the hand. Those pixels end up the hand's either way, so the image is
//! the same; what changes is that the world is not shaded under the hand.

use std::sync::LazyLock;

use glam::{Mat4, Vec2, Vec4};

use super::Vertex;

/// `HORIZON_HAND_DEPTH`: `full` resets the whole target, `pass` draws the hand in its own render
/// pass with a depth clear (Build 40's way), `rect` resets the hand's rectangle; anything else
/// resets the rectangle and covers the hand's pixels before the terrain (`Cover`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HandDepth {
    Full,
    Rect,
    Cover,
    Pass,
}

impl HandDepth {
    /// Whether the reset may be limited to the hand's rectangle.
    pub(super) fn rect(self) -> bool {
        matches!(self, HandDepth::Rect | HandDepth::Cover)
    }
}

pub(super) fn mode() -> HandDepth {
    static MODE: LazyLock<HandDepth> =
        LazyLock::new(|| parse(std::env::var("HORIZON_HAND_DEPTH").ok().as_deref()));
    *MODE
}

fn parse(value: Option<&str>) -> HandDepth {
    match value {
        Some("full") => HandDepth::Full,
        Some("pass") => HandDepth::Pass,
        Some("rect") => HandDepth::Rect,
        _ => HandDepth::Cover,
    }
}

/// Draws the hand's cover into the world pass, over `size`: the nearest depth (a viewport
/// depth range of 1..1) where `cover` keeps a hand pixel, then the farthest (0..0) where
/// `uncover` keeps one, whatever their order on screen; the viewport is the whole target again
/// after. The pass's index buffer must be the shared quad indices and the hand's group bound.
pub(super) fn cover(
    pass: &mut wgpu::RenderPass<'_>,
    [cover, uncover]: [&wgpu::RenderPipeline; 2],
    size: [u32; 2],
    quads: u32,
    first_quad: u32,
) {
    let [w, h] = size.map(|s| s as f32);
    for (pipeline, depth) in [(cover, 1.0), (uncover, 0.0)] {
        pass.set_viewport(0.0, 0.0, w, h, depth, depth);
        pass.set_pipeline(pipeline);
        pass.draw_indexed(0..quads * 6, (first_quad * 4) as i32, 0..1);
    }
    pass.set_viewport(0.0, 0.0, w, h, 0.0, 1.0);
}

/// The scissor rect (x, y, width, height) of a `size` target that holds every pixel the hand's
/// quads can cover under `view_proj`, with a pixel to spare on each side; `None` when a vertex
/// is not in front of the eye (its projection bounds nothing) or the hand covers no pixel.
pub(super) fn rect(hand: &[Vertex], view_proj: Mat4, size: [u32; 2]) -> Option<[u32; 4]> {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for v in hand {
        let clip = view_proj * Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
        if !(clip.w > 1e-6) {
            return None;
        }
        let ndc = clip.truncate().truncate() / clip.w;
        lo = lo.min(ndc);
        hi = hi.max(ndc);
    }
    let [w, h] = size.map(|s| s as f32);
    // Normalized device coordinates to pixels, y down.
    let x0 = ((lo.x * 0.5 + 0.5) * w).floor() - 1.0;
    let x1 = ((hi.x * 0.5 + 0.5) * w).ceil() + 1.0;
    let y0 = ((0.5 - hi.y * 0.5) * h).floor() - 1.0;
    let y1 = ((0.5 - lo.y * 0.5) * h).ceil() + 1.0;
    if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
        return None;
    }
    let [x0, x1] = [x0, x1].map(|x| x.clamp(0.0, w) as u32);
    let [y0, y1] = [y0, y1].map(|y| y.clamp(0.0, h) as u32);
    (x1 > x0 && y1 > y0).then(|| [x0, y0, x1 - x0, y1 - y0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(pos: [f32; 3]) -> Vertex {
        Vertex {
            pos,
            uv: [0.0; 2],
            layer: 0,
            color: 0,
            light: 0,
            normal: [0; 4],
            glint_uv: [0.0; 2],
        }
    }

    #[test]
    fn parses_the_switch() {
        assert_eq!(parse(Some("full")), HandDepth::Full);
        assert_eq!(parse(Some("pass")), HandDepth::Pass);
        assert_eq!(parse(Some("rect")), HandDepth::Rect);
        assert_eq!(parse(Some("cover")), HandDepth::Cover);
        assert_eq!(parse(None), HandDepth::Cover);
    }

    #[test]
    fn bounds_the_projected_hand() {
        let proj = Mat4::perspective_rh(70f32.to_radians(), 16.0 / 9.0, 100.0, 0.05);
        let hand = [at([0.2, -0.3, -0.5]), at([0.5, -0.6, -0.6]), at([0.3, -0.1, -0.4])];
        let [x, y, w, h] = rect(&hand, proj, [1280, 720]).expect("in front");
        for v in &hand {
            let clip = proj * Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
            let px = (clip.x / clip.w * 0.5 + 0.5) * 1280.0;
            let py = (0.5 - clip.y / clip.w * 0.5) * 720.0;
            assert!(px >= x as f32 && px <= (x + w) as f32, "{px} in {x}+{w}");
            assert!(py >= y as f32 && py <= (y + h) as f32, "{py} in {y}+{h}");
        }
        assert!(x + w <= 1280 && y + h <= 720);
        assert!(w < 1280 && h < 720);
    }

    #[test]
    fn behind_the_eye_bounds_nothing() {
        let proj = Mat4::perspective_rh(70f32.to_radians(), 16.0 / 9.0, 100.0, 0.05);
        assert_eq!(rect(&[at([0.0, 0.0, -1.0]), at([0.0, 0.0, 1.0])], proj, [1280, 720]), None);
    }

    #[test]
    fn off_screen_covers_nothing() {
        let proj = Mat4::perspective_rh(70f32.to_radians(), 16.0 / 9.0, 100.0, 0.05);
        assert_eq!(rect(&[at([50.0, 0.0, -1.0]), at([60.0, 1.0, -1.0])], proj, [1280, 720]), None);
    }
}
