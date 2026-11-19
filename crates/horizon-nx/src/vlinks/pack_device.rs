//! The SD card device's side of the resources pack (`pack.rs`): which folder is packed, the open
//! pack, the files opened from it, and the background build.

use super::pack::{self, Entry, Found, Index, Kind};
use super::{Parts, normalize, render};
use core::ffi::{c_int, c_void};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

type Stat = libc::stat;

unsafe extern "C" {
    /// libnx: reads `size` bytes at `off` of an open file, by offset (no shared position).
    fn fsFileRead(
        f: *mut c_void,
        off: i64,
        buf: *mut c_void,
        size: u64,
        option: u32,
        read: *mut u64,
    ) -> u32;
}

/// A packed folder in use.
pub(super) struct Mount {
    /// The folder, as absolute parts.
    root: Parts,
    folder: PathBuf,
    pub(super) index: Index,
    /// libnx's file record for the open pack (`fsdev_file_t`, whose first field is the `FsFile`).
    file: Vec<u64>,
}

static MOUNT: OnceLock<Mount> = OnceLock::new();
/// The folder [`use_pack`] was asked about, to build its pack later when it has none.
static FOLDER: OnceLock<(Parts, PathBuf)> = OnceLock::new();
/// Changes noted while no pack is in use (lines already in the changes file).
static NOTED: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);
static BUILDING: AtomicBool = AtomicBool::new(false);

fn log(line: std::fmt::Arguments) {
    crate::logging::write_log_line(line);
    log::info!("{line}");
}

fn disabled() -> bool {
    std::env::var_os("HORIZON_NX_PACK").is_some_and(|v| v == "0")
}

/// Uses `folder`'s pack from now on, when it has one that still fits it. Call once, before the
/// game reads the folder.
pub fn use_pack(folder: &Path) {
    if disabled() {
        log(format_args!("[Pack] off (HORIZON_NX_PACK=0)"));
        return;
    }
    let Some(text) = folder.to_str() else { return };
    let root = normalize(text, &super::devoptab::cwd());
    let folder = PathBuf::from(render(&root));
    let _ = FOLDER.set((root.clone(), folder.clone()));
    let started = Instant::now();
    match open(&root, &folder) {
        Ok(mount) => {
            let files = mount.index.entries.len();
            if MOUNT.set(mount).is_ok() {
                log(format_args!(
                    "[Pack] {} in use: {files} files and folders, opened in {} ms",
                    pack::pack_path(&folder).display(),
                    started.elapsed().as_millis()
                ));
            }
        }
        Err(why) => log(format_args!(
            "[Pack] {} not used: {why}; it is built once the game has loaded",
            pack::pack_path(&folder).display()
        )),
    }
}

fn open(root: &[String], folder: &Path) -> Result<Mount, String> {
    let path = pack::pack_path(folder);
    let c_path = CString::new(path.to_str().unwrap_or_default()).map_err(|e| e.to_string())?;
    let o = super::devoptab::original();
    let mut file = super::devoptab::file_struct();
    let fd = file.as_mut_ptr().cast::<c_void>();
    let r = super::devoptab::reent();
    if unsafe { o.open_r.unwrap()(r, fd, c_path.as_ptr(), libc::O_RDONLY, 0) } == -1 {
        return Err("there is none yet".into());
    }
    let close = |file: &mut Vec<u64>| unsafe {
        o.close_r.unwrap()(r, file.as_mut_ptr().cast());
    };
    let len = unsafe { o.seek_r.unwrap()(r, fd, 0, libc::SEEK_END) };
    let read = |at: u64, n: usize| -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; n];
        let mut got = 0u64;
        let rc = unsafe {
            fsFileRead(
                fd,
                at as i64,
                bytes.as_mut_ptr().cast(),
                n as u64,
                0,
                &mut got,
            )
        };
        if rc != 0 || got as usize != n {
            return Err(format!("reading it failed (0x{rc:x})"));
        }
        Ok(bytes)
    };
    let index = (|| {
        if len < 24 + 8 {
            return Err("it is cut short".to_string());
        }
        let (at, n) = pack::footer(&read(len as u64 - 24, 24)?).map_err(|e| e.to_string())?;
        pack::decode(&read(at, n as usize)?).map_err(|e| e.to_string())
    })();
    let index = match index {
        Ok(index) => index,
        Err(e) => {
            close(&mut file);
            return Err(e);
        }
    };
    if let Ok(changes) = std::fs::read_to_string(pack::changes_path(folder)) {
        index.load_changes(&changes);
    }
    let now = pack::top_level(folder).map_err(|e| {
        close(&mut file);
        format!("listing the folder failed: {e}")
    })?;
    if !index.fits(&now) {
        close(&mut file);
        return Err("the folder changed since it was packed".into());
    }
    Ok(Mount {
        root: root.to_vec(),
        folder: folder.to_path_buf(),
        index,
        file,
    })
}

