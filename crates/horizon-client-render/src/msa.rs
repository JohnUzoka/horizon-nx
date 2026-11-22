//! In-game Microsoft sign-in for Switch: OAuth device code, Xbox Live, XSTS and
//! Minecraft ownership/profile. The launcher supplies the desktop account as before.
//! Switch stores its account in the game folder on the SD card until sign-out.
//! Configure a Minecraft-services-approved Azure app through HORIZON_MSA_CLIENT_ID
//! at build time or in the runtime environment (env.txt on Switch).
mod persistence;
pub use persistence::{SignIn, State, has_saved_credentials, resume_in_background, sign_out};

use serde_json::{Value, json};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCOPE: &str = "XboxLive.signin offline_access";
const MICROSOFT: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const XBOX_USER: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MINECRAFT: &str = "https://api.minecraftservices.com";

/// The configured Azure app: the run-time variable wins over the build-time one; empty means none.
fn client_id() -> Option<String> {
    std::env::var("HORIZON_MSA_CLIENT_ID")
        .ok()
        .or_else(|| option_env!("HORIZON_MSA_CLIENT_ID").map(str::to_owned))
        .map(|id| match id.trim() {
            trimmed if trimmed.len() == id.len() => id,
            trimmed => trimmed.to_owned(),
        })
        .filter(|id| !id.is_empty())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn request(method: &str, url: &str) -> ureq::Request {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(&format!("Horizon/{}", horizon_core::release::VERSION))
        .build()
        .request(method, url)
}

enum Body<'a> {
    Form(Vec<(&'a str, &'a str)>),
    Json(Value),
}

/// One request: the status and the JSON body (null when there is none). Transport errors name
/// the service, never the URL or the request.
fn send(request: ureq::Request, body: Option<Body>, service: &str) -> Result<(u16, Value), String> {
    let result = match body {
        None => request.call(),
        Some(Body::Form(fields)) => request.send_form(&fields),
        Some(Body::Json(value)) => request
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .send_string(&value.to_string()),
    };
    let response = match result {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(_)) => {
            return Err(format!(
                "Could not reach {service}. Check the Switch's internet connection."
            ));
        }
    };
    let status = response.status();
    let text = response.into_string().unwrap_or_default();
    Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

// ----- The device-code flow ----------------------------------------------------------------------

/// What the player types at the verification page.
struct DeviceCode {
    user_code: String,
    verification_uri: String,
    interval: u64,
    expires_at: u64,
    device_code: String,
}

struct MsTokens {
    access: String,
    refresh: String,
}

fn device_code(client_id: &str) -> Result<DeviceCode, String> {
    let (status, v) = send(
        request("POST", &format!("{MICROSOFT}/devicecode")),
        Some(Body::Form(vec![("client_id", client_id), ("scope", SCOPE)])),
        "Microsoft",
    )?;
    match (
        status,
        str_field(&v, "user_code"),
        str_field(&v, "device_code"),
    ) {
        (200, Some(user_code), Some(device_code)) => Ok(DeviceCode {
            user_code,
            verification_uri: str_field(&v, "verification_uri")
                .unwrap_or_else(|| "https://www.microsoft.com/link".into()),
            interval: v
                .get("interval")
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .max(1),
            expires_at: now() + v.get("expires_in").and_then(Value::as_u64).unwrap_or(900),
            device_code,
        }),
        _ => Err(oauth_error(&v)),
    }
}

fn oauth_error(v: &Value) -> String {
    match v.get("error").and_then(Value::as_str) {
        Some("authorization_declined") => "Sign-in was declined in the browser.".into(),
        Some("expired_token") => "The sign-in code expired. Try again.".into(),
        Some("bad_verification_code") => "The sign-in code was not recognised. Try again.".into(),
        Some("invalid_grant") => "Your Microsoft sign-in expired. Sign in again.".into(),
        Some("invalid_client" | "unauthorized_client") => {
            "Microsoft refused this build's app registration (client ID).".into()
        }
        Some(other) => format!("Microsoft sign-in failed ({other})."),
        None => "Microsoft sign-in failed.".into(),
    }
}

fn ms_tokens(status: u16, v: &Value) -> Result<MsTokens, String> {
    match (
        status,
        str_field(v, "access_token"),
        str_field(v, "refresh_token"),
    ) {
        (200, Some(access), Some(refresh)) => Ok(MsTokens { access, refresh }),
        _ => Err(oauth_error(v)),
    }
}

/// Poll the token endpoint as asked until the player has signed in, the code expires, or
/// `cancelled` says stop.
fn wait_for_sign_in(
    client_id: &str,
    code: &DeviceCode,
    cancelled: &dyn Fn() -> bool,
) -> Result<MsTokens, String> {
    let mut interval = code.interval;
    loop {
        let until = Instant::now() + Duration::from_secs(interval);
        while Instant::now() < until {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if now() > code.expires_at {
            return Err("The sign-in code expired. Try again.".into());
        }
        let (status, v) = send(
            request("POST", &format!("{MICROSOFT}/token")),
            Some(Body::Form(vec![
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id),
                ("device_code", &code.device_code),
            ])),
            "Microsoft",
        )?;
        match v.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            _ => return ms_tokens(status, &v),
        }
    }
}

