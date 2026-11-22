use super::*;
use crate::msa::{self, SignIn, State};

pub(super) const OPEN: u8 = 109;
pub(super) const SIGN_IN: u8 = 110;
pub(super) const SIGN_OUT: u8 = 111;
pub(super) const CANCEL: u8 = 112;
pub(super) const BACK: u8 = 113;

pub(super) fn shown() -> bool {
    cfg!(target_os = "horizon") || std::env::var_os("HORIZON_ACCOUNT_BUTTON").is_some()
}

fn title() -> String {
    lang::get_or("horizon.account.title", "Microsoft Account")
}

#[derive(Clone, Copy, Default)]
enum Buttons {
    #[default]
    SignedOut,
    Saved,
    SignedIn,
    Working,
    RetryCancel,
}

#[derive(Default)]
pub(super) struct Screen {
    sign_in: Option<SignIn>,
    error: Option<String>,
    // Input must target the controls that were drawn, even if a worker just finished.
    buttons: Cell<Buttons>,
    retry_sign_in: Cell<bool>,
}

fn game_dir(saves: &Path) -> &Path {
    saves.parent().unwrap_or_else(|| Path::new("."))
}

impl Screen {
    fn dismiss(&mut self) -> bool {
        if matches!(self.buttons.get(), Buttons::Working | Buttons::RetryCancel) {
            self.cancel()
        } else {
            true
        }
    }

    fn cancel(&mut self) -> bool {
        if let Some(sign_in) = &self.sign_in
            && let Err(e) = sign_in.cancel()
        {
            self.error = Some(e);
            self.buttons.set(Buttons::RetryCancel);
            return false;
        }
        self.sign_in = None;
        self.error = None;
        self.retry_sign_in.set(false);
        self.buttons.set(Buttons::SignedOut);
        true
    }

    fn click(&mut self, button: u8, dir: &Path) {
        match button {
            SIGN_IN => {
                self.error = None;
                self.buttons.set(Buttons::Working);
                self.sign_in = Some(SignIn::start(dir.to_owned()));
            }
            SIGN_OUT => match msa::sign_out(dir) {
                Ok(()) => {
                    self.sign_in = None;
                    self.error = None;
                    self.buttons.set(Buttons::SignedOut);
                    self.retry_sign_in.set(false);
                }
                Err(e) => self.error = Some(e),
            },
            CANCEL => {
                self.cancel();
            }
            _ => {}
        }
    }

    fn lines(
        &self,
        state: Option<State>,
        session: Option<&crate::auth::AuthSession>,
    ) -> Vec<(String, u32)> {
        let white = u32::MAX;
        let red = rgba(255, 85, 85, 255);
        if let Some(error) = &self.error {
            return vec![(error.clone(), red)];
        }
        match state {
            Some(State::Contacting) => vec![(
                lang::get_or("horizon.account.contacting", "Contacting Microsoft..."),
                white,
            )],
            Some(State::Code { url, code }) => {
                let yellow = rgba(255, 255, 85, 255);
                vec![
                    (
                        lang::get_or(
                            "horizon.account.code.step",
                            "On your phone or computer, open this page:",
                        ),
                        white,
                    ),
                    (url, yellow),
                    (lang::get_or("horizon.account.code.enter", "and enter this code:"), white),
                    (code, yellow),
                    (String::new(), white),
                    (
                        lang::get_or(
                            "horizon.account.code.waiting",
                            "Waiting for you to sign in...",
                        ),
                        rgba(170, 170, 170, 255),
                    ),
                ]
            }
            Some(State::Finishing) => vec![(
                lang::get_or("horizon.account.finishing", "Signing in to Minecraft..."),
                white,
            )],
            Some(State::Failed(reason)) => vec![
                (lang::get_or("horizon.account.failed", "Sign-in failed"), red),
                (reason, white),
            ],
            Some(State::SignedIn) | None => vec![(
                match session {
                    Some(session) => translate(
                        "horizon.account.signed_in",
                        "Signed in as %s",
                        &[&session.name],
                    ),
                    None => lang::get_or(
                        "horizon.account.signed_out",
                        "Sign in with the Microsoft account that owns Minecraft: Java Edition to play on online servers as yourself. Your sign-in is kept on the SD card until you sign out.",
                    ),
                },
                white,
            )],
        }
    }

    fn switch_buttons(&self, dir: &Path, state: Option<&State>, signed_in: bool) {
        if matches!(self.buttons.get(), Buttons::RetryCancel) {
            return;
        }
        self.retry_sign_in.set(matches!(state, Some(State::Failed(_))));
        self.buttons.set(match state {
            Some(State::Contacting | State::Code { .. } | State::Finishing) => Buttons::Working,
            _ if signed_in => Buttons::SignedIn,
            _ if msa::has_saved_credentials(dir) => Buttons::Saved,
            _ => Buttons::SignedOut,
        });
    }
}

