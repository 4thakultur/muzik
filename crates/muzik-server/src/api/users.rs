use axum::{extract::State, Json};
use muzik_common::{
    CreateUserRequest, InkultSettings, InkultTestRequest, InkultTestResult, SecurityPolicy,
    UpdateInkultSettings, UpdateUserRequest, User,
};

use super::auth::{
    disable_totp, fetch_user, reauthenticate, require_second_factor, validate_display_name,
    validate_username,
};
use super::extract::{Json as JsonBody, Path};
use crate::{
    auth::{client_ip::ClientIp, password, session, AdminUser, CurrentUser},
    db::now,
    error::{ApiResult, AppError},
    inkult,
    state::AppState,
};

pub async fn list(State(state): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Vec<User>>> {
    let users = sqlx::query_as!(
        User,
        r#"SELECT id AS "id!", username, display_name, is_admin AS "is_admin: bool",
                  disabled AS "disabled: bool", avatar_file IS NOT NULL AS "has_avatar!: bool",
                  (CASE WHEN auth_provider = 'inkult' THEN inkult_two_factor <> 0
                        ELSE totp_secret IS NOT NULL END) AS "totp_enabled!: bool",
                  totp_required AS "totp_required: bool",
                  (totp_required <> 0 OR (SELECT require_totp FROM server_settings WHERE id = 1) <> 0)
                    AS "totp_enforced!: bool",
                  auth_provider = 'inkult' AS "inkult!: bool"
           FROM users ORDER BY username"#
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(users))
}

pub async fn create(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    JsonBody(req): JsonBody<CreateUserRequest>,
) -> ApiResult<Json<User>> {
    let username = validate_username(&req.username)?;
    let display = validate_display_name(req.display_name.as_deref(), &username)?;
    // An Inkult account is linked at its first sign-in to the Inkult account
    // with this handle; it has no Muzik password.
    let (hash, provider) = if req.inkult {
        if !req.password.is_empty() {
            return Err(AppError::BadRequest(
                "an Inkult account has no Muzik password".into(),
            ));
        }
        (inkult::NO_LOCAL_PASSWORD.to_owned(), "inkult")
    } else {
        password::validate(&req.password)?;
        (password::hash_async(req.password).await?, "local")
    };
    let now = now();
    let id = sqlx::query_scalar!(
        r#"INSERT INTO users (username, display_name, password_hash, auth_provider, is_admin, totp_required, created_at)
           VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(username) DO NOTHING RETURNING id AS "id!""#,
        username,
        display,
        hash,
        provider,
        req.is_admin,
        req.totp_required,
        now
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Conflict("username already taken".into()))?;
    tracing::info!(target: "auth", event = "user_created", by = admin.id, user_id = id, provider, totp_required = req.totp_required, "user created");
    Ok(Json(fetch_user(&state.pool, id).await?))
}

pub async fn update(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    JsonBody(req): JsonBody<UpdateUserRequest>,
) -> ApiResult<Json<User>> {
    let current = fetch_user(&state.pool, id).await?;
    if id == admin.id && (req.disabled == Some(true) || req.is_admin == Some(false)) {
        return Err(AppError::BadRequest(
            "you cannot disable or demote your own account".into(),
        ));
    }
    // Your own password and 2FA change from Settings > Security, which asks for
    // the current password (and code); the admin shortcut would skip that.
    if id == admin.id && (req.password.is_some() || req.reset_totp || req.inkult.is_some()) {
        return Err(AppError::BadRequest(
            "change your own password and 2FA from your account settings".into(),
        ));
    }
    // Moving between a Muzik password and Inkult sign-in.
    let to_inkult = req.inkult == Some(true) && !current.inkult;
    let to_local = current.inkult && req.inkult != Some(true) && req.password.is_some();
    if req.inkult == Some(true) && req.password.is_some() {
        return Err(AppError::BadRequest(
            "an Inkult account has no Muzik password".into(),
        ));
    }
    if req.inkult == Some(false) && current.inkult && req.password.is_none() {
        return Err(AppError::BadRequest(
            "set a password to turn it into a Muzik account".into(),
        ));
    }
    if req.reset_totp && current.inkult && !to_local {
        return Err(AppError::BadRequest(
            "the two-factor authentication of an Inkult account is reset in Inkult".into(),
        ));
    }
    let new_hash = match req.password {
        Some(p) => {
            password::validate(&p)?;
            Some(password::hash_async(p).await?)
        }
        None => None,
    };
    let display = match req.display_name.as_deref() {
        Some(n) => validate_display_name(Some(n), &current.username)?,
        None => current.display_name,
    };
    let disabled = req.disabled.unwrap_or(current.disabled);
    let is_admin = req.is_admin.unwrap_or(current.is_admin);
    // Requiring 2FA doesn't sign anyone out: their next request is limited to
    // enrollment until it's set up (see `CurrentUser`).
    let totp_required = req.totp_required.unwrap_or(current.totp_required);
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "UPDATE users SET display_name = ?, disabled = ?, is_admin = ?, totp_required = ? WHERE id = ?",
        display,
        disabled,
        is_admin,
        totp_required,
        id
    )
    .execute(&mut *tx)
    .await?;
    if let Some(hash) = &new_hash {
        sqlx::query!("UPDATE users SET password_hash = ? WHERE id = ?", hash, id)
            .execute(&mut *tx)
            .await?;
    }
    if to_local {
        // Forgets the link: the Inkult account no longer reaches this one.
        sqlx::query!(
            "UPDATE users SET auth_provider = 'local', inkult_user_id = NULL, inkult_two_factor = 0
             WHERE id = ?",
            id
        )
        .execute(&mut *tx)
        .await?;
    }
    if to_inkult {
        let marker = inkult::NO_LOCAL_PASSWORD;
        sqlx::query!(
            "UPDATE users SET auth_provider = 'inkult', password_hash = ?, inkult_user_id = NULL,
                    inkult_two_factor = 0
             WHERE id = ?",
            marker,
            id
        )
        .execute(&mut *tx)
        .await?;
        disable_totp(&mut tx, id).await?;
    }
    if req.reset_totp {
        disable_totp(&mut tx, id).await?;
    }
    // Any of these must end every existing session of that user.
    if disabled || new_hash.is_some() || req.reset_totp || to_inkult || to_local {
        session::revoke_all(&mut tx, id, None).await?;
        sqlx::query!("DELETE FROM login_challenges WHERE user_id = ?", id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    tracing::info!(
        target: "auth",
        event = "user_updated",
        by = admin.id,
        user_id = id,
        disabled,
        is_admin,
        password_reset = new_hash.is_some(),
        totp_reset = req.reset_totp,
        totp_required,
        to_inkult,
        to_local,
        "user updated"
    );
    Ok(Json(fetch_user(&state.pool, id).await?))
}

/// `GET /api/admin/security`: the server-wide security policy.
pub async fn security_policy(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> ApiResult<Json<SecurityPolicy>> {
    let require_totp = sqlx::query_scalar!(
        r#"SELECT require_totp AS "require_totp: bool" FROM server_settings WHERE id = 1"#
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(SecurityPolicy { require_totp }))
}

/// `PUT /api/admin/security`: makes 2FA mandatory (or not) for every user.
/// Users without 2FA aren't signed out: their sessions are limited to setting
/// it up. The admin must use 2FA themselves before requiring it, so turning
/// the policy on never locks them into enrollment by surprise.
pub async fn set_security_policy(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    JsonBody(req): JsonBody<SecurityPolicy>,
) -> ApiResult<Json<SecurityPolicy>> {
    if req.require_totp && !fetch_user(&state.pool, admin.id).await?.totp_enabled {
        return Err(AppError::Conflict(
            "turn on two-factor authentication for your own account first".into(),
        ));
    }
    sqlx::query!(
        "UPDATE server_settings SET require_totp = ? WHERE id = 1",
        req.require_totp
    )
    .execute(&state.pool)
    .await?;
    tracing::info!(target: "auth", event = "security_policy_changed", by = admin.id, require_totp = req.require_totp, "server-wide 2FA policy changed");
    Ok(Json(req))
}

/// `GET /api/admin/inkult`: the Inkult connector settings.
pub async fn inkult_settings(
    State(state): State<AppState>,
    _admin: AdminUser,
) -> ApiResult<Json<InkultSettings>> {
    Ok(Json(inkult::settings(&state.pool).await?))
}

/// How recent an Inkult admin's sign-in must be to confirm a sensitive change
/// (they have no Muzik password to type).
const RECENT_SIGN_IN_SECS: i64 = 10 * 60;

/// Proves the admin is at the keyboard, not just holding their session:
/// the password (and a 2FA code when on), or, for an account that signs in
/// with Inkult, a sign-in from the last few minutes.
async fn confirm_admin(
    state: &AppState,
    admin: &CurrentUser,
    ip: &str,
    password: Option<String>,
    code: Option<&str>,
) -> ApiResult<()> {
    if admin.inkult {
        let created = sqlx::query_scalar!(
            "SELECT created_at FROM sessions WHERE id = ?",
            admin.session_id
        )
        .fetch_one(&state.pool)
        .await?;
        if now() - created > RECENT_SIGN_IN_SECS {
            return Err(AppError::RecentSignInRequired);
        }
        return Ok(());
    }
    let Some(password) = password.filter(|p| !p.is_empty()) else {
        return Err(AppError::ReauthRequired);
    };
    reauthenticate(state, admin, ip, password).await?;
    let totp_on = sqlx::query_scalar!(
        r#"SELECT totp_secret IS NOT NULL AS "on!: bool" FROM users WHERE id = ?"#,
        admin.id
    )
    .fetch_one(&state.pool)
    .await?;
    if totp_on {
        require_second_factor(state, admin, ip, code.unwrap_or_default()).await?;
    }
    Ok(())
}

/// `PUT /api/admin/inkult`: turns Inkult sign-in on or off and sets the URL.
/// While on, every Inkult account can sign in (its Muzik account is created
/// at the first sign-in). Turning it off doesn't sign anyone out; Inkult
/// accounts just can't sign in again until it's back on (or an admin gives
/// them a Muzik password).
///
/// Muzik forwards Inkult users' passwords and 2FA codes to this URL, so
/// turning sign-in on or pointing it elsewhere needs [`confirm_admin`]: a
/// stolen admin session alone must not be able to aim every sign-in at a
/// server that collects them. Turning it off, or clearing it, doesn't.
pub async fn set_inkult_settings(
    State(state): State<AppState>,
    ip: ClientIp,
    AdminUser(admin): AdminUser,
    JsonBody(req): JsonBody<UpdateInkultSettings>,
) -> ApiResult<Json<InkultSettings>> {
    let url = match req.url.trim() {
        "" => None,
        raw => Some(inkult::normalize_url(raw).map_err(AppError::BadRequest)?),
    };
    if req.enabled && url.is_none() {
        return Err(AppError::BadRequest(
            "enter the address of your Inkult server first".into(),
        ));
    }
    let current = inkult::settings(&state.pool).await?;
    let sensitive =
        (req.enabled && !current.enabled) || url.as_deref().is_some_and(|u| u != current.url);
    if sensitive {
        let ip = ip.to_string();
        if let Err(e) = confirm_admin(&state, &admin, &ip, req.password, req.code.as_deref()).await
        {
            tracing::warn!(target: "auth", event = "inkult_settings_refused", by = admin.id, ip = %ip, reason = %e, "Inkult settings change not confirmed");
            return Err(e);
        }
    }
    sqlx::query!(
        "UPDATE server_settings SET inkult_enabled = ?, inkult_url = ? WHERE id = 1",
        req.enabled,
        url
    )
    .execute(&state.pool)
    .await?;
    let insecure = url.as_deref().is_some_and(inkult::is_insecure_url);
    tracing::info!(target: "auth", event = "inkult_settings_changed", by = admin.id, enabled = req.enabled, url = ?url, insecure, "Inkult sign-in settings changed");
    Ok(Json(inkult::settings(&state.pool).await?))
}

/// `POST /api/admin/inkult/test`: checks that the URL reaches an Inkult server.
pub async fn test_inkult(
    State(state): State<AppState>,
    _admin: AdminUser,
    JsonBody(req): JsonBody<InkultTestRequest>,
) -> ApiResult<Json<InkultTestResult>> {
    let url = inkult::normalize_url(&req.url).map_err(AppError::BadRequest)?;
    let version = state
        .inkult
        .probe(&url)
        .await
        .map_err(AppError::BadRequest)?;
    Ok(Json(InkultTestResult {
        version,
        insecure: inkult::is_insecure_url(&url),
    }))
}
