//! Thin typed client for the Muzik HTTP API. Every request is same-origin and
//! carries the session cookie automatically.

use gloo_net::http::{Request, Response};
use muzik_common::{
    AddTracksRequest, AddTracksResult, Album, AlbumDetail, ApiErrorBody, Artist, ArtistDetail,
    ChangePasswordRequest, CreatePlaylistRequest, CreateUserRequest, InkultSettings,
    InkultTestRequest, InkultTestResult, LoginRequest, LoginResponse, MfaLoginRequest,
    MoveEntryRequest, Page, PlaylistDetail, PlaylistSummary, Quality, RecoveryCodes, ScanMode,
    ScanProgress, ScanRequest, SearchResults, SecurityPolicy, SetupRequest, SetupStatus,
    TotpConfirmRequest, TotpEnableRequest, TotpSetup, TotpSetupRequest, TotpStatus, Track,
    UpdateInkultSettings, UpdatePlaylistRequest, UpdateUserRequest, User,
};
use serde::{de::DeserializeOwned, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl ApiError {
    fn network(e: impl std::fmt::Display) -> Self {
        Self {
            status: 0,
            code: "network".into(),
            message: format!("Could not reach the server ({e})"),
        }
    }

    pub fn is_unauthorized(&self) -> bool {
        self.status == 401
    }

    /// The session must set up a mandatory 2FA before doing anything else.
    pub fn needs_totp_setup(&self) -> bool {
        self.code == "mfa_setup_required"
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

async fn check(resp: Response) -> ApiResult<Response> {
    if resp.ok() {
        return Ok(resp);
    }
    let status = resp.status();
    match resp.json::<ApiErrorBody>().await {
        Ok(b) => Err(ApiError {
            status,
            code: b.error,
            message: b.message,
        }),
        Err(_) => Err(ApiError {
            status,
            code: "http".into(),
            message: format!("Request failed ({status})"),
        }),
    }
}

async fn get<T: DeserializeOwned>(url: &str) -> ApiResult<T> {
    let resp = Request::get(url).send().await.map_err(ApiError::network)?;
    check(resp).await?.json().await.map_err(ApiError::network)
}

async fn send<B: Serialize>(method: &str, url: &str, body: &B) -> ApiResult<Response> {
    let builder = match method {
        "PATCH" => Request::patch(url),
        "PUT" => Request::put(url),
        _ => Request::post(url),
    };
    let resp = builder
        .json(body)
        .map_err(ApiError::network)?
        .send()
        .await
        .map_err(ApiError::network)?;
    check(resp).await
}

/// Body-less request (PUT / DELETE / POST) whose response body is ignored.
async fn send_empty(method: &str, url: &str) -> ApiResult<()> {
    let builder = match method {
        "PUT" => Request::put(url),
        "DELETE" => Request::delete(url),
        _ => Request::post(url),
    };
    let resp = builder.send().await.map_err(ApiError::network)?;
    check(resp).await.map(|_| ())
}

async fn send_json<B: Serialize, T: DeserializeOwned>(
    method: &str,
    url: &str,
    body: &B,
) -> ApiResult<T> {
    send(method, url, body)
        .await?
        .json()
        .await
        .map_err(ApiError::network)
}

fn encode(s: &str) -> String {
    String::from(js_sys::encode_uri_component(s))
}

// ---------- Auth ----------

pub async fn setup_status() -> ApiResult<SetupStatus> {
    get("/api/auth/setup").await
}

pub async fn setup(req: &SetupRequest) -> ApiResult<User> {
    send_json("POST", "/api/auth/setup", req).await
}

pub async fn login(req: &LoginRequest) -> ApiResult<LoginResponse> {
    send_json("POST", "/api/auth/login", req).await
}

/// Second sign-in step (authenticator or recovery code).
pub async fn login_2fa(code: String) -> ApiResult<LoginResponse> {
    send_json("POST", "/api/auth/login/2fa", &MfaLoginRequest { code }).await
}

pub async fn logout() -> ApiResult<()> {
    let resp = Request::post("/api/auth/logout")
        .send()
        .await
        .map_err(ApiError::network)?;
    check(resp).await.map(|_| ())
}

pub async fn me() -> ApiResult<User> {
    get("/api/me").await
}

// ---------- Account security ----------

pub async fn change_password(req: &ChangePasswordRequest) -> ApiResult<()> {
    send("POST", "/api/me/password", req).await.map(|_| ())
}

pub async fn totp_status() -> ApiResult<TotpStatus> {
    get("/api/me/2fa").await
}

pub async fn totp_setup(password: String) -> ApiResult<TotpSetup> {
    send_json("POST", "/api/me/2fa/setup", &TotpSetupRequest { password }).await
}

pub async fn totp_enable(code: String) -> ApiResult<RecoveryCodes> {
    send_json("POST", "/api/me/2fa/enable", &TotpEnableRequest { code }).await
}

pub async fn totp_disable(req: &TotpConfirmRequest) -> ApiResult<()> {
    send("POST", "/api/me/2fa/disable", req).await.map(|_| ())
}

pub async fn totp_recovery_codes(req: &TotpConfirmRequest) -> ApiResult<RecoveryCodes> {
    send_json("POST", "/api/me/2fa/recovery-codes", req).await
}

// ---------- Users (admin) ----------

pub async fn users() -> ApiResult<Vec<User>> {
    get("/api/users").await
}

pub async fn create_user(req: &CreateUserRequest) -> ApiResult<User> {
    send_json("POST", "/api/users", req).await
}

pub async fn update_user(id: i64, req: &UpdateUserRequest) -> ApiResult<User> {
    send_json("PATCH", &format!("/api/users/{id}"), req).await
}

pub async fn security_policy() -> ApiResult<SecurityPolicy> {
    get("/api/admin/security").await
}

pub async fn set_security_policy(policy: &SecurityPolicy) -> ApiResult<SecurityPolicy> {
    send_json("PUT", "/api/admin/security", policy).await
}

pub async fn inkult_settings() -> ApiResult<InkultSettings> {
    get("/api/admin/inkult").await
}

pub async fn set_inkult_settings(settings: &UpdateInkultSettings) -> ApiResult<InkultSettings> {
    send_json("PUT", "/api/admin/inkult", settings).await
}

/// Turns separate libraries per Inkult account on or off.
pub async fn set_inkult_libraries(separate: bool) -> ApiResult<InkultSettings> {
    send_json(
        "PUT",
        "/api/admin/inkult/libraries",
        &muzik_common::UpdateInkultLibraries { separate },
    )
    .await
}

/// Checks that `url` reaches an Inkult server; returns its version.
pub async fn test_inkult(url: String) -> ApiResult<InkultTestResult> {
    send_json("POST", "/api/admin/inkult/test", &InkultTestRequest { url }).await
}

// ---------- Library ----------

pub async fn artists(offset: i64, limit: i64) -> ApiResult<Page<Artist>> {
    get(&format!("/api/artists?offset={offset}&limit={limit}")).await
}

pub async fn artist(id: i64) -> ApiResult<ArtistDetail> {
    get(&format!("/api/artists/{id}")).await
}

pub async fn albums(offset: i64, limit: i64, sort: &str) -> ApiResult<Page<Album>> {
    get(&format!(
        "/api/albums?offset={offset}&limit={limit}&sort={sort}"
    ))
    .await
}

pub async fn album(id: i64) -> ApiResult<AlbumDetail> {
    get(&format!("/api/albums/{id}")).await
}

/// Local suggestions to queue after `track_id` (other albums only).
pub async fn suggestions(track_id: i64, limit: i64) -> ApiResult<Vec<Track>> {
    get(&format!("/api/tracks/{track_id}/suggestions?limit={limit}")).await
}

pub async fn search(q: &str) -> ApiResult<SearchResults> {
    get(&format!("/api/search?q={}&limit=12", encode(q))).await
}

// ---------- Playlists ----------

pub async fn playlists() -> ApiResult<Vec<PlaylistSummary>> {
    get("/api/playlists").await
}

pub async fn shared_playlists() -> ApiResult<Vec<PlaylistSummary>> {
    get("/api/playlists/shared").await
}

pub async fn playlist(id: i64) -> ApiResult<PlaylistDetail> {
    get(&format!("/api/playlists/{id}")).await
}

pub async fn create_playlist(req: &CreatePlaylistRequest) -> ApiResult<PlaylistSummary> {
    send_json("POST", "/api/playlists", req).await
}

pub async fn update_playlist(id: i64, req: &UpdatePlaylistRequest) -> ApiResult<PlaylistSummary> {
    send_json("PATCH", &format!("/api/playlists/{id}"), req).await
}

pub async fn delete_playlist(id: i64) -> ApiResult<()> {
    send_empty("DELETE", &format!("/api/playlists/{id}")).await
}

pub async fn add_to_playlist(id: i64, req: &AddTracksRequest) -> ApiResult<AddTracksResult> {
    send_json("POST", &format!("/api/playlists/{id}/tracks"), req).await
}

pub async fn remove_entry(id: i64, entry_id: i64) -> ApiResult<()> {
    send_empty("DELETE", &format!("/api/playlists/{id}/entries/{entry_id}")).await
}

pub async fn move_entry(id: i64, entry_id: i64, position: i64) -> ApiResult<()> {
    send(
        "PATCH",
        &format!("/api/playlists/{id}/entries/{entry_id}"),
        &MoveEntryRequest { position },
    )
    .await
    .map(|_| ())
}

pub async fn follow_playlist(id: i64, follow: bool) -> ApiResult<()> {
    let method = if follow { "PUT" } else { "DELETE" };
    send_empty(method, &format!("/api/playlists/{id}/follow")).await
}

pub async fn duplicate_playlist(id: i64) -> ApiResult<PlaylistSummary> {
    let resp = Request::post(&format!("/api/playlists/{id}/duplicate"))
        .send()
        .await
        .map_err(ApiError::network)?;
    check(resp).await?.json().await.map_err(ApiError::network)
}

// ---------- Scanning (admin) ----------

pub async fn scan_status() -> ApiResult<ScanProgress> {
    get("/api/scan").await
}

pub async fn start_scan(mode: ScanMode) -> ApiResult<()> {
    send("POST", "/api/scan", &ScanRequest { mode })
        .await
        .map(|_| ())
}

// ---------- URLs ----------

pub fn cover_url(album_id: i64, size: u32) -> String {
    format!("/api/albums/{album_id}/cover?size={size}")
}

pub fn stream_url(track_id: i64, quality: Quality) -> String {
    let q = match quality {
        Quality::Original => "original",
        Quality::High => "high",
        Quality::Normal => "normal",
        Quality::Low => "low",
    };
    format!("/api/stream/{track_id}?quality={q}")
}
