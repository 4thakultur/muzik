use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    Json,
};
use axum_extra::extract::CookieJar;
use muzik_common::{
    ChangePasswordRequest, LoginRequest, LoginResponse, MfaLoginRequest, RecoveryCodes,
    SetupRequest, SetupStatus, TotpConfirmRequest, TotpEnableRequest, TotpSetup, TotpSetupRequest,
    TotpStatus, UpdateOwnLibrary, User,
};

use super::extract::Json as JsonBody;
use crate::{
    auth::{client_ip::ClientIp, password, session, totp, CurrentUser, SessionUser},
    db::now,
    error::{ApiResult, AppError},
    inkult::{self, InkultAccount, InkultError, InkultLogin},
    state::AppState,
};

/// Wrong codes allowed per sign-in challenge before it is discarded.
const MAX_CHALLENGE_ATTEMPTS: i64 = 5;
/// Sign-in challenges kept per account; older ones are dropped. A correct
/// password never counts as a failure, so without this cap whoever knows it
/// could pile up challenges without limit.
const MAX_CHALLENGES_PER_USER: i64 = 3;

/// Usernames are ASCII so that case-insensitive matching (SQLite `NOCASE`)
/// is exact and look-alike Unicode names can't impersonate another user.
pub fn validate_username(username: &str) -> ApiResult<String> {
    let u = username.trim();
    let ok = (2..=32).contains(&u.len())
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(u.to_owned())
    } else {
        Err(AppError::BadRequest(
            "username must be 2-32 characters: letters, digits, '_', '-', '.'".into(),
        ))
    }
}

/// Control characters and bidirectional overrides, which could make a name
/// render as something else (e.g. `U+202E` reversing the text after it).
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
}

pub fn validate_display_name(name: Option<&str>, fallback: &str) -> ApiResult<String> {
    let n: String = name
        .unwrap_or("")
        .chars()
        .filter(|c| !is_unsafe_char(*c))
        .collect();
    let n = match n.trim() {
        "" => fallback,
        n => n,
    };
    if n.chars().count() > 64 {
        return Err(AppError::BadRequest("display name is too long".into()));
    }
    Ok(n.to_owned())
}

pub async fn fetch_user(pool: &sqlx::SqlitePool, id: i64) -> ApiResult<User> {
    let u = sqlx::query_as!(
        User,
        r#"SELECT id AS "id!", username, display_name, is_admin AS "is_admin: bool",
                  disabled AS "disabled: bool", avatar_file IS NOT NULL AS "has_avatar!: bool",
                  (CASE WHEN auth_provider = 'inkult' THEN inkult_two_factor <> 0
                        ELSE totp_secret IS NOT NULL END) AS "totp_enabled!: bool",
                  totp_required AS "totp_required: bool",
                  (totp_required <> 0 OR (SELECT require_totp FROM server_settings WHERE id = 1) <> 0)
                    AS "totp_enforced!: bool",
                  auth_provider = 'inkult' AS "inkult!: bool",
                  CASE WHEN auth_provider = 'inkult' AND inkult_captain <> 0
                            AND (SELECT inkult_separate_libraries FROM server_settings WHERE id = 1) <> 0
                       THEN inkult_own_library <> 0 END AS "own_library?: bool"
           FROM users WHERE id = ?"#,
        id
    )
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(u)
}

