# Horizon NX: Minecraft: Java Edition on the Nintendo Switch

<p>
  <img src="https://img.shields.io/badge/Minecraft-Java%2026.3-5D9C3F" alt="Minecraft Java 26.3">
  <img src="https://img.shields.io/badge/platform-Nintendo%20Switch%20(homebrew)-E60012" alt="Nintendo Switch homebrew">
  <img src="https://img.shields.io/badge/status-playable,%20experimental-F0A030" alt="Playable, experimental">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-3B2F7E" alt="MIT license"></a>
</p>

This is a fork of [JohnUzoka/horizon-nx](https://github.com/JohnUzoka/horizon-nx), the Nintendo Switch port of
[Horizon](https://github.com/horizon-rust/horizon): Minecraft: Java Edition 26.3 rewritten from scratch in Rust,
checked against the real game by hundreds of differential tests. Horizon runs natively on the Switch as homebrew,
with no Java and no emulation.

The fork builds the Switch version in the open: the original port's build setup isn't public, so this repository
rebuilds it, fixes what broke on a real console, and adds Microsoft sign-in on the console itself.

> **Unofficial.** Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.
> This repository contains no Minecraft code, assets or game files: you need your own copy of Minecraft: Java Edition.
> Running homebrew requires a Switch with custom firmware; do it at your own risk.

## Status

Tested on a Switch with Atmosphère, November 2026. Current release: **v1.0.0**.

| | |
|---|---|
| Single player: create and play worlds | ✅ Works |
| Joy-Con and touch screen | ✅ Works |
| HOME button (leave and come back) | ✅ Works with Nintendo's HOME menu ([custom HOME menus](#the-console-freezes-when-i-press-home) can freeze) |
| Microsoft sign-in on the title screen | ✅ Works (approved by Mojang) |
| Multiplayer, online-mode servers included | ✅ Works (sign in first for online-mode servers) |
| Server resource packs | ❌ Disconnects (for example Hypixel SkyBlock); being worked on |
| Horizon mods | ❌ Not on the Switch yet; being worked on |
| In-game updates | ✅ From this repository's releases (1.0.0 and later) |
| Docked mode | ❔ Not tested yet |
| Frame rate | Around 35–45 fps in handheld mode |
| First start | Slow: a few minutes to the title screen, more to open a world the first time |

## What you need

- A Nintendo Switch running **[Atmosphère](https://github.com/Atmosphere-NX/Atmosphere)** custom firmware.
- About **1 GB free** on the SD card.
- A computer (Windows, macOS or Linux) to prepare the game files, and **your own copy of Minecraft: Java Edition**.
- To play on online servers: the Microsoft account that owns Minecraft: Java Edition.

## Install

### 1. Get the Switch build

Download `horizon-switch-….zip` from the [latest release](https://github.com/KoHaku743/horizon-nx/releases/latest)
and unzip it to the root of the SD card. It contains:

```
switch/horizon/horizon.nro
switch/horizon/client.txt
```

Builds of every change are also on [GitHub Actions](https://github.com/KoHaku743/horizon-nx/actions/workflows/switch-nro.yml?query=branch%3Aswitch-v010+is%3Asuccess)
(the `horizon-switch-…` file at the bottom of a run; you need to be signed in to GitHub).

`client.txt` makes `horizon.nro` start the game; without it, it starts a dedicated server.

### 2. Prepare the game files on your computer

Horizon reads its data from your own Minecraft, converted once by Horizon's tools:

1. Install the **Horizon launcher** from [playhorizon.dev](https://playhorizon.dev), sign in with your Microsoft
   account, and press **Play** once. It downloads Minecraft 26.3 for your account and prepares it.
2. Find the prepared folder: it's the folder that contains `horizon-resources.json`.
   - macOS / Linux: `find ~ -name horizon-resources.json 2>/dev/null`
   - Windows (PowerShell): `Get-ChildItem $env:USERPROFILE -Recurse -Filter horizon-resources.json -ErrorAction SilentlyContinue`

### 3. Copy everything to the SD card

```
SD card
└── switch/
    └── horizon/
        ├── horizon.nro
        ├── client.txt
        └── resources/        ← the contents of the prepared folder from step 2
            ├── horizon-resources.json
            ├── assets/
            ├── data/
            └── …
```

Copy the *contents* of the prepared folder into `switch/horizon/resources/`, not the folder itself.

**On a Mac**, Finder adds hidden `._` files next to every file it copies to the SD card, and the game can't read
them. Either copy over FTP or MTP (they never create these files), or clean the card before ejecting it:

```sh
dot_clean -m "/Volumes/YOUR SD CARD"
```

### 4. Start it

Horizon needs the memory of a full application, so **don't start it from the Album**. Hold **R** while starting
any game to open the homebrew menu as an application, then choose Horizon. (A title override or a forwarder works too.)

The first start is slow: the graphics driver compiles all of the game's shaders and keeps them on the SD card,
so later starts are faster.

## Microsoft account

On the title screen, **Sign In** (to the right of Realms) opens the Microsoft Account screen:

1. Press **Sign In**. The Switch shows a web address and a code.
2. Open the address on your phone or computer, enter the code, and sign in with the Microsoft account that owns
   Minecraft: Java Edition.
3. The Switch finishes on its own and shows your Minecraft name.

The sign-in is kept in `switch/horizon/account.json`, so you stay signed in after a restart. **Anyone with your SD
card can use that file to sign in as you** until you press **Sign Out**, which deletes it.

When signed in, worlds play under your real account. A world you played before under the offline name starts you
as a new player (as in Minecraft itself); the old character stays saved.

## Settings: `env.txt`

A text file `switch/horizon/env.txt`, next to `horizon.nro`, sets options at startup, one `NAME=value` per line
(`#` starts a comment). Nothing in it is needed to play. Useful ones:

| Setting | What it does |
|---|---|
| `NAK_DEBUG=-plat,-ofold` | The graphics shader compiler's optimizations: the build's default. `-name` turns one off. See [the GPU hang](#the-game-closes-about-10-seconds-after-the-world-appears). |
| `MESA_SHADER_CACHE_DISABLE=true` | Compile every shader at every start instead of keeping them on the SD card. |
| `HORIZON_MSA_CLIENT_ID=…` | A different Azure app for the Microsoft sign-in. |
| `HORIZON_NX_CPU_BOOST=0` | Keep normal CPU clocks while loading (they're raised by default). |

## Troubleshooting

The game's logs are in `switch/horizon/`: `log.txt`, `logs/latest.log`, `panic.txt` and `crash-reports/`.
Atmosphère's own reports are in `atmosphere/crash_reports/`.

### The game closes after loading with "expected value at line 1 column 1"

macOS's hidden `._` files are in the resources folder. Run `dot_clean -m` on the SD card (see
[step 3](#3-copy-everything-to-the-sd-card)), or copy over FTP or MTP.

### The game closes about 10 seconds after the world appears

The log shows `channel 2 lost: … (fifo idle timeout)`: the GPU hung. Two optimizations of the public Switch
graphics driver's shader compiler, `plat` and `ofold`, each hang the GPU once terrain is drawn (found by turning
them on one at a time on a console). Builds of this fork turn both off by default. If you set `NAK_DEBUG` in
`env.txt`, keep `-plat,-ofold` in it.

### The console freezes when I press HOME

Custom HOME menus (SwitchU, and front ends built on it such as HomeNX and CardsNX) froze the console when HOME
was pressed over Horizon; with Nintendo's own HOME menu it works. Remove or turn off the custom HOME menu.

### The world takes very long to load the first time

The first time a world opens, the driver compiles the terrain's shaders and the world generates around you.
Later starts read the compiled shaders from the SD card (`.mesa/` at the card's root; deleting it is safe).

## Building

The build runs in a container (Docker or Podman, on Linux):

```sh
git clone -b switch-v010 https://github.com/KoHaku743/horizon-nx
cd horizon-nx
packaging/switch/build.sh all     # the toolchain image, the graphics driver (slow, once), then the NRO
```

It clones [bevy-horizon](https://github.com/JohnUzoka/bevy-horizon) (the Switch target, toolchain image and
patched libraries) and [mesa-switch](https://github.com/danfromtico/mesa-switch) (the graphics driver) next to the
repository. The result is `target/aarch64-nintendo-horizon/release/horizon.nro`. Every push to `switch-v010` runs
the same build in [GitHub Actions](.github/workflows/switch-nro.yml); its log is also kept on the `ci-logs`
branch. To publish a release, run the "Switch NRO" workflow by hand (**Run workflow**) with a version in
`release` (for example `v0.1.0-nx.2`): it builds, tags the commit and attaches the zip. Details:
[docs/switch.md](docs/switch.md).

### Branches

| Branch | What it is |
|---|---|
| **`switch-v010`** | **The one to use.** The source of the original port's release 0.1.0, with this fork's build setup and fixes. |
| `switch` | The original port's newer development branch. It builds, but on a console it loads 2–3× slower and doesn't run well. |
| `switch-build` | The build setup's first version, on top of `switch`. |
| `switch-skip-mac-metadata`, `switch-portable-data-file` | Fixes for macOS `._` files and a data file that works from the SD card; not yet moved to `switch-v010`. |
| `ci-logs` | The CI build logs. |

### What this fork changes

- **A public build:** `packaging/switch/` and the patched graphics, window and network libraries in `third_party/`,
  which the original port built in a private setup.
- **GPU hang fixed:** the shader compiler's `plat` and `ofold` optimizations are off by default (`crates/horizon-nx`).
- **HOME:** the game waits in the background without drawing instead of asking the system to suspend it.
- **Microsoft sign-in on the console** (`crates/horizon-client-render/src/msa.rs`, the title screen's account screen).
- **No mimalloc on the Switch:** the console's own allocator is used.

## Credits

- **[Horizon](https://github.com/horizon-rust/horizon)**: Ludovic Marie and the Horizon contributors.
- **The Switch port**: [JohnUzoka](https://github.com/JohnUzoka) ([horizon-nx](https://github.com/JohnUzoka/horizon-nx),
  [bevy-horizon](https://github.com/JohnUzoka/bevy-horizon)).
- **The Switch graphics driver**: [mesa-switch](https://github.com/danfromtico/mesa-switch) by danfromtico and
  NaGaa95, built on Mesa's NVK.
- **The Switch homebrew toolchain**: [devkitPro](https://devkitpro.org) and [libnx](https://github.com/switchbrew/libnx).
- **This fork**: [KoHaku743](https://github.com/KoHaku743).

## License

MIT, as Horizon: see [LICENSE](LICENSE). Third-party code and its licenses: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Minecraft is a trademark of Mojang Synergies AB.
