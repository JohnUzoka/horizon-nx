# rlsf 0.2.3, vendored

From crates.io (`rlsf` 0.2.3, MIT OR Apache-2.0, by yvt). The only change: `src/lib.rs`'s
`if_supported_target!` leaves out `target_os = "horizon"`, which counts as unix but has no `mmap`
for `GlobalTlsf`. horizon-nx's allocator uses the `Tlsf` core (`crates/horizon-nx/src/allocator/tlsf_heap.rs`).