/// Whether 2FA is mandatory for `user_id` (own flag or server-wide policy).
pub async fn totp_enforced(conn: &mut sqlx::SqliteConnection, user_id: i64) -> ApiResult<bool> {
    let enforced = sqlx::query_scalar!(
        r#"SELECT (totp_required <> 0 OR (SELECT require_totp FROM server_settings WHERE id = 1) <> 0)
             AS "enforced!: bool"
           FROM users WHERE id = ?"#,
        user_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or(false);
    Ok(enforced)
}

fn user_agent(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
}

pub async fn setup_status(State(state): State<AppState>) -> ApiResult<Json<SetupStatus>> {
    let count = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "n!: i64" FROM users"#)
        .fetch_one(&state.pool)
        .await?;
    let inkult = inkult::settings(&state.pool).await?;
    Ok(Json(SetupStatus {
        needs_setup: count == 0,
        inkult_sign_in: inkult::active_url(&inkult).is_some(),
    }))
}

/// Creates the first (admin) account. Only allowed while no user exists.
pub async fn setup(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    jar: CookieJar,
    JsonBody(req): JsonBody<SetupRequest>,
) -> ApiResult<(CookieJar, Json<User>)> {
    let username = validate_username(&req.username)?;
    let display = validate_display_name(req.display_name.as_deref(), &username)?;
    password::validate(&req.password)?;
    let hash = password::hash_async(req.password).await?;
    let now = now();
    // Atomic "only if empty" insert, so two racing setup calls can't both win.
    let id = sqlx::query_scalar!(
        r#"INSERT INTO users (username, display_name, password_hash, is_admin, created_at)
           SELECT ?, ?, ?, 1, ? WHERE NOT EXISTS (SELECT 1 FROM users)
           RETURNING id AS "id!""#,
        username,
        display,
        hash,
        now
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Conflict("setup already completed".into()))?;
    tracing::info!(target: "auth", event = "setup", user_id = id, username = %username, ip = %ip, "admin account created");

    let token = session::create(&state.pool, id, user_agent(&headers)).await?;
    let jar = jar.add(session::session_cookie(token, state.config.secure_cookies));
    Ok((jar, Json(fetch_user(&state.pool, id).await?)))
}

pub async fn login(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    jar: CookieJar,
    JsonBody(req): JsonBody<LoginRequest>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let username = req.username.trim().to_ascii_lowercase();
    let client_ip = ip.0;
    let ip = ip.to_string();
    if username.len() > 64 || req.password.len() > 1024 {
        return Err(AppError::InvalidCredentials);
    }
    if state.auth_limiter.is_locked(&username, &ip) {
        tracing::warn!(target: "auth", event = "login_locked", username = %username, ip = %ip, "login rejected: rate limited");
        return Err(AppError::RateLimited);
    }

    let row = sqlx::query!(
        r#"SELECT id AS "id!", password_hash, disabled AS "disabled: bool",
                  totp_secret IS NOT NULL AS "totp!: bool",
                  auth_provider = 'inkult' AS "inkult!: bool"
           FROM users WHERE username = ?"#,
        username
    )
    .fetch_optional(&state.pool)
    .await?;

    // Inkult accounts, and unknown names while Inkult sign-in is on (a new
    // Inkult user, or a linked one renamed in Inkult), are checked by Inkult.
    // A local account never falls back to Inkult.
    let is_inkult = row.as_ref().is_some_and(|r| r.inkult);
    if is_inkult || row.is_none() {
        let settings = inkult::settings(&state.pool).await?;
        if let Some(url) = inkult::active_url(&settings) {
            let login = InkultSignIn {
                state: &state,
                ip: &ip,
                client_ip,
                username: &username,
                user_agent: user_agent(&headers),
            };
            return login.password(url, &req.password, jar).await;
        }
    }

    // Always run argon2 so response time doesn't reveal whether the user exists.
    let hash = row
        .as_ref()
        .map(|r| r.password_hash.clone())
        .unwrap_or_else(|| password::DUMMY_HASH.to_owned());
    let valid = password::verify_async(req.password, hash).await;

    let user = match row {
        Some(r) if valid && !r.disabled => r,
        other => {
            state.auth_limiter.record_failure(&username, &ip);
            let reason = match other {
                None => "unknown_user",
                Some(r) if r.inkult => "inkult_sign_in_off",
                Some(r) if r.disabled && valid => "disabled",
                Some(_) => "bad_password",
            };
            tracing::warn!(target: "auth", event = "login_failure", reason, username = %username, ip = %ip, "login failed");
            return Err(AppError::InvalidCredentials);
        }
    };

    if user.totp {
        // Password is right; the session is only issued after the second factor.
        let token = session::new_token();
        let id = session::token_hash(&token);
        let now = now();
        let expires = now + session::MFA_CHALLENGE_TTL_SECS;
        let mut tx = state.pool.begin().await?;
        sqlx::query!(
            "INSERT INTO login_challenges (id, user_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
            id,
            user.id,
            now,
            expires
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "DELETE FROM login_challenges WHERE user_id = ? AND id NOT IN
               (SELECT id FROM login_challenges WHERE user_id = ?
                ORDER BY created_at DESC, rowid DESC LIMIT ?)",
            user.id,
            user.id,
            MAX_CHALLENGES_PER_USER
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        tracing::info!(target: "auth", event = "login_mfa_required", user_id = user.id, username = %username, ip = %ip, "password accepted, waiting for second factor");
        let jar = jar.add(session::mfa_cookie(token, state.config.secure_cookies));
        return Ok((jar, Json(LoginResponse::MfaRequired)));
    }

    state.auth_limiter.record_success(&username, &ip);
    let token = session::create(&state.pool, user.id, user_agent(&headers)).await?;
    let user = fetch_user(&state.pool, user.id).await?;
    tracing::info!(target: "auth", event = "login_success", method = "password", user_id = user.id, username = %username, ip = %ip, "login succeeded");
    if user.needs_totp_setup() {
        // The session is limited to 2FA enrollment until it's done.
        tracing::info!(target: "auth", event = "totp_setup_required", user_id = user.id, ip = %ip, "2FA is mandatory and not set up yet");
    }
    let jar = jar.add(session::session_cookie(token, state.config.secure_cookies));
    Ok((jar, Json(LoginResponse::Ok { user })))
}

/// Second sign-in step: finishes a password-verified login with an
/// authenticator code or a recovery code.
pub async fn login_2fa(
    State(state): State<AppState>,
    ip: ClientIp,
    headers: HeaderMap,
    jar: CookieJar,
    JsonBody(req): JsonBody<MfaLoginRequest>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let secure = state.config.secure_cookies;
    let ip_addr = ip.0;
    let ip = ip.to_string();
    let token = jar
        .get(session::mfa_cookie_name(secure))
        .map(|c| c.value().to_owned())
        .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or(AppError::ChallengeExpired)?;
    let challenge_id = session::token_hash(&token);
    if let Some(pending) = state.inkult.attempt(&challenge_id) {
        let username = pending.username.clone();
        let login = InkultSignIn {
            state: &state,
            ip: &ip,
            client_ip: ip_addr,
            username: &username,
            user_agent: user_agent(&headers),
        };
        return login
            .second_factor(&challenge_id, pending, &req.code, jar)
            .await;
    }
    let now = now();

    // Count the attempt atomically before checking anything else.
    let challenge = sqlx::query!(
        r#"UPDATE login_challenges SET attempts = attempts + 1
           WHERE id = ? AND expires_at > ?
           RETURNING user_id AS "user_id!", attempts AS "attempts!""#,
        challenge_id,
        now
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::ChallengeExpired)?;

    let user = sqlx::query!(
        r#"SELECT username, disabled AS "disabled: bool" FROM users WHERE id = ?"#,
        challenge.user_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::ChallengeExpired)?;
    let username = user.username.to_ascii_lowercase();

    let discard = || async {
        sqlx::query!("DELETE FROM login_challenges WHERE id = ?", challenge_id)
            .execute(&state.pool)
            .await
    };
    if challenge.attempts > MAX_CHALLENGE_ATTEMPTS || user.disabled {
        discard().await?;
        tracing::warn!(target: "auth", event = "mfa_challenge_discarded", user_id = challenge.user_id, ip = %ip, "too many attempts or account disabled");
        return Err(AppError::ChallengeExpired);
    }
    if state.auth_limiter.is_locked(&username, &ip) {
        tracing::warn!(target: "auth", event = "login_locked", username = %username, ip = %ip, "second factor rejected: rate limited");
        return Err(AppError::RateLimited);
    }

    let Some(method) = verify_second_factor(&state.pool, challenge.user_id, &req.code).await?
    else {
        state
            .auth_limiter
            .record_second_factor_failure(&username, &ip);
        tracing::warn!(target: "auth", event = "mfa_failure", user_id = challenge.user_id, username = %username, ip = %ip, "invalid second factor");
        return Err(AppError::InvalidCode);
    };

    discard().await?;
    state.auth_limiter.record_success(&username, &ip);
    let token = session::create(&state.pool, challenge.user_id, user_agent(&headers)).await?;
    tracing::info!(target: "auth", event = "login_success", method, user_id = challenge.user_id, username = %username, ip = %ip, "login succeeded");
    let jar = jar
        .add(session::mfa_removal_cookie(secure))
        .add(session::session_cookie(token, secure));
    Ok((
        jar,
        Json(LoginResponse::Ok {
            user: fetch_user(&state.pool, challenge.user_id).await?,
        }),
    ))
}

// ---------- Sign-in with an Inkult account ----------

/// One sign-in attempt checked by the Inkult server (see `crate::inkult`).
/// Failures count toward Muzik's own limits like local ones, so Muzik can't
/// be used to brute-force Inkult accounts.
struct InkultSignIn<'a> {
    state: &'a AppState,
    ip: &'a str,
    client_ip: std::net::IpAddr,
    /// Name typed at sign-in, lowercase: the rate-limit key.
    username: &'a str,
    user_agent: Option<&'a str>,
}

impl InkultSignIn<'_> {
    async fn password(
        &self,
        url: &str,
        password: &str,
        jar: CookieJar,
    ) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
        let (state, ip, username) = (self.state, self.ip, self.username);
        if state.auth_limiter.is_locked(username, ip) {
            tracing::warn!(target: "auth", event = "login_locked", username = %username, ip = %ip, "login rejected: rate limited");
            return Err(AppError::RateLimited);
        }
        match state
            .inkult
            .login(url, username, password, self.client_ip)
            .await
        {
            Ok(InkultLogin::Done(account)) => self.finish(account, jar).await,
            Ok(InkultLogin::SecondFactor { challenge }) => {
                let token = session::new_token();
                if !state
                    .inkult
                    .stash(session::token_hash(&token), challenge, username.to_owned())
                {
                    return Err(AppError::RateLimited);
                }
                tracing::info!(target: "auth", event = "login_mfa_required", provider = "inkult", username = %username, ip = %ip, "Inkult accepted the password, waiting for second factor");
                let jar = jar.add(session::mfa_cookie(token, state.config.secure_cookies));
                Ok((jar, Json(LoginResponse::MfaRequired)))
            }
            Err(e) => Err(self.failed(e, "login_failure")),
        }
    }

    async fn second_factor(
        &self,
        challenge_id: &str,
        pending: inkult::PendingLogin,
        code: &str,
        jar: CookieJar,
    ) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
        let (state, ip, username) = (self.state, self.ip, self.username);
        if pending.attempts > MAX_CHALLENGE_ATTEMPTS as u32 {
            state.inkult.discard(challenge_id);
            tracing::warn!(target: "auth", event = "mfa_challenge_discarded", provider = "inkult", username = %username, ip = %ip, "too many attempts");
            return Err(AppError::ChallengeExpired);
        }
        if state.auth_limiter.is_locked(username, ip) {
            tracing::warn!(target: "auth", event = "login_locked", username = %username, ip = %ip, "second factor rejected: rate limited");
            return Err(AppError::RateLimited);
        }
        let settings = inkult::settings(&state.pool).await?;
        let Some(url) = inkult::active_url(&settings) else {
            state.inkult.discard(challenge_id);
            return Err(AppError::ChallengeExpired);
        };
        if code.len() > 64 {
            return Err(AppError::InvalidCode);
        }
        match state
            .inkult
            .second_factor(url, &pending.challenge, code, self.client_ip)
            .await
        {
            Ok(account) => {
                state.inkult.discard(challenge_id);
                let jar = jar.add(session::mfa_removal_cookie(state.config.secure_cookies));
                self.finish(account, jar).await
            }
            Err(e) => {
                if matches!(
                    e,
                    InkultError::ChallengeExpired | InkultError::AccountSetupRequired
                ) {
                    state.inkult.discard(challenge_id);
                }
                Err(self.failed(e, "mfa_failure"))
            }
        }
    }

    /// Inkult vouched for `account`: find or create its Muzik account and
    /// open a session.
    async fn finish(
        &self,
        account: InkultAccount,
        jar: CookieJar,
    ) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
        let (state, ip, username) = (self.state, self.ip, self.username);
        let user_id = match link_inkult_account(&state.pool, &account).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(target: "auth", event = "login_failure", provider = "inkult", reason = %e, username = %username, inkult_user = %account.username, ip = %ip, "Inkult account has no usable Muzik account");
                return Err(e);
            }
        };
        let user = fetch_user(&state.pool, user_id).await?;
        if user.disabled {
            state.auth_limiter.record_failure(username, ip);
            tracing::warn!(target: "auth", event = "login_failure", provider = "inkult", reason = "disabled", user_id, username = %username, ip = %ip, "login failed");
            return Err(AppError::InvalidCredentials);
        }
        // Muzik's mandatory 2FA is met by the Inkult account's own 2FA; it
        // can't be set up in Muzik for an Inkult account.
        if user.totp_enforced && !account.two_factor {
            tracing::warn!(target: "auth", event = "login_failure", provider = "inkult", reason = "mfa_required_by_policy", user_id, ip = %ip, "Inkult account without 2FA while 2FA is mandatory");
            return Err(AppError::InkultMfaRequired);
        }
        state.auth_limiter.record_success(username, ip);
        let token = session::create(&state.pool, user_id, self.user_agent).await?;
        let method = if account.two_factor {
            "inkult_2fa"
        } else {
            "inkult"
        };
        tracing::info!(target: "auth", event = "login_success", method, user_id, username = %user.username, ip = %ip, "login succeeded");
        let jar = jar.add(session::session_cookie(token, state.config.secure_cookies));
        Ok((jar, Json(LoginResponse::Ok { user })))
    }

    fn failed(&self, e: InkultError, event: &'static str) -> AppError {
        let (state, ip, username) = (self.state, self.ip, self.username);
        match e {
            InkultError::InvalidCredentials | InkultError::InvalidCode => {
                if matches!(e, InkultError::InvalidCode) {
                    state
                        .auth_limiter
                        .record_second_factor_failure(username, ip);
                } else {
                    state.auth_limiter.record_failure(username, ip);
                }
                tracing::warn!(target: "auth", event, provider = "inkult", reason = %e, username = %username, ip = %ip, "login failed");
                if matches!(e, InkultError::InvalidCode) {
                    AppError::InvalidCode
                } else {
                    AppError::InvalidCredentials
                }
            }
            InkultError::ChallengeExpired => AppError::ChallengeExpired,
            InkultError::Throttled => {
                tracing::warn!(target: "auth", event = "login_locked", provider = "inkult", username = %username, ip = %ip, "Inkult rate-limited the sign-in");
                AppError::RateLimited
            }
            InkultError::AccountSetupRequired => {
                tracing::info!(target: "auth", event = "login_failure", provider = "inkult", reason = "inkult_account_setup", username = %username, ip = %ip, "Inkult account must finish its setup in Inkult");
                AppError::InkultAccountSetup
            }
            InkultError::Unavailable(detail) => {
                tracing::error!(target: "auth", event = "inkult_unavailable", error = %detail, username = %username, ip = %ip, "Inkult sign-in failed");
                AppError::InkultUnavailable
            }
        }
    }
}

