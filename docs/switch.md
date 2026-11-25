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

| | NVK, packed terrain vertices (3 runs) | NVK, wide terrain vertices (1) | Build 40, NVK (1) | deko3d before (2) | deko3d now (2) |
| --- | --- | --- | --- | --- | --- |
| avg FPS | 24.4 | 36.9 | 42.2 | 21.2 | 25.7 |
| 1% low FPS | 14.3 | 23.3 | 24.4 | 11.6 | 16.1 |
| p50 / p99 frame ms | 35.8 / 69.5 | 24.2 / 42.7 | 20.1 / 40.2 | 49.6 / 83.2 | 34.7 / 60.0 |
| CPU p50 ms | 7.1 | 6.3 | 5.2 | 31.1 | 6.6 |
| GPU p50 ms | 34.2 | 23.0 | 16.4 | 32.4 | 33.8 |

Packed terrain vertices (the default; `HORIZON_PACKED_QUADS=0` turns them off) cost NVK about
11 ms of median GPU and make no difference on deko3d (25.6 FPS with wide ones). Against NVK with wide
vertices, deko3d is slower: solid terrain is 93% of the heavy frame on both backends, and takes about
56 ms on deko3d against about 38 ms on NVK for the same 650–775 draws. Build 40, whose terrain shader
is simpler, takes 24–27 ms. Closing both gaps is the current work.

What moved deko3d from "before" to "now", as measured on the console:

| Change | Effect |
| --- | --- |
| Indirect multi-draws as one macro call | gpfifo entries per submission ~580 → 26; CPU 32 → 6.4 ms |
| Submission doesn't wait for the GPU (a backlog instead) | the CPU no longer waits for the previous frame |
| Barriers only for real hazards | full barriers ~43 → 9 per frame |
| Occlusion culling on deko3d | −2.4 ms GPU where terrain is hidden; about +1 ms in this flight |
| Queue command memory 64 KiB → 1 MiB | `dkQueueSubmitCommands` 16–30 → 0.01 ms per frame; 1% low 6.2 → 16.1 FPS, p99 116.5 → 60 ms |

Tried without gain: hardware compression of colour targets (`DkImageFlags_HwCompression`: GPU 36.3
against 35.9 ms) and Mailbox (above).

Both renderers are GPU-bound here. With Fifo a frame takes 33.4 or 50 ms (two or three refreshes),
and deko3d's GPU time sits at the two-refresh line, so small GPU savings don't show in the FPS. By
Resolution Scale (deko3d, one run each except 100%):

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

## Known issues

- On the title screen, GPU time per frame grew from 6–8 ms to 19–23 ms after minutes in the menus
  (seen on deko3d during the input-lag work, not investigated).
- The splitscreen pad reader and ZL/ZR (read as digital triggers) don't keep taps shorter than a
  frame yet.
- Whether deko3d's ZCULL (the hardware's coarse depth rejection) engages for Horizon's depth buffer
  is unmeasured.
- The effect of render distance and far-terrain detail on deko3d's GPU time is untested.
