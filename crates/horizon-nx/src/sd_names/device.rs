//! The SD card's newlib device, with non-ASCII names in ASCII (`sd_names.rs`): a copy of
//! libnx's `fsdev` device table whose path operations hand libnx each path in its card form
//! (adopting a folder an earlier build wrote under the raw name first), and whose listings give
//! back the names those forms stand for. Handle operations (read, write, seek, ...) stay libnx's
//! own.

use core::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::sync::{Mutex, OnceLock};

type R = *mut c_void;
/// newlib's `struct stat` and `struct statvfs`, passed through as they are.
type Stat = c_void;

/// newlib `devoptab_t` (`sys/iosupport.h`, devkitPro's newlib), field for field.
#[repr(C)]
#[derive(Clone, Copy)]
struct DevOptab {
    name: *const c_char,
    struct_size: usize,
    open_r: Option<unsafe extern "C" fn(R, *mut c_void, *const c_char, c_int, c_int) -> c_int>,
    close_r: Option<unsafe extern "C" fn(R, *mut c_void) -> c_int>,
    write_r: Option<unsafe extern "C" fn(R, *mut c_void, *const c_char, usize) -> isize>,
    read_r: Option<unsafe extern "C" fn(R, *mut c_void, *mut c_char, usize) -> isize>,
    seek_r: Option<unsafe extern "C" fn(R, *mut c_void, i64, c_int) -> i64>,
    fstat_r: Option<unsafe extern "C" fn(R, *mut c_void, *mut Stat) -> c_int>,
    stat_r: Option<unsafe extern "C" fn(R, *const c_char, *mut Stat) -> c_int>,
    link_r: Option<unsafe extern "C" fn(R, *const c_char, *const c_char) -> c_int>,
    unlink_r: Option<unsafe extern "C" fn(R, *const c_char) -> c_int>,
    chdir_r: Option<unsafe extern "C" fn(R, *const c_char) -> c_int>,
    rename_r: Option<unsafe extern "C" fn(R, *const c_char, *const c_char) -> c_int>,
    mkdir_r: Option<unsafe extern "C" fn(R, *const c_char, c_int) -> c_int>,
    dir_state_size: usize,
    diropen_r: Option<unsafe extern "C" fn(R, *mut c_void, *const c_char) -> *mut c_void>,
    dirreset_r: Option<unsafe extern "C" fn(R, *mut c_void) -> c_int>,
    dirnext_r: Option<unsafe extern "C" fn(R, *mut c_void, *mut c_char, *mut Stat) -> c_int>,
    dirclose_r: Option<unsafe extern "C" fn(R, *mut c_void) -> c_int>,
    statvfs_r: Option<unsafe extern "C" fn(R, *const c_char, *mut Stat) -> c_int>,
    ftruncate_r: Option<unsafe extern "C" fn(R, *mut c_void, i64) -> c_int>,
    fsync_r: Option<unsafe extern "C" fn(R, *mut c_void) -> c_int>,
    device_data: *mut c_void,
    chmod_r: Option<unsafe extern "C" fn(R, *const c_char, u32) -> c_int>,
    fchmod_r: Option<unsafe extern "C" fn(R, *mut c_void, u32) -> c_int>,
    rmdir_r: Option<unsafe extern "C" fn(R, *const c_char) -> c_int>,
    lstat_r: Option<unsafe extern "C" fn(R, *const c_char, *mut Stat) -> c_int>,
    utimes_r: Option<unsafe extern "C" fn(R, *const c_char, *const c_void) -> c_int>,
    fpathconf_r: Option<unsafe extern "C" fn(R, *mut c_void, c_int) -> c_long>,
    pathconf_r: Option<unsafe extern "C" fn(R, *const c_char, c_int) -> c_long>,
    symlink_r: Option<unsafe extern "C" fn(R, *const c_char, *const c_char) -> c_int>,
    readlink_r: Option<unsafe extern "C" fn(R, *const c_char, *mut c_char, usize) -> isize>,
}
const _: () = assert!(size_of::<DevOptab>() == crate::logging::ffi::DEVOPTAB_WORDS * 8);

