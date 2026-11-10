use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use muzik_common::ApiErrorBody;

pub type ApiResult<T> = Result<T, AppError>;

/// Errors returned by HTTP handlers. Rendered as `{ "error", "message" }` JSON.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("authentication required")]
    Unauthorized,
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("incorrect password")]
    WrongPassword,
    #[error("invalid verification code")]
    InvalidCode,
    #[error("the sign-in request expired, please sign in again")]
    ChallengeExpired,
    #[error("forbidden")]
    Forbidden,
    #[error("set up two-factor authentication to continue")]
    MfaSetupRequired,
    #[error("two-factor authentication is required for your account by an administrator")]
    MfaRequiredByPolicy,
    #[error("Inkult could not be reached, try again later")]
    InkultUnavailable,
    #[error("finish setting up your Inkult account in Inkult first (new password, two-factor login), then sign in here")]
    InkultAccountSetup,
    #[error(
        "a Muzik account with this name already exists, ask an administrator to link it to Inkult"
    )]
    InkultNameTaken,
    #[error(
        "this server requires two-factor authentication: turn it on in Inkult, then sign in again"
    )]
    InkultMfaRequired,
    #[error("the password and two-factor authentication of this account are managed in Inkult")]
    ManagedByInkult,
    #[error("confirm with your password to change this")]
    ReauthRequired,
    #[error("for your security, sign out and sign in again, then retry within 10 minutes")]
    RecentSignInRequired,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("too many attempts, try again later")]
    RateLimited,
    #[error("range not satisfiable")]
    RangeNotSatisfiable { size: u64 },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::InvalidCredentials => (StatusCode::UNAUTHORIZED, "invalid_credentials"),
            Self::WrongPassword => (StatusCode::FORBIDDEN, "wrong_password"),
            // 403, not 401: a wrong code must not look like an expired session.
            Self::InvalidCode => (StatusCode::FORBIDDEN, "invalid_code"),
            Self::ChallengeExpired => (StatusCode::UNAUTHORIZED, "challenge_expired"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::MfaSetupRequired => (StatusCode::FORBIDDEN, "mfa_setup_required"),
            Self::MfaRequiredByPolicy => (StatusCode::FORBIDDEN, "mfa_required_by_policy"),
            Self::InkultUnavailable => (StatusCode::SERVICE_UNAVAILABLE, "inkult_unavailable"),
            Self::InkultAccountSetup => (StatusCode::FORBIDDEN, "inkult_account_setup"),
            Self::InkultNameTaken => (StatusCode::CONFLICT, "inkult_name_taken"),
            Self::InkultMfaRequired => (StatusCode::FORBIDDEN, "inkult_mfa_required"),
            Self::ManagedByInkult => (StatusCode::CONFLICT, "managed_by_inkult"),
            Self::ReauthRequired => (StatusCode::FORBIDDEN, "reauth_required"),
            Self::RecentSignInRequired => (StatusCode::FORBIDDEN, "recent_sign_in_required"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            Self::RangeNotSatisfiable { .. } => {
                (StatusCode::RANGE_NOT_SATISFIABLE, "range_not_satisfiable")
            }
            Self::Db(sqlx::Error::RowNotFound) => (StatusCode::NOT_FOUND, "not_found"),
            Self::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
                (StatusCode::NOT_FOUND, "not_found")
            }
            Self::Db(_) | Self::Io(_) | Self::Internal(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        // Only internal failures hide their text (it may name files or SQL).
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "request failed");
            "internal server error".to_owned()
        } else {
            self.to_string()
        };
        let mut res = (
            status,
            Json(ApiErrorBody {
                error: code.to_owned(),
                message,
            }),
        )
            .into_response();
        if let Self::RangeNotSatisfiable { size } = self {
            if let Ok(v) = format!("bytes */{size}").parse() {
                res.headers_mut()
                    .insert(axum::http::header::CONTENT_RANGE, v);
            }
        }
        res
    }
}

pub fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(e.to_string())
}
