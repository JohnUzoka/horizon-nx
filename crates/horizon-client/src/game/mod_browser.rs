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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Where the registry publishes its index (GitHub Pages of horizon-rust/horizon-mods).
const INDEX_URL: &str = "https://horizon-rust.github.io/horizon-mods/index.json";
/// The index is read again after this long.
const REFRESH: Duration = Duration::from_secs(10 * 60);
const INDEX_MAX: u64 = 32 << 20;
const GAME: &str = horizon_core::version::VERSION_ID;

#[derive(Default)]
pub(super) struct State {
    index: Option<(Arc<Index>, Instant)>,
    fetching: Option<mpsc::Receiver<Result<Index, String>>>,
    /// The project being installed, and what its install says when done.
    installing: Option<(String, mpsc::Receiver<Result<String, String>>)>,
    /// The mods being checked in the background (whether this game reads them), one by one.
    checking: Option<mpsc::Receiver<(String, Check)>>,
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
/// `tag` keeps two downloads of the same file (a check and an install) apart.
fn download_and_check(index: &Index, p: &Project, v: &Version, tag: &str) -> Check {
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
    let part = folder.join(format!(".{}.{tag}.part", v.file.sha256));
    let downloaded = (|| {
        let response = agent().get(url).call().map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("download: HTTP {code}"),
            ureq::Error::Transport(e) => format!("download: {e}"),
        })?;
        let mut input = response.into_reader().take(v.file.size + 1);
        let mut out =
            std::io::BufWriter::new(std::fs::File::create(&part).map_err(|e| e.to_string())?);
        std::io::copy(&mut input, &mut out).map_err(|e| format!("download: {e}"))?;
        let file = out.into_inner().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        // Closed before it is read again: the Switch's file system won't open a file twice.
        drop(file);
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
fn install(index: &Index, p: &Project, v: &Version, dest: &Path) -> Result<(), String> {
    match known(v).unwrap_or_else(|| download_and_check(index, p, v, "install")) {
        Check::Passed => {}
        Check::Refused(e) => return Err(format!("{} needs a newer Horizon: {e}", p.name)),
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
                #[cfg(target_os = "horizon")]
                horizon_nx::favour_waiting_thread();
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
            .map(|(id, _)| id.clone());
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
        let mut to_check = Vec::new();
        let checking = self.mods.browser.checking.is_some();
        for p in projects {
            let Some((kind, v)) = offered(&index, p) else {
                continue;
            };
            // A mod's file may use what this game can't read (made for a newer Horizon): each is
            // downloaded and checked once, in the background, before it is offered.
            let mut verdict = || match (kind, known(v)) {
                (BrowserKind::ResourcePacks, _) | (_, Some(Check::Passed)) => {
                    BrowserState::Available
                }
                (_, Some(Check::Refused(_))) => BrowserState::Unsupported,
                _ if failed.contains(&p.id) => BrowserState::Failed,
                _ if checking => BrowserState::Checking,
                _ => {
                    to_check.push((p.clone(), v.clone()));
                    BrowserState::Checking
                }
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
        if !to_check.is_empty() {
            self.check_mods(index.clone(), to_check);
        }
        let browser = &mut self.ui.options.browser;
        if entries.is_empty() {
            browser.status = Some("Horizon's registry has nothing to install here yet.".into());
        } else if browser.status.as_deref() == Some("Reading Horizon's registry...") {
            browser.status = None;
        }
        browser.entries = entries;
    }

    /// Downloads and checks `mods` one after another on a thread; each verdict comes back to
    /// [`Self::drive_mod_browser`].
    fn check_mods(&mut self, index: Arc<Index>, mods: Vec<(Project, Version)>) {
        let (send, receive) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("mod-check".into())
            .spawn(move || {
                #[cfg(target_os = "horizon")]
                horizon_nx::favour_waiting_thread();
                for (p, v) in mods {
                    let started = Instant::now();
                    let check =
                        known(&v).unwrap_or_else(|| download_and_check(&index, &p, &v, "check"));
                    log::info!(
                        "Horizon: Get Mods: checked {} ({} KB) in {:.1} s",
                        p.id,
                        v.file.size / 1000,
                        started.elapsed().as_secs_f64()
                    );
                    if send.send((p.id.clone(), check)).is_err() {
                        return;
                    }
                }
            });
        if spawned.is_ok() {
            self.mods.browser.checking = Some(receive);
        }
    }

    /// Get Mods' Install: one at a time, on a thread.
    pub(super) fn install_mod(&mut self, id: &str) {
        if let Some((busy, _)) = &self.mods.browser.installing {
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
        let spawned = std::thread::Builder::new()
            .name("mod-install".into())
            .spawn(move || {
                #[cfg(target_os = "horizon")]
                horizon_nx::favour_waiting_thread();
                let done = install(&index, &p, &v, &dest).map(|()| match kind {
                    BrowserKind::Mods => {
                        format!("{name} is installed and on: restart Horizon to play with it.")
                    }
                    BrowserKind::ResourcePacks => {
                        format!("{name} is installed: turn it on in Options > Resource Packs.")
                    }
                });
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
        self.mods.browser.installing = Some((id.to_owned(), receive));
        self.ui.options.browser.status = Some(format!("Installing {}...", self.browser_name(id)));
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

    /// Each frame: the index, the checks and the installs that finished.
    pub(super) fn drive_mod_browser(&mut self) {
        while let Some(receive) = &self.mods.browser.checking {
            match receive.try_recv() {
                Ok((id, check)) => {
                    let installed = self.ui.options.browser.entries.iter().any(|e| {
                        e.id == id
                            && matches!(e.state, BrowserState::Installing | BrowserState::Installed)
                    });
                    if installed {
                        continue;
                    }
                    let state = match check {
                        Check::Passed => BrowserState::Available,
                        Check::Refused(why) => {
                            log::info!("Horizon: Get Mods: {id} needs a newer Horizon: {why}");
                            BrowserState::Unsupported
                        }
                        Check::Unreachable(why) => {
                            log::info!("Horizon: Get Mods: couldn't check {id}: {why}");
                            BrowserState::Available
                        }
                    };
                    let update = self.ui.options.browser.entries.iter().any(|e| {
                        e.id == id && e.state == BrowserState::Checking && {
                            let dest = Game::options_path()
                                .with_file_name(mod_list::DIR)
                                .join(format!("{id}.hmod"));
                            dest.exists()
                        }
                    });
                    let state = if update && state == BrowserState::Available {
                        BrowserState::Update
                    } else {
                        state
                    };
                    self.set_browser_state(&id, state);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.mods.browser.checking = None;
                    // What wasn't checked (the page reopened meanwhile) is checked now.
                    if self
                        .ui
                        .options
                        .browser
                        .entries
                        .iter()
                        .any(|e| e.state == BrowserState::Checking)
                    {
                        self.fill_mod_browser();
                    }
                }
            }
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
        let Some((id, receive)) = &self.mods.browser.installing else {
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
            Err(e) => {
                log::warn!("Horizon: Get Mods: {e}");
                self.ui.options.browser.status = Some(e);
                self.set_browser_state(&id, BrowserState::Failed);
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
