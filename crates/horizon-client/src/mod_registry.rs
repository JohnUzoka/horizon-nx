//! Horizon's registry in Options > Mods > Browse Mods. Content mods and resource packs
//! use the same index, file checks and `mod-browser.json` as the launcher. Downloads
//! are staged and checked before they reach `horizon-mods/` or `resourcepacks/`.
//!
//! The launcher is a separate workspace, so this is the small part of it the
//! game needs. Everything here blocks: the game runs it on a worker
//! (`game/interface/mod_browser.rs`).
use crate::mod_browser::{Listing, ProjectKind};
use horizon_mod_package::index::{self, Index, Kind, Project, Trust, Version};
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Where the registry publishes its index (the launcher's `INDEX_URL`).
pub const INDEX_URL: &str = "https://horizon-rust.github.io/horizon-mods/index.json";
/// The Minecraft version this game is: the registry's `game`.
pub const GAME: &str = horizon_core::version::VERSION_ID;
/// The registry's pages on the website, `<SITE>/<id>`.
pub const SITE: &str = "https://playhorizon.dev/mods";
/// The last index fetched, beside `options.txt`: the launcher keeps its own copy there too.
pub const CACHE: &str = "mod-browser/horizon-index.json";
/// What the launcher's browser installed (`installed::MANIFEST`).
pub const MANIFEST: &str = "mod-browser.json";
const PACKS: &str = "resourcepacks";
/// Checked files that contain code, by SHA-256. Separate from the shared installed
/// manifest: these files were never installed, and only Switch filters them out.
const CODE_MODS: &str = "mod-browser/code-mods.txt";
/// Where a download waits for its checks.
const DOWNLOADS: &str = "mod-browser/downloads";
/// Larger than any index should be.
const MAX_INDEX: u64 = 16 << 20;
/// The game asks the registry by this name.
const AGENT: &str = concat!(
    "horizon-rust/horizon/",
    env!("CARGO_PKG_VERSION"),
    " (in-game mod browser)"
);

/// Where the index comes from: `HORIZON_MOD_INDEX` (a file or an `https://`
/// address, for a test registry), else [`INDEX_URL`]; as the launcher reads it.
pub fn location() -> String {
    std::env::var("HORIZON_MOD_INDEX")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| INDEX_URL.to_owned())
}

/// A local test registry: its files may be `file://` paths.
fn is_local(at: &str) -> bool {
    !at.starts_with("https://")
}

/// The index from `at`, kept in `home`'s [`CACHE`]; the kept copy when the
/// registry can't be reached.
pub fn load_index(home: &Path, at: &str) -> Result<Index, String> {
    let local = is_local(at);
    let parse = |text: &str| match local {
        true => Index::parse_local(text),
        false => Index::parse(text),
    };
    let fetched = fetch_text(at).and_then(|text| parse(&text).map(|index| (index, text)));
    let cache = home.join(CACHE);
    match fetched {
        Ok((index, text)) => {
            if let Some(dir) = cache.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&cache, text);
            Ok(index)
        }
        Err(e) => {
            let kept = fs::read_to_string(&cache).map_err(|_| e.clone())?;
            parse(&kept).map_err(|_| e)
        }
    }
}

fn fetch_text(at: &str) -> Result<String, String> {
    if !at.starts_with("https://") {
        return fs::read_to_string(at).map_err(|e| format!("{at}: {e}"));
    }
    let response = ureq::get(at)
        .set("User-Agent", AGENT)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("the mod registry answered {code}"),
            ureq::Error::Transport(t) => format!("the mod registry can't be reached: {t}"),
        })?;
    let mut text = String::new();
    response
        .into_reader()
        .take(MAX_INDEX)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    Ok(text)
}

/// Parsed indexes already enforce each kind's filename and extension. A wasm mod can
/// also declare only `registries`: the browser must inspect it before installing.
fn project_kind(p: &Project, v: &Version) -> Option<ProjectKind> {
    match p.kind {
        Kind::Mod if v.capabilities.known() == [horizon_mod_package::Capability::Registries] => {
            Some(ProjectKind::Mod)
        }
        Kind::ResourcePack => Some(ProjectKind::ResourcePack),
        _ => None,
    }
}

