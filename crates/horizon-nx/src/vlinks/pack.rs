//! The game's resources folder as one file on the SD card.
//!
//! On the console every open of a file costs about 10 ms and every `stat` 5-15 ms (the measuring
//! build, 2026-11-19: 5594 opens took 58.6 s of one loading step), while reading runs at up to
//! 65 MiB/s. The prepared resources are 28,000 files, so loading waits on opening them, not on
//! reading them.
//!
//! So the folder is also packed into `<folder>.hzpak`: every file of up to [`MAX_PACKED`] bytes, one
//! after another, then an index of every file and folder (path, size, modification time, and where
//! a packed file's bytes are). The SD card device answers opens, `stat`s and listings under the
//! folder from the index, and reads from the one open pack, so a file costs one read.
//!
//! The folder stays as it is, and it stays the truth: the pack is a copy that the game reads when
//! nothing says the copy may be out of date.
//! - A pack is built in the background once the game has loaded ([`build`]), into a temporary
//!   file renamed into place only when complete, and used from the next start.
//! - A pack is used only while the folder's top level matches what it recorded (names, kinds and
//!   sizes): preparing the resources again changes their sizes.
//! - Whatever the game changes under the folder (writes, creates, deletes, renames: its caches)
//!   is listed in `<folder>.hzpak.changed` and read from the folder from then on, and a folder
//!   whose files changed is listed from the folder.
//! - `HORIZON_NX_PACK=0` turns it off; deleting the `.hzpak` makes it build again.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Files larger than this stay in the folder: they are few, so opening them costs little.
pub(crate) const MAX_PACKED: u64 = 8 << 20;
const MAGIC: &[u8; 8] = b"HZPAK001";
const END: &[u8; 8] = b"HZPAKEND";
const FOOTER: usize = 24;

/// What the index says of one path under the folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Dir,
    /// Its bytes are in the pack at this offset.
    Packed(u64),
    /// Too large to pack: read from the folder.
    Loose,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) kind: Kind,
    pub(crate) size: u64,
    /// Seconds since 1970, as the SD card reports them.
    pub(crate) mtime: i64,
}

/// Where a path under the folder is to be found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found<'a> {
    /// In the index.
    Indexed(&'a Entry),
    /// Not anywhere: the index knows the folder and it hasn't changed.
    Missing,
    /// Ask the folder itself.
    Folder,
}

/// One entry of the top level, to tell whether the folder is still the one packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Top {
    pub(crate) name: String,
    pub(crate) dir: bool,
    pub(crate) size: u64,
}

/// The paths the game changed under the folder.
#[derive(Default)]
pub(crate) struct Changed {
    /// These and everything under them come from the folder.
    pub(crate) paths: HashSet<String>,
    /// These folders are listed from the folder (their files still come from the pack).
    pub(crate) listings: HashSet<String>,
}

/// A loaded index.
pub(crate) struct Index {
    pub(crate) entries: HashMap<String, Entry>,
    /// Each folder's names, in the order packed.
    pub(crate) children: HashMap<String, Vec<String>>,
    pub(crate) top: Vec<Top>,
    pub(crate) changed: Mutex<Changed>,
}

fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