impl Title {
    pub(super) fn open_account(&mut self) {
        self.account = Screen::default();
        self.account
            .switch_buttons(game_dir(&self.saves), None, crate::auth::current().is_some());
        self.message_lines.set(1);
        self.focused_button = None;
        self.set_mode(Mode::Account);
    }

    pub(super) fn account_click(&mut self, button: u8) -> Action {
        if button == BACK {
            self.leave_account();
        } else {
            self.account.click(button, game_dir(&self.saves));
        }
        self.focused_button = None;
        Action::None
    }

    pub(super) fn leave_account(&mut self) {
        if self.account.dismiss() {
            self.focused_button = None;
            self.set_mode(Mode::Main);
        }
    }

    pub(super) fn account_narration(&self) -> crate::widgets::ScreenInfo {
        let session = crate::auth::current();
        let state = self.account.sign_in.as_ref().map(SignIn::state);
        let mut narration = title();
        for (line, _) in self.account.lines(state, session.as_deref()) {
            if !line.is_empty() {
                narration.push_str(". ");
                narration.push_str(&line);
            }
        }
        crate::widgets::ScreenInfo::new("account", narration)
    }

    pub(super) fn draw_account(&self, atlas: &HudAtlas, batch: &mut HudBatch, w: f32, h: f32) {
        let state = self.account.sign_in.as_ref().map(SignIn::state);
        let session = crate::auth::current();
        self.account.switch_buttons(game_dir(&self.saves), state.as_ref(), session.is_some());
        let mut lines = Vec::new();
        for (text, color) in self.account.lines(state, session.as_deref()) {
            if text.is_empty() {
                lines.push((text, color));
            } else {
                lines.extend(wrapped(atlas, &text, w - 50.0).into_iter().map(|line| (line, color)));
            }
        }
        self.message_lines.set(lines.len());
        let top = confirm_top(h, lines.len());
        centered(atlas, batch, &title(), top, u32::MAX);
        for (i, (line, color)) in lines.iter().enumerate() {
            centered(atlas, batch, line, top + 17.0 + i as f32 * 9.0, *color);
        }
    }

    pub(super) fn account_buttons(&self, w: f32, h: f32) -> Vec<Button> {
        let lines = self.message_lines.get();
        let y = confirm_top(h, lines) + 41.0 + lines as f32 * 9.0;
        let cx = (w / 2.0).floor();
        let b = |x, w, key, fallback, id| Button {
            x,
            y,
            w,
            label: lang::get_or(key, fallback),
            enabled: true,
            id,
            icon: None,
            trailing: None,
            action: None,
        };
        let sign_in = |x, w| {
            let (key, label) = if self.account.retry_sign_in.get() {
                ("horizon.account.retry", "Try Again")
            } else {
                ("horizon.account.sign_in", "Sign In")
            };
            b(x, w, key, label, SIGN_IN)
        };
        let sign_out = |x, w| b(x, w, "horizon.account.sign_out", "Sign Out", SIGN_OUT);
        let back = |x, w| b(x, w, "gui.back", "Back", BACK).does(crate::widgets::Action::Back);
        let pair = ((w - 20.0) / 2.0).min(150.0);
        let left = cx - pair - 2.0;
        let right = cx + 2.0;
        match self.account.buttons.get() {
            Buttons::Working => vec![
                b(cx - 75.0, 150.0, "gui.cancel", "Cancel", CANCEL)
                    .does(crate::widgets::Action::Cancel),
            ],
            Buttons::RetryCancel => vec![
                b(
                    cx - 75.0,
                    150.0,
                    "horizon.account.retry_cancel",
                    "Retry Cancel",
                    CANCEL,
                )
                .does(crate::widgets::Action::Cancel),
            ],
            Buttons::SignedIn => vec![
                sign_out(left, pair),
                b(right, pair, "gui.done", "Done", BACK).does(crate::widgets::Action::Done),
            ],
            Buttons::SignedOut => vec![sign_in(left, pair), back(right, pair)],
            Buttons::Saved => {
                let trio = ((w - 24.0) / 3.0).min(98.0);
                let middle = cx - trio / 2.0;
                vec![
                    sign_in(middle - trio - 4.0, trio),
                    sign_out(middle, trio),
                    back(middle + trio + 4.0, trio),
                ]
            }
        }
    }
}
