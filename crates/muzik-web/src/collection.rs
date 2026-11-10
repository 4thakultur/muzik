//! The user's collection: their playlists (owned and saved).
//! Loaded once per session and kept in sync after every change, so the
//! sidebar and the "Add to Playlist" picker never need a refetch.

use leptos::{prelude::*, task::spawn_local};
use muzik_common::PlaylistSummary;

use crate::{
    api,
    components::{use_toasts, Toasts},
    use_auth, Auth, AuthState,
};

#[derive(Clone, Copy)]
pub struct Collection {
    pub playlists: RwSignal<Vec<PlaylistSummary>>,
    /// `(playlist id, n)`: bumped when a playlist's songs or details change
    /// outside its page (e.g. from the "Add to Playlist" picker).
    pub touched: RwSignal<(i64, u64)>,
    toasts: Toasts,
    auth: Auth,
}

pub fn use_collection() -> Collection {
    expect_context::<Collection>()
}

impl Collection {
    pub fn install() -> Self {
        let c = Self {
            playlists: RwSignal::new(Vec::new()),
            touched: RwSignal::new((0, 0)),
            toasts: use_toasts(),
            auth: use_auth(),
        };
        provide_context(c);
        c.refresh();
        c
    }

    /// Reloads the playlist list from the server.
    pub fn refresh(self) {
        spawn_local(async move {
            if let Ok(list) = api::playlists().await {
                self.playlists.try_set(list);
            }
        });
    }

    /// Inserts or replaces a playlist and moves it to the top (most recent).
    pub fn upsert(self, p: PlaylistSummary) {
        self.playlists.update(|list| {
            list.retain(|x| x.id != p.id);
            list.insert(0, p);
        });
    }

    pub fn forget(self, id: i64) {
        self.playlists.update(|list| list.retain(|x| x.id != id));
    }

    pub fn touch(self, id: i64) {
        self.touched.update(|(pid, n)| {
            *pid = id;
            *n += 1;
        });
    }

    /// Playlists the user may add songs to.
    pub fn editable(self) -> Vec<PlaylistSummary> {
        self.playlists
            .get()
            .into_iter()
            .filter(|p| p.can_edit)
            .collect()
    }

    /// Default name for a new playlist: "My Playlist #n".
    pub fn next_name(self) -> String {
        let n = self
            .playlists
            .with_untracked(|l| l.iter().filter(|p| p.is_owner).count())
            + 1;
        format!("My Playlist #{n}")
    }
}

impl Collection {
    pub fn toast(self, text: impl Into<String>) {
        self.toasts.show(text);
    }

    /// Shows an API error as a toast (or returns to the login screen on 401).
    pub fn error(self, e: api::ApiError) {
        report(self.auth, self.toasts, e);
    }
}

fn report(auth: Auth, toasts: Toasts, e: api::ApiError) {
    if e.is_unauthorized() {
        auth.0.set(AuthState::LoggedOut);
    } else if e.needs_totp_setup() {
        auth.refresh();
    } else {
        toasts.show(e.message);
    }
}
