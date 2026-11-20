//! Get Mods (Options > Mods > Get Mods): Horizon's registry (docs/api/mod-index.md) inside the game,
//! for players with no launcher, such as on the Switch. It lists what the game installs by itself,
//! content mods (`.hmod` that only add content, capability `registries`) and resource packs made
//! for this version, and installs one the way the launcher does (`launcher/src/browser/horizon.rs`):
//! downloaded from its GitHub release, checked against the index (`index::check_file`: size,
//! SHA-256, the author's signature, the file's own manifest), then moved into place: a mod to
//! `horizon-mods/<id>.hmod`, turned on, a resource pack to `resourcepacks/<file>`.
use super::Game;
use horizon_client::mod_list::{self, BrowserEntry, BrowserKind, BrowserState};
use horizon_mods::index::{self, Index, Kind, Project, Version};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Where the registry publishes its index (GitHub Pages of horizon-rust/horizon-mods).
const INDEX_URL: &str = "https://horizon-rust.github.io/horizon-mods/index.json";
/// The index is read again after this long.
const REFRESH: Duration = Duration::from_secs(10 * 60);
const INDEX_MAX: u64 = 32 << 20;
const GAME: &str = horizon_core::version::VERSION_ID;
/// What a refused mod's install says.
const NEEDS_NEWER: &str = "needs a newer Horizon";
/// An install stopped because the page was left.
const CANCELLED: &str = "stopped: Get Mods was closed";

#[derive(Default)]
pub(super) struct State {
    index: Option<(Arc<Index>, Instant)>,
    fetching: Option<mpsc::Receiver<Result<Index, String>>>,
    /// The project being installed, what its install says when done, and its stop switch.
    installing: Option<(
        String,
        mpsc::Receiver<Result<String, String>>,
        Arc<AtomicBool>,
    )>,
}

/// While it lives, the calling thread waits on the network ahead of the game's threads
/// (`horizon_nx::favour_waiting_thread`).
struct Waiting;
impl Waiting {
    fn start() -> Self {
        #[cfg(target_os = "horizon")]
        horizon_nx::favour_waiting_thread(true);
        Waiting
    }
}
impl Drop for Waiting {
    fn drop(&mut self) {
        #[cfg(target_os = "horizon")]
        horizon_nx::favour_waiting_thread(false);
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .https_only(true)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent(&format!("horizon/{GAME} (in-game mod browser)"))
        .build()
}

/// The body of `url`, at most `limit` bytes.
fn fetch(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let _waiting = Waiting::start();
    let response = agent().get(url).call().map_err(|e| match e {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        ureq::Error::Transport(e) => e.to_string(),
    })?;
    let mut body = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|e| e.to_string())?;
    if body.len() as u64 > limit {
        return Err(format!("larger than {limit} bytes"));
    }
    Ok(body)
}

/// The version of `p` this game installs, if the page offers it: its newest for this game that
/// wasn't taken down, a content mod's (`.hmod` with capability `registries` only) or a resource
/// pack's. Native mods are built by the launcher; code mods don't run.
fn offered<'a>(index: &Index, p: &'a Project) -> Option<(BrowserKind, &'a Version)> {
    let kind = match p.kind {
        Kind::Mod => BrowserKind::Mods,
        Kind::ResourcePack => BrowserKind::ResourcePacks,
        _ => return None,
    };
    let v = index.installable(p, GAME, horizon_mods::api_level::LEVEL)?;
    let content_only = v
        .capabilities
        .iter()
        .all(|c| *c == horizon_mods::Capability::Registries);
    if kind == BrowserKind::Mods && !(v.file.name.ends_with(".hmod") && content_only) {
        return None;
    }
    Some((kind, v))
}

/// A plain file name: no folders, no hidden or reserved names.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\', ':', '\0', '\n'])
        && name != ".."
}

/// Where `p`'s file goes in the game folder.
fn destination(kind: BrowserKind, p: &Project, v: &Version) -> Result<PathBuf, String> {
    match kind {
        BrowserKind::Mods if index::valid_id(&p.id) && plain(&p.id) => Ok(Game::options_path()
            .with_file_name(mod_list::DIR)
            .join(format!("{}.hmod", p.id))),
        BrowserKind::ResourcePacks if plain(&v.file.name) => {
            Ok(super::packs::packs_dir().join(&v.file.name))
        }
        _ => Err(format!("{}: the index names its file oddly", p.name)),
    }
}

