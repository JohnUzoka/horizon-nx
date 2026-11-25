#!/bin/sh
# Builds horizon-client for the Switch (aarch64-nintendo-horizon) in bevy-horizon's toolchain
# container and packs it into an NRO with pack.sh.
# Usage: sh packaging/switch/build.sh [vulkan|deko3d] [output.nro]
#   vulkan  draws through Vulkan on NVK (horizon-nx/vulkan), the released renderer (default)
#   deko3d  draws through deko3d, the console's own GPU API (third_party/wgpu_deko3d); no NVK
#   output  where the NRO goes (default target/aarch64-nintendo-horizon/release/horizon-<renderer>.nro)
#
# What the container mounts, each overridable:
#   HX_BEVY_HORIZON  bevy-horizon checkout with the target spec, .cargo/config.toml and its crate
#                    patches (default ~/workshop/horizon-port/bevy-horizon)
#   HX_PATCHES       the workshop's crate patches and horizon-patches.toml
#                    (default ~/workshop/horizon-port/patches)
#   HX_NVK_LIB       vulkan: NVK's libvulkan_bh.a (default ~/bevy-horizon/nvk/lib)
#   HX_DEKO3D_LIB    deko3d: libuam.a from bevy-horizon's scripts/build_uam.sh, and its patched
#                    libdeko3d.a/libdeko3dd.a, which wgpu_deko3d links ahead of devkitPro's
#                    (default ~/bevy-horizon/deko3d/lib)
# Other settings:
#   HX_IMAGE         toolchain image (default bevy-horizon-toolchain:pre-curl, the v0.2.1 build's)
#   HX_JOBS          cargo's parallel jobs (default 2: each rustc on the client takes GBs of RAM)
#   HX_FEATURES      horizon-game's features besides the renderer's
#                    (default audio,controllers,horizon-extras)
#   HX_TARGET_DIR    a cargo target directory to mount instead of this checkout's target/
#   HORIZON_SWITCH_VERSION, HX_NACP_VERSION  passed through (release builds)
#   HORIZON_GAME_BUILD_ID  the build's identity (default: this checkout's commit, read here: a git
#                    worktree's .git points outside the mount, where the container's git can't go)
#
# std is built from source (-Z build-std) as its own workspace, locked by the Cargo.lock in the
# toolchain's rust-src, which pins crates.io's libc (3DS types). The config files' [patch] reaches
# that workspace, but cargo keeps the version the lock pins, so std's lock first moves onto the
# patched libc; the lock is the container's copy, gone with it (--rm). The grep stops the build if
# std would still use crates.io's: every `stat` would be misread on the console.
set -eu

renderer=${1:-vulkan}
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)
out=${2:-$root/target/aarch64-nintendo-horizon/release/horizon-$renderer.nro}
mkdir -p "$(dirname -- "$out")"
out_dir=$(CDPATH= cd -- "$(dirname -- "$out")" && pwd)
out_name=$(basename -- "$out")

bevy_horizon=${HX_BEVY_HORIZON:-$HOME/workshop/horizon-port/bevy-horizon}
patches=${HX_PATCHES:-$HOME/workshop/horizon-port/patches}
features=${HX_FEATURES:-audio,controllers,horizon-extras}

case $renderer in
vulkan)
    lib=${HX_NVK_LIB:-$HOME/bevy-horizon/nvk/lib}
    need=libvulkan_bh.a
    mount="$lib:/src/gpu-lib:ro"
    env_lib=HORIZON_NX_NVK_LIB=/src/gpu-lib
    features="$features,horizon-nx/vulkan"
    ;;
deko3d)
    lib=${HX_DEKO3D_LIB:-$HOME/bevy-horizon/deko3d/lib}
    need=libuam.a
    mount="$lib:/src/gpu-lib:ro"
    env_lib=BEVY_HORIZON_UAM_LIB=/src/gpu-lib
    features="$features,deko3d"
    ;;
*)
    printf 'Usage: %s [vulkan|deko3d] [output.nro]\n' "$0" >&2
    exit 2
    ;;
esac
if [ ! -f "$lib/$need" ]; then
    printf 'error: %s has no %s (set HX_%s_LIB)\n' "$lib" "$need" \
        "$(if [ "$renderer" = vulkan ]; then echo NVK; else echo DEKO3D; fi)" >&2
    exit 1
fi

target_mount=
if [ -n "${HX_TARGET_DIR:-}" ]; then
    mkdir -p "$HX_TARGET_DIR"
    target_mount="-v $HX_TARGET_DIR:/src/horizon/target"
fi
if [ -z "${HORIZON_GAME_BUILD_ID:-}" ]; then
    HORIZON_GAME_BUILD_ID=$(git -C "$root" rev-parse HEAD)
fi
export HORIZON_GAME_BUILD_ID

echo "renderer: $renderer; features: $features; output: $out_dir/$out_name"
# shellcheck disable=SC2086 # target_mount is empty or one -v argument
docker run --rm --ulimit core=0 \
    -v "$root:/src/horizon" \
    $target_mount \
    -v "$bevy_horizon:/src/bevy-horizon:ro" \
    -v "$patches:/src/patches:ro" \
    -v "$mount" \
    -v bh-cargo-registry:/usr/local/cargo/registry \
    -v "$out_dir:/out" \
    -w /src/horizon \
    -e "$env_lib" \
    -e CARGO_HTTP_MULTIPLEXING=false \
    -e FEATURES="$features" \
    -e JOBS="${HX_JOBS:-2}" \
    -e OUT_NAME="$out_name" \
    -e HORIZON_SWITCH_VERSION \
    -e HX_NACP_VERSION \
    -e HORIZON_GAME_BUILD_ID \
    "${HX_IMAGE:-bevy-horizon-toolchain:pre-curl}" \
    sh -euc '
        cfg="--config /src/bevy-horizon/.cargo/config.toml --config /src/patches/horizon-patches.toml"
        std_lib="$(rustc --print sysroot)/lib/rustlib/src/rust/library"
        cargo update $cfg -p libc --manifest-path "$std_lib/Cargo.toml"
        if grep -A2 "^name = \"libc\"" "$std_lib/Cargo.lock" | grep -q "^source = "; then
            echo "error: std would build against crates.io libc, not patches/libc" >&2
            exit 1
        fi
        cargo build --locked --release -j "$JOBS" \
            -Z build-std=core,alloc,std,panic_abort -Z json-target-spec \
            --target /src/bevy-horizon/aarch64-nintendo-horizon.json \
            $cfg --config "patch.crates-io.if-addrs.path=\"/src/horizon/packaging/switch/stubs/if-addrs\"" \
            -p horizon-game --bin horizon-client --no-default-features --features "$FEATURES"
        sh packaging/switch/pack.sh target/aarch64-nintendo-horizon/release/horizon-client "/out/$OUT_NAME"'
ls -l "$out"
