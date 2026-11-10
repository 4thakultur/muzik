use axum::{extract::FromRequestParts, http::request::Parts};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::{db::now, error::AppError, state::AppState};

/// Cookie names. Over HTTPS the `__Host-` prefix makes browsers refuse the
/// cookie unless it is `Secure`, host-only and `Path=/`, so a sibling
/// subdomain or a plain-HTTP response can never set or shadow it.
const COOKIE_NAME_SECURE: &str = "__Host-muzik_session";
const COOKIE_NAME_PLAIN: &str = "muzik_session";
const MFA_COOKIE_SECURE: &str = "__Host-muzik_mfa";
const MFA_COOKIE_PLAIN: &str = "muzik_mfa";
pub const SESSION_TTL_SECS: i64 = 30 * 24 * 3600;
/// How long a password-verified sign-in waits for its second factor.
pub const MFA_CHALLENGE_TTL_SECS: i64 = 5 * 60;

pub fn cookie_name(secure: bool) -> &'static str {
    if secure {
        COOKIE_NAME_SECURE
    } else {
        COOKIE_NAME_PLAIN
    }
}

pub fn mfa_cookie_name(secure: bool) -> &'static str {
    if secure {
        MFA_COOKIE_SECURE
    } else {
        MFA_COOKIE_PLAIN
    }
}
/// Only bump `last_seen_at`/expiry when older than this, to avoid a write per request.
const TOUCH_INTERVAL_SECS: i64 = 300;

pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn session_cookie(token: String, secure: bool) -> Cookie<'static> {
    Cookie::build((cookie_name(secure), token))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time_duration(SESSION_TTL_SECS))
        .build()
}

pub fn removal_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((cookie_name(secure), ""))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(time_duration(0))
        .build()
}

/// Short-lived cookie identifying a pending second-factor challenge.
pub fn mfa_cookie(token: String, secure: bool) -> Cookie<'static> {
    Cookie::build((mfa_cookie_name(secure), token))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Strict)
        .path("/")
        .max_age(time_duration(MFA_CHALLENGE_TTL_SECS))
        .build()
}

pub fn mfa_removal_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((mfa_cookie_name(secure), ""))
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Strict)
        .path("/")
        .max_age(time_duration(0))
        .build()
}

/// Deletes every session of `user_id`, except `keep` (a session id hash).
pub async fn revoke_all(
    conn: &mut sqlx::SqliteConnection,
    user_id: i64,
    keep: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let keep = keep.unwrap_or("");
    let r = sqlx::query!(
        "DELETE FROM sessions WHERE user_id = ? AND id <> ?",
        user_id,
        keep
    )
    .execute(&mut *conn)
    .await?;
    Ok(r.rows_affected())
}

/// Removes expired sessions and sign-in challenges.
pub async fn purge_expired(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let now = now();
    sqlx::query!("DELETE FROM sessions WHERE expires_at <= ?", now)
        .execute(pool)
        .await?;
    sqlx::query!("DELETE FROM login_challenges WHERE expires_at <= ?", now)
        .execute(pool)
        .await?;
    Ok(())
}

fn time_duration(secs: i64) -> time::Duration {
    time::Duration::seconds(secs)
}

pub async fn create(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    user_agent: Option<&str>,
) -> Result<String, sqlx::Error> {
    let token = new_token();
    let id = token_hash(&token);
    let now = now();
    let expires = now + SESSION_TTL_SECS;
    let ua = user_agent.map(|s| s.chars().take(256).collect::<String>());
    sqlx::query!(
        "INSERT INTO sessions (id, user_id, created_at, expires_at, last_seen_at, user_agent)
         VALUES (?, ?, ?, ?, ?, ?)",
        id,
        user_id,
        now,
        expires,
        now,
        ua
    )
    .execute(pool)
    .await?;
    Ok(token)
}

#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub session_id: String,
    /// 2FA is mandatory for this user (own flag or server policy) but not set
    /// up yet: the session may only reach the enrollment endpoints.
    pub totp_setup_pending: bool,
    /// Signs in through Inkult: no Muzik password or 2FA to manage.
    pub inkult: bool,
    /// Separate libraries: the Inkult account id whose songs this user may
    /// see (plus songs not in Inkult's index). `None` = the whole library:
    /// separation off, a local account, a Muzik admin or an Inkult captain.
    /// Every library query passes it to its ownership predicate.
    pub library_owner: Option<i64>,
}

impl CurrentUser {
    async fn load(parts: &Parts, state: &AppState) -> Result<Self, AppError> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(cookie_name(state.config.secure_cookies))
            .map(|c| c.value().to_owned())
            .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or(AppError::Unauthorized)?;
        let id = token_hash(&token);
        let now = now();
        let row = sqlx::query!(
            r#"SELECT u.id AS "user_id!", u.username, u.is_admin AS "is_admin: bool", s.last_seen_at,
                      u.auth_provider = 'inkult' AS "inkult!: bool",
                      -- Inkult accounts get their 2FA from Inkult.
                      (CASE WHEN u.auth_provider = 'inkult' THEN u.inkult_two_factor = 0
                            ELSE u.totp_secret IS NULL END
                       AND (u.totp_required <> 0
                        OR (SELECT require_totp FROM server_settings WHERE id = 1) <> 0))
                        AS "totp_setup_pending!: bool",
                      -- -1 (owns nothing) for an Inkult account not linked yet.
                      CASE WHEN u.auth_provider = 'inkult' AND u.is_admin = 0 AND u.inkult_captain = 0
                                AND (SELECT inkult_separate_libraries FROM server_settings WHERE id = 1) <> 0
                           THEN COALESCE(u.inkult_user_id, -1) END AS "library_owner?: i64"
               FROM sessions s JOIN users u ON u.id = s.user_id
               WHERE s.id = ? AND s.expires_at > ? AND u.disabled = 0"#,
            id,
            now
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;

        if now - row.last_seen_at > TOUCH_INTERVAL_SECS {
            let expires = now + SESSION_TTL_SECS;
            sqlx::query!(
                "UPDATE sessions SET last_seen_at = ?, expires_at = ? WHERE id = ?",
                now,
                expires,
                id
            )
            .execute(&state.pool)
            .await?;
        }

        Ok(Self {
            id: row.user_id,
            username: row.username,
            is_admin: row.is_admin,
            session_id: id,
            totp_setup_pending: row.totp_setup_pending,
            inkult: row.inkult,
            library_owner: row.library_owner,
        })
    }
}

/// A signed-in user allowed to use Muzik: sessions that still have to set up
/// a mandatory 2FA are rejected with `mfa_setup_required`.
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = Self::load(parts, state).await?;
        if user.totp_setup_pending {
            return Err(AppError::MfaSetupRequired);
        }
        Ok(user)
    }
}

/// Any valid session, including one that must set up 2FA first. Only for
/// `/me`, sign-out and the 2FA enrollment endpoints.
#[derive(Debug, Clone)]
pub struct SessionUser(pub CurrentUser);

impl FromRequestParts<AppState> for SessionUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        CurrentUser::load(parts, state).await.map(Self)
    }
}

/// A logged-in administrator.
#[derive(Debug, Clone)]
pub struct AdminUser(pub CurrentUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        if user.is_admin {
            Ok(Self(user))
        } else {
            Err(AppError::Forbidden)
        }
    }
}