/// The Muzik account of an Inkult account, linking or creating it:
/// 1. the account already linked to this Inkult id (survives renames);
/// 2. an Inkult account an admin created for this handle, not linked yet;
/// 3. a new account: every Inkult account is a Muzik user.
///
/// A local account with the same name is never taken over.
async fn link_inkult_account(pool: &sqlx::SqlitePool, account: &InkultAccount) -> ApiResult<i64> {
    let mut tx = pool.begin().await?;
    if let Some(id) = sqlx::query_scalar!(
        r#"UPDATE users SET inkult_two_factor = ?, inkult_captain = ?
           WHERE inkult_user_id = ? AND auth_provider = 'inkult' RETURNING id AS "id!""#,
        account.two_factor,
        account.captain,
        account.id
    )
    .fetch_optional(&mut *tx)
    .await?
    {
        tx.commit().await?;
        return Ok(id);
    }
    let by_name = sqlx::query!(
        r#"SELECT id AS "id!", auth_provider = 'inkult' AS "inkult!: bool", inkult_user_id
           FROM users WHERE username = ?"#,
        account.username
    )
    .fetch_optional(&mut *tx)
    .await?;
    let id = match by_name {
        Some(u) if u.inkult && u.inkult_user_id.is_none() => {
            sqlx::query!(
                "UPDATE users SET inkult_user_id = ?, inkult_two_factor = ?, inkult_captain = ?
                 WHERE id = ?",
                account.id,
                account.two_factor,
                account.captain,
                u.id
            )
            .execute(&mut *tx)
            .await?;
            tracing::info!(target: "auth", event = "inkult_linked", user_id = u.id, inkult_user_id = account.id, "Muzik account linked to its Inkult account");
            u.id
        }
        // A local account, or one linked to another (since deleted) Inkult
        // account of the same name: an admin decides.
        Some(_) => return Err(AppError::InkultNameTaken),
        None => {
            let username = validate_username(&account.username)?;
            let now = now();
            let id = sqlx::query_scalar!(
                r#"INSERT INTO users (username, display_name, password_hash, auth_provider,
                                      inkult_user_id, inkult_two_factor, inkult_captain, created_at)
                   VALUES (?, ?, ?, 'inkult', ?, ?, ?, ?)
                   ON CONFLICT DO NOTHING RETURNING id AS "id!""#,
                username,
                username,
                inkult::NO_LOCAL_PASSWORD,
                account.id,
                account.two_factor,
                account.captain,
                now
            )
            .fetch_optional(&mut *tx)
            .await?
            // Two first sign-ins raced; the other one created it.
            .ok_or_else(|| AppError::Conflict("please sign in again".into()))?;
            tracing::info!(target: "auth", event = "user_created", provider = "inkult", user_id = id, inkult_user_id = account.id, username = %username, "account created on first Inkult sign-in");
            id
        }
    };
    tx.commit().await?;
    Ok(id)
}

