use super::*;

// The running game has one credential store and one active account.
static SERIAL: Mutex<()> = Mutex::new(());

fn saved(name: &str, expires_at: u64) -> Saved {
    Saved {
        name: name.into(),
        uuid: "069a79f444e94726a5befca90e38aaf5".into(),
        xuid: Some("2535".into()),
        refresh_token: "test-refresh".into(),
        minecraft_token: "test-minecraft".into(),
        expires_at,
    }
}

fn activate(dir: &Path, saved: Saved) -> u64 {
    let mut store = lock(&STORE);
    let generation = store.start();
    assert!(store.activate(dir, generation, saved, true).unwrap().is_some());
    generation
}

#[test]
fn fresh_credentials_resume_without_network_and_sign_out_even_if_absent() {
    let _serial = lock(&SERIAL);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let generation = activate(dir, saved("Steve", now() + 2 * RENEW_BEFORE));
    auth::replace_current(None);
    assert_eq!(
        resume(dir, generation, |_| unreachable!()).unwrap().as_deref(),
        Some("Steve"),
    );
    let session = auth::current().unwrap();
    assert_eq!(session.name, "Steve");
    assert_eq!(session.uuid, 0x069a79f444e94726a5befca90e38aaf5);
    sign_out(dir).unwrap();
    sign_out(dir).unwrap();
    assert!(!has_saved_credentials(dir));
    assert!(auth::current().is_none());
}

#[test]
fn failed_rename_does_not_publish_or_leave_a_temporary_credential() {
    let _serial = lock(&SERIAL);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    auth::replace_current(None);
    let obstruction = dir.join(FILE_NAME);
    fs::create_dir(&obstruction).unwrap();
    let mut store = lock(&STORE);
    let generation = store.start();
    let error = store.activate(dir, generation, saved("Steve", 0), true).unwrap_err();
    assert!(error.contains("Could not save"));
    assert!(store.temp_by.is_none());
    assert!(auth::current().is_none());
    assert!(!dir.join(TEMP_NAME).exists());
    assert!(obstruction.is_dir());
}

#[test]
fn a_non_not_found_delete_failure_keeps_the_account_and_can_be_retried() {
    let _serial = lock(&SERIAL);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    activate(dir, saved("Steve", 0));
    let obstruction = dir.join(TEMP_NAME);
    fs::create_dir(&obstruction).unwrap();
    assert!(sign_out(dir).unwrap_err().contains("Could not remove"));
    assert!(dir.join(FILE_NAME).is_file());
    assert_eq!(auth::current().unwrap().name, "Steve");
    fs::remove_dir(obstruction).unwrap();
    sign_out(dir).unwrap();
    assert!(!has_saved_credentials(dir));
    assert!(auth::current().is_none());
}

#[test]
fn late_cancel_reports_delete_failure_and_removes_credentials_on_retry() {
    let _serial = lock(&SERIAL);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let generation = activate(dir, saved("Steve", 0));
    let sign_in = SignIn {
        attempt: Arc::new(Mutex::new(Attempt {
            state: State::SignedIn,
            cancelled: false,
        })),
        dir: dir.to_owned(),
        generation,
    };
    let path = dir.join(FILE_NAME);
    let credentials = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(sign_in.cancel().unwrap_err().contains("Could not remove"));
    assert!(lock(&sign_in.attempt).cancelled);
    assert_eq!(auth::current().unwrap().name, "Steve");
    fs::remove_dir(&path).unwrap();
    fs::write(&path, credentials).unwrap();
    sign_in.cancel().unwrap();
    assert!(!has_saved_credentials(dir));
    assert!(auth::current().is_none());
}

#[test]
fn replacement_sign_in_wins_over_an_in_flight_resume() {
    let _serial = lock(&SERIAL);
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let generation = activate(dir, saved("Steve", 0));
    let restored = resume(dir, generation, |_| {
        let mut store = STORE.try_lock().expect("renewal must not hold the store lock");
        let replacement = store.start();
        assert!(
            store
                .activate(dir, replacement, saved("Alex", 0), true)
                .unwrap()
                .is_some()
        );
        // A Cancel still belonging to the old attempt cannot remove the replacement.
        store.cancel(dir, generation).unwrap();
        Ok(saved("Steve", 0))
    })
    .unwrap();
    assert_eq!(restored, None);
    let saved: Saved = serde_json::from_slice(&fs::read(dir.join(FILE_NAME)).unwrap()).unwrap();
    assert_eq!(saved.name, "Alex");
    assert_eq!(auth::current().unwrap().name, "Alex");
    sign_out(dir).unwrap();
}
