# wgpu_deko3d

Generated from bevy-horizon's `crates/wgpu_deko3d` (https://github.com/JohnUzoka/bevy-horizon)
by its `scripts/vendor_deko3d.sh`, at commit
164e7e45a2c524fc0ebbf0e68129354140aa8af9, with uncommitted changes.
Do not edit these files by hand: change the backend in bevy-horizon and run the script again.

A wgpu-hal backend on deko3d, the Nintendo Switch's own GPU API; `wgpu_deko3d::instance()` makes a
`wgpu::Instance` on it. Building it for the Switch needs `BEVY_HORIZON_UAM_LIB` set to a directory
holding `libuam.a` (bevy-horizon's `deko3d/lib`, from its `scripts/build_uam.sh`).
bevy-horizon's `docs/deko3d.md` has the design and limits.