/// Checks an authenticator code (never accepted twice) or consumes a
/// recovery code. Returns which one matched.
async fn verify_second_factor(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    code: &str,
) -> ApiResult<Option<&'static str>> {
    if code.len() > 64 {
        return Ok(None);
    }
    let row = sqlx::query!(
        "SELECT totp_secret, totp_last_step FROM users WHERE id = ?",
        user_id
    )
    .fetch_optional(pool)
    .await?;
    let Some(secret) = row.as_ref().and_then(|r| r.totp_secret.clone()) else {
        return Ok(None);
    };
    let last_step = row.and_then(|r| r.totp_last_step);

    if totp::normalize_code(code).is_some() {
        let Some(step) = totp::verify(&secret, code, now(), last_step) else {
            return Ok(None);
        };
        // Conditional update: two concurrent requests with the same code can't both win.
        let updated = sqlx::query!(
            "UPDATE users SET totp_last_step = ?
             WHERE id = ? AND (totp_last_step IS NULL OR totp_last_step < ?)",
            step,
            user_id,
            step
        )
        .execute(pool)
        .await?
        .rows_affected();
        return Ok((updated == 1).then_some("totp"));
    }

    if let Some(normalized) = totp::normalize_recovery_code(code) {
        let hash = totp::recovery_hash(&normalized);
        let now = now();
        let used = sqlx::query!(
            "UPDATE recovery_codes SET used_at = ?
             WHERE user_id = ? AND code_hash = ? AND used_at IS NULL",
            now,
            user_id,
            hash
        )
        .execute(pool)
        .await?
        .rows_affected();
        if used == 1 {
            tracing::info!(target: "auth", event = "recovery_code_used", user_id, "recovery code consumed");
            return Ok(Some("recovery"));
        }
    }
    Ok(None)
}