/// Packs the folder in the background when it has no pack in use. Call once the game has loaded,
/// so the build doesn't slow the loading down.
pub fn build_in_background() {
    if disabled() || MOUNT.get().is_some() || BUILDING.swap(true, Relaxed) {
        return;
    }
    let Some((_, folder)) = FOLDER.get().cloned() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("pack-build".into())
        .spawn(move || {
            let started = Instant::now();
            log(format_args!(
                "[Pack] packing {} into {} for the next start",
                folder.display(),
                pack::pack_path(&folder).display()
            ));
            let result = pack::build(&folder, |files| {
                if files % 4000 == 0 {
                    log(format_args!(
                        "[Pack] {files} files packed, {:.0} s",
                        started.elapsed().as_secs_f64()
                    ));
                }
            });
            match result {
                Ok((files, bytes)) => log(format_args!(
                    "[Pack] done: {files} files, {:.1} MiB in {:.0} s; used from the next start",
                    bytes as f64 / 1048576.0,
                    started.elapsed().as_secs_f64()
                )),
                Err(e) => log(format_args!(
                    "[Pack] packing failed: {e}; the folder is used as it is"
                )),
            }
        });
    if spawned.is_err() {
        BUILDING.store(false, Relaxed);
    }
}

/// The pack in use and `real`'s path in it, when `real` is under the packed folder.
pub(super) fn locate(real: &[String]) -> Option<(&'static Mount, String)> {
    let mount = MOUNT.get()?;
    if real.len() < mount.root.len() || real[..mount.root.len()] != mount.root[..] {
        return None;
    }
    Some((mount, real[mount.root.len()..].join("/")))
}

/// Notes that the game changes `real` (when it's under the packed folder): from now on it comes
/// from the folder.
///
/// Noted without a pack in use too (the first start, while the pack is built): the pack built then
/// may hold the file as it was before the change.
pub(super) fn changing(real: &[String]) {
    let (folder, rel, lines) = if let Some((mount, rel)) = locate(real) {
        let Some(lines) = mount.index.note_change(&rel) else {
            return;
        };
        (&mount.folder, rel, lines)
    } else {
        let Some((root, folder)) = FOLDER.get() else {
            return;
        };
        if real.len() < root.len() || real[..root.len()] != root[..] {
            return;
        }
        let rel = real[root.len()..].join("/");
        let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
        if !noted
            .get_or_insert_with(Default::default)
            .insert(rel.clone())
        {
            return;
        }
        let parent = rel.rsplit_once('/').map_or("", |(dir, _)| dir);
        let mut lines = format!("P {rel}\n");
        if !rel.is_empty() {
            lines.push_str(&format!("L {parent}\n"));
        }
        (folder, rel, lines)
    };
    use std::io::Write;
    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(pack::changes_path(folder))
        .and_then(|mut f| f.write_all(lines.as_bytes()));
    if let Err(e) = appended {
        log(format_args!("[Pack] could not note a change to {rel}: {e}"));
    }
}

/// Fills `st` from the index.
pub(super) unsafe fn fill(st: *mut Stat, entry: &Entry) {
    unsafe {
        core::ptr::write_bytes(st, 0, 1);
        (*st).st_mode = match entry.kind {
            Kind::Dir => libc::S_IFDIR | 0o777,
            _ => libc::S_IFREG | 0o666,
        };
        (*st).st_nlink = 1;
        (*st).st_size = entry.size as _;
        (*st).st_blksize = 512;
        (*st).st_mtim.tv_sec = entry.mtime as _;
        (*st).st_atim.tv_sec = entry.mtime as _;
        (*st).st_ctim.tv_sec = entry.mtime as _;
    }
}

/// Whether the index can say that the folder `dir` (`sdmc:/...`) holds no links' manifest, so
/// looking for one on the card can be skipped.
pub(super) fn no_manifest(dir: &str) -> bool {
    let parts = normalize(dir, "sdmc:/");
    let Some((mount, rel)) = locate(&parts) else {
        return false;
    };
    let manifest = if rel.is_empty() {
        super::MANIFEST.to_string()
    } else {
        format!("{rel}/{}", super::MANIFEST)
    };
    matches!(
        mount.index.find(&rel),
        Found::Indexed(Entry {
            kind: Kind::Dir,
            ..
        })
    ) && mount.index.find(&manifest) == Found::Missing
}

/// A file opened from the pack.
struct Open {
    start: u64,
    size: u64,
    at: u64,
    entry: Entry,
}

/// Files open from the pack, by libsysbase's file record (which it gives every call on them).
static OPEN: Mutex<Option<HashMap<usize, Open>>> = Mutex::new(None);
static OPEN_COUNT: AtomicUsize = AtomicUsize::new(0);

