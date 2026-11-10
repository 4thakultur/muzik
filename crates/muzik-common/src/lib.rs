//! Types shared between the Muzik server and the web frontend.

use serde::{Deserialize, Serialize};

/// API error body: `{ "error": "code", "message": "..." }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub offset: i64,
    pub limit: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    #[default]
    Original,
    High,
    Normal,
    Low,
}

// ---------- Auth / users ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
    pub disabled: bool,
    pub has_avatar: bool,
    /// Two-factor authentication (authenticator app) is on for this account.
    #[serde(default)]
    pub totp_enabled: bool,
    /// An admin requires 2FA for this account specifically.
    #[serde(default)]
    pub totp_required: bool,
    /// 2FA is mandatory for this account (its own flag or the server-wide
    /// policy). When it is not set up yet, the session can only enroll.
    #[serde(default)]
    pub totp_enforced: bool,
    /// Signs in with an Inkult account: the password and 2FA are managed in
    /// Inkult (`totp_enabled` then tells whether the Inkult account used 2FA
    /// at its last sign-in).
    #[serde(default)]
    pub inkult: bool,
}

impl User {
    /// Signed in, but must set up 2FA before using Muzik. For an Inkult
    /// account this means turning 2FA on in Inkult and signing in again.
    pub fn needs_totp_setup(&self) -> bool {
        self.totp_enforced && !self.totp_enabled
    }
}

/// Inkult connector settings (admin only). While enabled, every account of
/// the Inkult server at `url` is a Muzik user: it signs in with its Inkult
/// handle and password, and its Muzik account is created at the first sign-in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InkultSettings {
    pub enabled: bool,
    /// Base URL of the Inkult server, e.g. `http://inkult:9876`. Empty = not set.
    pub url: String,
    /// `url` is plain `http://` to a host outside the local network:
    /// passwords would travel unencrypted (set by the server).
    #[serde(default)]
    pub insecure: bool,
}

/// `PUT /api/admin/inkult`. Turning sign-in on, or changing the address
/// while it is on, decides where every Inkult user's password is sent, so it
/// needs the admin's password (and 2FA code when on). Admins who sign in
/// with Inkult have no Muzik password: they need a recent sign-in instead.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateInkultSettings {
    pub enabled: bool,
    pub url: String,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
}

/// `POST /api/admin/inkult/test`: checks that `url` answers like Inkult.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InkultTestRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InkultTestResult {
    /// Version reported by the Inkult server.
    pub version: String,
    /// See [`InkultSettings::insecure`].
    #[serde(default)]
    pub insecure: bool,
}

/// Server-wide security policy (admin only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicy {
    /// Every user must use two-factor authentication.
    pub require_totp: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatus {
    /// True when no user exists yet and the first (admin) account must be created.
    pub needs_setup: bool,
    /// Inkult accounts can sign in (the login screen mentions it).
    #[serde(default)]
    pub inkult_sign_in: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupRequest {
    pub username: String,
    pub display_name: Option<String>,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Result of `POST /api/auth/login`. With 2FA on, the password step answers
/// `mfa_required` (and sets a short-lived challenge cookie); the sign-in is
/// then finished with `POST /api/auth/login/2fa`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LoginResponse {
    Ok { user: User },
    MfaRequired,
}

/// Second sign-in step: a 6-digit authenticator code or a recovery code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MfaLoginRequest {
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpStatus {
    pub enabled: bool,
    pub enabled_at: Option<i64>,
    /// Unused recovery codes left.
    pub recovery_codes_left: i64,
}

/// Starts 2FA enrollment (re-authenticates with the password).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpSetupRequest {
    pub password: String,
}

/// A pending secret to add to an authenticator app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpSetup {
    /// Base32 secret, for manual entry.
    pub secret: String,
    pub otpauth_uri: String,
    /// Standalone SVG QR code of `otpauth_uri`.
    pub qr_svg: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpEnableRequest {
    pub code: String,
}

/// Password plus a current authenticator (or recovery) code, required to turn
/// 2FA off or to replace the recovery codes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpConfirmRequest {
    pub password: String,
    pub code: String,
}