pub async fn logout(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    jar: CookieJar,
) -> ApiResult<(CookieJar, StatusCode)> {
    sqlx::query!("DELETE FROM sessions WHERE id = ?", user.session_id)
        .execute(&state.pool)
        .await?;
    tracing::info!(target: "auth", event = "logout", user_id = user.id, "logged out");
    Ok((
        jar.add(session::removal_cookie(state.config.secure_cookies)),
        StatusCode::NO_CONTENT,
    ))
}

/// Answers for any session, including one that must set up 2FA first
/// (`User::needs_totp_setup`), so the client knows to show the enrollment.
pub async fn me(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
) -> ApiResult<Json<User>> {
    Ok(Json(fetch_user(&state.pool, user.id).await?))
}

/// `PUT /api/me/library`: an Inkult captain chooses, while libraries are
/// separate, between hearing everything or only the songs they own.
pub async fn set_own_library(
    State(state): State<AppState>,
    user: CurrentUser,
    JsonBody(req): JsonBody<UpdateOwnLibrary>,
) -> ApiResult<Json<User>> {
    let current = fetch_user(&state.pool, user.id).await?;
    if current.own_library.is_none() {
        return Err(AppError::BadRequest(
            "only Inkult captains choose their library, while libraries are separate".into(),
        ));
    }
    sqlx::query!(
        "UPDATE users SET inkult_own_library = ? WHERE id = ?",
        req.own_library,
        user.id
    )
    .execute(&state.pool)
    .await?;
    tracing::info!(target: "auth", event = "own_library_changed", user_id = user.id, own_library = req.own_library, "captain library choice changed");
    Ok(Json(fetch_user(&state.pool, user.id).await?))
}