const CANCELLED: &str = "Sign-in cancelled.";

fn refresh_ms(client_id: &str, refresh: &str) -> Result<MsTokens, String> {
    let (status, v) = send(
        request("POST", &format!("{MICROSOFT}/token")),
        Some(Body::Form(vec![
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh),
            ("scope", SCOPE),
        ])),
        "Microsoft",
    )?;
    ms_tokens(status, &v)
}

// ----- Xbox Live, XSTS, Minecraft ----------------------------------------------------------------

fn xsts_error(code: u64) -> String {
    match code {
        2148916233 => "This Microsoft account has no Xbox profile yet. Sign in once at xbox.com to create one, then try again.".into(),
        2148916235 => "Xbox Live is not available in your country or region.".into(),
        2148916236 | 2148916237 => "This account needs adult verification on xbox.com before it can play.".into(),
        2148916238 => "This is a child account: an adult has to add it to a Microsoft family first.".into(),
        other => format!("Xbox Live refused the sign-in (error {other})."),
    }
}

struct Minecraft {
    token: String,
    expires_at: u64,
    xuid: Option<String>,
}

fn minecraft_login(ms_access: &str) -> Result<Minecraft, String> {
    let (status, xbl) = send(
        request("POST", XBOX_USER),
        Some(Body::Json(json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))),
        "Xbox Live",
    )?;
    let xbl_token = str_field(&xbl, "Token")
        .filter(|_| status == 200)
        .ok_or("Xbox Live refused the sign-in.")?;
    let (status, xsts) = send(
        request("POST", XSTS),
        Some(Body::Json(json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }))),
        "Xbox Live",
    )?;
    if let Some(code) = xsts.get("XErr").and_then(Value::as_u64) {
        return Err(xsts_error(code));
    }
    let claims = xsts.pointer("/DisplayClaims/xui/0");
    let (Some(xsts_token), Some(uhs)) = (
        str_field(&xsts, "Token").filter(|_| status == 200),
        claims.and_then(|c| str_field(c, "uhs")),
    ) else {
        return Err("Xbox Live refused the sign-in.".into());
    };
    let xuid = claims.and_then(|c| str_field(c, "xid"));
    let xtoken = format!("XBL3.0 x={uhs};{xsts_token}");
    let lifetime = |v: &Value| v.get("expires_in").and_then(Value::as_u64).unwrap_or(86400);
    // `launcher/login` first, as the launcher does (a new account's `login_with_xbox` token names
    // no profile); `login_with_xbox` stays the fallback.
    if let Ok((200, v)) = send(
        request("POST", &format!("{MINECRAFT}/launcher/login")),
        Some(Body::Json(
            json!({ "xtoken": xtoken, "platform": "PC_LAUNCHER" }),
        )),
        "the Minecraft services",
    ) && let Some(token) = str_field(&v, "access_token")
    {
        return Ok(Minecraft {
            token,
            expires_at: now() + lifetime(&v),
            xuid,
        });
    }
    let (status, mc) = send(
        request(
            "POST",
            &format!("{MINECRAFT}/authentication/login_with_xbox"),
        ),
        Some(Body::Json(json!({ "identityToken": xtoken }))),
        "the Minecraft services",
    )?;
    match (status, str_field(&mc, "access_token")) {
        (200, Some(token)) => Ok(Minecraft {
            token,
            expires_at: now() + lifetime(&mc),
            xuid,
        }),
        // New Azure apps are refused until Mojang approves them (Minecraft API access).
        (403, _) => Err(
            "The Minecraft services refused this build: its app is not approved by Mojang yet."
                .into(),
        ),
        (429, _) => Err("Too many sign-ins in a short time. Wait a minute and try again.".into()),
        _ => Err("The Minecraft services refused the sign-in.".into()),
    }
}

fn bearer(request: ureq::Request, token: &str) -> ureq::Request {
    request.set("Authorization", &format!("Bearer {token}"))
}

/// Whether the account owns Minecraft: Java Edition (Game Pass included).
fn owns_game(token: &str) -> Result<bool, String> {
    let (status, v) = send(
        bearer(
            request("GET", &format!("{MINECRAFT}/entitlements/mcstore")),
            token,
        ),
        None,
        "the Minecraft services",
    )?;
    if status != 200 {
        return Err("Could not check which games this account owns.".into());
    }
    Ok(v.get("items")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|i| {
                matches!(
                    i.get("name").and_then(Value::as_str),
                    Some("game_minecraft" | "product_minecraft")
                )
            })
        }))
}

/// The profile's name and UUID (without dashes); none when the account has no profile.
fn profile(token: &str) -> Result<Option<(String, String)>, String> {
    let (status, v) = send(
        bearer(
            request("GET", &format!("{MINECRAFT}/minecraft/profile")),
            token,
        ),
        None,
        "the Minecraft services",
    )?;
    if status == 404 {
        return Ok(None);
    }
    match (str_field(&v, "name"), str_field(&v, "id")) {
        (Some(name), Some(id)) => Ok(Some((name, id.replace('-', "").to_lowercase()))),
        _ => Err("Could not read the Minecraft profile.".into()),
    }
}
