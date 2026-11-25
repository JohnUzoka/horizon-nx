# wgpu_deko3d

Generated from bevy-horizon's `crates/wgpu_deko3d` (https://github.com/JohnUzoka/bevy-horizon)
by its `scripts/vendor_deko3d.sh`, at commit
34191a09491e794a086bd3a7fce8637128562b8c.
Do not edit these files by hand: change the backend in bevy-horizon and run the script again.

A wgpu-hal backend on deko3d, the Nintendo Switch's own GPU API; `wgpu_deko3d::instance()` makes a
`wgpu::Instance` on it. Building it for the Switch needs `BEVY_HORIZON_UAM_LIB` set to a directory
holding `libuam.a` and `libdeko3d_bh.a` (bevy-horizon's `deko3d/lib`, from its
`scripts/build_uam.sh`). An instance made with `wgpu::Instance::from_hal` takes wgpu-core's default
flags, so the game's wgpu-core patch should drop `VALIDATION_INDIRECT_CALL` there as bevy-horizon's
does. Licensed MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). bevy-horizon's
`docs/deko3d.md` has the design and limits.