struct Original(DevOptab);
// SAFETY: a copy of libnx's static table: function pointers, a name and libnx's device record,
// only ever read.
unsafe impl Send for Original {}
unsafe impl Sync for Original {}

/// libnx's own SD card device, which every operation ends up in.
static ORIGINAL: OnceLock<Original> = OnceLock::new();
/// The device in newlib's table, which holds a pointer to it for the rest of the process.
static NAMED: OnceLock<Original> = OnceLock::new();

fn original() -> &'static DevOptab {
    // Set before the device that calls this is in newlib's table.
    &ORIGINAL.get().expect("sd_names: the SD card device isn't installed").0
}

unsafe extern "C" {
    /// libsysbase: the device a name such as `sdmc:` names (a `devoptab_t`; `compat.rs` declares
    /// it with this signature too).
    fn GetDeviceOpTab(name: *const c_char) -> *const usize;
}

/// readdir's name buffer: `NAME_MAX` + 1.
const NAME_BUF: usize = 256;

/// Card paths of non-ASCII parts known to be on the card in their ASCII form: no earlier
/// build's folder to look for there again.
static PRESENT: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Whether libnx's device finds `path` (a card path).
fn found(r: R, path: &str) -> bool {
    let Ok(path) = CString::new(path) else {
        return false;
    };
    // newlib's `struct stat` is 128 bytes on AArch64.
    let mut st = [0u64; 32];
    unsafe { original().stat_r.unwrap()(r, path.as_ptr(), st.as_mut_ptr().cast()) == 0 }
}

/// [`super::adopt_legacy`] through libnx's device, leaving `r`'s errno as it was.
fn adopt(r: R, path: &str) {
    let errno = unsafe { *r.cast::<c_int>() };
    let mut present = PRESENT.lock().unwrap_or_else(|e| e.into_inner());
    super::adopt_legacy(
        path,
        |card| {
            if present.contains(card) {
                return true;
            }
            let there = found(r, card);
            // Only ASCII forms: a raw name found is renamed next.
            if there && card.is_ascii() {
                present.insert(card.to_owned());
            }
            there
        },
        |raw, card| {
            let (Ok(raw_c), Ok(card_c)) = (CString::new(raw), CString::new(card)) else {
                return;
            };
            let rc = unsafe { original().rename_r.unwrap()(r, raw_c.as_ptr(), card_c.as_ptr()) };
            crate::logging::write_log_line(format_args!(
                "[Files] {raw}: written by an earlier build under its raw name, which no listing shows; {}",
                if rc == 0 {
                    format!("renamed to {card}, so it lists again")
                } else {
                    format!("could NOT rename it to {card} (errno {})", unsafe { *r.cast::<c_int>() })
                }
            ));
        },
    );
    unsafe { *r.cast::<c_int>() = errno };
}

/// Calls `f` with `path` as the SD card holds it. ASCII paths go through as they are.
unsafe fn on_card<T>(r: R, path: *const c_char, f: impl FnOnce(*const c_char) -> T) -> T {
    let asked = unsafe { CStr::from_ptr(path) };
    if asked.to_bytes().is_ascii() {
        return f(path);
    }
    let Ok(asked) = asked.to_str() else {
        return f(path);
    };
    adopt(r, asked);
    // From a C string: no NUL inside, and the ASCII form adds none.
    let card = CString::new(super::to_fs(asked).into_owned()).unwrap_or_default();
    f(card.as_ptr())
}

macro_rules! one_path {
    ($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty) => {
        unsafe extern "C" fn $name(r: R, path: *const c_char, $($arg: $ty),*) -> $ret {
            unsafe { on_card(r, path, |path| original().$name.unwrap()(r, path, $($arg),*)) }
        }
    };
}