/// Shown once; only hashes are stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCodes {
    pub codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub display_name: Option<String>,
    /// Required for a local account; leave empty with `inkult`.
    #[serde(default)]
    pub password: String,
    /// The account signs in with the Inkult account of the same handle.
    #[serde(default)]
    pub inkult: bool,
    #[serde(default)]
    pub is_admin: bool,
    /// Must set up 2FA on first sign-in.
    #[serde(default)]
    pub totp_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserRequest {
    pub display_name: Option<String>,
    pub disabled: Option<bool>,
    pub is_admin: Option<bool>,
    /// Admin password reset (signs the user out everywhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Turns the user's 2FA off, for a lost device (signs them out everywhere).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reset_totp: bool,
    /// Makes 2FA mandatory for this user (on top of the server-wide policy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp_required: Option<bool>,
    /// `Some(true)`: sign in with the Inkult account of the same handle from
    /// now on (forgets the Muzik password and 2FA). `Some(false)`: back to a
    /// Muzik password, which `password` must provide. Both sign the user out
    /// everywhere. A `password` alone on an Inkult account also makes it local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inkult: Option<bool>,
}

// ---------- Library ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub album_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: String,
    pub year: Option<i64>,
    pub track_count: i64,
    pub duration_ms: i64,
    pub has_cover: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist_name: String,
    pub album_id: i64,
    pub album_title: String,
    pub album_artist_id: i64,
    pub disc_no: Option<i64>,
    pub track_no: Option<i64>,
    pub duration_ms: i64,
    pub codec: String,
    pub sample_rate: Option<i64>,
    pub bit_depth: Option<i64>,
    pub channels: Option<i64>,
    pub bitrate_kbps: Option<i64>,
    pub lossless: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlbumDetail {
    pub album: Album,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistDetail {
    pub artist: Artist,
    pub albums: Vec<Album>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SearchResults {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub playlists: Vec<PlaylistSummary>,
}

// ---------- Playlists ----------

/// Limits enforced by the server (and mirrored in the UI).
pub const PLAYLIST_MAX_TRACKS: i64 = 10_000;
pub const PLAYLIST_NAME_MAX: usize = 100;
pub const PLAYLIST_DESCRIPTION_MAX: usize = 300;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub owner_id: i64,
    pub owner_name: String,
    /// Visible to every user of the server.
    pub public: bool,
    /// Other users may edit the songs (only meaningful when `public`).
    pub collaborative: bool,
    pub track_count: i64,
    pub duration_ms: i64,
    /// Up to four distinct albums (with art) from the start of the playlist,
    /// used to draw the cover mosaic.
    pub cover_album_ids: Vec<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    // Relationship with the requesting user.
    pub is_owner: bool,
    pub can_edit: bool,
    /// Saved to the user's library (playlists of other users only).
    pub following: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaylistEntry {
    /// Identifies this occurrence (a track can appear several times).
    pub entry_id: i64,
    pub added_at: i64,
    /// Display name of who added it (shown for collaborative playlists).
    pub added_by: Option<String>,
    // Snapshot taken when the song was added, kept fresh by scans; used to
    // display entries whose file is no longer in the library.
    pub title: String,
    pub artist_name: String,
    pub album_title: String,
    pub duration_ms: i64,
    /// `None` when the file is no longer in the library.
    pub track: Option<Track>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistDetail {
    pub playlist: PlaylistSummary,
    pub entries: Vec<PlaylistEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreatePlaylistRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub collaborative: bool,
    /// Songs to add right away, in order.
    #[serde(default)]
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdatePlaylistRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub public: Option<bool>,
    pub collaborative: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddTracksRequest {
    pub track_ids: Vec<i64>,
    /// Insert before this 0-based position; appended when absent.
    #[serde(default)]
    pub position: Option<i64>,
    /// When false, songs already in the playlist are skipped (and counted in
    /// `duplicates`) so the client can ask "Add anyway?".
    #[serde(default)]
    pub allow_duplicates: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddTracksResult {
    pub added: i64,
    pub duplicates: i64,
    pub playlist: PlaylistSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveEntryRequest {
    /// Final 0-based position of the entry.
    pub position: i64,
}

// ---------- Scanning ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanMode {
    Incremental,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanPhase {
    Idle,
    Walking,
    Reading,
    Finalizing,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub mode: Option<ScanMode>,
    pub files_seen: u64,
    pub files_to_read: u64,
    pub files_read: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub errors: u64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub message: Option<String>,
}

impl Default for ScanProgress {
    fn default() -> Self {
        Self {
            phase: ScanPhase::Idle,
            mode: None,
            files_seen: 0,
            files_to_read: 0,
            files_read: 0,
            added: 0,
            updated: 0,
            removed: 0,
            errors: 0,
            started_at: None,
            finished_at: None,
            message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRequest {
    pub mode: ScanMode,
}