impl Index {
    /// Where `rel` (a path under the folder, `a/b`, `""` for the folder) is to be found.
    pub(crate) fn find(&self, rel: &str) -> Found<'_> {
        {
            let changed = self.changed.lock().unwrap_or_else(|e| e.into_inner());
            if !changed.paths.is_empty() {
                let mut at = rel;
                loop {
                    if changed.paths.contains(at) {
                        return Found::Folder;
                    }
                    if at.is_empty() {
                        break;
                    }
                    at = parent(at);
                }
            }
            if !self.entries.contains_key(rel) && changed.listings.contains(parent(rel)) {
                // A file the game made after packing.
                return Found::Folder;
            }
        }
        match self.entries.get(rel) {
            Some(entry) => Found::Indexed(entry),
            None => Found::Missing,
        }
    }

    /// The names in the folder `rel` when the index can list it.
    pub(crate) fn list(&self, rel: &str) -> Option<&[String]> {
        if !matches!(
            self.find(rel),
            Found::Indexed(Entry {
                kind: Kind::Dir,
                ..
            })
        ) {
            return None;
        }
        if self
            .changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .listings
            .contains(rel)
        {
            return None;
        }
        Some(self.children.get(rel).map_or(&[], Vec::as_slice))
    }

    /// Notes that the game changed `rel` (and so its folder's listing); the line to keep in the
    /// changes file when it is news.
    pub(crate) fn note_change(&self, rel: &str) -> Option<String> {
        let mut changed = self.changed.lock().unwrap_or_else(|e| e.into_inner());
        let mut lines = String::new();
        if changed.paths.insert(rel.to_owned()) {
            lines.push_str(&format!("P {rel}\n"));
        }
        if !rel.is_empty() && changed.listings.insert(parent(rel).to_owned()) {
            lines.push_str(&format!("L {}\n", parent(rel)));
        }
        (!lines.is_empty()).then_some(lines)
    }

    /// Takes in a changes file's lines.
    pub(crate) fn load_changes(&self, text: &str) {
        let mut changed = self.changed.lock().unwrap_or_else(|e| e.into_inner());
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("P ") {
                changed.paths.insert(path.to_owned());
            } else if let Some(dir) = line.strip_prefix("L ") {
                changed.listings.insert(dir.to_owned());
            }
        }
    }

    /// Whether `now` (the folder's top level today) is the folder this index was packed from.
    /// Entries the game changed don't count: its own caches live there.
    pub(crate) fn fits(&self, now: &[Top]) -> bool {
        let changed = self.changed.lock().unwrap_or_else(|e| e.into_inner());
        let kept = |t: &&Top| !changed.paths.contains(&t.name);
        let mut packed: Vec<&Top> = self.top.iter().filter(kept).collect();
        let mut today: Vec<&Top> = now.iter().filter(kept).collect();
        packed.sort_by(|a, b| a.name.cmp(&b.name));
        today.sort_by(|a, b| a.name.cmp(&b.name));
        packed == today
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u16).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pack index cut short",
            ));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn str(&mut self) -> io::Result<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pack index path not UTF-8"))
    }
}

/// The index block: entries, then the top level.
fn encode(entries: &[(String, Entry)], top: &[Top]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (rel, e) in entries {
        put_str(&mut out, rel);
        let (kind, offset) = match e.kind {
            Kind::Dir => (0u8, 0),
            Kind::Packed(offset) => (1, offset),
            Kind::Loose => (2, 0),
        };
        out.push(kind);
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&e.size.to_le_bytes());
        out.extend_from_slice(&e.mtime.to_le_bytes());
    }
    out.extend_from_slice(&(top.len() as u32).to_le_bytes());
    for t in top {
        put_str(&mut out, &t.name);
        out.push(u8::from(t.dir));
        out.extend_from_slice(&t.size.to_le_bytes());
    }
    out
}

pub(crate) fn decode(block: &[u8]) -> io::Result<Index> {
    let mut r = Reader(block);
    let count = r.u32()? as usize;
    let mut entries = HashMap::with_capacity(count);
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    for _ in 0..count {
        let rel = r.str()?;
        let kind = r.u8()?;
        let offset = r.u64()?;
        let size = r.u64()?;
        let mtime = r.u64()? as i64;
        let kind = match kind {
            0 => Kind::Dir,
            1 => Kind::Packed(offset),
            2 => Kind::Loose,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "pack index kind",
                ));
            }
        };
        if !rel.is_empty() {
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_owned();
            children
                .entry(parent(&rel).to_owned())
                .or_default()
                .push(name);
        }
        entries.insert(rel, Entry { kind, size, mtime });
    }
    let count = r.u32()? as usize;
    let mut top = Vec::with_capacity(count);
    for _ in 0..count {
        let name = r.str()?;
        let dir = r.u8()? != 0;
        let size = r.u64()?;
        top.push(Top { name, dir, size });
    }
    Ok(Index {
        entries,
        children,
        top,
        changed: Mutex::new(Changed::default()),
    })
}

/// Where the index block is, from the pack's last [`FOOTER`] bytes.
pub(crate) fn footer(tail: &[u8]) -> io::Result<(u64, u64)> {
    if tail.len() != FOOTER || &tail[16..] != END {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a complete pack",
        ));
    }
    Ok((
        u64::from_le_bytes(tail[..8].try_into().unwrap()),
        u64::from_le_bytes(tail[8..16].try_into().unwrap()),
    ))
}

pub(crate) fn pack_path(folder: &Path) -> PathBuf {
    let mut name = folder.as_os_str().to_os_string();
    name.push(".hzpak");
    PathBuf::from(name)
}

pub(crate) fn changes_path(folder: &Path) -> PathBuf {
    let mut name = pack_path(folder).into_os_string();
    name.push(".changed");
    PathBuf::from(name)
}