// ---------- Account security (signed-in user) ----------

/// Re-checks the signed-in user's password before a sensitive change. Wrong
/// guesses count toward the same limits as sign-in, so a stolen session can't
/// be used to brute-force the password.
pub(super) async fn reauthenticate(
    state: &AppState,
    user: &CurrentUser,
    ip: &str,
    password: String,
) -> ApiResult<()> {
    if user.inkult {
        return Err(AppError::ManagedByInkult);
    }
    if state.auth_limiter.is_locked(&user.username, ip) {
        return Err(AppError::RateLimited);
    }
    let hash = sqlx::query_scalar!("SELECT password_hash FROM users WHERE id = ?", user.id)
        .fetch_one(&state.pool)
        .await?;
    if password.len() > 1024 || !password::verify_async(password, hash).await {
        state.auth_limiter.record_failure(&user.username, ip);
        tracing::warn!(target: "auth", event = "reauth_failure", user_id = user.id, ip = %ip, "wrong password on sensitive action");
        return Err(AppError::WrongPassword);
    }
    Ok(())
}

/// Like [`verify_second_factor`], counting failures toward the auth limits.
pub(super) async fn require_second_factor(
    state: &AppState,
    user: &CurrentUser,
    ip: &str,
    code: &str,
) -> ApiResult<()> {
    if verify_second_factor(&state.pool, user.id, code)
        .await?
        .is_none()
    {
        state
            .auth_limiter
            .record_second_factor_failure(&user.username, ip);
        tracing::warn!(target: "auth", event = "mfa_failure", user_id = user.id, ip = %ip, "invalid second factor on sensitive action");
        return Err(AppError::InvalidCode);
    }
    Ok(())
}