fn code_mod_refusal(switch: bool) -> String {
    if switch {
        "Nintendo Switch can't run native or WebAssembly code mods".into()
    } else {
        "code mods are installed and updated in the launcher".into()
    }
}

fn read_code_mods(home: &Path) -> String {
    fs::read_to_string(home.join(CODE_MODS)).unwrap_or_default()
}

/// The install lock is held and `known` came from disk inside it. Only verified
/// registry downloads may add a hash, not a conflict with a local installation.
fn remember_code_mod(home: &Path, mut known: String, sha256: &str) {
    if !known.is_empty() && !known.ends_with('\n') {
        known.push('\n');
    }
    known.push_str(sha256);
    known.push('\n');
    let path = home.join(CODE_MODS);
    let part = path.with_extension("txt.part");
    if let Err(error) = fs::write(&part, known).and_then(|()| fs::rename(&part, &path)) {
        let _ = fs::remove_file(part);
        log::warn!("Cannot remember incompatible mod in {}: {error}", path.display());
    }
}

/// The registry's mods and resource packs for this game, most downloaded first.
pub fn listings(index: &Index, home: &Path) -> Vec<Listing> {
    listings_for(index, home, cfg!(target_os = "horizon"))
}

fn listings_for(index: &Index, home: &Path, switch: bool) -> Vec<Listing> {
    let cached = if switch { read_code_mods(home) } else { String::new() };
    let code: HashSet<&str> = cached.lines().collect();
    let installed = Installed::load(home).unwrap_or_default();
    let mut out: Vec<Listing> = index
        .projects
        .iter()
        .filter_map(|p| {
            let v = index.installable(p, GAME)?;
            let kind = project_kind(p, v)?;
            if kind == ProjectKind::Mod && code.contains(v.file.sha256.as_str()) {
                return None;
            }
            Some(Listing {
                id: p.id.clone(),
                kind,
                name: p.name.clone(),
                summary: p.summary.clone(),
                description: p.description.clone(),
                authors: p.authors.clone(),
                license: p.license.clone(),
                categories: p.categories.clone(),
                tags: p.tags.clone(),
                version: v.version.clone(),
                published: v.published.clone(),
                changelog: v.changelog.clone(),
                downloads: p.downloads,
                page: format!("{SITE}/{}", p.id),
                installed: installed.version_of(home, &p.id, kind),
            })
        })
        .collect();
    out.sort_by(|a, b| b.downloads.cmp(&a.downloads).then(a.name.cmp(&b.name)));
    out
}

/// What [`install`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub id: String,
    pub kind: ProjectKind,
    pub version: String,
    /// Checked against the author's key, or unsigned (the SHA-256 still pins
    /// the reviewed file).
    pub verified: bool,
}

/// Installs the registry's content mod or resource pack `id` into `home`, and records it
/// in `mod-browser.json`. The caller enables content mods; resource packs stay inactive
/// until selected in Options > Resource Packs. `get` downloads at most the listed size.
pub fn install(
    index: &Index,
    home: &Path,
    id: &str,
    get: &dyn Fn(&str, u64) -> Result<Vec<u8>, String>,
) -> Result<Done, String> {
    install_for(index, home, id, get, cfg!(target_os = "horizon"))
}