/// Get Mods' files: downloads in progress, and the files checked already (`checked/<sha256>`, or
/// `checked/<sha256>.refused` with why this game can't read it).
fn browser_dir() -> PathBuf {
    Game::options_path().with_file_name("mod-browser")
}

fn checked_path(v: &Version) -> PathBuf {
    browser_dir().join("checked").join(&v.file.sha256)
}

fn refused_path(v: &Version) -> PathBuf {
    browser_dir()
        .join("checked")
        .join(format!("{}.refused", v.file.sha256))
}

/// What a check of a file found.
enum Check {
    /// Checked: at [`checked_path`].
    Passed,
    /// The file is the index's, but this game can't read it (made for a newer Horizon).
    Refused(String),
    /// It couldn't be downloaded: nothing is known (not kept).
    Unreachable(String),
}

/// What is known of `v` already, from an earlier check.
fn known(v: &Version) -> Option<Check> {
    if checked_path(v).exists() {
        return Some(Check::Passed);
    }
    std::fs::read_to_string(refused_path(v))
        .ok()
        .map(Check::Refused)
}

/// Downloads `v` of `p` and checks it against the index (`check_file`: size, SHA-256, signature,
/// and that this game reads its manifest and content). A file that passes is kept at
/// [`checked_path`], ready to install; one this game can't read is noted at [`refused_path`].
/// It stops when `cancel` is set (the page was left).
fn download_and_check(index: &Index, p: &Project, v: &Version, cancel: &AtomicBool) -> Check {
    let url = &v.file.url;
    // The registry's own releases, or a project's (docs/api/mod-index.md, `file`).
    if !url.starts_with("https://github.com/") || !url.contains("/releases/download/") {
        return Check::Refused("not a GitHub release file".into());
    }
    if v.file.size > index::MAX_FILE || !plain(&v.file.name) || !plain(&v.file.sha256) {
        return Check::Refused("the index lists an odd file".into());
    }
    let folder = browser_dir().join("checked");
    if let Err(e) = std::fs::create_dir_all(&folder) {
        return Check::Unreachable(e.to_string());
    }
    let part = folder.join(format!(".{}.part", v.file.sha256));
    let downloaded = (|| {
        let _waiting = Waiting::start();
        let response = agent().get(url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("download: HTTP {code}"),
            ureq::Error::Transport(e) => format!("download: {e}"),
        })?;
        let mut input = response.into_reader().take(v.file.size + 1);
        let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut chunk = vec![0u8; 64 << 10];
        loop {
            if cancel.load(Relaxed) {
                return Err(CANCELLED.to_owned());
            }
            let n = input
                .read(&mut chunk)
                .map_err(|e| format!("download: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
        }
        out.sync_all().map_err(|e| e.to_string())?;
        // Closed before it is read again: the Switch's file system won't open a file twice.
        drop(out);
        Ok::<(), String>(())
    })();
    if let Err(e) = downloaded {
        let _ = std::fs::remove_file(&part);
        return Check::Unreachable(e);
    }
    match index::check_file(index, p, v, &part) {
        Ok(_) => match std::fs::rename(&part, checked_path(v)) {
            Ok(()) => Check::Passed,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Check::Unreachable(e.to_string())
            }
        },
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            // A wrong size or hash is a bad download, not the file's verdict.
            if e.contains("bytes, the index lists") || e.contains("SHA-256") {
                return Check::Unreachable(e);
            }
            let _ = std::fs::write(refused_path(v), &e);
            Check::Refused(e)
        }
    }
}