/// `POST /api/me/password`: changes the password and signs out every other device.
pub async fn change_password(
    State(state): State<AppState>,
    ip: ClientIp,
    user: CurrentUser,
    JsonBody(req): JsonBody<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    let ip = ip.to_string();
    reauthenticate(&state, &user, &ip, req.current_password).await?;
    password::validate(&req.new_password)?;
    let hash = password::hash_async(req.new_password).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "UPDATE users SET password_hash = ? WHERE id = ?",
        hash,
        user.id
    )
    .execute(&mut *tx)
    .await?;
    let revoked = session::revoke_all(&mut tx, user.id, Some(&user.session_id)).await?;
    tx.commit().await?;
    tracing::info!(target: "auth", event = "password_changed", user_id = user.id, ip = %ip, revoked_sessions = revoked, "password changed");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn totp_status(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
) -> ApiResult<Json<TotpStatus>> {
    let r = sqlx::query!(
        r#"SELECT totp_secret IS NOT NULL AS "enabled!: bool", totp_enabled_at,
                  (SELECT COUNT(*) FROM recovery_codes rc
                    WHERE rc.user_id = users.id AND rc.used_at IS NULL) AS "left!: i64"
           FROM users WHERE id = ?"#,
        user.id
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(TotpStatus {
        enabled: r.enabled,
        enabled_at: r.totp_enabled_at,
        recovery_codes_left: if r.enabled { r.left } else { 0 },
    }))
}

/// `POST /api/me/2fa/setup`: creates a pending secret to scan into an app.
pub async fn totp_setup(
    State(state): State<AppState>,
    ip: ClientIp,
    SessionUser(user): SessionUser,
    JsonBody(req): JsonBody<TotpSetupRequest>,
) -> ApiResult<Json<TotpSetup>> {
    let ip = ip.to_string();
    reauthenticate(&state, &user, &ip, req.password).await?;
    let secret = totp::new_secret();
    let now = now();
    let updated = sqlx::query!(
        "UPDATE users SET totp_pending_secret = ?, totp_pending_at = ?
         WHERE id = ? AND totp_secret IS NULL",
        secret,
        now,
        user.id
    )
    .execute(&state.pool)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::Conflict(
            "two-factor authentication is already on".into(),
        ));
    }
    let uri = totp::otpauth_uri(&secret, &user.username);
    let qr_svg =
        totp::qr_svg(&uri).ok_or_else(|| AppError::Internal("could not render QR code".into()))?;
    tracing::info!(target: "auth", event = "totp_setup_started", user_id = user.id, ip = %ip, "2FA enrollment started");
    Ok(Json(TotpSetup {
        secret,
        otpauth_uri: uri,
        qr_svg,
    }))
}

