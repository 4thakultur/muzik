use std::{net::SocketAddr, path::Path};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use muzik_common::{
    AddTracksResult, Album, AlbumDetail, Page, PlaylistDetail, PlaylistSummary, ScanMode,
    SearchResults, Track, User,
};
use muzik_server::{config::Config, db, state::AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

struct TestApp {
    app: Router,
    state: AppState,
    library: tempfile::TempDir,
    _data: tempfile::TempDir,
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let target = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

async fn setup() -> TestApp {
    let library = tempfile::tempdir().unwrap();
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library"),
        library.path(),
    );
    let data = tempfile::tempdir().unwrap();
    let config = Config {
        library_paths: vec![library.path().to_path_buf()],
        data_dir: data.path().join("data"),
        cache_dir: data.path().join("cache"),
        port: 0,
        scan_interval: None,
        cache_max_size: 1 << 30,
        web_dir: data.path().join("web"),
        secure_cookies: false,
        ffmpeg: "ffmpeg".into(),
        trusted_proxies: Vec::new(),
        max_transcodes: 2,
        inkult_library: Some(data.path().join("inkult/library.json")),
        inkult_music_dir: None,
    };
    std::fs::create_dir_all(&config.data_dir).unwrap();
    let pool = db::connect(&config.data_dir.join("muzik.db"))
        .await
        .unwrap();
    let state = AppState::new(pool, config).unwrap();
    TestApp {
        app: muzik_server::app(state.clone()),
        state,
        library,
        _data: data,
    }
}

impl TestApp {
    async fn request(
        &self,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        body: Option<Value>,
        extra: &[(header::HeaderName, &str)],
    ) -> axum::response::Response {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        for (k, v) in extra {
            req = req.header(k, *v);
        }
        let body = match body {
            Some(b) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let mut req = req.body(body).unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
        self.app.clone().oneshot(req).await.unwrap()
    }

    async fn json<T: serde::de::DeserializeOwned>(&self, uri: &str, cookie: &str) -> T {
        let res = self.request("GET", uri, Some(cookie), None, &[]).await;
        assert_eq!(res.status(), StatusCode::OK, "GET {uri}");
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap()
    }
}

fn session_cookie(res: &axum::response::Response) -> String {
    let set = res.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set.contains("HttpOnly"));
    assert!(set.contains("SameSite=Lax"));
    set.split(';').next().unwrap().to_owned()
}

#[tokio::test]
async fn full_flow() {
    let t = setup().await;

    // Health is public; library is not.
    assert_eq!(
        t.request("GET", "/api/health", None, None, &[])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        t.request("GET", "/api/albums", None, None, &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // First-run setup creates an admin; a second setup is refused.
    let res = t
        .request(
            "POST",
            "/api/auth/setup",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let admin_cookie = session_cookie(&res);
    let user: User =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(user.is_admin);
    let res = t
        .request(
            "POST",
            "/api/auth/setup",
            None,
            Some(json!({"username": "evil", "password": "supersecret"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Login: wrong password rejected, right one accepted (case-insensitive username).
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "nope-nope"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "ADMIN", "password": "supersecret"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Admin creates a regular user, who cannot trigger scans.
    let res = t
        .request(
            "POST",
            "/api/users",
            Some(&admin_cookie),
            Some(json!({"username": "bob", "password": "bobspassword"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "bobspassword"})),
            &[],
        )
        .await;
    let bob_cookie = session_cookie(&res);
    let res = t
        .request(
            "POST",
            "/api/scan",
            Some(&bob_cookie),
            Some(json!({"mode": "full"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // Scan.
    t.state.scanner.run_blocking(ScanMode::Incremental).await;
    let progress = t.state.scanner.current();
    assert_eq!(progress.added, 3, "{progress:?}");
    assert_eq!(progress.errors, 0);

    let albums: Page<Album> = t.json("/api/albums?limit=1&sort=year", &bob_cookie).await;
    assert_eq!((albums.total, albums.items.len()), (2, 1));
    let res = t
        .request("GET", "/api/albums?limit=abc", Some(&bob_cookie), None, &[])
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let err: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(err["error"], "bad_request");
    let albums: Page<Album> = t.json("/api/albums", &bob_cookie).await;
    assert_eq!(albums.total, 2);
    let first = albums
        .items
        .iter()
        .find(|a| a.title == "First Album")
        .unwrap();
    assert_eq!(first.artist_name, "Test Artist");
    assert_eq!(first.year, Some(2021));
    assert_eq!(first.track_count, 2);
    // Untagged files fall back to the folder structure, and pick up cover.jpg.
    let loose = albums
        .items
        .iter()
        .find(|a| a.title == "Loose Album")
        .unwrap();
    assert_eq!(loose.artist_name, "Untagged Band");
    assert!(loose.has_cover);

    let detail: AlbumDetail = t
        .json(&format!("/api/albums/{}", first.id), &bob_cookie)
        .await;
    let titles: Vec<_> = detail.tracks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Opening", "Second Song"]);
    let flac = &detail.tracks[0];
    assert!(flac.lossless);
    assert_eq!(flac.codec, "flac");
    assert!(
        (900..=1100).contains(&flac.duration_ms),
        "{}",
        flac.duration_ms
    );
    assert_eq!(detail.tracks[1].artist_name, "Test Artist feat. Guest");

    // Search (prefix, diacritics/case-insensitive).
    let found: SearchResults = t.json("/api/search?q=seco", &bob_cookie).await;
    assert_eq!(found.tracks.len(), 1);
    let found: SearchResults = t.json("/api/search?q=test%20art", &bob_cookie).await;
    assert_eq!(found.artists.len(), 1);

    // Cover thumbnail.
    let res = t
        .request(
            "GET",
            &format!("/api/albums/{}/cover?size=96", loose.id),
            Some(&bob_cookie),
            None,
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "image/webp");

    // Streaming the original with a Range request is byte-exact.
    let original = std::fs::read(
        t.library
            .path()
            .join("Test Artist/First Album/01 - Opening.flac"),
    )
    .unwrap();
    let res = t
        .request(
            "GET",
            &format!("/api/stream/{}", flac.id),
            Some(&bob_cookie),
            None,
            &[(header::RANGE, "bytes=10-1033")],
        )
        .await;
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
    assert!(res.headers().get(header::CONTENT_ENCODING).is_none());
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], &original[10..1034]);

    let res = t
        .request("GET", "/api/stream/999999", Some(&bob_cookie), None, &[])
        .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // Incremental rescan: unchanged files are not re-read; deleted ones are removed.
    std::fs::remove_file(
        t.library
            .path()
            .join("Untagged Band/Loose Album/03 Untagged.wav"),
    )
    .unwrap();
    t.state.scanner.run_blocking(ScanMode::Incremental).await;
    let progress = t.state.scanner.current();
    assert_eq!(
        (progress.files_to_read, progress.removed),
        (0, 1),
        "{progress:?}"
    );
    let albums: Page<Album> = t.json("/api/albums", &bob_cookie).await;
    assert_eq!(albums.total, 1);

    // Logout invalidates the session server-side.
    let res = t
        .request("POST", "/api/auth/logout", Some(&bob_cookie), None, &[])
        .await;
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = t
        .request("GET", "/api/me", Some(&bob_cookie), None, &[])
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn transcodes_wav_to_flac_for_high_quality() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("ffmpeg not installed, skipping");
        return;
    }
    let t = setup().await;
    let res = t
        .request(
            "POST",
            "/api/auth/setup",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[],
        )
        .await;
    let cookie = session_cookie(&res);
    t.state.scanner.run_blocking(ScanMode::Incremental).await;

    let found: SearchResults = t.json("/api/search?q=untagged", &cookie).await;
    let wav = found.tracks.first().expect("wav track indexed");
    assert_eq!(wav.codec, "pcm");

    let res = t
        .request(
            "GET",
            &format!("/api/stream/{}?quality=high", wav.id),
            Some(&cookie),
            None,
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CONTENT_TYPE], "audio/flac");
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..4], b"fLaC");
}

#[tokio::test]
async fn login_is_rate_limited() {
    let t = setup().await;
    t.request(
        "POST",
        "/api/auth/setup",
        None,
        Some(json!({"username": "admin", "password": "supersecret"})),
        &[],
    )
    .await;
    for _ in 0..10 {
        let res = t
            .request(
                "POST",
                "/api/auth/login",
                None,
                Some(json!({"username": "admin", "password": "wrong-pass"})),
                &[],
            )
            .await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
    // Even the right password is refused while locked out.
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
}

impl TestApp {
    /// Sends a JSON request and returns (status, parsed body or Null).
    async fn call(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let res = self.request(method, uri, Some(cookie), body, &[]).await;
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, v)
    }
}

fn titles(d: &PlaylistDetail) -> Vec<String> {
    d.entries.iter().map(|e| e.title.clone()).collect()
}

#[tokio::test]
async fn playlists() {
    let t = setup().await;
    let res = t
        .request(
            "POST",
            "/api/auth/setup",
            None,
            Some(json!({"username": "alice", "password": "supersecret"})),
            &[],
        )
        .await;
    let alice = session_cookie(&res);
    t.request(
        "POST",
        "/api/users",
        Some(&alice),
        Some(json!({"username": "bob", "password": "bobspassword"})),
        &[],
    )
    .await;
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "bobspassword"})),
            &[],
        )
        .await;
    let bob = session_cookie(&res);
    t.state.scanner.run_blocking(ScanMode::Incremental).await;

    let found: SearchResults = t.json("/api/search?q=opening", &alice).await;
    let opening = found.tracks[0].id;
    let found: SearchResults = t.json("/api/search?q=second", &alice).await;
    let second = found.tracks[0].id;
    let found: SearchResults = t.json("/api/search?q=untagged", &alice).await;
    let wav = found.tracks[0].id;

    // Create with songs; names are validated.
    let (s, _) = t
        .call(
            "POST",
            "/api/playlists",
            &alice,
            Some(json!({"name": "  "})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = t
        .call(
            "POST",
            "/api/playlists",
            &alice,
            Some(json!({"name": " Road Trip ", "track_ids": [opening, second]})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let p: PlaylistSummary = serde_json::from_value(v).unwrap();
    assert_eq!((p.name.as_str(), p.track_count), ("Road Trip", 2));
    assert!(p.is_owner && p.can_edit && !p.public);
    assert_eq!(p.cover_album_ids.len(), 0, "fixture FLAC album has no art");
    let url = format!("/api/playlists/{}", p.id);

    // Duplicates are skipped unless explicitly allowed; insertion position is honored.
    let (s, v) = t
        .call(
            "POST",
            &format!("{url}/tracks"),
            &alice,
            Some(json!({"track_ids": [second, wav], "position": 0})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let r: AddTracksResult = serde_json::from_value(v).unwrap();
    assert_eq!((r.added, r.duplicates, r.playlist.track_count), (1, 1, 3));
    assert_eq!(
        r.playlist.cover_album_ids.len(),
        1,
        "loose album has cover.jpg"
    );
    let (_, v) = t
        .call(
            "POST",
            &format!("{url}/tracks"),
            &alice,
            Some(json!({"track_ids": [opening], "allow_duplicates": true})),
        )
        .await;
    let r: AddTracksResult = serde_json::from_value(v).unwrap();
    assert_eq!((r.added, r.duplicates), (1, 0));
    let (s, _) = t
        .call(
            "POST",
            &format!("{url}/tracks"),
            &alice,
            Some(json!({"track_ids": [999999]})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let d: PlaylistDetail = t.json(&url, &alice).await;
    assert_eq!(
        titles(&d),
        ["03 Untagged", "Opening", "Second Song", "Opening"]
    );

    // Reorder: move the first entry to the end, then the last back to index 1.
    let first = d.entries[0].entry_id;
    let (s, _) = t
        .call(
            "PATCH",
            &format!("{url}/entries/{first}"),
            &alice,
            Some(json!({"position": 99})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let d: PlaylistDetail = t.json(&url, &alice).await;
    assert_eq!(
        titles(&d),
        ["Opening", "Second Song", "Opening", "03 Untagged"]
    );
    let last = d.entries[3].entry_id;
    t.call(
        "PATCH",
        &format!("{url}/entries/{last}"),
        &alice,
        Some(json!({"position": 1})),
    )
    .await;
    let d: PlaylistDetail = t.json(&url, &alice).await;
    assert_eq!(
        titles(&d),
        ["Opening", "03 Untagged", "Second Song", "Opening"]
    );

    // Remove the duplicate.
    let dup = d.entries[3].entry_id;
    let (s, _) = t
        .call("DELETE", &format!("{url}/entries/{dup}"), &alice, None)
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let d: PlaylistDetail = t.json(&url, &alice).await;
    assert_eq!(titles(&d), ["Opening", "03 Untagged", "Second Song"]);

    // Private: invisible to bob (404, not 403), absent from his search.
    let (s, _) = t.call("GET", &url, &bob, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let found: SearchResults = t.json("/api/search?q=road", &bob).await;
    assert!(found.playlists.is_empty());
    let found: SearchResults = t.json("/api/search?q=road", &alice).await;
    assert_eq!(found.playlists.len(), 1);

    // Collaborative requires shared.
    let (_, v) = t
        .call("PATCH", &url, &alice, Some(json!({"collaborative": true})))
        .await;
    assert_eq!(v["collaborative"], false);

    // Shared: bob can see, save and copy it, but not edit or rename it.
    let (_, v) = t
        .call("PATCH", &url, &alice, Some(json!({"public": true})))
        .await;
    assert_eq!(v["public"], true);
    let shared: Vec<PlaylistSummary> = t.json("/api/playlists/shared", &bob).await;
    assert_eq!(shared.len(), 1);
    assert!(!shared[0].can_edit && !shared[0].is_owner);
    let (s, _) = t
        .call(
            "POST",
            &format!("{url}/tracks"),
            &bob,
            Some(json!({"track_ids": [wav]})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = t
        .call("PATCH", &url, &bob, Some(json!({"name": "Mine"})))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = t.call("PUT", &format!("{url}/follow"), &bob, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["following"], true);
    let lib: Vec<PlaylistSummary> = t.json("/api/playlists", &bob).await;
    assert_eq!(lib.len(), 1);
    let (s, _) = t.call("PUT", &format!("{url}/follow"), &alice, None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Collaborative: bob can now add songs.
    t.call("PATCH", &url, &alice, Some(json!({"collaborative": true})))
        .await;
    let (s, _) = t
        .call(
            "POST",
            &format!("{url}/tracks"),
            &bob,
            Some(json!({"track_ids": [wav], "allow_duplicates": true})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let d: PlaylistDetail = t.json(&url, &bob).await;
    assert_eq!(d.entries[3].added_by.as_deref(), Some("bob"));

    // Bob's private copy.
    let (s, v) = t
        .call("POST", &format!("{url}/duplicate"), &bob, None)
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let copy: PlaylistSummary = serde_json::from_value(v).unwrap();
    assert!(copy.is_owner && !copy.public);
    assert_eq!((copy.name.as_str(), copy.track_count), ("Road Trip", 4));

    // Unshared again: bob loses access and it leaves his library.
    t.call("PATCH", &url, &alice, Some(json!({"public": false})))
        .await;
    let (s, _) = t.call("GET", &url, &bob, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let lib: Vec<PlaylistSummary> = t.json("/api/playlists", &bob).await;
    assert_eq!(lib.iter().map(|p| p.id).collect::<Vec<_>>(), [copy.id]);

    // Moving a file: the entry survives the rescan, relinked to the new track.
    let lib = t.library.path().join("Test Artist/First Album");
    std::fs::create_dir(lib.join("moved")).unwrap();
    std::fs::rename(
        lib.join("01 - Opening.flac"),
        lib.join("moved/01 - Opening.flac"),
    )
    .unwrap();
    t.state.scanner.run_blocking(ScanMode::Incremental).await;
    let d: PlaylistDetail = t.json(&url, &alice).await;
    let entry = d.entries.iter().find(|e| e.title == "Opening").unwrap();
    let new_id = entry.track.as_ref().expect("relinked").id;
    assert_ne!(new_id, opening);

    // Deleting a file: the entry stays, marked unavailable.
    std::fs::remove_file(lib.join("moved/01 - Opening.flac")).unwrap();
    t.state.scanner.run_blocking(ScanMode::Incremental).await;
    let d: PlaylistDetail = t.json(&url, &alice).await;
    let entry = d.entries.iter().find(|e| e.title == "Opening").unwrap();
    assert!(entry.track.is_none());
    assert_eq!(entry.album_title, "First Album");

    // Only the owner deletes.
    let (s, _) = t.call("DELETE", &url, &bob, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = t.call("DELETE", &url, &alice, None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = t.call("GET", &url, &alice, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn suggestions_exclude_the_seed_album() {
    let t = setup().await;
    let res = t
        .request(
            "POST",
            "/api/auth/setup",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[],
        )
        .await;
    let cookie = session_cookie(&res);
    t.state.scanner.run_blocking(ScanMode::Incremental).await;

    let found: SearchResults = t.json("/api/search?q=opening", &cookie).await;
    let seed = &found.tracks[0];
    let tracks: Vec<Track> = t
        .json(&format!("/api/tracks/{}/suggestions", seed.id), &cookie)
        .await;
    assert!(!tracks.is_empty(), "other albums are suggested");
    assert!(tracks.iter().all(|s| s.album_id != seed.album_id));

    let tracks: Vec<Track> = t
        .json(
            &format!("/api/tracks/{}/suggestions?limit=0", seed.id),
            &cookie,
        )
        .await;
    assert_eq!(tracks.len(), 1, "limit is clamped to at least 1");

    let (s, _) = t
        .call("GET", "/api/tracks/999999/suggestions", &cookie, None)
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let res = t
        .request(
            "GET",
            &format!("/api/tracks/{}/suggestions", seed.id),
            None,
            None,
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// ---------- Security ----------

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// `name=value` of the cookie called `name` among the response's Set-Cookie headers.
fn cookie_named(res: &axum::response::Response, name: &str) -> Option<String> {
    res.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap().to_owned())
        .find(|c| c.starts_with(&format!("{name}=")) && c.len() > name.len() + 1)
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}

async fn admin_session(t: &TestApp) -> String {
    let res = t
        .request(
            "POST",
            "/api/auth/setup",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    session_cookie(&res)
}

#[tokio::test]
async fn two_factor_sign_in() {
    use muzik_server::auth::totp;
    let t = setup().await;
    let admin = admin_session(&t).await;
    // A second device signed in before 2FA is turned on.
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[],
        )
        .await;
    let other_device = session_cookie(&res);
    assert_eq!(body_json(res).await["status"], "ok");

    // Enrollment needs the password.
    let (s, _) = t
        .call(
            "POST",
            "/api/me/2fa/setup",
            &admin,
            Some(json!({"password": "nope-nope"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, setup) = t
        .call(
            "POST",
            "/api/me/2fa/setup",
            &admin,
            Some(json!({"password": "supersecret"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_owned();
    assert!(setup["otpauth_uri"].as_str().unwrap().contains(&secret));
    assert!(setup["qr_svg"].as_str().unwrap().contains("<svg"));

    // Wrong code doesn't enable it; the right one does and returns recovery codes.
    let (s, _) = t
        .call(
            "POST",
            "/api/me/2fa/enable",
            &admin,
            Some(json!({"code": "000000"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let now = now_secs();
    let code = totp::generate(&secret, now).unwrap();
    let (s, rc) = t
        .call(
            "POST",
            "/api/me/2fa/enable",
            &admin,
            Some(json!({"code": code})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let recovery: Vec<String> = serde_json::from_value(rc["codes"].clone()).unwrap();
    assert_eq!(recovery.len(), 10);
    let me: User = t.json("/api/me", &admin).await;
    assert!(me.totp_enabled);
    // Other sessions were signed out; this one stays.
    let res = t
        .request("GET", "/api/me", Some(&other_device), None, &[])
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    // Setup can't be restarted while on.
    let (s, _) = t
        .call(
            "POST",
            "/api/me/2fa/setup",
            &admin,
            Some(json!({"password": "supersecret"})),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Password alone no longer signs in: no session cookie, a challenge instead.
    let login = || async {
        let res = t
            .request(
                "POST",
                "/api/auth/login",
                None,
                Some(json!({"username": "ADMIN", "password": "supersecret"})),
                &[],
            )
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert!(cookie_named(&res, "muzik_session").is_none());
        let challenge = cookie_named(&res, "muzik_mfa").expect("challenge cookie");
        assert_eq!(body_json(res).await["status"], "mfa_required");
        challenge
    };
    let challenge = login().await;
    let second = |cookie: Option<String>, code: String| {
        let t = &t;
        async move {
            t.request(
                "POST",
                "/api/auth/login/2fa",
                cookie.as_deref(),
                Some(json!({ "code": code })),
                &[],
            )
            .await
        }
    };
    // No challenge cookie → expired.
    let res = second(None, "123456".into()).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    // The code used to enable is spent (replay protection).
    let res = second(Some(challenge.clone()), code.clone()).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // Next step's code works and issues a session.
    let next = totp::generate(&secret, now + 30).unwrap();
    let res = second(Some(challenge.clone()), next.clone()).await;
    assert_eq!(res.status(), StatusCode::OK);
    let session = cookie_named(&res, "muzik_session").expect("session cookie");
    assert_eq!(body_json(res).await["user"]["username"], "admin");
    let me: User = t.json("/api/me", &session).await;
    assert_eq!(me.username, "admin");
    // The challenge is single-use.
    let res = second(Some(challenge), next.clone()).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Same code on a new challenge: refused (already used).
    let challenge = login().await;
    let res = second(Some(challenge.clone()), next).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // A recovery code works once, in any format.
    let rcode = recovery[0].to_lowercase().replace('-', " ");
    let res = second(Some(challenge.clone()), rcode.clone()).await;
    assert_eq!(res.status(), StatusCode::OK);
    let challenge = login().await;
    let res = second(Some(challenge.clone()), rcode).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let (s, st) = t.call("GET", "/api/me/2fa", &session, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st["recovery_codes_left"], 9);

    // A challenge dies after 5 wrong codes (one was spent above), even
    // before the rate limit kicks in.
    for _ in 0..4 {
        let res = second(Some(challenge.clone()), "000000".into()).await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
    let res = second(Some(challenge), recovery[1].clone()).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Disabling needs password + a code.
    let (s, _) = t
        .call(
            "POST",
            "/api/me/2fa/disable",
            &session,
            Some(json!({"password": "supersecret", "code": "000000"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = t
        .call(
            "POST",
            "/api/me/2fa/disable",
            &session,
            Some(json!({"password": "supersecret", "code": recovery[2]})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[],
        )
        .await;
    assert_eq!(body_json(res).await["status"], "ok");
}

#[tokio::test]
async fn admin_can_reset_password_and_2fa() {
    use muzik_server::auth::totp;
    let t = setup().await;
    let admin = admin_session(&t).await;
    let (s, bob) = t
        .call(
            "POST",
            "/api/users",
            &admin,
            Some(json!({"username": "bob", "password": "bobspassword"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let bob_id = bob["id"].as_i64().unwrap();
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "bobspassword"})),
            &[],
        )
        .await;
    let bob_session = session_cookie(&res);
    let (_, setup) = t
        .call(
            "POST",
            "/api/me/2fa/setup",
            &bob_session,
            Some(json!({"password": "bobspassword"})),
        )
        .await;
    let code = totp::generate(setup["secret"].as_str().unwrap(), now_secs()).unwrap();
    let (s, _) = t
        .call(
            "POST",
            "/api/me/2fa/enable",
            &bob_session,
            Some(json!({"code": code})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // Users change their own password with the current one; other devices are signed out.
    let (s, _) = t
        .call(
            "POST",
            "/api/me/password",
            &bob_session,
            Some(json!({"current_password": "wrong-one", "new_password": "newpassword1"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = t
        .call(
            "POST",
            "/api/me/password",
            &bob_session,
            Some(json!({"current_password": "bobspassword", "new_password": "newpassword1"})),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = t.call("GET", "/api/me", &bob_session, None).await;
    assert_eq!(s, StatusCode::OK);

    // Admins can't use the shortcut on themselves.
    let me: User = t.json("/api/me", &admin).await;
    let (s, _) = t
        .call(
            "PATCH",
            &format!("/api/users/{}", me.id),
            &admin,
            Some(json!({"password": "whatever123"})),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Lost device: admin resets bob's 2FA and password; bob is signed out.
    let (s, u) = t
        .call(
            "PATCH",
            &format!("/api/users/{bob_id}"),
            &admin,
            Some(json!({"reset_totp": true, "password": "temporary-pass"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(u["totp_enabled"], false);
    let (s, _) = t.call("GET", "/api/me", &bob_session, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "bob", "password": "temporary-pass"})),
            &[],
        )
        .await;
    assert_eq!(body_json(res).await["status"], "ok");
}

#[tokio::test]
async fn cross_site_writes_are_blocked() {
    let t = setup().await;
    let admin = admin_session(&t).await;
    let body = || Some(json!({"name": "Mix"}));
    let res = t
        .request(
            "POST",
            "/api/playlists",
            Some(&admin),
            body(),
            &[(
                header::HeaderName::from_static("sec-fetch-site"),
                "cross-site",
            )],
        )
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = t
        .request(
            "POST",
            "/api/playlists",
            Some(&admin),
            body(),
            &[
                (header::ORIGIN, "https://evil.example"),
                (header::HOST, "music.example"),
            ],
        )
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = t
        .request(
            "POST",
            "/api/playlists",
            Some(&admin),
            body(),
            &[(
                header::HeaderName::from_static("sec-fetch-site"),
                "same-origin",
            )],
        )
        .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    // Reads stay possible (e.g. <img> covers) and API responses aren't cacheable.
    let res = t
        .request(
            "GET",
            "/api/me",
            Some(&admin),
            None,
            &[(
                header::HeaderName::from_static("sec-fetch-site"),
                "cross-site",
            )],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
}

#[tokio::test]
async fn forwarded_for_is_ignored_without_trusted_proxy() {
    let t = setup().await;
    admin_session(&t).await;
    // Rotating a spoofed X-Forwarded-For must not dodge the lockout.
    for i in 0..10 {
        let ip = format!("198.51.100.{i}");
        let res = t
            .request(
                "POST",
                "/api/auth/login",
                None,
                Some(json!({"username": "admin", "password": "wrong-pass"})),
                &[(header::HeaderName::from_static("x-forwarded-for"), &ip)],
            )
            .await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": "admin", "password": "supersecret"})),
            &[(
                header::HeaderName::from_static("x-forwarded-for"),
                "203.0.113.50",
            )],
        )
        .await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
}

/// Signs `username` in (password only) and returns the session cookie.
async fn login_session(t: &TestApp, username: &str, password: &str) -> String {
    let res = t
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"username": username, "password": password})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    session_cookie(&res)
}

/// Sets up 2FA for the signed-in user and returns the secret.
async fn enroll_totp(t: &TestApp, session: &str, password: &str) -> String {
    use muzik_server::auth::totp;
    let (s, setup) = t
        .call(
            "POST",
            "/api/me/2fa/setup",
            session,
            Some(json!({"password": password})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_owned();
    let code = totp::generate(&secret, now_secs()).unwrap();
    let (s, _) = t
        .call(
            "POST",
            "/api/me/2fa/enable",
            session,
            Some(json!({"code": code})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    secret
}

#[tokio::test]
async fn two_factor_codes_cannot_be_brute_forced() {
    use muzik_server::auth::totp;
    let t = setup().await;
    let admin = admin_session(&t).await;

    // Guessing the code of a pending enrollment (e.g. from a stolen
    // session) is rate limited: even the right code is refused afterwards.
    let (s, setup) = t
        .call(
            "POST",
            "/api/me/2fa/setup",
            &admin,
            Some(json!({"password": "supersecret"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let secret = setup["secret"].as_str().unwrap().to_owned();
    let right = totp::generate(&secret, now_secs()).unwrap();
    let wrong = format!(
        "{:06}",
        (right.parse::<u32>().unwrap() + 500_000) % 1_000_000
    );
    for _ in 0..10 {
        let (s, _) = t
            .call(
                "POST",
                "/api/me/2fa/enable",
                &admin,
                Some(json!({"code": wrong})),
            )
            .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }
    let (s, e) = t
        .call(
            "POST",
            "/api/me/2fa/enable",
            &admin,
            Some(json!({"code": right})),
        )
        .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{e}");
    let me: User = t.json("/api/me", &admin).await;
    assert!(!me.totp_enabled);
}

#[tokio::test]
async fn sign_in_challenges_are_capped_per_account() {
    let t = setup().await;
    let admin = admin_session(&t).await;
    enroll_totp(&t, &admin, "supersecret").await;
    let mut cookies = Vec::new();
    for _ in 0..5 {
        let res = t
            .request(
                "POST",
                "/api/auth/login",
                None,
                Some(json!({"username": "admin", "password": "supersecret"})),
                &[],
            )
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        cookies.push(cookie_named(&res, "muzik_mfa").unwrap());
    }
    let left = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM login_challenges")
        .fetch_one(&t.state.pool)
        .await
        .unwrap();
    assert_eq!(left, 3);
    // The oldest challenge is gone; the newest still takes a code.
    for (cookie, alive) in [(&cookies[0], false), (&cookies[4], true)] {
        let res = t
            .request(
                "POST",
                "/api/auth/login/2fa",
                Some(cookie),
                Some(json!({"code": "000000"})),
                &[],
            )
            .await;
        let err = body_json(res).await["error"].clone();
        assert_eq!(err != "challenge_expired", alive, "{err}");
    }
}

#[tokio::test]
async fn admin_can_require_two_factor() {
    let t = setup().await;
    let admin = admin_session(&t).await;

    // Per user: bob must use 2FA, carol doesn't have to.
    let (s, bob) = t
        .call(
            "POST",
            "/api/users",
            &admin,
            Some(json!({"username": "bob", "password": "bobspassword", "totp_required": true})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(bob["totp_required"], true);
    assert_eq!(bob["totp_enforced"], true);
    let bob_id = bob["id"].as_i64().unwrap();
    let (s, _) = t
        .call(
            "POST",
            "/api/users",
            &admin,
            Some(json!({"username": "carol", "password": "carolspassword"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // Bob's password sign-in gives a session limited to enrollment.
    let bob_s = login_session(&t, "bob", "bobspassword").await;
    let me: User = t.json("/api/me", &bob_s).await;
    assert!(me.needs_totp_setup());
    let (s, e) = t.call("GET", "/api/albums", &bob_s, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["error"], "mfa_setup_required");
    let (s, _) = t.call("GET", "/api/me/2fa", &bob_s, None).await;
    assert_eq!(s, StatusCode::OK);

    // Once enrolled, everything works, but 2FA can't be turned off.
    enroll_totp(&t, &bob_s, "bobspassword").await;
    let (s, _) = t.call("GET", "/api/albums", &bob_s, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, e) = t
        .call(
            "POST",
            "/api/me/2fa/disable",
            &bob_s,
            Some(json!({"password": "bobspassword", "code": "000000"})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["error"], "mfa_required_by_policy");

    // Server-wide: only admins, and only once they use 2FA themselves.
    let carol_s = login_session(&t, "carol", "carolspassword").await;
    let (s, _) = t
        .call(
            "PUT",
            "/api/admin/security",
            &carol_s,
            Some(json!({"require_totp": true})),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = t
        .call(
            "PUT",
            "/api/admin/security",
            &admin,
            Some(json!({"require_totp": true})),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    enroll_totp(&t, &admin, "supersecret").await;
    let (s, p) = t
        .call(
            "PUT",
            "/api/admin/security",
            &admin,
            Some(json!({"require_totp": true})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["require_totp"], true);
    let (_, p) = t.call("GET", "/api/admin/security", &admin, None).await;
    assert_eq!(p["require_totp"], true);

    // Carol's existing session is now limited to enrollment, without sign-out.
    let (s, e) = t.call("GET", "/api/albums", &carol_s, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["error"], "mfa_setup_required");
    let me: User = t.json("/api/me", &carol_s).await;
    assert!(me.needs_totp_setup() && !me.totp_required);

    // Lifting bob's own flag doesn't help while the server requires 2FA.
    let (s, u) = t
        .call(
            "PATCH",
            &format!("/api/users/{bob_id}"),
            &admin,
            Some(json!({"totp_required": false})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(u["totp_required"], false);
    assert_eq!(u["totp_enforced"], true);

    // Policy off: carol is free again and bob may turn 2FA off.
    let (s, _) = t
        .call(
            "PUT",
            "/api/admin/security",
            &admin,
            Some(json!({"require_totp": false})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = t.call("GET", "/api/albums", &carol_s, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, users) = t.call("GET", "/api/users", &admin, None).await;
    assert_eq!(s, StatusCode::OK);
    let bob = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == bob_id)
        .unwrap();
    assert_eq!(bob["totp_enforced"], false);
    assert_eq!(bob["totp_enabled"], true);
}

// ---------- Inkult connector ----------

/// A stand-in for an Inkult server's sign-in API, on a random local port.
/// Accounts (handle → id, password, 2FA code): `pirate` (42, no 2FA),
/// `captain` (7, code 123456), `temp` (9, temporary password), `admin` (99),
/// `bosun` (43, no 2FA), `quartermaster` (8, no 2FA). 7 and 8 are Inkult
/// captains (administrators), the others pirates.
async fn fake_inkult() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use axum::{routing::post, Json};
    use std::sync::{atomic::AtomicUsize, Arc};
    let logouts = Arc::new(AtomicUsize::new(0));
    let account = |id: i64, name: &str, two_factor: bool, change: bool| {
        json!({
            "id": id, "username": name, "created_at": 0,
            "role": if matches!(id, 7 | 8) { "captain" } else { "pirate" },
            "two_factor_enabled": two_factor, "password_temporary": change,
            "password_change_required": change, "two_factor_enrollment_required": false,
        })
    };
    let signed_in = |body: Value| {
        (
            [(
                header::SET_COOKIE,
                "inkult_session=abcdef0123; Path=/; HttpOnly",
            )],
            Json(body),
        )
            .into_response()
    };
    use axum::response::IntoResponse;
    let counter = logouts.clone();
    let app = Router::new()
        .route(
            "/api/auth/state",
            axum::routing::get(|| async {
                Json(json!({ "setup_required": false, "version": "1.6.0" }))
            }),
        )
        .route(
            "/api/auth/login",
            post(move |headers: axum::http::HeaderMap, Json(b): Json<Value>| async move {
                // Muzik forwards the client address for Inkult's per-client limits.
                assert_eq!(headers["x-forwarded-for"], "127.0.0.1");
                let user = b["username"].as_str().unwrap_or("").to_ascii_lowercase();
                match (user.as_str(), b["password"].as_str().unwrap_or("")) {
                    ("pirate", "arrrrrrr") => signed_in(account(42, "pirate", false, false)),
                    ("captain", "seaworthy") => Json(json!({
                        "second_factor_required": true, "challenge": "ch-7", "expires_in_secs": 300,
                    }))
                    .into_response(),
                    ("temp", "temporary") => signed_in(account(9, "temp", false, true)),
                    ("admin", "inkultpw") => signed_in(account(99, "admin", false, false)),
                    ("bosun", "aye-aye-aye") => signed_in(account(43, "bosun", false, false)),
                    ("quartermaster", "ration-the-rum") => {
                        signed_in(account(8, "quartermaster", false, false))
                    }
                    _ => (StatusCode::UNAUTHORIZED, Json(json!({ "error": "wrong" }))).into_response(),
                }
            }),
        )
        .route(
            "/api/auth/login/2fa",
            post(move |Json(b): Json<Value>| async move {
                match (b["challenge"].as_str(), b["code"].as_str()) {
                    (Some("ch-7"), Some("123456")) => signed_in(account(7, "captain", true, false)),
                    (Some("ch-7"), _) => {
                        (StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad code" }))).into_response()
                    }
                    _ => (StatusCode::GONE, Json(json!({ "error": "expired" }))).into_response(),
                }
            }),
        )
        .route(
            "/api/auth/logout",
            post(move |headers: axum::http::HeaderMap| async move {
                assert_eq!(headers[header::COOKIE], "inkult_session=abcdef0123");
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                StatusCode::NO_CONTENT
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/"), logouts)
}

async fn inkult_login(t: &TestApp, username: &str, password: &str) -> axum::response::Response {
    t.request(
        "POST",
        "/api/auth/login",
        None,
        Some(json!({"username": username, "password": password})),
        &[],
    )
    .await
}

#[tokio::test]
async fn sign_in_with_inkult() {
    let t = setup().await;
    let admin = admin_session(&t).await;
    let (url, logouts) = fake_inkult().await;

    // Off by default: nobody signs in through Inkult, Muzik doesn't call it.
    let status: Value = t.json("/api/auth/setup", &admin).await;
    assert_eq!(status["inkult_sign_in"], false);
    assert_eq!(
        inkult_login(&t, "pirate", "arrrrrrr").await.status(),
        StatusCode::UNAUTHORIZED
    );

    // Admin settings: URL is validated and normalized; the test reaches Inkult.
    let res = t
        .request(
            "PUT",
            "/api/admin/inkult",
            Some(&admin),
            Some(json!({"enabled": true, "url": ""})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = t
        .request(
            "POST",
            "/api/admin/inkult/test",
            Some(&admin),
            Some(json!({"url": url})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let probe = body_json(res).await;
    assert_eq!(probe["version"], "1.6.0");
    assert_eq!(probe["insecure"], false); // plain http, but on loopback
                                          // Where Inkult users' passwords go: the admin's session alone can't
                                          // decide that, the password is needed too.
    let put = |body: Value| {
        let admin = admin.clone();
        let t = &t;
        async move {
            let res = t
                .request("PUT", "/api/admin/inkult", Some(&admin), Some(body), &[])
                .await;
            (res.status(), body_json(res).await)
        }
    };
    let (s, e) = put(json!({"enabled": true, "url": url})).await;
    assert_eq!(
        (s, e["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("reauth_required"))
    );
    let (s, e) = put(json!({"enabled": true, "url": url, "password": "not-it-at-all"})).await;
    assert_eq!(
        (s, e["error"].as_str()),
        (StatusCode::FORBIDDEN, Some("wrong_password"))
    );
    let (s, _) = put(json!({"enabled": false, "url": "http://elsewhere.example:1"})).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "changing the address while off too"
    );
    let (s, body) = put(json!({"enabled": true, "url": url, "password": "supersecret"})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["url"], url.trim_end_matches('/'));
    assert_eq!(body["insecure"], false);
    // Off and on again at the same, already confirmed address: off is free.
    let (s, _) = put(json!({"enabled": false, "url": url})).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = put(json!({"enabled": true, "url": url})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = put(json!({"enabled": true, "url": url, "password": "supersecret"})).await;
    assert_eq!(s, StatusCode::OK);
    let status: Value = t.json("/api/auth/setup", &admin).await;
    assert_eq!(status["inkult_sign_in"], true);

    assert_eq!(
        inkult_login(&t, "pirate", "wrong").await.status(),
        StatusCode::UNAUTHORIZED
    );
    // Every Inkult account is a Muzik user: the first sign-in creates it.
    let res = inkult_login(&t, "Pirate", "arrrrrrr").await;
    assert_eq!(res.status(), StatusCode::OK);
    let pirate = session_cookie(&res);
    let body = body_json(res).await;
    assert_eq!(body["status"], "ok");
    assert_eq!(body["user"]["username"], "pirate");
    assert_eq!(body["user"]["inkult"], true);
    let pirate_id = body["user"]["id"].as_i64().unwrap();

    // The next sign-in reaches the same account.
    let res = inkult_login(&t, "pirate", "arrrrrrr").await;
    assert_eq!(body_json(res).await["user"]["id"].as_i64(), Some(pirate_id));

    // Muzik ends every Inkult session it was handed.
    for _ in 0..50 {
        if logouts.load(std::sync::atomic::Ordering::SeqCst) >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(logouts.load(std::sync::atomic::Ordering::SeqCst), 2);

    // Password and 2FA belong to Inkult.
    let res = t
        .request(
            "POST",
            "/api/me/password",
            Some(&pirate),
            Some(json!({"current_password": "arrrrrrr", "new_password": "something-else"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(res).await["error"], "managed_by_inkult");

    // A local account is never taken over by the Inkult account of that name.
    assert_eq!(
        inkult_login(&t, "admin", "inkultpw").await.status(),
        StatusCode::UNAUTHORIZED
    );

    // An account an admin set up beforehand (here as an administrator) is
    // linked at the first sign-in, through Inkult's second factor.
    let res = t
        .request(
            "POST",
            "/api/users",
            Some(&admin),
            Some(json!({"username": "captain", "inkult": true, "is_admin": true})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let captain_id = body_json(res).await["id"].as_i64().unwrap();
    let res = inkult_login(&t, "captain", "seaworthy").await;
    assert_eq!(res.status(), StatusCode::OK);
    let challenge = cookie_named(&res, "muzik_mfa").expect("challenge cookie");
    assert_eq!(body_json(res).await["status"], "mfa_required");
    let res = t
        .request(
            "POST",
            "/api/auth/login/2fa",
            Some(&challenge),
            Some(json!({"code": "000000"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(res).await["error"], "invalid_code");
    let res = t
        .request(
            "POST",
            "/api/auth/login/2fa",
            Some(&challenge),
            Some(json!({"code": "123456"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let captain = cookie_named(&res, "muzik_session").expect("session cookie");
    let body = body_json(res).await;
    assert_eq!(body["user"]["username"], "captain");
    assert_eq!(body["user"]["id"].as_i64(), Some(captain_id));
    assert_eq!(body["user"]["is_admin"], true);
    assert_eq!(body["user"]["inkult"], true);
    assert_eq!(body["user"]["totp_enabled"], true);
    // The challenge is single-use.
    let res = t
        .request(
            "POST",
            "/api/auth/login/2fa",
            Some(&challenge),
            Some(json!({"code": "123456"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // An Inkult admin has no Muzik password: a fresh sign-in confirms
    // instead. Re-saving the same settings needs nothing.
    let captain_put = |body: Value| {
        let captain = captain.clone();
        let t = &t;
        async move {
            let res = t
                .request("PUT", "/api/admin/inkult", Some(&captain), Some(body), &[])
                .await;
            (res.status(), body_json(res).await)
        }
    };
    let (s, _) = captain_put(json!({"enabled": true, "url": url})).await;
    assert_eq!(s, StatusCode::OK);
    sqlx::query("UPDATE sessions SET created_at = created_at - 3600 WHERE user_id = ?")
        .bind(captain_id)
        .execute(&t.state.pool)
        .await
        .unwrap();
    let (s, e) = captain_put(json!({"enabled": true, "url": "http://inkult.example.org"})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["error"], "recent_sign_in_required");

    // An Inkult account still on a temporary password must finish in Inkult.
    let res = inkult_login(&t, "temp", "temporary").await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(res).await["error"], "inkult_account_setup");

    // Mandatory 2FA is met only by Inkult's own 2FA.
    t.request(
        "PATCH",
        &format!("/api/users/{pirate_id}"),
        Some(&admin),
        Some(json!({"totp_required": true})),
        &[],
    )
    .await;
    let res = inkult_login(&t, "pirate", "arrrrrrr").await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(body_json(res).await["error"], "inkult_mfa_required");
    // ...and the existing session is limited like any account missing 2FA.
    let res = t
        .request("GET", "/api/albums", Some(&pirate), None, &[])
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    t.request(
        "PATCH",
        &format!("/api/users/{pirate_id}"),
        Some(&admin),
        Some(json!({"totp_required": false})),
        &[],
    )
    .await;

    // Inkult 2FA can't be reset from Muzik; a Muzik password makes it local.
    let res = t
        .request(
            "PATCH",
            &format!("/api/users/{pirate_id}"),
            Some(&admin),
            Some(json!({"reset_totp": true})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = t
        .request(
            "PATCH",
            &format!("/api/users/{pirate_id}"),
            Some(&admin),
            Some(json!({"password": "local-password"})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["inkult"], false);
    assert_eq!(
        inkult_login(&t, "pirate", "arrrrrrr").await.status(),
        StatusCode::UNAUTHORIZED
    );
    let res = inkult_login(&t, "pirate", "local-password").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["user"]["inkult"], false);

    // Turning the connector off stops Inkult sign-ins.
    t.request(
        "PUT",
        "/api/admin/inkult",
        Some(&admin),
        Some(json!({"enabled": false, "url": url})),
        &[],
    )
    .await;
    assert_eq!(
        inkult_login(&t, "captain", "seaworthy").await.status(),
        StatusCode::UNAUTHORIZED
    );
}

// ---------- Separate libraries per Inkult account ----------

/// Writes Inkult's `library.json` (as Inkult does: temp file + rename).
fn write_inkult_index(t: &TestApp, body: &str) {
    let path = t.state.config.inkult_library.clone().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).unwrap();
    std::fs::rename(tmp, path).unwrap();
}

fn inkult_index(entries: &[(&str, &[u64])]) -> String {
    let entries: Vec<Value> = entries
        .iter()
        .map(|(path, owners)| {
            json!({
                "key": {"provider": "youtube", "id": path},
                "path": path, "title": "x", "artists": [], "album": null,
                "source_url": "https://example.com", "artwork_url": null,
                "quality": {}, "output": "original", "added_at": 1,
                "owners": owners,
            })
        })
        .collect();
    json!({ "version": 1, "entries": entries }).to_string()
}

#[tokio::test]
async fn separate_inkult_libraries() {
    let t = setup().await;
    let admin = admin_session(&t).await;
    t.state.scanner.run_blocking(ScanMode::Incremental).await;
    let (url, _) = fake_inkult().await;
    sqlx::query("UPDATE server_settings SET inkult_enabled = 1, inkult_url = ? WHERE id = 1")
        .bind(url.trim_end_matches('/'))
        .execute(&t.state.pool)
        .await
        .unwrap();
    let sign_in = |name: &'static str, password: &'static str| {
        let t = &t;
        async move {
            let res = inkult_login(t, name, password).await;
            assert_eq!(res.status(), StatusCode::OK, "{name} signs in");
            session_cookie(&res)
        }
    };
    let pirate = sign_in("pirate", "arrrrrrr").await; // Inkult id 42
    let bosun = sign_in("bosun", "aye-aye-aye").await; // 43
    let captain = sign_in("quartermaster", "ration-the-rum").await; // 8, captain

    // Track ids, as the admin sees them.
    let albums: Page<Album> = t.json("/api/albums", &admin).await;
    let first = albums
        .items
        .iter()
        .find(|a| a.title == "First Album")
        .unwrap();
    let detail: AlbumDetail = t.json(&format!("/api/albums/{}", first.id), &admin).await;
    let id_of = |title: &str| detail.tracks.iter().find(|t| t.title == title).unwrap().id;
    let (opening, second) = (id_of("Opening"), id_of("Second Song"));

    let visible = |cookie: String| {
        let t = &t;
        async move {
            let albums: Page<Album> = t.json("/api/albums?limit=500", &cookie).await;
            let mut titles = Vec::new();
            for a in albums.items {
                let d: AlbumDetail = t.json(&format!("/api/albums/{}", a.id), &cookie).await;
                assert_eq!(d.album.track_count, d.tracks.len() as i64);
                titles.extend(d.tracks.into_iter().map(|t| t.title));
            }
            titles.sort();
            titles
        }
    };
    let everything = vec!["03 Untagged", "Opening", "Second Song"];
    // Off by default: everyone sees the whole library.
    assert_eq!(visible(pirate.clone()).await, everything);

    let separate = |on: bool| {
        let (t, admin) = (&t, admin.clone());
        async move {
            let res = t
                .request(
                    "PUT",
                    "/api/admin/inkult/libraries",
                    Some(&admin),
                    Some(json!({ "separate": on })),
                    &[],
                )
                .await;
            (res.status(), body_json(res).await)
        }
    };
    // Admins only; refused while Inkult's index can't be read.
    let res = t
        .request(
            "PUT",
            "/api/admin/inkult/libraries",
            Some(&pirate),
            Some(json!({ "separate": true })),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let (s, _) = separate(true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // An index whose files aren't in the library (wrong mount) is refused too:
    // every song would look hand-added and stay shared.
    write_inkult_index(&t, &inkult_index(&[("Elsewhere/x.flac", &[42])]));
    let (s, body) = separate(true).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");

    // Opening is the pirate's, Second Song the bosun's; the untagged WAV was
    // copied in by hand (not in Inkult's index), so it is shared.
    write_inkult_index(
        &t,
        &inkult_index(&[
            ("Test Artist/First Album/01 - Opening.flac", &[42]),
            ("Test Artist/First Album/02 - Second Song.mp3", &[43]),
            ("Somewhere/Deleted.flac", &[42, 43]),
        ]),
    );
    let (s, body) = separate(true).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["separate_libraries"], true);
    assert_eq!(body["library_index"]["files"], 3);
    assert_eq!(body["library_index"]["matched"], 2);

    assert_eq!(visible(pirate.clone()).await, ["03 Untagged", "Opening"]);
    assert_eq!(visible(bosun.clone()).await, ["03 Untagged", "Second Song"]);
    // Inkult captains and Muzik admins see everything, as in Inkult.
    assert_eq!(visible(captain.clone()).await, everything);
    assert_eq!(visible(admin.clone()).await, everything);

    // Captains choose: everything by default, or only their own songs (the
    // quartermaster owns none, so only the hand-added one). Nobody else can.
    let own_library = |cookie: String, on: bool| {
        let t = &t;
        async move {
            let res = t
                .request(
                    "PUT",
                    "/api/me/library",
                    Some(&cookie),
                    Some(json!({ "own_library": on })),
                    &[],
                )
                .await;
            (res.status(), body_json(res).await)
        }
    };
    let me: Value = t.json("/api/me", &captain).await;
    assert_eq!(me["own_library"], false);
    let me: Value = t.json("/api/me", &pirate).await;
    assert!(me["own_library"].is_null());
    assert_eq!(
        own_library(pirate.clone(), true).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        own_library(admin.clone(), true).await.0,
        StatusCode::BAD_REQUEST
    );
    let (s, body) = own_library(captain.clone(), true).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["own_library"], true);
    assert_eq!(visible(captain.clone()).await, ["03 Untagged"]);
    let (s, _) = own_library(captain.clone(), false).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(visible(captain.clone()).await, everything);

    // Someone else's song: not streamable, not found, not searchable.
    let status = |uri: String, cookie: String| {
        let t = &t;
        async move {
            t.request("GET", &uri, Some(&cookie), None, &[])
                .await
                .status()
        }
    };
    for uri in [
        format!("/api/stream/{second}"),
        format!("/api/tracks/{second}"),
        format!("/api/tracks/{second}/suggestions"),
    ] {
        assert_eq!(
            status(uri.clone(), pirate.clone()).await,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
        assert_eq!(status(uri, bosun.clone()).await, StatusCode::OK);
    }
    assert_eq!(
        status(format!("/api/stream/{opening}"), pirate.clone()).await,
        StatusCode::OK
    );
    let found: SearchResults = t.json("/api/search?q=second", &pirate).await;
    assert!(found.tracks.is_empty());
    let found: SearchResults = t.json("/api/search?q=second", &bosun).await;
    assert_eq!(found.tracks.len(), 1);
    let suggested: Vec<Track> = t
        .json(&format!("/api/tracks/{opening}/suggestions"), &pirate)
        .await;
    assert!(suggested.iter().all(|s| s.id != second));

    // Playlists: songs outside your library can't be added, and show as
    // unavailable in someone else's shared playlist.
    let res = t
        .request(
            "POST",
            "/api/playlists",
            Some(&pirate),
            Some(json!({"name": "Mine", "track_ids": [second]})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = t
        .request(
            "POST",
            "/api/playlists",
            Some(&admin),
            Some(json!({"name": "Both", "track_ids": [opening, second]})),
            &[],
        )
        .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let shared: PlaylistSummary =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let res = t
        .request(
            "PATCH",
            &format!("/api/playlists/{}", shared.id),
            Some(&admin),
            Some(json!({"public": true})),
            &[],
        )
        .await;
    assert!(res.status().is_success());
    let seen: PlaylistDetail = t
        .json(&format!("/api/playlists/{}", shared.id), &pirate)
        .await;
    let available: Vec<Option<i64>> = seen
        .entries
        .iter()
        .map(|e| e.track.as_ref().map(|t| t.id))
        .collect();
    assert_eq!(available, [Some(opening), None]);

    // Ownership follows Inkult without a rescan: the pirate downloads the
    // bosun's song too (Inkult only adds an owner).
    write_inkult_index(
        &t,
        &inkult_index(&[
            ("Test Artist/First Album/01 - Opening.flac", &[42]),
            ("Test Artist/First Album/02 - Second Song.mp3", &[43, 42]),
        ]),
    );
    t.state.inkult_index.sync(false).await.unwrap();
    assert_eq!(visible(pirate.clone()).await, everything);

    // A broken index keeps the previous copy instead of opening everything up.
    write_inkult_index(&t, "{ not json");
    assert!(t.state.inkult_index.sync(false).await.is_err());
    assert_eq!(visible(bosun.clone()).await, ["03 Untagged", "Second Song"]);
    let settings: Value = t.json("/api/admin/inkult", &admin).await;
    assert!(settings["library_index"]["error"].is_string());

    // Off again: everyone sees everything.
    let (s, _) = separate(false).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(visible(bosun).await, everything);
}