/// Installs `v` of `p` at `dest`: its checked file, downloaded and checked now if it wasn't.
fn install(
    index: &Index,
    p: &Project,
    v: &Version,
    dest: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    match known(v).unwrap_or_else(|| download_and_check(index, p, v, cancel)) {
        Check::Passed => {}
        Check::Refused(e) => return Err(format!("{} {NEEDS_NEWER}: {e}", p.name)),
        Check::Unreachable(e) if e == CANCELLED => return Err(e),
        Check::Unreachable(e) => return Err(format!("{}: {e}", p.name)),
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(checked_path(v), dest).map_err(|e| format!("{}: {e}", p.name))
}

impl Game<'_> {
    /// Get Mods opened: the registry's index, read again when older than [`REFRESH`].
    pub(super) fn browse_mods(&mut self) {
        let fresh = self
            .mods
            .browser
            .index
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() < REFRESH);
        if fresh {
            self.fill_mod_browser();
            return;
        }
        if self.mods.browser.fetching.is_some() {
            return;
        }
        self.ui.options.browser.status = Some("Reading Horizon's registry...".into());
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("mod-index".into())
            .spawn(move || {
                let index = fetch(INDEX_URL, INDEX_MAX).and_then(|body| {
                    let text = String::from_utf8(body).map_err(|e| e.to_string())?;
                    // An entry newer than this build (a native mod's new format) must not hide
                    // the rest: it is left out, with why in the log.
                    let (index, skipped) = Index::parse_lenient(&text)?;
                    for why in skipped.iter().take(5) {
                        log::info!("Horizon: Get Mods: left out {why}");
                    }
                    if skipped.len() > 5 {
                        log::info!("Horizon: Get Mods: {} more left out", skipped.len() - 5);
                    }
                    Ok(index)
                });
                let _ = send.send(index);
            });
        match spawned {
            Ok(_) => self.mods.browser.fetching = Some(receive),
            Err(e) => {
                self.ui.options.browser.status = Some(format!("Can't read the registry: {e}"))
            }
        }
    }

    /// The page's rows from the index, with what is installed already.
    fn fill_mod_browser(&mut self) {
        let Some((index, _)) = &self.mods.browser.index else {
            return;
        };
        let index = index.clone();
        let installing = self
            .mods
            .browser
            .installing
            .as_ref()
            .map(|(id, _, _)| id.clone());
        let failed: Vec<String> = self
            .ui
            .options
            .browser
            .entries
            .iter()
            .filter(|e| e.state == BrowserState::Failed)
            .map(|e| e.id.clone())
            .collect();
        let mut projects: Vec<&Project> = index.projects.iter().collect();
        projects.sort_by(|a, b| b.downloads.cmp(&a.downloads).then(a.name.cmp(&b.name)));
        let mut entries = Vec::new();
        for p in projects {
            let Some((kind, v)) = offered(&index, p) else {
                continue;
            };
            // A mod is checked when its Install is pressed (whether this game reads it); one
            // checked before and refused says so.
            let verdict = || match known(v) {
                Some(Check::Refused(_)) if kind == BrowserKind::Mods => BrowserState::Unsupported,
                _ if failed.contains(&p.id) => BrowserState::Failed,
                _ => BrowserState::Available,
            };
            let state = if installing.as_deref() == Some(&p.id) {
                BrowserState::Installing
            } else {
                match destination(kind, p, v) {
                    Ok(dest) if dest.exists() => match kind {
                        // A mod keeps its name across versions: its hash tells.
                        BrowserKind::Mods => match index::sha256_file(&dest) {
                            Ok(hash) if hash == v.file.sha256 => BrowserState::Installed,
                            _ => match verdict() {
                                BrowserState::Available => BrowserState::Update,
                                other => other,
                            },
                        },
                        BrowserKind::ResourcePacks => BrowserState::Installed,
                    },
                    _ => verdict(),
                }
            };
            entries.push(BrowserEntry {
                id: p.id.clone(),
                kind,
                name: p.name.clone(),
                version: v.version.clone(),
                summary: p.summary.clone(),
                size: v.file.size,
                state,
            });
        }
        let browser = &mut self.ui.options.browser;
        if entries.is_empty() {
            browser.status = Some("Horizon's registry has nothing to install here yet.".into());
        } else if browser.status.as_deref() == Some("Reading Horizon's registry...") {
            browser.status = None;
        }
        browser.entries = entries;
    }

    /// Get Mods' Install: one at a time, on a thread.
    pub(super) fn install_mod(&mut self, id: &str) {
        if let Some((busy, _, _)) = &self.mods.browser.installing {
            let name = self.browser_name(busy);
            self.ui.options.browser.status = Some(format!("Installing {name} first..."));
            return;
        }
        let Some((index, _)) = &self.mods.browser.index else {
            return;
        };
        let index = index.clone();
        let Some(p) = index.project(id).cloned() else {
            return;
        };
        let Some((kind, v)) = offered(&index, &p).map(|(k, v)| (k, v.clone())) else {
            return;
        };
        let dest = match destination(kind, &p, &v) {
            Ok(dest) => dest,
            Err(e) => {
                self.ui.options.browser.status = Some(e);
                return;
            }
        };
        let (send, receive) = mpsc::channel();
        let name = p.name.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let spawned = std::thread::Builder::new()
            .name("mod-install".into())
            .spawn(move || {
                let started = Instant::now();
                let done = install(&index, &p, &v, &dest, &stop).map(|()| match kind {
                    BrowserKind::Mods => {
                        format!("{name} is installed and on: restart Horizon to play with it.")
                    }
                    BrowserKind::ResourcePacks => {
                        format!("{name} is installed: turn it on in Options > Resource Packs.")
                    }
                });
                log::info!(
                    "Horizon: Get Mods: {} ({} KB) took {:.1} s",
                    p.id,
                    v.file.size / 1000,
                    started.elapsed().as_secs_f64()
                );
                let _ = send.send(done);
            });
        if let Err(e) = spawned {
            self.ui.options.browser.status = Some(format!("Can't install: {e}"));
            return;
        }
        log::info!(
            "Horizon: Get Mods: installing {id} {}",
            self.browser_name(id)
        );
        self.mods.browser.installing = Some((id.to_owned(), receive, cancel));
        self.ui.options.browser.status = Some(format!(
            "Downloading and checking {}...",
            self.browser_name(id)
        ));
        self.set_browser_state(id, BrowserState::Installing);
    }

    fn browser_name(&self, id: &str) -> String {
        self.ui
            .options
            .browser
            .entries
            .iter()
            .find(|e| e.id == id)
            .map_or_else(|| id.to_owned(), |e| e.name.clone())
    }

    fn set_browser_state(&mut self, id: &str, state: BrowserState) {
        if let Some(e) = self
            .ui
            .options
            .browser
            .entries
            .iter_mut()
            .find(|e| e.id == id)
        {
            e.state = state;
        }
    }

    /// Each frame: the index and the install that finished.
    pub(super) fn drive_mod_browser(&mut self) {
        // Only while the page shows: leaving it stops an install that is still downloading.
        let open = self.screen == super::Screen::Options
            && self.ui.options.page() == horizon_client::options_ui::Page::ModBrowser;
        if !open && let Some((_, _, cancel)) = &self.mods.browser.installing {
            cancel.store(true, Relaxed);
        }
        if let Some(receive) = &self.mods.browser.fetching {
            match receive.try_recv() {
                Ok(Ok(index)) => {
                    self.mods.browser.fetching = None;
                    log::info!(
                        "Horizon: Get Mods: the registry lists {} projects",
                        index.projects.len()
                    );
                    self.mods.browser.index = Some((Arc::new(index), Instant::now()));
                    self.fill_mod_browser();
                }
                Ok(Err(e)) => {
                    self.mods.browser.fetching = None;
                    log::warn!("Horizon: Get Mods: the registry's index: {e}");
                    self.ui.options.browser.status =
                        Some(format!("Can't read Horizon's registry: {e}"));
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.mods.browser.fetching = None,
            }
        }
        let Some((id, receive, _)) = &self.mods.browser.installing else {
            return;
        };
        let id = id.clone();
        let done = match receive.try_recv() {
            Ok(done) => done,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the install stopped".into()),
        };
        self.mods.browser.installing = None;
        match done {
            Ok(message) => {
                log::info!("Horizon: Get Mods: {message}");
                let is_mod = self
                    .ui
                    .options
                    .browser
                    .entries
                    .iter()
                    .any(|e| e.id == id && e.kind == BrowserKind::Mods);
                if is_mod {
                    // On, as the launcher's install leaves it; listed on the Mods page.
                    self.settings.mods.disabled.remove(&id);
                    self.settings.mods.enabled.insert(id.clone());
                    self.settings.save(&Self::options_path());
                    self.start_mods();
                }
                self.ui.options.browser.status = Some(message);
                self.set_browser_state(&id, BrowserState::Installed);
            }
            Err(e) if e == CANCELLED => {
                log::info!("Horizon: Get Mods: {id}: {e}");
                self.ui.options.browser.status = None;
                self.set_browser_state(&id, BrowserState::Available);
            }
            Err(e) => {
                log::warn!("Horizon: Get Mods: {e}");
                let state = if e.contains(NEEDS_NEWER) {
                    BrowserState::Unsupported
                } else {
                    BrowserState::Failed
                };
                self.ui.options.browser.status = Some(e);
                self.set_browser_state(&id, state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::plain;

    #[test]
    fn only_plain_file_names_are_written() {
        assert!(plain("faithful-1.0.zip"));
        for odd in ["", ".hidden", "../x.zip", "a/b.zip", "c:x", "a\\b"] {
            assert!(!plain(odd), "{odd}");
        }
    }
}