/// `POST /api/me/2fa/enable`: confirms the pending secret with a code, turns
/// 2FA on, signs out every other device and returns fresh recovery codes.
pub async fn totp_enable(
    State(state): State<AppState>,
    ip: ClientIp,
    SessionUser(user): SessionUser,
    JsonBody(req): JsonBody<TotpEnableRequest>,
) -> ApiResult<Json<RecoveryCodes>> {
    let ip = ip.to_string();
    // Wrong codes count like any second factor: otherwise a stolen session
    // could guess the code of an enrollment the owner just started, turn 2FA
    // on and walk away with the recovery codes.
    if state.auth_limiter.is_locked(&user.username, &ip) {
        return Err(AppError::RateLimited);
    }
    let now = now();
    let pending = sqlx::query!(
        "SELECT totp_pending_secret, totp_pending_at FROM users WHERE id = ? AND totp_secret IS NULL",
        user.id
    )
    .fetch_optional(&state.pool)
    .await?;
    let secret = match pending {
        Some(p) => match (p.totp_pending_secret, p.totp_pending_at) {
            (Some(s), Some(at)) if now - at <= totp::PENDING_TTL_SECS => s,
            _ => {
                return Err(AppError::BadRequest(
                    "setup expired, please start again".into(),
                ))
            }
        },
        None => {
            return Err(AppError::Conflict(
                "two-factor authentication is already on".into(),
            ))
        }
    };
    let Some(step) = totp::verify(&secret, &req.code, now, None) else {
        state
            .auth_limiter
            .record_second_factor_failure(&user.username, &ip);
        tracing::info!(target: "auth", event = "totp_setup_bad_code", user_id = user.id, ip = %ip, "wrong code while enabling 2FA");
        return Err(AppError::InvalidCode);
    };

    let codes = totp::new_recovery_codes();
    let mut tx = state.pool.begin().await?;
    let updated = sqlx::query!(
        "UPDATE users SET totp_secret = totp_pending_secret, totp_enabled_at = ?,
                totp_last_step = ?, totp_pending_secret = NULL, totp_pending_at = NULL
         WHERE id = ? AND totp_secret IS NULL AND totp_pending_secret = ?",
        now,
        step,
        user.id,
        secret
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(AppError::Conflict(
            "setup changed meanwhile, please start again".into(),
        ));
    }
    replace_recovery_codes(&mut tx, user.id, &codes).await?;
    let revoked = session::revoke_all(&mut tx, user.id, Some(&user.session_id)).await?;
    sqlx::query!("DELETE FROM login_challenges WHERE user_id = ?", user.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(target: "auth", event = "totp_enabled", user_id = user.id, ip = %ip, revoked_sessions = revoked, "2FA enabled");
    Ok(Json(RecoveryCodes { codes }))
}

/// `POST /api/me/2fa/disable`: needs the password and a current code.
/// Refused while an admin makes 2FA mandatory for the user.
pub async fn totp_disable(
    State(state): State<AppState>,
    ip: ClientIp,
    user: CurrentUser,
    JsonBody(req): JsonBody<TotpConfirmRequest>,
) -> ApiResult<StatusCode> {
    let ip = ip.to_string();
    {
        let mut conn = state.pool.acquire().await?;
        if totp_enforced(&mut conn, user.id).await? {
            return Err(AppError::MfaRequiredByPolicy);
        }
    }
    reauthenticate(&state, &user, &ip, req.password).await?;
    require_second_factor(&state, &user, &ip, &req.code).await?;
    let mut tx = state.pool.begin().await?;
    // Re-checked inside the write transaction, in case the policy just changed.
    if totp_enforced(&mut tx, user.id).await? {
        return Err(AppError::MfaRequiredByPolicy);
    }
    disable_totp(&mut tx, user.id).await?;
    tx.commit().await?;
    tracing::info!(target: "auth", event = "totp_disabled", user_id = user.id, ip = %ip, "2FA disabled by user");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/me/2fa/recovery-codes`: replaces every recovery code.
pub async fn totp_recovery_codes(
    State(state): State<AppState>,
    ip: ClientIp,
    user: CurrentUser,
    JsonBody(req): JsonBody<TotpConfirmRequest>,
) -> ApiResult<Json<RecoveryCodes>> {
    let ip = ip.to_string();
    reauthenticate(&state, &user, &ip, req.password).await?;
    require_second_factor(&state, &user, &ip, &req.code).await?;
    let codes = totp::new_recovery_codes();
    let mut tx = state.pool.begin().await?;
    replace_recovery_codes(&mut tx, user.id, &codes).await?;
    tx.commit().await?;
    tracing::info!(target: "auth", event = "recovery_codes_regenerated", user_id = user.id, ip = %ip, "recovery codes replaced");
    Ok(Json(RecoveryCodes { codes }))
}

async fn replace_recovery_codes(
    conn: &mut sqlx::SqliteConnection,
    user_id: i64,
    codes: &[String],
) -> ApiResult<()> {
    sqlx::query!("DELETE FROM recovery_codes WHERE user_id = ?", user_id)
        .execute(&mut *conn)
        .await?;
    for code in codes {
        let normalized = totp::normalize_recovery_code(code)
            .ok_or_else(|| AppError::Internal("malformed recovery code".into()))?;
        let hash = totp::recovery_hash(&normalized);
        sqlx::query!(
            "INSERT INTO recovery_codes (user_id, code_hash) VALUES (?, ?)",
            user_id,
            hash
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Turns 2FA off and forgets the secret and recovery codes.
pub async fn disable_totp(conn: &mut sqlx::SqliteConnection, user_id: i64) -> ApiResult<()> {
    sqlx::query!(
        "UPDATE users SET totp_secret = NULL, totp_enabled_at = NULL, totp_last_step = NULL,
                totp_pending_secret = NULL, totp_pending_at = NULL
         WHERE id = ?",
        user_id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM recovery_codes WHERE user_id = ?", user_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM login_challenges WHERE user_id = ?", user_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_ascii() {
        assert_eq!(validate_username(" alice.b-1 ").unwrap(), "alice.b-1");
        assert!(validate_username("élise").is_err());
        assert!(validate_username("аdmin").is_err()); // Cyrillic "а"
        assert!(validate_username("a").is_err());
        assert!(validate_username("has space").is_err());
    }

    #[test]
    fn display_names_drop_control_chars() {
        assert_eq!(
            validate_display_name(Some(" Al\u{202e}ice\n"), "x").unwrap(),
            "Alice"
        );
        assert_eq!(
            validate_display_name(Some("  "), "fallback").unwrap(),
            "fallback"
        );
    }
}
