//! Switch device paths (`sdmc:/switch/horizon`, `romfs:/resources`). libnx resolves them from
//! their device, but Rust's Unix `Path` reads them as relative.
use std::path::{Path, PathBuf};

/// `std::path::absolute`, except that a Switch device path is already rooted and comes back as it
/// is, not after the working directory (`sdmc:/switch/horizon/sdmc:/switch/horizon/resources`).
pub fn absolute<P: AsRef<Path>>(path: P) -> std::io::Result<PathBuf> {
    let path = path.as_ref();
    #[cfg(target_os = "horizon")]
    if path
        .components()
        .next()
        .is_some_and(|first| first.as_os_str().as_encoded_bytes().ends_with(b":"))
    {
        return Ok(path.to_path_buf());
    }
    std::path::absolute(path)
}
