# Switch builds and renderers

The Switch build is an NRO of `horizon-client` for `aarch64-nintendo-horizon`, built in
[bevy-horizon](https://github.com/JohnUzoka/bevy-horizon)'s toolchain container. It draws through one
of two GPU drivers, picked when it is built:

| Renderer | Feature | Driver | Status |
| --- | --- | --- | --- |
| Vulkan | `horizon-nx/vulkan` | NVK (Mesa's Vulkan driver for NVIDIA), `libvulkan_bh.a` | the released renderer, the default |
| deko3d | `deko3d` | deko3d, the console's own GPU API, through `third_party/wgpu_deko3d` | built from the `deko3d` branch; not released |

Loading and testing on the console: [Native client](native-client.md#switch-loading-and-hardware-tests).
Updates and releases: [Switch updates](switch-updates.md). Controls: [Controllers](controllers.md#nintendo-switch).
The deko3d backend's design (shaders, memory, submission, barriers, multi-draws, the shader cache, its
limits and tests) is in bevy-horizon's
[docs/deko3d.md](https://github.com/JohnUzoka/bevy-horizon/blob/deko3d/docs/deko3d.md); this page covers
what Horizon does with it.

## Building an NRO

```sh
sh packaging/switch/build.sh            # Vulkan on NVK
sh packaging/switch/build.sh deko3d     # deko3d
sh packaging/switch/build.sh deko3d /tmp/horizon.nro
```

The NRO goes to `target/aarch64-nintendo-horizon/release/horizon-<renderer>.nro` unless a path is
given. The script's header lists every setting; the ones a first build needs:

- `HX_BEVY_HORIZON`: a bevy-horizon checkout (target spec, `.cargo/config.toml`, crate patches) and
  its toolchain image (`HX_IMAGE`).
- `HX_PATCHES`: the workshop's crate patches and `horizon-patches.toml`.
- Vulkan: `HX_NVK_LIB`, the folder holding NVK's `libvulkan_bh.a` (default `~/bevy-horizon/nvk/lib`).
- deko3d: `HX_DEKO3D_LIB`, the folder bevy-horizon's `scripts/build_uam.sh` fills (default
  `~/bevy-horizon/deko3d/lib`). Run that script once in the bevy-horizon checkout: it builds UAM
  v1.1.0 (deko3d's shader compiler) as `libuam.a` and deko3d v0.5.0 with bevy-horizon's patches as
  `libdeko3d_bh.a` and `libdeko3dd_bh.a` (the validating debug library), and records the tags and
  patches in `deko3d/BUILD_INFO`. The script stops if `libuam.a` is missing. NVK isn't needed.

`BEVY_HORIZON_DEKO3D_DEBUG=1` links the debug library, which also reports API misuse to the log
below. `HX_JOBS` (default 2) sets cargo's parallel jobs; each rustc on the client takes gigabytes of
RAM.

`third_party/wgpu_deko3d` is generated: bevy-horizon's `scripts/vendor_deko3d.sh <horizon checkout>`
replaces the folder from its `crates/wgpu_deko3d`, and the folder's `README.md` names the commit it
came from. Change the backend in bevy-horizon and copy it again rather than editing the copy.
Licenses of everything a deko3d build links: [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md#wgpu_deko3d).

## What differs on deko3d

- **One driver per NRO.** `deko3d` (on `horizon-game`, `horizon-client` and `horizon-nx`) can't be
  built with `horizon-nx/vulkan` or `nx-allocator`, which needs NVK's top-down memory region: the
  build stops with a `compile_error!` (`crates/horizon-nx/src/lib.rs`). NVK isn't linked.
- **The backend is reported as `Noop`.** wgpu has no name for a backend outside it, so the adapter's
  backend is `Noop`, and a benchmark report's `hardware.gpu` reads `"backend": "noop"`, name
  `Nintendo Switch Tegra X1 (deko3d)`. The device is made from `wgpu_deko3d::instance()`
  (`gfx/device.rs`), whatever the Graphics API setting says; errors at the renderer's start name it
  deko3d.
- **GPU Culling is unavailable.** It needs count draws (`MULTI_DRAW_INDIRECT_COUNT`), which the
  backend doesn't offer, so the render thread's walk decides what is drawn (`gpu_culling::supported`).
- **Occlusion culling runs.** UAM compiles the shaders' `layout(early_fragment_tests)` and deko3d
  forces the early depth test for them, so the test runs as on Vulkan (`gfx/cull.rs`). In the forest
  benchmark below it saves 2.4 ms of GPU time per frame where terrain is hidden and costs about 1 ms
  over the whole flight.
- **Shaders compile on the console.** The backend translates WGSL to GLSL with naga and compiles it
  with UAM the first time a shader is used, then keeps the program in `sdmc:/.deko3d/shader_cache`
  (one folder per UAM build), as NVK keeps its programs in `sdmc:/.mesa`. The first launch is
  slower; deleting the folder clears the cache. The GLSL writer's depth-texture change for this is in
  `third_party/naga` ([THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md#naga)).
- **GPU faults are logged.** deko3d's fatal errors and warnings (a GPU fault's details, a failed
  submission) go to `sdmc:/switch/deko3d.log`, where upstream's release library aborts without a
  word; the debug library adds misuse reports there.
- **Fifo only.** Both renderers present with Fifo. Mailbox (swap interval 0) was tried on deko3d:
  25.7 FPS over 2 runs against Fifo's 26.0, with a one-second hitch in one run.

## Performance against NVK

Horizon's own benchmark (`--benchmark forest`, [Benchmark your own machine](benchmark.md#benchmark-your-own-machine)),
render distance 12, 1280x720, stock clocks, the 12-second forest flight, on one console. "Before" is
deko3d without the changes in the next table; "now" is the `deko3d` branch with them.

| | NVK, packed terrain vertices (3 runs) | NVK, wide terrain vertices (1) | Build 40, NVK (1) | deko3d before (2) | deko3d now (2) | deko3d now, `HORIZON_NEAREST_TEXELS=1` (2) |
| --- | --- | --- | --- | --- | --- | --- |
| avg FPS | 24.4 | 36.9 | 42.2 | 21.2 | 35.7 | 41.3 |
| 1% low FPS | 14.3 | 23.3 | 24.4 | 11.6 | 6.3, 23.8 | 26.7 |
| p50 / p99 frame ms | 35.8 / 69.5 | 24.2 / 42.7 | 20.1 / 40.2 | 49.6 / 83.2 | 25.1 / 40.4 | 21.5 / 35.4 |
| CPU p50 ms | 7.1 | 6.3 | 5.2 | 31.1 | 5.4 | 5.0 |
| GPU p50 ms | 34.2 | 23.0 | 16.4 | 32.4 | 24.2 | 20.2 |

"deko3d now", 1% low: one value per run; the first run had a one-second hitch.

Packed terrain vertices cost NVK about 11 ms of median GPU and make no difference on deko3d (25.6
FPS with wide ones). The Switch now defaults to the wide ones (0.2.0's choice for NVK);
`HORIZON_PACKED_QUADS=1` turns packed ones on. The table's runs above used packed ones.

### Solid terrain against Build 40

The gap to Build 40 is the terrain shader's code, not deko3d: Build 40's solid fragment and vertex
shader (adapted to today's bindings, today's vertex pulling kept) run on deko3d at GPU p50 15.2 and
16.3 ms (2 runs, 42.0 and 46.2 FPS) with `HORIZON_NEAREST_TEXELS=1`, against 25.7 ms for today's
shader in the same build. That build replaced terrain.wgsl from a file on the SD card, a
measurement harness that isn't in the branch.

Most of it was registers. A shader on the Switch's GM20B gets the registers of its largest path,
taken or not, and above 32 fewer warps run at once. UAM 1.1.0's counts (registers / static
instructions) of the solid fragment shaders, with no texture filtering:

| Solid fragment shader | `fs_opaque` | `fs_solid` |
| --- | --- | --- |
| Texture Filtering and emissive overlays read per frame (before) | 35 / 792 | 38 / 954 |
| Built for the Texture Filtering option | 20 / 306 | 20 / 456 |
| ... and without emissive overlays (now, default resources) | 17 / 150 | 17 / 300 |
| now, `HORIZON_NEAREST_TEXELS=1` | 9 / 90 | 13 / 240 |
| Build 40 | 6 / 60 | 13 / 210 |

RGSS's four taps set the register count of every pipeline while the option was read per frame,
and the emissive overlays' second sprite path doubled the code. Both are now pipeline constants
(terrain.wgsl's `TEXTURE_FILTERING` and `EMISSIVE_OVERLAYS`, gfx.rs's `TerrainBuild`): changing
Texture Filtering, or a frame with emissive overlays (OptiFine-style packs, native mods) in a
renderer built without them, has the game make the renderer again, as 4x MSAA does. Split-screen's
renderers aren't made again, so they keep reading both per frame. The image doesn't change.

The other options' pipelines run too: RGSS (Texture Filtering 1) at GPU p50 47.5 and 46.1 ms
against 50.1 ms before (one run), anisotropic (2) at 26.5 ms (one run). RGSS's four taps keep it
the slowest option on the Switch, with or without this change: its 1% low was 1.7–5.3 FPS on both
builds.

Forest benchmark, GPU p50 (two runs each; builds of the `deko3d` branch from the same day; the last
row's also has the GLSL loop fix of `third_party/naga`, about 0.5 ms):

| Build | default | `HORIZON_NEAREST_TEXELS=1` |
| --- | --- | --- |
| before | 30.1 ms, 28.9 FPS | 26.0 ms, 34.0 FPS |
| Texture Filtering as a pipeline constant | 26.7 ms (27.5, 26.0) | – |
| ... and emissive overlays too (now) | 24.2 ms (24.3, 24.2), 35.7 FPS | 20.2 ms (20.1, 20.3), 41.3 FPS |

Build 40 on NVK flies the same benchmark at 16.4 ms, 42.2 FPS; with Build 40's shader the deko3d
build above was at 15.7 (`HORIZON_NEAREST_TEXELS=1`). Between 20.2 and 15.7 ms the two shaders
still differ in the sprite sampled at the UV's gradients (`textureSampleGrad`, which keeps distant
tiles' mip level across block seams) where Build 40 took implicit ones, the chunk fade-in's mix
toward the fog colour, and today's wider vertex output. Without the fade-in's mix (the harness
build, one run) it was 19.3 against 20.2 ms; the mix is vanilla's visible fade-in, so it stays.

Tried without gain: moving the vertex shader's colored-light loop first (39 → 32 registers in the
section vertex shaders; 30.4 ms in one run against 30.1) and, earlier, a minimal vertex shader.

`HORIZON_NEAREST_TEXELS=1` (in `env.txt`) samples terrain sprites as Build 40 does: the sampler
takes the nearest texel, with no snapping in the shader, and mips stay blended. It changes the image
(sharper texel edges up close), so it's off by default.

What moved deko3d from "before" to "now", as measured on the console:

| Change | Effect |
| --- | --- |
| Indirect multi-draws as one macro call | gpfifo entries per submission ~580 → 26; CPU 32 → 6.4 ms |
| Submission doesn't wait for the GPU (a backlog instead) | the CPU no longer waits for the previous frame |
| Barriers only for real hazards | full barriers ~43 → 9 per frame |
| Occlusion culling on deko3d | −2.4 ms GPU where terrain is hidden; about +1 ms in this flight |
| Queue command memory 64 KiB → 1 MiB | `dkQueueSubmitCommands` 16–30 → 0.01 ms per frame; 1% low 6.2 → 16.1 FPS, p99 116.5 → 60 ms |
| Solid terrain in two passes (opaque, then cutout with `discard`) and sprites sampled at an explicit level | GPU 33.7 → 30.1 ms; 25.7 → 28.9 FPS |
| Solid terrain built for the Texture Filtering option and the emissive overlays in use ([above](#solid-terrain-against-build-40)) | GPU 30.1 → 24.2 ms; 28.9 → 35.7 FPS |

Tried without gain: hardware compression of colour targets (`DkImageFlags_HwCompression`: GPU 36.3
against 35.9 ms) and Mailbox (above).

Both renderers are GPU-bound here. With Fifo a frame takes 33.4 or 50 ms (two or three refreshes),
so small GPU savings show in the FPS only when they move frames across that line. By Resolution
Scale (deko3d before the two terrain passes, one run each except 100%):

| Resolution Scale | GPU p50 ms | avg FPS |
| --- | --- | --- |
| 100% | 33.8 | 25.7 |
| 75% | 27.7 | 31.7 |
| 50% | 20.7 | 39.0 |

A straight-line fit gives about 17 ms that scales with the pixel count and 16 ms that doesn't
(geometry and culling); that split is an inference from three points. Other Switch ports on deko3d
(sa2-nx, nfsmw-nx) were about twice as fast as on NVK because they were CPU-bound (PM4 emulation and
NVK's cost per draw); Horizon's CPU time was already below its GPU time on NVK.

To run it on the console, put an `args.txt` beside the NRO (its lines are added to the command line):

```text
--resources ../horizon/resources --render-distance 12 --benchmark forest --name Player15839
```

and an empty `env.txt` (stock clocks, no CPU boost). The fixed `--name` gives every build the same
offline player and skin. The game creates `horizon-benchmark`, flies the scene, writes
`benchmarks/game-<time>-forest.json` beside the NRO and quits. The table's rows are the report's
`frames[0]` fields `avg_fps`, `low_1pct_fps`, `p50_ms`, `p99_ms`, `cpu_p50_ms` and `gpu_p50_ms`
([Benchmark report format](benchmark-report.md)). Compare reports from the same render distance,
handheld or docked mode and clocks.

## Input and loading fixes

Found while measuring input lag on deko3d; they apply to both renderers.

- Pad taps shorter than a frame register: [Controllers](controllers.md#nintendo-switch).
- Menu blur (behind Create World and Options) runs at half size on the Switch: the six full-size
  passes cost 37–107 ms of GPU at 1280x720 (7–11 FPS on deko3d, 8–9 on NVK); at half size Create
  World runs at 20–22 FPS on deko3d.
- The game reads version 3 of `resources/horizon-data.bin`, which the 0.2.x Switch release writes.
  Reading only version 2, it opened about 23,000 files one at a time (core tables 62–101 s instead
  of about 7 s) and then rewrote the shared file as version 2, which the release rejects. The rewrite
  now replaces only an unusable file and keeps 128 MiB of the card free.

## Frame drops while playing

`--benchmark travel` (render distance 12, the 32-second flight over new terrain, then four 4-second
holds), two runs per build; frames of 50 ms or more are hitches at Fifo's 16.7 ms:

| Build | travel avg FPS | 1% low FPS | max frame ms | frames ≥ 100 ms | threads created per 10 s |
| --- | --- | --- | --- | --- | --- |
| before | 48.0, 55.2 | 3.1, 15.7 | 2542, 458 | 4, 2 | 11.8, 11.7 |
| `[Thread]` lines queued | 58.4, 56.6 | 39.8, 31.0 | 37.5, 37.1 | 0, 0 | 12.4, 11.9 |
| ... and one audio device thread | 56.6, 56.6 | 31.2, 31.0 | 32.7, 36.2 | 0, 0 | 2.5, 2.3 |
| ... and one freeing thread | 56.6, 58.3 | 31.2, 38.4 | 32.5, 40.5 | 0, 0 | 0.7, 0.7 |

The holds ran at 60 FPS in every build. The hitches were frames of 0.3–2.5 s whose timed steps added
up to a few ms, each with one thread created in it. horizon-nx wrote every thread's `[Thread]` line
to the SD card on the creating thread, behind the log writer's lock, which the writer holds while
it writes and syncs `log.txt` (a sync can take a second). The lines now go through the writer's
queue. The threads came from the render thread: the audio output's device check started one every
second (as `SoundEngine` looks at the devices), and freed buffers and unloaded chunk columns got one
per batch. Each now has one long-lived thread.

`HORIZON_SPIKE_LOG=MS` (in `env.txt`) logs each frame of `MS` or more with its timed steps, the
threads created in it and the uploads waiting, and every 10 s a count of slow frames; each
`[Thread]` line names its creator's return addresses (NRO offsets, `nm` of the ELF names them).
What it still shows in this benchmark: the section arena growing a page (`arena_grow`, 63–65 ms
on the render thread, about once a run) and frames waiting up to 90 ms for the GPU. Pausing (which
saves) gave one frame of 135 ms before and 70 ms after the queued lines (one session each).

## World loading

`--benchmark forest` (render distance 12, seed 1234), two runs per row, on the bench's settings
(Battery Mode, automatic workers: 1 while playing) and on 3 workers without Battery Mode. Every
row ran with `HORIZON_NO_DATA_FILE=1` (`hx-bench-var.py` always sets it), which makes the world's
open slower than a player's (open → join about 130 s instead of about 22 s). "First frame" and
"spawn loaded" count from the process start; "forest" is the teleport's load.
The new rows are one build with the changes switched off by `env.txt` (`HORIZON_SPAWN_HOLD=0`,
`HORIZON_NX_WORLD_BOOST=0`); the bench's held row ran before the 120 s fix below, which only
matters without the hold.

| Build (Battery Mode, 1 worker) | open → join s | first world frame s | spawn loaded s | forest s | chunks/s | flight FPS | 1% low |
| --- | --- | --- | --- | --- | --- | --- | --- |
| before | 127.8, 133.5 | 239.3, 232.7 | 285.3, 292.1 | 79.4, 78.2 | 8.0, 8.1 | 36.1, 37.7 | 24.0, 24.0 |
| all workers while loading | 132.1, 133.6 | 238.1, 233.4 | 273.5, 267.1 | 55.8, 53.2 | 11.4, 12.0 | 36.2, 36.4 | 23.9, 23.9 |
| ... and FastLoad through the open | 90.3, 90.1 | 197.0, 190.1 | 235.0, 228.7 | 51.7, 56.3 | 12.3, 11.3 | 36.3, 36.4 | 24.8, 23.7 |
| ... and the loading screen held until the spawn view is in | 90.3, 88.8 | 219.3, 207.1 | 227.8, 213.3 | 48.8, 48.8 | 13.0, 13.1 | 36.1, 36.2 | 23.8, 24.5 |
| ... with 3 workers while loading | 90.6, 88.5 | 212.4, 204.3 | 211.8, 212.9 | 51.0, 51.0 | 12.5, 12.5 | 36.2, 36.1 | 23.6, 24.0 |

| Build (3 workers, no Battery Mode) | open → join s | first world frame s | spawn loaded s | forest s | flight FPS | 1% low |
| --- | --- | --- | --- | --- | --- | --- |
| before | 132.8, 133.0 | 239.0, 232.0 | 268.0, 260.6 | 56.8, 44.5 | 29.5, 29.6 | 23.3, 23.5 |
| FastLoad through the open | 89.7, 86.6 | 194.3, 188.5 | 223.7, 217.0 | 56.7, 50.5 | 28.8, 29.4 | 23.4, 22.4 |
| ... and the loading screen held | 89.7, 88.3 | 233.3, 203.2 | 210.4, 208.2 | 34.3, 52.6 | 28.8, 28.2 | 23.4, 5.1 |

- **Workers.** The job pool starts its loading count (the automatic count without Battery Mode's
  halving, 2 on the Switch, or the player's own if higher) and runs only the playing count until
  a world loads: its open and loading screens, a teleport (more than 4 chunks in a frame) or a
  dimension change, until the view's outer columns are in and nothing is in flight for 1 s (at
  most 120 s after the last loading screen). Flight runs on the playing count, and its frame rate
  did not change. Two flights of the fourteen on the new builds had one long frame (952 ms and
  648 ms, 1% low 4.5 and 5.1 FPS: a workers-only run before the 120 s fix, not in the table, and
  the last row); the four before had none. Not attributed. `HORIZON_LOAD_JOB_THREADS=N` sets the
  loading count; 3 instead of 2 measured no faster.
- **FastLoad.** The benchmark's open (no loading screen is drawn while it waits) now asks for the
  boost: open → join 130 → 89 s. With the loading screen held, single player's boost lasts until
  the spawn view is in instead of ending at the first mesh.
- **Holding the loading screen** shows the world later than showing it at the first mesh (run for
  run: 22 and 17 s on the bench's settings, 39 and 15 s on 3 workers; the screen stays up 20–41 s
  after join instead of 2–3 s), with the spawn area loaded when it shows; the spawn area is in
  7–15 s sooner. The hold's own test (outer columns in, nothing meshing) is stricter than the
  benchmark's: in two runs the benchmark counted the spawn loaded before the screen went away, and
  in one of those its teleport ran behind the held screen (the 34.3 s forest).
  `HORIZON_SPAWN_HOLD=0` shows the world at its first mesh.
- **Where open → join goes** (`HORIZON_STARTUP_TRACE`, one run before, unboosted): the world's
  preparation on its thread (`world.prepare`, 66.5 s, of which the worldgen data 11.9 s), then the
  server's `open_world` (51.6 s), then 4.1 s waiting for the recipe and registry preload (which ran
  beside them for 75 s; `sync.recipes.item_catalog` 5.4 s) and 2.1 s settling the spawn.
- **A generated chunk's stages** (the job log's new line, wall time on a worker, about 2,250 chunks
  a run): noise 18.6–19.3 ms, surface 10.5–10.9, carvers 5.4–5.5, structures 2.7–5.1, biomes
  2.8–3.1.

## Known issues

- On the title screen, GPU time per frame grew from 6–8 ms to 19–23 ms after minutes in the menus
  (seen on deko3d during the input-lag work, not investigated).
- The splitscreen pad reader and ZL/ZR (read as digital triggers) don't keep taps shorter than a
  frame yet.
- Whether deko3d's ZCULL (the hardware's coarse depth rejection) engages for Horizon's depth buffer
  is unmeasured.
- The effect of render distance and far-terrain detail on deko3d's GPU time is untested.