fn install_for(
    index: &Index,
    home: &Path,
    id: &str,
    get: &dyn Fn(&str, u64) -> Result<Vec<u8>, String>,
    switch: bool,
) -> Result<Done, String> {
    let p = index
        .project(id)
        .ok_or_else(|| format!("{id} isn't in Horizon's registry"))?;
    let no_version = || format!("{} has no version this game can install", p.name);
    let v = index.installable(p, GAME).ok_or_else(no_version)?;
    let kind = project_kind(p, v).ok_or_else(|| {
        if switch && matches!(p.kind, Kind::Mod | Kind::NativeMod) {
            code_mod_refusal(switch)
        } else {
            no_version()
        }
    })?;
    // Share the entire read/modify/write transaction with the launcher's installers on desktop.
    let _lock = horizon_mod_package::install_lock::lock_home(home)
        .map_err(|e| format!("cannot lock mod browser in {}: {e}", home.display()))?;
    let mut list = Installed::load(home)?;
    let ours = list.position(id).map(|i| &list.entries[i]);
    let old_file = ours.and_then(|entry| entry["file"].as_str());
    let (relative, extension, label) = match kind {
        ProjectKind::Mod => (format!("{}/{id}.hmod", crate::mod_list::DIR), "hmod", "mod"),
        ProjectKind::ResourcePack => (format!("{PACKS}/{}", v.file.name), "zip", "resource-pack"),
    };
    let target = home.join(&relative);
    let same_file = old_file == Some(relative.as_str());
    let checksum = format!("SHA-256:{}", v.file.sha256);
    let done = |verified| Done {
        id: id.to_owned(),
        kind,
        version: v.version.clone(),
        verified,
    };

    let mut code = String::new();
    match kind {
        ProjectKind::Mod => {
            if switch {
                code = read_code_mods(home);
                if code.lines().any(|sha| sha == v.file.sha256) {
                    return Err(code_mod_refusal(switch));
                }
            }
            let native = format!("{}/{id}.hmod", crate::mod_list::NATIVE_DIR);
            // Even a missing file does not relinquish the launcher's native pin.
            if old_file == Some(native.as_str()) || home.join(&native).exists() {
                return Err(code_mod_refusal(switch));
            }
            if ours.is_some() && !same_file {
                return Err("remove the existing managed entry in the launcher first".into());
            }
            if same_file && target.exists() {
                let package = horizon_mod_package::Package::read(&target)?;
                if package.manifest.native.is_some() || package.manifest.wasm.is_some() {
                    return Err(code_mod_refusal(switch));
                }
            }
            if home.join(crate::mod_list::DIR).join(id).is_dir() {
                return Err(format!(
                    "a mod with the id {id} is already in {}: move it away first",
                    crate::mod_list::DIR
                ));
            }
        }
        ProjectKind::ResourcePack => {
            if let Some(entry) = ours
                && (entry["kind"] != label || !old_file.is_some_and(resource_pack_file))
            {
                return Err("remove the existing managed entry in the launcher first".into());
            }
        }
    }

    if let Some(entry) = ours
        && same_file
        && entry["version"].as_str() == Some(v.version.as_str())
        && entry["checksum"].as_str() == Some(checksum.as_str())
        && target.is_file()
    {
        return Ok(done(entry["verdict"] == "Verified"));
    }
    // Never replace a file the player put there themselves, including a dangling symlink.
    if !same_file && fs::symlink_metadata(&target).is_ok() {
        let dir = match kind {
            ProjectKind::Mod => crate::mod_list::DIR,
            ProjectKind::ResourcePack => PACKS,
        };
        return Err(format!("{relative} is already in {dir}: move it away first"));
    }
    if let Some(other) = list.taken_by(&relative, id) {
        return Err(format!("{relative} belongs to {other}: remove it first"));
    }
    let bytes = get(&v.file.url, v.file.size)?;
    let staging = home.join(DOWNLOADS);
    fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let download = staging.join(format!("{id}.{extension}.part"));
    fs::write(&download, &bytes).map_err(|e| format!("{}: {e}", download.display()))?;
    let checked = || -> Result<_, String> {
        let trust = index::check_file(index, p, v, &download)?;
        if kind == ProjectKind::Mod {
            let package = horizon_mod_package::Package::read(&download)?;
            if package.manifest.native.is_some() || package.manifest.wasm.is_some() {
                if switch {
                    remember_code_mod(home, code, &v.file.sha256);
                }
                return Err(code_mod_refusal(switch));
            }
        }
        Ok(trust)
    };
    let trust = checked().inspect_err(|_| {
        let _ = fs::remove_file(&download);
    })?;
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::rename(&download, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    let obsolete_pack = if kind == ProjectKind::ResourcePack && !same_file {
        old_file
            .filter(|old| list.taken_by(old, id).is_none())
            .map(|old| home.join(old))
    } else {
        None
    };
    let verified = trust == Trust::Verified;
    list.record(serde_json::json!({
        "source": "horizon",
        "project": p.id,
        "slug": p.id,
        "title": p.name,
        "kind": label,
        "version": v.version,
        "number": v.version,
        "file": relative,
        "checksum": checksum,
        "page": format!("{SITE}/{}", p.id),
        "dependency": false,
        "verdict": if verified { "Verified" } else { "Unsigned" },
    }));
    list.save(home)?;
    if let Some(old) = obsolete_pack
        && let Err(error) = fs::remove_file(&old)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("Cannot remove obsolete resource pack {}: {error}", old.display());
    }
    Ok(done(verified))
}