one_path!(stat_r(st: *mut Stat) -> c_int);
one_path!(lstat_r(st: *mut Stat) -> c_int);
one_path!(unlink_r() -> c_int);
one_path!(chdir_r() -> c_int);
one_path!(mkdir_r(mode: c_int) -> c_int);
one_path!(statvfs_r(buf: *mut Stat) -> c_int);
one_path!(chmod_r(mode: u32) -> c_int);
one_path!(rmdir_r() -> c_int);
one_path!(utimes_r(times: *const c_void) -> c_int);
one_path!(pathconf_r(name: c_int) -> c_long);

unsafe extern "C" fn open_r(
    r: R,
    file: *mut c_void,
    path: *const c_char,
    flags: c_int,
    mode: c_int,
) -> c_int {
    unsafe { on_card(r, path, |path| original().open_r.unwrap()(r, file, path, flags, mode)) }
}

unsafe extern "C" fn rename_r(r: R, old: *const c_char, new: *const c_char) -> c_int {
    unsafe {
        on_card(r, old, |old| {
            on_card(r, new, |new| original().rename_r.unwrap()(r, old, new))
        })
    }
}

unsafe extern "C" fn link_r(r: R, existing: *const c_char, new: *const c_char) -> c_int {
    unsafe {
        on_card(r, existing, |existing| {
            on_card(r, new, |new| original().link_r.unwrap()(r, existing, new))
        })
    }
}

unsafe extern "C" fn diropen_r(r: R, dir: *mut c_void, path: *const c_char) -> *mut c_void {
    unsafe { on_card(r, path, |path| original().diropen_r.unwrap()(r, dir, path)) }
}

unsafe extern "C" fn dirnext_r(r: R, dir: *mut c_void, name: *mut c_char, st: *mut Stat) -> c_int {
    let rc = unsafe { original().dirnext_r.unwrap()(r, dir, name, st) };
    if rc == 0 {
        let listed = unsafe { CStr::from_ptr(name) }.to_bytes();
        // One that won't fit readdir's buffer stays as it came: passed back, it opens as it is.
        if let Some(utf8) = super::from_fs(listed)
            && utf8.len() < NAME_BUF
        {
            unsafe {
                core::ptr::copy_nonoverlapping(utf8.as_ptr(), name.cast::<u8>(), utf8.len());
                *name.add(utf8.len()) = 0;
            }
        }
    }
    rc
}

/// Puts the SD card device with ASCII names in newlib's table, in place of libnx's (newlib's
/// `AddDevice` replaces the device of the same name, at its index). Call it once at boot,
/// before any other thread runs.
pub(crate) fn install() -> Result<(), &'static str> {
    let sd = unsafe { GetDeviceOpTab(c"sdmc:".as_ptr()) }.cast::<DevOptab>();
    if sd.is_null() {
        return Err("no SD card device in newlib's table");
    }
    let o = unsafe { sd.read() };
    if o.open_r.is_none() || o.diropen_r.is_none() || o.dirnext_r.is_none() {
        return Err("the SD card device lacks open or listing");
    }
    if ORIGINAL.set(Original(o)).is_err() {
        return Err("already installed");
    }
    let mut named = o;
    macro_rules! wrap {
        ($($field:ident),*) => {$(
            named.$field = o.$field.map(|_| $field as _);
        )*};
    }
    wrap!(
        open_r, stat_r, lstat_r, link_r, unlink_r, chdir_r, rename_r, mkdir_r, diropen_r,
        dirnext_r, statvfs_r, chmod_r, rmdir_r, utimes_r, pathconf_r
    );
    let named = &NAMED.get_or_init(|| Original(named)).0;
    if unsafe { crate::logging::ffi::AddDevice((named as *const DevOptab).cast()) } < 0 {
        return Err("newlib's device table is full");
    }
    Ok(())
}
