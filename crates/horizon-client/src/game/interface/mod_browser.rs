//! Options > Mods > Browse mods (docs/design/mod-hotload.md): reading the
//! registry's index and installing a content mod or resource pack run on a worker
//! (`horizon_client::mod_registry`); each frame takes what it finished into the
//! options screen's `mod_browser`. The window's game and the startup menu each
//! have one.
use super::Game;
use horizon_client::{
    lang,
    mod_browser::{InstallMessage, Listing, ModBrowser, ProjectKind, Status},
    mod_list::ModList,
    mod_registry::{self, Done},
};
use horizon_mod_package::index::Index;
use std::path::PathBuf;
use std::sync::{
    Arc,
    mpsc::{Receiver, TryRecvError, channel},
};

/// What the worker sends back: the index and its listings, or an install's
/// outcome with the listings after it.
enum Finished {
    Index(Result<(Arc<Index>, Vec<Listing>), String>),
    Installed(String, Result<Done, String>, Vec<Listing>),
}

/// One job at a time, and the index read last.
#[derive(Default)]
pub(super) struct Jobs {
    running: Option<Receiver<Finished>>,
    index: Option<Arc<Index>>,
}

impl Jobs {
    /// Browse mods opened: the index, read again from the registry (the
    /// launcher's copy when it can't be reached).
    pub(super) fn browse(&mut self, ui: &mut ModBrowser, home: PathBuf) {
        if self.running.is_some() {
            return;
        }
        if ui.status != Status::Ready {
            ui.status = Status::Loading;
        }
        let (send, receive) = channel();
        self.running = Some(receive);
        spawn("mod-browser-index", move || {
            let read = mod_registry::load_index(&home, &mod_registry::location()).map(|index| {
                let listings = mod_registry::listings(&index, &home);
                (Arc::new(index), listings)
            });
            let _ = send.send(Finished::Index(read));
        });
    }

    /// Install or update `id` from the index read last.
    pub(super) fn install(&mut self, ui: &mut ModBrowser, home: PathBuf, id: String) {
        let Some(index) = self.index.clone() else {
            return;
        };
        if self.running.is_some() {
            return;
        }
        ui.installing = Some(id.clone());
        ui.message = None;
        let (send, receive) = channel();
        self.running = Some(receive);
        spawn("mod-browser-install", move || {
            let done = mod_registry::install(&index, &home, &id, &mod_registry::download);
            let listings = mod_registry::listings(&index, &home);
            let _ = send.send(Finished::Installed(id, done, listings));
        });
    }

    /// What the worker finished: `(installed kind, changed)`. Content mods are
    /// turned on in `mods`; packs are left for the player to select separately.
    /// The caller lays out the page again for `changed`.
    /// Completion waits while a world owns its frozen registry set.
    pub(super) fn poll(
        &mut self,
        ui: &mut ModBrowser,
        mods: &mut ModList,
        in_world: bool,
    ) -> (Option<ProjectKind>, bool) {
        if in_world {
            return (None, false);
        }
        let Some(running) = &self.running else {
            return (None, false);
        };
        let finished = match running.try_recv() {
            Ok(finished) => finished,
            Err(TryRecvError::Empty) => return (None, false),
            Err(TryRecvError::Disconnected) => {
                self.running = None;
                let changed = ui.installing.is_some() || ui.status == Status::Loading;
                ui.installing = None;
                if ui.status == Status::Loading {
                    ui.status = Status::Failed("the worker stopped".into());
                }
                return (None, changed);
            }
        };
        self.running = None;
        match finished {
            Finished::Index(Ok((index, listings))) => {
                self.index = Some(index);
                let changed = ui.status != Status::Ready || ui.listings != listings;
                ui.listings = listings;
                ui.status = Status::Ready;
                (None, changed)
            }
            Finished::Index(Err(why)) => {
                log::warn!("Horizon: the mod registry wasn't read: {why}");
                let mut changed = false;
                if ui.status != Status::Ready {
                    let status = Status::Failed(why);
                    changed = ui.status != status;
                    ui.status = status;
                }
                (None, changed)
            }
            Finished::Installed(project, done, listings) => {
                ui.installing = None;
                ui.listings = listings;
                let (kind, text) = match done {
                    Ok(done) => {
                        log::info!(
                            "Horizon: installed {} {} from the registry ({})",
                            done.id,
                            done.version,
                            if done.verified { "signed" } else { "unsigned" }
                        );
                        let name = ui
                            .listing(&done.id)
                            .map_or(done.id.as_str(), |l| l.name.as_str());
                        let key = match done.kind {
                            ProjectKind::Mod => "horizon.mods.browse.done",
                            ProjectKind::ResourcePack => "horizon.mods.browse.pack_done",
                        };
                        let text = lang::tr(key, &[&name, &done.version]);
                        if done.kind == ProjectKind::Mod {
                            mods.disabled.remove(&done.id);
                            mods.enabled.insert(done.id);
                        }
                        (Some(done.kind), text)
                    }
                    Err(why) => {
                        log::warn!("Horizon: registry install of {project} failed: {why}");
                        (None, lang::tr("horizon.mods.browse.install_failed", &[&why]))
                    }
                };
                ui.message = Some(InstallMessage {
                    project,
                    text,
                    success: kind.is_some(),
                });
                (kind, true)
            }
        }
    }
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(work) {
        log::warn!("Horizon: {name} did not start: {e}");
    }
}

