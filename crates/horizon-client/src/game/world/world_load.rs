//! Every job worker while a world loads (`horizon_threads::jobs::set_loading`): from the
//! world's loading screens until its view is in, and again after a teleport or a change of
//! dimension. While playing, the pool runs its playing count (Battery Mode's half), which
//! leaves cores to the render and server threads; while the view loads the render thread
//! mostly waits on the GPU, and the cores it leaves would sit idle.

use super::*;

/// A move of more chunks than this between two frames is a teleport: a new view loads.
const TELEPORT_CHUNKS: i32 = 4;
/// How long the view stays in before loading ends: the last sections mesh after the
/// columns arrive.
const SETTLED: Duration = Duration::from_secs(1);
/// Loading ends after this once no loading screen is up, view in or not: a player who
/// moved on at once, or a view a server never fills.
const LONGEST: Duration = Duration::from_secs(120);
/// How often the view is checked while loading.
const CHECK_EVERY: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(in crate::game) struct WorldLoad {
    /// When loading began; None while playing.
    since: Option<Instant>,
    /// Since when the view has been in, while loading.
    complete_since: Option<Instant>,
    checked: Option<Instant>,
    /// The share of the view's columns in, at the last check.
    progress: f32,
    /// The player's dimension and column at the last frame.
    place: Option<(Dim, (i32, i32))>,
    /// The last frame a loading screen was up: [`LONGEST`] counts from it (an open can
    /// take longer than that behind its screens).
    screen_at: Option<Instant>,
}

impl WorldLoad {
    /// A world's view is loading and not in yet (for [`SETTLED`]).
    pub(in crate::game) fn loading(&self) -> bool {
        self.since.is_some()
            && !self
                .complete_since
                .is_some_and(|at| at.elapsed() >= SETTLED)
    }

    /// The share of the view's columns that are in.
    pub(in crate::game) fn progress(&self) -> f32 {
        self.progress
    }
}

impl Game<'_> {
    /// A world is opening, or its view moved (a teleport, a dimension): every worker
    /// until the view is in.
    pub(in crate::game) fn world_load_begins(&mut self, why: &str) {
        let load = &mut self.world_load;
        if load.since.is_none() {
            load.since = Some(Instant::now());
            load.complete_since = None;
            load.progress = 0.0;
            horizon_threads::jobs::set_loading(true);
            log::info!(
                "Horizon: world loading ({why}): {} job workers",
                horizon_threads::jobs::pool().active()
            );
        }
    }

    fn world_load_ends(&mut self, why: &str) {
        let Some(since) = self.world_load.since.take() else {
            return;
        };
        self.world_load.complete_since = None;
        horizon_threads::jobs::set_loading(false);
        log::info!(
            "Horizon: world loaded ({why}) after {:.1} s: {} job workers",
            since.elapsed().as_secs_f64(),
            horizon_threads::jobs::pool().active()
        );
    }

    /// Once a frame: begins loading at a world's loading screens, a teleport or a change of
    /// dimension, and ends it once no loading screen is up and the view has been in for
    /// [`SETTLED`].
    pub(in crate::game) fn update_world_load(&mut self, now: Instant) {
        #[cfg(target_os = "horizon")]
        let entering = self.switch_world_entry.is_some();
        #[cfg(not(target_os = "horizon"))]
        let entering = false;
        let screen = entering
            || self.integrated_starting.is_some()
            || self.pending_entry.is_some()
            || self.remote.as_ref().is_some_and(|r| r.level_load.is_active());
        let place = self.player.as_ref().map(|p| {
            (
                self.dim,
                (
                    (p.position.x.floor() as i32) >> 4,
                    (p.position.z.floor() as i32) >> 4,
                ),
            )
        });
        let moved = match (self.world_load.place, place) {
            (Some((was, (x0, z0))), Some((dim, (x1, z1)))) => {
                was != dim || (x1 - x0).abs().max((z1 - z0).abs()) > TELEPORT_CHUNKS
            }
            _ => false,
        };
        self.world_load.place = place;
        if moved {
            // A new view: whatever was in is not.
            self.world_load.complete_since = None;
            self.world_load_begins("teleport");
        } else if screen {
            self.world_load_begins("loading screen");
        }
        if screen {
            self.world_load.screen_at = Some(now);
        }
        let Some(since) = self.world_load.since else {
            return;
        };
        if self.title.is_some() && !screen {
            return self.world_load_ends("back at the title");
        }
        if self
            .world_load
            .checked
            .is_some_and(|at| now.saturating_duration_since(at) < CHECK_EVERY)
        {
            return;
        }
        self.world_load.checked = Some(now);
        let waited_from = self.world_load.screen_at.map_or(since, |at| at.max(since));
        if !screen && now.saturating_duration_since(waited_from) >= LONGEST {
            return self.world_load_ends("gave up waiting for the view");
        }
        // As far as the server fills it: its view distance and one (`ViewArrival`).
        let radius = match &self.remote {
            Some(r) => self.streaming.radius.min(r.view_distance + 1),
            None => self.streaming.radius,
        };
        let center = self.streaming.center;
        let (mut wanted, mut have) = (0u32, 0u32);
        for x in center.0 - radius..=center.0 + radius {
            for z in center.1 - radius..=center.1 + radius {
                if horizon_client::streaming::in_view_distance(center, radius, x, z) {
                    wanted += 1;
                    have += u32::from(self.world.chunks.contains_key(&(x, z)));
                }
            }
        }
        self.world_load.progress = if wanted == 0 {
            0.0
        } else {
            have as f32 / wanted as f32
        };
        let complete = place.is_some()
            && horizon_client::streaming::view_edges_in((center, radius), |c| {
                self.world.chunks.contains_key(&c)
            })
            && !self.streaming.terrain_loading()
            && self.pending_uploads.is_empty()
            && self.remote_backlog() == 0;
        if !complete {
            self.world_load.complete_since = None;
            return;
        }
        let complete_since = *self.world_load.complete_since.get_or_insert(now);
        if !screen && now.saturating_duration_since(complete_since) >= SETTLED {
            self.world_load_ends("view in");
        }
    }
}