fn mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// The folder's top level now: one listing.
pub(crate) fn top_level(folder: &Path) -> io::Result<Vec<Top>> {
    let mut top = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        top.push(Top {
            name: entry.file_name().to_string_lossy().into_owned(),
            dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
    }
    Ok(top)
}

/// The top-level entry a pack keeps for its folder's stamp ([`build`]): a name no file has.
pub(crate) fn stamp_top(stamp: &str) -> Top {
    Top {
        name: format!("\u{1}stamp {stamp}"),
        dir: false,
        size: 0,
    }
}

/// A stamp for the file `marker` (the cache's own record of what a cached folder was built
/// from): its contents' hash, `None` without the file.
pub(crate) fn stamp_of(marker: &Path) -> Option<String> {
    let bytes = std::fs::read(marker).ok()?;
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3)
    });
    Some(format!("{hash:016x}-{}", bytes.len()))
}

/// Packs `folder` into its `.hzpak`, through a temporary file renamed into place at the end.
/// `progress` hears each file packed. Returns how many files and bytes were packed.
///
/// A folder with links (`super::MANIFEST`s) packs its own files only: the links stay links,
/// read from their manifests, which the index lists as loose so they are still looked for.
/// `stamp`, when given, is kept with the top level: the pack only fits while the stamp does.
pub(crate) fn build(
    folder: &Path,
    stamp: Option<&str>,
    mut progress: impl FnMut(usize),
) -> io::Result<(usize, u64)> {
    let target = pack_path(folder);
    let mut temporary = target.clone().into_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let mut top = top_level(folder)?;
    top.extend(stamp.map(stamp_top));
    let mut out = BufWriter::with_capacity(1 << 20, std::fs::File::create(&temporary)?);
    out.write_all(MAGIC)?;
    let mut offset = MAGIC.len() as u64;
    let mut entries: Vec<(String, Entry)> = Vec::new();
    let root_meta = std::fs::metadata(folder)?;
    entries.push((
        String::new(),
        Entry {
            kind: Kind::Dir,
            size: 0,
            mtime: mtime(&root_meta),
        },
    ));
    let (mut files, mut bytes) = (0usize, 0u64);
    let mut pending = vec![String::new()];
    while let Some(dir) = pending.pop() {
        let path = if dir.is_empty() {
            folder.to_path_buf()
        } else {
            folder.join(&dir)
        };
        let mut names: Vec<_> = std::fs::read_dir(&path)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<io::Result<_>>()?;
        names.sort();
        for name in names {
            let rel = if dir.is_empty() {
                name.clone()
            } else {
                format!("{dir}/{name}")
            };
            if rel.contains('\n') || name == super::MANIFEST {
                continue;
            }
            let meta = std::fs::symlink_metadata(folder.join(&rel))?;
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                entries.push((
                    rel.clone(),
                    Entry {
                        kind: Kind::Dir,
                        size: 0,
                        mtime: mtime(&meta),
                    },
                ));
                pending.push(rel);
                continue;
            }
            let size = meta.len();
            let kind = if size <= MAX_PACKED {
                let mut file = std::fs::File::open(folder.join(&rel))?;
                let mut data = Vec::with_capacity(size as usize);
                file.read_to_end(&mut data)?;
                if data.len() as u64 != size {
                    // Changed while packing: leave it to the folder.
                    Kind::Loose
                } else {
                    out.write_all(&data)?;
                    let at = offset;
                    offset += size;
                    bytes += size;
                    Kind::Packed(at)
                }
            } else {
                Kind::Loose
            };
            entries.push((
                rel,
                Entry {
                    kind,
                    size,
                    mtime: mtime(&meta),
                },
            ));
            files += 1;
            progress(files);
        }
        let manifest = if dir.is_empty() {
            super::MANIFEST.to_string()
        } else {
            format!("{dir}/{}", super::MANIFEST)
        };
        if let Ok(meta) = std::fs::metadata(folder.join(&manifest)) {
            entries.push((
                manifest,
                Entry {
                    kind: Kind::Loose,
                    size: meta.len(),
                    mtime: mtime(&meta),
                },
            ));
        }
    }
    let block = encode(&entries, &top);
    out.write_all(&block)?;
    out.write_all(&offset.to_le_bytes())?;
    out.write_all(&(block.len() as u64).to_le_bytes())?;
    out.write_all(END)?;
    let file = out.into_inner().map_err(|e| e.into_error())?;
    file.sync_all().ok();
    drop(file);
    std::fs::rename(&temporary, &target)?;
    Ok((files, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::FileExt;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hzpak-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("res/assets/minecraft/textures")).unwrap();
        std::fs::write(dir.join("res/facts.json"), b"{\"a\":1}").unwrap();
        std::fs::write(
            dir.join("res/assets/minecraft/textures/stone.png"),
            b"PNGDATA",
        )
        .unwrap();
        std::fs::write(dir.join("res/assets/minecraft/empty.txt"), b"").unwrap();
        dir
    }

    fn load(folder: &Path) -> (std::fs::File, Index) {
        let file = std::fs::File::open(pack_path(folder)).unwrap();
        let len = file.metadata().unwrap().len();
        let mut tail = [0u8; FOOTER];
        file.read_exact_at(&mut tail, len - FOOTER as u64).unwrap();
        let (at, n) = footer(&tail).unwrap();
        let mut block = vec![0u8; n as usize];
        file.read_exact_at(&mut block, at).unwrap();
        (file, decode(&block).unwrap())
    }

    #[test]
    fn a_pack_holds_every_file_and_folder() {
        let dir = scratch("build");
        let folder = dir.join("res");
        let (files, bytes) = build(&folder, None, |_| {}).unwrap();
        assert_eq!((files, bytes), (3, 14));
        let (file, index) = load(&folder);
        let Found::Indexed(stone) = index.find("assets/minecraft/textures/stone.png") else {
            panic!("stone is packed");
        };
        let Kind::Packed(at) = stone.kind else {
            panic!()
        };
        let mut bytes = vec![0u8; stone.size as usize];
        file.read_exact_at(&mut bytes, at).unwrap();
        assert_eq!(bytes, b"PNGDATA");
        assert!(matches!(
            index.find("assets"),
            Found::Indexed(Entry {
                kind: Kind::Dir,
                ..
            })
        ));
        assert_eq!(index.find("assets/minecraft/nope.json"), Found::Missing);
        let mut names = index.list("assets/minecraft").unwrap().to_vec();
        names.sort();
        assert_eq!(names, ["empty.txt", "textures"]);
        assert_eq!(index.list("").unwrap().len(), 2);
        assert!(index.fits(&top_level(&folder).unwrap()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_the_game_changes_comes_from_the_folder() {
        let dir = scratch("changes");
        let folder = dir.join("res");
        build(&folder, None, |_| {}).unwrap();
        let (_, index) = load(&folder);
        // The game writes its sprite cache at the top: the file and its folder's listing.
        let lines = index.note_change("horizon-sprites.cache").unwrap();
        assert_eq!(lines, "P horizon-sprites.cache\nL \n");
        assert_eq!(index.note_change("horizon-sprites.cache"), None);
        assert_eq!(index.find("horizon-sprites.cache"), Found::Folder);
        assert_eq!(index.list(""), None);
        // Its siblings still come from the pack.
        assert!(matches!(index.find("facts.json"), Found::Indexed(_)));
        // A changed folder covers everything under it.
        index.note_change("assets/minecraft");
        assert_eq!(
            index.find("assets/minecraft/textures/stone.png"),
            Found::Folder
        );
        // The cache's new size doesn't make the pack stale; another file's does.
        std::fs::write(folder.join("horizon-sprites.cache"), b"cache").unwrap();
        assert!(index.fits(&top_level(&folder).unwrap()));
        std::fs::write(folder.join("facts.json"), b"{\"a\":12}").unwrap();
        assert!(!index.fits(&top_level(&folder).unwrap()));
        // Changes survive a restart.
        let (_, again) = load(&folder);
        again.load_changes(&lines);
        assert_eq!(again.find("horizon-sprites.cache"), Found::Folder);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cache_folder_keeps_its_links_and_its_stamp() {
        let dir = scratch("links");
        let folder = dir.join("res");
        let textures = folder.join("assets/minecraft/textures");
        std::fs::write(
            textures.join(crate::vlinks::MANIFEST),
            b"+linked\t/elsewhere\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(textures.join("stone.png"), textures.join("real-link.png"))
            .unwrap();
        build(&folder, Some("abc"), |_| {}).unwrap();
        let (_, index) = load(&folder);
        // The links stay links: the pack holds the folder's own files only.
        assert_eq!(
            index.find("assets/minecraft/textures/real-link.png"),
            Found::Missing
        );
        // Their manifest is listed, left to the folder, so it is still read.
        let manifest = format!("assets/minecraft/textures/{}", crate::vlinks::MANIFEST);
        let Found::Indexed(entry) = index.find(&manifest) else {
            panic!("the manifest is indexed");
        };
        assert_eq!(entry.kind, Kind::Loose);
        // The stamp is part of what must still fit.
        let mut now = top_level(&folder).unwrap();
        assert!(!index.fits(&now));
        now.push(stamp_top("abc"));
        assert!(index.fits(&now));
        now.pop();
        now.push(stamp_top("rebuilt"));
        assert!(!index.fits(&now));
    }

    #[test]
    fn a_cut_pack_is_refused() {
        assert!(footer(&[0u8; FOOTER]).is_err());
        assert!(decode(&[5, 0, 0, 0, 1]).is_err());
    }
}