fn open_files() -> std::sync::MutexGuard<'static, Option<HashMap<usize, Open>>> {
    OPEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Opens `entry` (packed at `start`) as the file record `file`.
pub(super) fn open_packed(file: *mut c_void, start: u64, entry: &Entry) {
    let mut open = open_files();
    open.get_or_insert_with(HashMap::new).insert(
        file as usize,
        Open {
            start,
            size: entry.size,
            at: 0,
            entry: *entry,
        },
    );
    OPEN_COUNT.fetch_add(1, Relaxed);
}

/// Whether `file` is a record [`open_packed`] filled. A record libnx filled is never one: closing
/// a packed file takes it out before libsysbase can reuse its memory.
pub(super) fn is_packed(file: *mut c_void) -> bool {
    OPEN_COUNT.load(Relaxed) != 0
        && open_files()
            .as_ref()
            .is_some_and(|open| open.contains_key(&(file as usize)))
}

pub(super) fn read(r: *mut c_void, file: *mut c_void, buf: *mut u8, len: usize) -> isize {
    let Some((start, at, size)) = open_files()
        .as_ref()
        .and_then(|open| open.get(&(file as usize)))
        .map(|f| (f.start, f.at, f.size))
    else {
        return super::devoptab::fail(r, libc::EBADF) as isize;
    };
    let n = (size.saturating_sub(at)).min(len as u64);
    if n == 0 {
        return 0;
    }
    let mount = MOUNT.get().expect("a packed file without a pack");
    let mut got = 0u64;
    let rc = unsafe {
        fsFileRead(
            mount.file.as_ptr().cast_mut().cast(),
            (start + at) as i64,
            buf.cast(),
            n,
            0,
            &mut got,
        )
    };
    if rc != 0 {
        return super::devoptab::fail(r, libc::EIO) as isize;
    }
    if let Some(f) = open_files()
        .as_mut()
        .and_then(|open| open.get_mut(&(file as usize)))
    {
        f.at = at + got;
    }
    got as isize
}

pub(super) fn seek(r: *mut c_void, file: *mut c_void, pos: i64, whence: c_int) -> i64 {
    let mut open = open_files();
    let Some(f) = open
        .as_mut()
        .and_then(|open| open.get_mut(&(file as usize)))
    else {
        return super::devoptab::fail(r, libc::EBADF) as i64;
    };
    let base = match whence {
        libc::SEEK_SET => 0,
        libc::SEEK_CUR => f.at as i64,
        libc::SEEK_END => f.size as i64,
        _ => return super::devoptab::fail(r, libc::EINVAL) as i64,
    };
    match base.checked_add(pos) {
        Some(to) if to >= 0 => {
            f.at = to as u64;
            to
        }
        _ => super::devoptab::fail(r, libc::EINVAL) as i64,
    }
}

pub(super) unsafe fn fstat(file: *mut c_void, st: *mut Stat) -> c_int {
    let entry = open_files()
        .as_ref()
        .and_then(|open| open.get(&(file as usize)))
        .map(|f| f.entry);
    match entry {
        Some(entry) => {
            unsafe { fill(st, &entry) };
            0
        }
        None => -1,
    }
}

pub(super) fn close(file: *mut c_void) {
    if open_files()
        .as_mut()
        .and_then(|open| open.remove(&(file as usize)))
        .is_some()
    {
        OPEN_COUNT.fetch_sub(1, Relaxed);
    }
}

/// What the index says of a path under the packed folder, for the device's calls.
pub(super) enum Answer {
    /// Not under the packed folder, or the index leaves it to the folder.
    Folder,
    Missing,
    Entry(Entry),
}

pub(super) fn answer(real: &[String]) -> Answer {
    let Some((mount, rel)) = locate(real) else {
        return Answer::Folder;
    };
    match mount.index.find(&rel) {
        Found::Indexed(entry) => Answer::Entry(*entry),
        Found::Missing => Answer::Missing,
        Found::Folder => Answer::Folder,
    }
}

/// The names and entries of the folder `real` when the index can list it.
pub(super) fn listing(real: &[String]) -> Option<Vec<(String, Entry)>> {
    let (mount, rel) = locate(real)?;
    let names = mount.index.list(&rel)?;
    Some(
        names
            .iter()
            .filter_map(|name| {
                let child = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                match mount.index.find(&child) {
                    Found::Indexed(entry) => Some((name.clone(), *entry)),
                    _ => None,
                }
            })
            .collect(),
    )
}

/// The `Kind::Packed` offset of an entry, if it's packed.
pub(super) fn packed_at(entry: &Entry) -> Option<u64> {
    match entry.kind {
        Kind::Packed(at) => Some(at),
        _ => None,
    }
}
