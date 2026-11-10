//! Inkult connector: lets users sign in to Muzik with their Inkult account,
//! the way Jellyseerr signs users in with Jellyfin.
//!
//! Muzik never sees or stores an Inkult password hash: it forwards the
//! credentials to Inkult's own sign-in API (`POST /api/auth/login`, then
//! `/api/auth/login/2fa` when the account uses 2FA), reads the account id and
//! handle from the answer, signs that Inkult session straight out again, and
//! issues its own Muzik session. Inkult's policies (account lock, temporary
//! passwords, 2FA) therefore apply unchanged.
//!
//! The connector is off until an admin turns it on (no request is made to
//! Inkult before that), and Muzik works without Inkult.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use muzik_common::InkultSettings;
use reqwest::{header, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};

/// Inkult's session cookie, ended right after a successful sign-in.
const INKULT_COOKIE: &str = "inkult_session";
/// Inkult's answers are a few hundred bytes; anything larger is refused.
const MAX_BODY: usize = 64 * 1024;
/// Second-factor sign-ins waiting at once; protects memory, not a real limit.
const MAX_PENDING: usize = 10_000;
/// Second-factor sign-ins kept per account; the oldest is dropped. A correct
/// password never counts as a failure, so without this one Inkult account
/// could fill `MAX_PENDING` and block every other 2FA sign-in.
const MAX_PENDING_PER_USER: usize = 3;
/// Inkult keeps its own challenge for about as long; Muzik's cookie matches.
pub const PENDING_TTL: Duration = Duration::from_secs(5 * 60);
/// Marker stored as `password_hash` of Inkult accounts: not a PHC string, so
/// it can never verify as a password.
pub const NO_LOCAL_PASSWORD: &str = "!inkult";

#[derive(Debug, thiserror::Error)]
pub enum InkultError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("invalid second factor")]
    InvalidCode,
    #[error("sign-in challenge expired")]
    ChallengeExpired,
    #[error("rate limited by Inkult")]
    Throttled,
    /// Temporary password or mandatory 2FA not set up yet in Inkult.
    #[error("the Inkult account must be set up in Inkult first")]
    AccountSetupRequired,
    #[error("{0}")]
    Unavailable(String),
}

/// The Inkult account that just proved its credentials.
#[derive(Debug, Clone)]
pub struct InkultAccount {
    /// Inkult's stable account id.
    pub id: i64,
    /// Inkult's handle (lowercase, `[a-z0-9._-]`).
    pub username: String,
    /// The sign-in went through Inkult's second factor.
    pub two_factor: bool,
}

pub enum InkultLogin {
    Done(InkultAccount),
    /// Password accepted; Inkult wants an authenticator or recovery code.
    SecondFactor {
        challenge: String,
    },
}

/// A sign-in waiting for its second factor, keyed by the SHA-256 of the
/// token in Muzik's `muzik_mfa` cookie.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    /// Inkult's challenge (a bearer secret; never leaves the server).
    pub challenge: String,
    /// Name typed at sign-in, lowercase (rate-limit key).
    pub username: String,
    pub attempts: u32,
    expires: Instant,
}

/// The account part of Inkult's sign-in answers.
#[derive(Deserialize)]
struct AccountBody {
    id: i64,
    username: String,
    #[serde(default)]
    two_factor_enabled: bool,
    #[serde(default)]
    password_change_required: bool,
    #[serde(default)]
    two_factor_enrollment_required: bool,
}

pub struct InkultClient {
    http: reqwest::Client,
    pending: Mutex<HashMap<String, PendingLogin>>,
}