impl Game<'_> {
    /// Each frame: what the browser's worker finished.
    pub(super) fn drive_mod_browser(&mut self) {
        // The Browse mods page's layout reads the browser: a worker's answer
        // arriving makes the built page again at its next draw.
        let (installed, changed) = self.mod_jobs.poll(
            &mut self.ui.options.mod_browser,
            &mut self.settings.mods,
            self.player.is_some(),
        );
        if changed {
            self.ui.options.layout_changed();
        }
        match installed {
            Some(ProjectKind::Mod) => {
                self.settings.save(&Self::options_path());
                self.start_mods();
                self.switch_mods();
                // A mod with blocks, items or worldgen waits for a restart: the
                // project page says so instead of "installed and on".
                if let (Some(note), Some(message)) = (
                    &self.ui.options.mods_note,
                    &mut self.ui.options.mod_browser.message,
                ) {
                    message.text.push(' ');
                    message.text.push_str(note);
                }
            }
            Some(ProjectKind::ResourcePack) => {
                let infos = self.resource_pack_infos();
                self.ui.options.update_pack_infos(infos);
                self.ui.options.pack_configs = super::packs::pack_configs(&self.vanilla_root());
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_completions_and_disconnects_report_visible_changes() {
        let (send, receive) = channel();
        let mut jobs = Jobs {
            running: Some(receive),
            index: None,
        };
        let mut ui = ModBrowser {
            status: Status::Loading,
            ..ModBrowser::default()
        };
        let mut mods = ModList::default();
        assert_eq!(jobs.poll(&mut ui, &mut mods, false), (None, false));
        send.send(Finished::Index(Err("offline".into()))).unwrap();
        assert_eq!(jobs.poll(&mut ui, &mut mods, false), (None, true));
        assert_eq!(ui.status, Status::Failed("offline".into()));
        assert_eq!(jobs.poll(&mut ui, &mut mods, false), (None, false));

        let (send, receive) = channel();
        jobs.running = Some(receive);
        ui.status = Status::Loading;
        ui.installing = Some("ruby".into());
        drop(send);
        assert_eq!(jobs.poll(&mut ui, &mut mods, false), (None, true));
        assert!(ui.installing.is_none());
        assert_eq!(ui.status, Status::Failed("the worker stopped".into()));
        assert_eq!(jobs.poll(&mut ui, &mut mods, false), (None, false));

        let (send, receive) = channel();
        jobs.running = Some(receive);
        ui.status = Status::Ready;
        drop(send);
        assert_eq!(jobs.poll(&mut ui, &mut mods, false), (None, false));
        assert_eq!(ui.status, Status::Ready);

        let (send, receive) = channel();
        jobs.running = Some(receive);
        ui.installing = Some("ruby".into());
        send.send(Finished::Installed("ruby".into(), Err("offline".into()), Vec::new()))
            .unwrap();
        assert_eq!(jobs.poll(&mut ui, &mut mods, false), (None, true));
        assert!(ui.installing.is_none());
        assert!(matches!(ui.message, Some(m) if m.project == "ruby" && !m.success));
        assert!(mods.enabled.is_empty());
    }

    #[test]
    fn an_install_finishing_after_world_open_waits_to_enable_its_content() {
        let (send, receive) = channel();
        let mut jobs = Jobs {
            running: Some(receive),
            index: None,
        };
        let mut ui = ModBrowser::default();
        let mut mods = ModList::default();
        mods.disabled.insert("ruby".into());
        send.send(Finished::Installed(
            "ruby".into(),
            Ok(Done {
                id: "ruby".into(),
                kind: ProjectKind::Mod,
                version: "1.0".into(),
                verified: false,
            }),
            Vec::new(),
        ))
        .unwrap();
        assert_eq!(jobs.poll(&mut ui, &mut mods, true), (None, false));
        assert!(mods.enabled.is_empty());
        assert!(mods.disabled.contains("ruby"));
        assert!(jobs.running.is_some());
        assert_eq!(
            jobs.poll(&mut ui, &mut mods, false),
            (Some(ProjectKind::Mod), true)
        );
        assert!(mods.enabled.contains("ruby"));
        assert!(!mods.disabled.contains("ruby"));
        assert!(jobs.running.is_none());
    }

    #[test]
    fn installing_a_resource_pack_does_not_change_mod_selection() {
        let (send, receive) = channel();
        let mut jobs = Jobs {
            running: Some(receive),
            index: None,
        };
        let mut ui = ModBrowser::default();
        let mut mods = ModList::default();
        mods.enabled.insert("ruby".into());
        mods.disabled.insert("crisp".into());
        send.send(Finished::Installed(
            "crisp".into(),
            Ok(Done {
                id: "crisp".into(),
                kind: ProjectKind::ResourcePack,
                version: "1.0".into(),
                verified: false,
            }),
            Vec::new(),
        ))
        .unwrap();
        assert_eq!(
            jobs.poll(&mut ui, &mut mods, false),
            (Some(ProjectKind::ResourcePack), true)
        );
        assert_eq!(mods.enabled.into_iter().collect::<Vec<_>>(), ["ruby"]);
        assert_eq!(mods.disabled.into_iter().collect::<Vec<_>>(), ["crisp"]);
        assert!(matches!(ui.message, Some(m) if m.project == "crisp" && m.success));
    }
}
