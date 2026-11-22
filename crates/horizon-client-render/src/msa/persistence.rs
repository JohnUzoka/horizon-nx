use super::{
    CANCELLED, MsTokens, client_id, device_code, minecraft_login, now, owns_game, profile,
    refresh_ms, wait_for_sign_in,
};
use crate::auth::{self, AuthSession};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

const FILE_NAME: &str = "account.json";
const TEMP_NAME: &str = "account.json.tmp";
const RENEW_BEFORE: u64 = 3600;
const NOT_CONFIGURED: &str = "Microsoft sign-in is not configured. Set HORIZON_MSA_CLIENT_ID in env.txt beside the NRO to an approved app's public client ID, then restart.";

#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    name: String,
    uuid: String,
    #[serde(default)]
    xuid: Option<String>,
    refresh_token: String,
    minecraft_token: String,
    expires_at: u64,
}

fn account(ms: MsTokens) -> Result<Saved, String> {
    let mc = minecraft_login(&ms.access)?;
    if !owns_game(&mc.token)? {
        return Err("This account doesn't own Minecraft: Java Edition.".into());
    }
    let (name, uuid) = profile(&mc.token)?.ok_or(
        "This account has no Minecraft profile yet. Create one at minecraft.net, then sign in again.",
    )?;
    Ok(Saved {
        name,
        uuid,
        xuid: mc.xuid,
        refresh_token: ms.refresh,
        minecraft_token: mc.token,
        expires_at: mc.expires_at,
    })
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Sign-in and sign-out invalidate an older resume/attempt before it can publish. The owners
/// of the committed file and temporary file are tracked separately: late Cancel must delete
/// its own credentials, not those of a replacement, even if another operation has started.
#[derive(Default)]
struct Store {
    generation: u64,
    saved_by: Option<u64>,
    temp_by: Option<u64>,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    generation: 0,
    saved_by: None,
    temp_by: None,
});

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Store {
    fn start(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    fn remove_temp(&mut self, dir: &Path) -> io::Result<()> {
        remove_if_present(&dir.join(TEMP_NAME))?;
        self.temp_by = None;
        Ok(())
    }

    fn save(&mut self, dir: &Path, saved: &Saved, generation: u64) -> Result<(), String> {
        use io::Write as _;
        let text = serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?;
        let temporary = dir.join(TEMP_NAME);
        let save_error = |e| format!("Could not save the sign-in: {e}");
        self.remove_temp(dir).map_err(save_error)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(save_error)?;
        self.temp_by = Some(generation);
        let written = file.write_all(&text);
        drop(file);
        // Write beside the final file, then rename; never read a partial account on restart.
        if let Err(error) = written.and_then(|()| fs::rename(&temporary, dir.join(FILE_NAME))) {
            let mut message = save_error(error);
            if let Err(cleanup) = self.remove_temp(dir) {
                message.push_str(&format!(". Could not remove the temporary sign-in: {cleanup}"));
            }
            return Err(message);
        }
        self.temp_by = None;
        self.saved_by = Some(generation);
        Ok(())
    }

    fn activate(
        &mut self,
        dir: &Path,
        generation: u64,
        saved: Saved,
        persist: bool,
    ) -> Result<Option<String>, String> {
        if self.generation != generation {
            return Ok(None);
        }
        let uuid = auth::parse_uuid(&saved.uuid).ok_or("The saved account is damaged.")?;
        if persist {
            self.save(dir, &saved, generation)?;
        }
        auth::replace_current(Some(AuthSession::signed_in(
            saved.name.clone(),
            uuid,
            saved.xuid,
            saved.minecraft_token,
        )));
        Ok(Some(saved.name))
    }

    fn forget(&mut self, dir: &Path) -> Result<(), String> {
        self.start();
        self.remove_temp(dir)
            .and_then(|()| remove_if_present(&dir.join(FILE_NAME)))
            .map_err(|e| format!("Could not remove the saved sign-in: {e}"))?;
        self.saved_by = None;
        auth::replace_current(None);
        Ok(())
    }

    fn cancel(&mut self, dir: &Path, generation: u64) -> Result<(), String> {
        if self.temp_by == Some(generation) {
            self.remove_temp(dir)
                .map_err(|e| format!("Could not remove the temporary sign-in: {e}"))?;
        }
        if self.saved_by == Some(generation) {
            remove_if_present(&dir.join(FILE_NAME))
                .map_err(|e| format!("Could not remove the saved sign-in: {e}"))?;
            self.saved_by = None;
            auth::replace_current(None);
        }
        Ok(())
    }
}

/// Show Sign Out even when an expired or damaged account couldn't be restored.
pub fn has_saved_credentials(dir: &Path) -> bool {
    [FILE_NAME, TEMP_NAME].into_iter().any(|name| {
        !matches!(fs::symlink_metadata(dir.join(name)), Err(e) if e.kind() == io::ErrorKind::NotFound)
    })
}

/// Forget the account. An absent file is already gone; any other deletion error is shown to
/// the player and the current account is not cleared as if sign-out had succeeded.
pub fn sign_out(dir: &Path) -> Result<(), String> {
    lock(&STORE).forget(dir)
}

fn resume(
    dir: &Path,
    generation: u64,
    renew: impl FnOnce(&Saved) -> Result<Saved, String>,
) -> Result<Option<String>, String> {
    let text = {
        let store = lock(&STORE);
        if store.generation != generation {
            return Ok(None);
        }
        match fs::read_to_string(dir.join(FILE_NAME)) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Could not read the saved sign-in: {e}")),
        }
    };
    let saved: Saved = serde_json::from_str(&text).map_err(|_| "The saved account is damaged.")?;
    let renewed = saved.expires_at <= now() + RENEW_BEFORE;
    // Network I/O must not delay sign-out, cancellation, or a replacement sign-in.
    let saved = if renewed { renew(&saved)? } else { saved };
    lock(&STORE).activate(dir, generation, saved, renewed)
}