impl InkultClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            // Credentials go to the configured URL only, never to a redirect.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            // Below Inkult's own 20 s sign-in deadline.
            .timeout(Duration::from_secs(15))
            .user_agent(concat!("Muzik/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// `GET /api/auth/state`: public, answers `{ setup_required, version }`.
    /// Returns the Inkult version, or a reason the admin can act on.
    pub async fn probe(&self, base: &str) -> Result<String, String> {
        let resp = self
            .http
            .get(format!("{base}/api/auth/state"))
            .send()
            .await
            .map_err(|e| describe(&e))?;
        let status = resp.status();
        if status != StatusCode::OK {
            return Err(status_hint(status));
        }
        let body = read_json(resp).await.map_err(|e| e.to_string())?;
        match (body.get("version"), body.get("setup_required")) {
            (Some(Value::String(v)), Some(Value::Bool(_))) => Ok(v
                .chars()
                .filter(|c| c.is_ascii_graphic())
                .take(32)
                .collect()),
            _ => Err("the server answered, but not like Inkult does".into()),
        }
    }

    /// First sign-in step. `client_ip` is passed as `X-Forwarded-For` so that
    /// Inkult's per-client limits see the real client when Muzik is listed in
    /// its `INKULT_TRUSTED_PROXIES` (Inkult ignores the header otherwise).
    pub async fn login(
        &self,
        base: &str,
        username: &str,
        password: &str,
        client_ip: IpAddr,
    ) -> Result<InkultLogin, InkultError> {
        let body = json!({ "username": username, "password": password });
        let (body, cookie) = self
            .post(base, "/api/auth/login", &body, client_ip, false)
            .await?;
        if body.get("second_factor_required") == Some(&Value::Bool(true)) {
            let challenge = body
                .get("challenge")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty() && c.len() <= 256)
                .ok_or_else(|| unexpected("challenge missing"))?;
            return Ok(InkultLogin::SecondFactor {
                challenge: challenge.to_owned(),
            });
        }
        self.account(base, body, cookie, false)
            .map(InkultLogin::Done)
    }

    /// Second sign-in step: an authenticator or recovery code.
    pub async fn second_factor(
        &self,
        base: &str,
        challenge: &str,
        code: &str,
        client_ip: IpAddr,
    ) -> Result<InkultAccount, InkultError> {
        let body = json!({ "challenge": challenge, "code": code });
        let (body, cookie) = self
            .post(base, "/api/auth/login/2fa", &body, client_ip, true)
            .await?;
        self.account(base, body, cookie, true)
    }

    /// Reads the signed-in account and ends the Inkult session Muzik was
    /// handed (Muzik only needed to know who it is).
    fn account(
        &self,
        base: &str,
        body: Value,
        cookie: Option<String>,
        via_second_factor: bool,
    ) -> Result<InkultAccount, InkultError> {
        if let Some(token) = cookie {
            let http = self.http.clone();
            let url = format!("{base}/api/auth/logout");
            tokio::spawn(async move {
                let sent = http
                    .post(url)
                    .header(header::COOKIE, format!("{INKULT_COOKIE}={token}"))
                    .send()
                    .await;
                if let Err(e) = sent {
                    tracing::debug!(error = %describe(&e), "could not end the Inkult session");
                }
            });
        }
        let a: AccountBody =
            serde_json::from_value(body).map_err(|_| unexpected("account missing"))?;
        if a.password_change_required || a.two_factor_enrollment_required {
            return Err(InkultError::AccountSetupRequired);
        }
        let username_ok = (1..=64).contains(&a.username.len())
            && a.username
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !username_ok || a.id < 0 {
            return Err(unexpected("malformed account"));
        }
        Ok(InkultAccount {
            id: a.id,
            username: a.username.to_ascii_lowercase(),
            two_factor: via_second_factor || a.two_factor_enabled,
        })
    }

    async fn post(
        &self,
        base: &str,
        path: &str,
        body: &Value,
        client_ip: IpAddr,
        second_step: bool,
    ) -> Result<(Value, Option<String>), InkultError> {
        let resp = self
            .http
            .post(format!("{base}{path}"))
            .header("x-forwarded-for", client_ip.to_string())
            .json(body)
            .send()
            .await
            .map_err(|e| InkultError::Unavailable(describe(&e)))?;
        let status = resp.status();
        match status {
            StatusCode::OK => {
                let cookie = session_cookie(resp.headers());
                Ok((read_json(resp).await?, cookie))
            }
            StatusCode::UNAUTHORIZED if second_step => Err(InkultError::InvalidCode),
            // 400: malformed handle; 423: locked by failed attempts. Neither
            // tells the client more than "wrong credentials".
            StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST | StatusCode::LOCKED => {
                Err(InkultError::InvalidCredentials)
            }
            StatusCode::GONE => Err(InkultError::ChallengeExpired),
            StatusCode::TOO_MANY_REQUESTS => Err(InkultError::Throttled),
            // An expired temporary password.
            StatusCode::FORBIDDEN => Err(InkultError::AccountSetupRequired),
            other => Err(InkultError::Unavailable(status_hint(other))),
        }
    }

    // ---------- Pending second-factor sign-ins ----------

    pub fn stash(&self, id: String, challenge: String, username: String) -> bool {
        let mut pending = self.lock();
        let now = Instant::now();
        pending.retain(|_, p| p.expires > now);
        let mut mine: Vec<(Instant, String)> = pending
            .iter()
            .filter(|(_, p)| p.username == username)
            .map(|(k, p)| (p.expires, k.clone()))
            .collect();
        if mine.len() >= MAX_PENDING_PER_USER {
            mine.sort();
            for (_, k) in &mine[..=mine.len() - MAX_PENDING_PER_USER] {
                pending.remove(k);
            }
        }
        if pending.len() >= MAX_PENDING {
            return false;
        }
        pending.insert(
            id,
            PendingLogin {
                challenge,
                username,
                attempts: 0,
                expires: now + PENDING_TTL,
            },
        );
        true
    }

    /// Counts an attempt on a live pending sign-in and returns it.
    pub fn attempt(&self, id: &str) -> Option<PendingLogin> {
        let mut pending = self.lock();
        let p = pending.get_mut(id)?;
        if p.expires <= Instant::now() {
            pending.remove(id);
            return None;
        }
        p.attempts += 1;
        Some(p.clone())
    }

    pub fn discard(&self, id: &str) {
        self.lock().remove(id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, PendingLogin>> {
        self.pending.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Validates an admin-entered base URL and returns it without trailing `/`.
pub fn normalize_url(raw: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "not a valid URL".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("the URL must start with http:// or https://".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the URL has no host".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the URL must not contain a user name or password".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("the URL must not contain ? or #".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Whether credentials sent to `url` could be read or forged on the way:
/// plain `http://` to a host that isn't on the local network. Docker service
/// names (`http://inkult:9876`), loopback, private and Tailscale/CGNAT
/// addresses and local-only domains are fine without TLS.
pub fn is_insecure_url(url: &str) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};
    let Ok(url) = reqwest::Url::parse(url) else {
        return false;
    };
    if url.scheme() != "http" {
        return false;
    }
    let v4_local = |ip: Ipv4Addr| {
        ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            // 100.64.0.0/10: carrier-grade NAT, used by Tailscale.
            || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64)
    };
    let v6_local = |ip: Ipv6Addr| {
        ip.is_loopback()
            || (ip.segments()[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
            || (ip.segments()[0] & 0xffc0) == 0xfe80 // link-local
            || ip.to_ipv4_mapped().is_some_and(v4_local)
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let local = match bare.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => v4_local(ip),
        Ok(IpAddr::V6(ip)) => v6_local(ip),
        Err(_) => {
            let d = host.trim_end_matches('.').to_ascii_lowercase();
            !d.contains('.')
                || [".localhost", ".local", ".internal", ".lan", ".home.arpa"]
                    .iter()
                    .any(|s| d.ends_with(s))
        }
    };
    !local
}

/// Current connector settings.
pub async fn settings(pool: &sqlx::SqlitePool) -> Result<InkultSettings, sqlx::Error> {
    let r = sqlx::query!(
        r#"SELECT inkult_enabled AS "enabled: bool", inkult_url
           FROM server_settings WHERE id = 1"#
    )
    .fetch_one(pool)
    .await?;
    let url = r.inkult_url.unwrap_or_default();
    Ok(InkultSettings {
        enabled: r.enabled,
        insecure: is_insecure_url(&url),
        url,
    })
}

/// The base URL when Inkult sign-in is on and configured.
pub fn active_url(s: &InkultSettings) -> Option<&str> {
    (s.enabled && !s.url.is_empty()).then_some(s.url.as_str())
}

fn unexpected(what: &str) -> InkultError {
    InkultError::Unavailable(format!("unexpected answer from Inkult: {what}"))
}

fn status_hint(status: StatusCode) -> String {
    match status {
        StatusCode::MISDIRECTED_REQUEST => "Inkult refused the host name in this URL: add it to \
             INKULT_ALLOWED_HOSTS on the Inkult server (421)"
            .into(),
        s if s.is_redirection() => format!(
            "Inkult answered with a redirect ({}): use the final URL, e.g. https:// instead of http://",
            s.as_u16()
        ),
        s => format!("Inkult answered HTTP {}", s.as_u16()),
    }
}

/// The error with its causes ("connection refused" is usually in a source).
fn describe(e: &reqwest::Error) -> String {
    let mut out = if e.is_timeout() {
        "Inkult did not answer in time".to_owned()
    } else if e.is_connect() {
        "could not connect to Inkult".to_owned()
    } else {
        "request to Inkult failed".to_owned()
    };
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

fn session_cookie(headers: &header::HeaderMap) -> Option<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|v| v.split(';').next()?.trim().split_once('='))
        .find(|(name, _)| *name == INKULT_COOKIE)
        .map(|(_, v)| v.to_owned())
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric()))
}

async fn read_json(mut resp: reqwest::Response) -> Result<Value, InkultError> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| InkultError::Unavailable(describe(&e)))?
    {
        if buf.len() + chunk.len() > MAX_BODY {
            return Err(unexpected("answer too large"));
        }
        buf.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&buf).map_err(|_| unexpected("not JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_normalized() {
        assert_eq!(
            normalize_url(" http://inkult:9876/ ").unwrap(),
            "http://inkult:9876"
        );
        assert_eq!(
            normalize_url("https://example.org/inkult/").unwrap(),
            "https://example.org/inkult"
        );
        assert!(normalize_url("ftp://inkult").is_err());
        assert!(normalize_url("http://user:pw@inkult").is_err());
        assert!(normalize_url("http://inkult/?x=1").is_err());
        assert!(normalize_url("inkult:9876").is_err());
    }

    #[test]
    fn plain_http_is_only_flagged_off_the_local_network() {
        for ok in [
            "https://inkult.example.org",
            "http://inkult:9876",
            "http://localhost:9876",
            "http://127.0.0.1",
            "http://10.1.2.3",
            "http://192.168.1.20:9876",
            "http://172.20.0.5",
            "http://100.101.102.103",
            "http://[::1]:9876",
            "http://[fd12::1]",
            "http://nas.local",
            "http://inkult.home.arpa",
        ] {
            assert!(!is_insecure_url(ok), "{ok}");
        }
        for bad in [
            "http://inkult.example.org",
            "http://203.0.113.7:9876",
            "http://[2001:db8::1]",
            "http://172.32.0.1",
        ] {
            assert!(is_insecure_url(bad), "{bad}");
        }
    }

    #[test]
    fn finds_the_inkult_session_cookie() {
        let mut h = header::HeaderMap::new();
        h.append(header::SET_COOKIE, "other=1; Path=/".parse().unwrap());
        h.append(
            header::SET_COOKIE,
            "inkult_session=ab12; Path=/; HttpOnly".parse().unwrap(),
        );
        assert_eq!(session_cookie(&h).as_deref(), Some("ab12"));
    }

    #[test]
    fn pending_sign_ins_count_attempts() {
        let c = InkultClient::new().unwrap();
        assert!(c.stash("k".into(), "ch".into(), "bob".into()));
        assert_eq!(c.attempt("k").unwrap().attempts, 1);
        assert_eq!(c.attempt("k").unwrap().attempts, 2);
        c.discard("k");
        assert!(c.attempt("k").is_none());
    }

    #[test]
    fn one_account_keeps_few_pending_sign_ins() {
        let c = InkultClient::new().unwrap();
        for i in 0..10 {
            assert!(c.stash(format!("k{i}"), "ch".into(), "bob".into()));
        }
        assert!(c.stash("other".into(), "ch".into(), "alice".into()));
        let live = (0..10).filter(|i| c.attempt(&format!("k{i}")).is_some());
        assert_eq!(live.collect::<Vec<_>>(), vec![7, 8, 9]);
        assert!(c.attempt("other").is_some());
    }
}