/// The installed manifest may have come from a modpack. Only replace or remove a
/// pack under the same one-name path the index and launcher use.
fn resource_pack_file(relative: &str) -> bool {
    relative
        .strip_prefix("resourcepacks/")
        .is_some_and(|name| {
            !name.starts_with('.')
                && name.ends_with(".zip")
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b))
        })
}

/// Downloads `url` (at most `max` bytes): `https://` only, or a `file://` path
/// when the index itself is a local test registry.
pub fn download(url: &str, max: u64) -> Result<Vec<u8>, String> {
    if let Some(path) = url.strip_prefix("file://") {
        if !is_local(&location()) {
            return Err(format!("{url} is not an https address"));
        }
        return fs::read(path).map_err(|e| format!("{path}: {e}"));
    }
    if !url.starts_with("https://") {
        return Err(format!("{url} is not an https address"));
    }
    let response = ureq::get(url)
        .set("User-Agent", AGENT)
        .call()
        .map_err(|e| e.to_string())?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(max + 1)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() as u64 > max {
        return Err(format!("{url} is larger than the index says"));
    }
    Ok(body)
}

/// `mod-browser.json`, kept as JSON values so the launcher's other entries
/// (Modrinth packs, ported Java mods) are written back as they were.
#[derive(Debug, Default)]
struct Installed {
    entries: Vec<serde_json::Value>,
}

impl Installed {
    fn load(home: &Path) -> Result<Self, String> {
        let path = home.join(MANIFEST);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        let entries = value["entries"]
            .as_array()
            .ok_or_else(|| format!("{}: entries must be an array", path.display()))?
            .clone();
        Ok(Self { entries })
    }

    fn save(&self, home: &Path) -> Result<(), String> {
        let path = home.join(MANIFEST);
        let part = home.join(format!(".{MANIFEST}.part"));
        let text = serde_json::to_string_pretty(&serde_json::json!({ "entries": self.entries }))
            .map_err(|e| e.to_string())?;
        fs::write(&part, text).map_err(|e| format!("{}: {e}", part.display()))?;
        fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The registry's entry for project `id`.
    fn position(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e["source"] == "horizon" && e["project"] == id)
    }

    /// The title of another entry that owns `file`.
    fn taken_by(&self, file: &str, id: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|e| e["file"] == file && !(e["source"] == "horizon" && e["project"] == id))
            .map(|e| e["title"].as_str().unwrap_or("another mod").to_owned())
    }

    fn record(&mut self, entry: serde_json::Value) {
        let id = entry["project"].clone();
        self.entries
            .retain(|e| !(e["source"] == "horizon" && e["project"] == id));
        self.entries.push(entry);
    }

    /// The version of `id` installed from the registry whose file is still
    /// there; for a `horizon-mods/<id>.hmod` put there another way, its
    /// manifest's version.
    fn version_of(&self, home: &Path, id: &str, kind: ProjectKind) -> Option<String> {
        if let Some(e) = self.position(id).map(|i| &self.entries[i])
            && let (Some(version), Some(file)) = (e["version"].as_str(), e["file"].as_str())
            && (kind == ProjectKind::Mod || (e["kind"] == "resource-pack" && resource_pack_file(file)))
            && home.join(file).exists()
        {
            return Some(version.to_owned());
        }
        if kind == ProjectKind::ResourcePack {
            return None;
        }
        let path: PathBuf = home.join(crate::mod_list::DIR).join(format!("{id}.hmod"));
        horizon_mod_package::Package::read(&path)
            .ok()
            .filter(|p| p.manifest.id == id)
            .map(|p| p.manifest.version)
    }
}

#[cfg(test)]
mod tests;