/// Restore the saved credentials on a startup thread. Register the operation before spawning
/// so a title sign-in that starts before this thread runs still takes precedence.
pub fn resume_in_background(dir: PathBuf) {
    let generation = lock(&STORE).generation;
    if !has_saved_credentials(&dir) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("Microsoft account restore".into())
        .spawn(move || {
            match resume(&dir, generation, |saved| {
                let id = client_id().ok_or(NOT_CONFIGURED)?;
                account(refresh_ms(&id, &saved.refresh_token)?)
            }) {
                Ok(Some(name)) => {
                    log::info!("Horizon: signed in to Microsoft as {name} (saved account)")
                }
                Ok(None) => {}
                Err(e) => log::warn!("Horizon: the saved Microsoft account could not sign in: {e}"),
            }
        });
    if spawned.is_err() {
        log::warn!("Horizon: could not start the saved Microsoft account restore");
    }
}

#[derive(Clone, Debug)]
pub enum State {
    Contacting,
    Code { url: String, code: String },
    Finishing,
    SignedIn,
    Failed(String),
}

struct Attempt {
    state: State,
    cancelled: bool,
}

/// A sign-in worker. The attempt lock covers the last save and publication, so Cancel either
/// stops that commit or removes its credentials. Network calls never take place under either lock.
pub struct SignIn {
    attempt: Arc<Mutex<Attempt>>,
    dir: PathBuf,
    generation: u64,
}

impl SignIn {
    pub fn start(dir: PathBuf) -> Self {
        let generation = lock(&STORE).start();
        let attempt = Arc::new(Mutex::new(Attempt {
            state: State::Contacting,
            cancelled: false,
        }));
        let shared = attempt.clone();
        let home = dir.clone();
        let spawned = std::thread::Builder::new()
            .name("Microsoft sign-in".into())
            .spawn(move || {
                let set = |state| {
                    let mut attempt = lock(&shared);
                    if attempt.cancelled {
                        return Err(CANCELLED);
                    }
                    attempt.state = state;
                    Ok(())
                };
                let result = (|| -> Result<(), String> {
                    let id = client_id().ok_or(NOT_CONFIGURED)?;
                    let mut code = device_code(&id)?;
                    set(State::Code {
                        url: std::mem::take(&mut code.verification_uri),
                        code: std::mem::take(&mut code.user_code),
                    })?;
                    let ms = wait_for_sign_in(&id, &code, &|| lock(&shared).cancelled)?;
                    set(State::Finishing)?;
                    let saved = account(ms)?;
                    let mut attempt = lock(&shared);
                    if attempt.cancelled {
                        return Err(CANCELLED.into());
                    }
                    let name = lock(&STORE)
                        .activate(&home, generation, saved, true)?
                        .ok_or(CANCELLED)?;
                    attempt.state = State::SignedIn;
                    drop(attempt);
                    log::info!("Horizon: signed in to Microsoft as {name}");
                    Ok(())
                })();
                if let Err(e) = result {
                    let mut attempt = lock(&shared);
                    if !attempt.cancelled {
                        log::warn!("Horizon: Microsoft sign-in: {e}");
                        attempt.state = State::Failed(e);
                    }
                }
            });
        if spawned.is_err() {
            lock(&attempt).state = State::Failed("Could not start the sign-in.".into());
        }
        Self {
            attempt,
            dir,
            generation,
        }
    }

    pub fn state(&self) -> State {
        lock(&self.attempt).state.clone()
    }

    /// A late Cancel must undo even a completed sign-in if the player still saw Cancel.
    /// If deletion fails, the caller can show the error and retry the same cancellation.
    pub fn cancel(&self) -> Result<(), String> {
        let mut attempt = lock(&self.attempt);
        attempt.cancelled = true;
        lock(&STORE).cancel(&self.dir, self.generation)
    }
}

impl Drop for SignIn {
    fn drop(&mut self) {
        lock(&self.attempt).cancelled = true;
    }
}

#[cfg(test)]
mod tests;
