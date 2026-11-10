//! Muzik web frontend (Leptos, client-side rendered).

mod api;
mod backswipe;
mod collection;
mod components;
mod gestures;
mod icons;
mod overlays;
mod pages;
mod player;
mod theme;
mod util;

use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    hooks::use_navigate,
    path,
};
use muzik_common::User;

use components::{MiniPlayer, NowPlaying, PlayerBar, Sidebar, Spinner, TabBar, Toast};
use player::Player;

#[derive(Clone)]
pub enum AuthState {
    Loading,
    NeedsSetup,
    LoggedOut,
    In(User),
    Offline(String),
}

#[derive(Clone, Copy)]
pub struct Auth(pub RwSignal<AuthState>);

impl Auth {
    pub fn user(&self) -> Option<User> {
        self.0.with(|s| match s {
            AuthState::In(u) => Some(u.clone()),
            _ => None,
        })
    }

    pub fn is_admin(&self) -> bool {
        self.0.with(|s| matches!(s, AuthState::In(u) if u.is_admin))
    }

    /// Re-checks the session (called at startup, after any 401 and when the
    /// server says 2FA must be set up first).
    pub fn refresh(self) {
        leptos::task::spawn_local(async move {
            let next = match api::setup_status().await {
                Ok(s) if s.needs_setup => AuthState::NeedsSetup,
                Ok(_) => match api::me().await {
                    Ok(u) => AuthState::In(u),
                    Err(e) if e.is_unauthorized() => AuthState::LoggedOut,
                    Err(e) => AuthState::Offline(e.message),
                },
                Err(e) => AuthState::Offline(e.message),
            };
            self.0.set(next);
        });
    }
}

pub fn use_auth() -> Auth {
    expect_context::<Auth>()
}

/// WASM entry point, called by the generated JS loader. The frontend is a
/// library (cdylib) so the workspace has a single binary: `cargo run` starts the server.
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let auth = Auth(RwSignal::new(AuthState::Loading));
    provide_context(auth);
    provide_context(components::Toasts::new());
    theme::Looks::install();
    auth.refresh();

    // Only switch screens when the kind of state changes: updating the signed-in
    // user (e.g. its 2FA flags) must not remount the app and stop playback.
    let screen = Memo::new(move |_| match auth.0.get() {
        AuthState::Loading => Screen::Loading,
        AuthState::NeedsSetup => Screen::NeedsSetup,
        AuthState::LoggedOut => Screen::LoggedOut,
        AuthState::Offline(msg) => Screen::Offline(msg),
        // An admin made 2FA mandatory: nothing else until it's set up.
        AuthState::In(u) if u.needs_totp_setup() => Screen::TotpSetup,
        AuthState::In(_) => Screen::App,
    });

    view! {
        <Router>
            {move || match screen.get() {
                Screen::Loading => view! { <div class="splash"><Spinner/></div> }.into_any(),
                Screen::NeedsSetup => view! { <pages::auth::Setup/> }.into_any(),
                Screen::LoggedOut => view! { <pages::auth::Login/> }.into_any(),
                Screen::Offline(msg) => view! {
                    <div class="splash">
                        <div class="auth-card">
                            <icons::Brand big=true/>
                            <h1 class="auth-title">"Can't reach Muzik"</h1>
                            <p class="auth-sub">{msg}</p>
                            <button class="btn primary large" on:click=move |_| auth.refresh()>"Try Again"</button>
                        </div>
                    </div>
                }.into_any(),
                Screen::TotpSetup => view! { <pages::auth::RequiredTotpSetup/> }.into_any(),
                Screen::App => view! { <Shell/> }.into_any(),
            }}
        </Router>
    }
}

/// Top-level screen derived from [`AuthState`].
#[derive(Clone, PartialEq)]
enum Screen {
    Loading,
    NeedsSetup,
    LoggedOut,
    Offline(String),
    TotpSetup,
    App,
}

#[component]
fn Shell() -> impl IntoView {
    let player = Player::install();
    provide_context(player);
    collection::Collection::install();
    let overlays = overlays::Overlays::install();
    install_shortcuts(player, overlays);
    let back = backswipe::BackSwipe::install();

    view! {
        <div class="app">
            <Sidebar/>
            <div
                class="main back-prev"
                class:settling=move || back.settling()
                class:covering=move || back.covering()
                style:transform=move || back.layer_transform()
                style:filter=move || back.layer_filter()
                node_ref=back.layer_ref()
                aria-hidden="true"
            ></div>
            <main
                class="main"
                id="main"
                node_ref=back.main_ref()
                class:back-lifted=move || back.lifted()
                class:back-settling=move || back.settling()
                style:transform=move || back.main_transform()
                on:scroll=move |_| back.scrolled()
                on:touchstart:capture=move |ev| back.touch_start(&ev)
                on:touchmove:capture=move |ev| back.touch_move(&ev)
                on:touchend:capture=move |ev| back.touch_end(&ev)
                on:touchcancel:capture=move |_| back.touch_cancel()
            >
                <Routes fallback=|| view! { <div class="page"><h1>"Not found"</h1></div> }>
                    <Route path=path!("/") view=pages::home::Home/>
                    <Route path=path!("/search") view=pages::search::Search/>
                    <Route path=path!("/albums") view=pages::library::Albums/>
                    <Route path=path!("/artists") view=pages::library::Artists/>
                    <Route path=path!("/artist/:id") view=pages::library::ArtistPage/>
                    <Route path=path!("/album/:id") view=pages::library::AlbumPage/>
                    <Route path=path!("/library") view=pages::playlists::LibraryHub/>
                    <Route path=path!("/playlist/:id") view=pages::playlists::PlaylistPage/>
                    <Route path=path!("/queue") view=pages::queue::Queue/>
                    <Route path=path!("/settings") view=pages::settings::Settings/>
                </Routes>
            </main>
            <PlayerBar/>
            <MiniPlayer/>
            <TabBar/>
            <NowPlaying/>
            <overlays::OverlayHost/>
            <Toast/>
        </div>
    }
}

/// Desktop keyboard shortcuts: space (play/pause), ←/→ (seek), shift+←/→
/// (previous/next), ↑/↓ (volume), `m` (mute), `s` (shuffle),
/// `r` (repeat), `/` (search).
/// Disabled while a menu or dialog is open (it handles its own keys).
fn install_shortcuts(player: Player, overlays: overlays::Overlays) {
    let navigate = use_navigate();
    let handle = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.ctrl_key() || ev.meta_key() || ev.alt_key() || overlays.is_open() {
            return;
        }
        let typing = ev
            .target()
            .and_then(|t| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(t).ok())
            .is_some_and(|el| {
                matches!(el.tag_name().as_str(), "INPUT" | "TEXTAREA" | "SELECT")
                    || el.has_attribute("contenteditable")
            });
        if typing {
            if ev.key() == "Escape" {
                if let Some(el) = document().active_element() {
                    if let Ok(el) = wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(el) {
                        let _ = el.blur();
                    }
                }
            }
            return;
        }
        match ev.key().as_str() {
            " " => {
                ev.prevent_default();
                player.toggle();
            }
            "ArrowRight" if ev.shift_key() => player.next(),
            "ArrowLeft" if ev.shift_key() => player.prev(),
            "ArrowRight" => player.seek_by(10.0),
            "ArrowLeft" => player.seek_by(-10.0),
            "ArrowUp" => {
                ev.prevent_default();
                player.volume_by(0.05);
            }
            "ArrowDown" => {
                ev.prevent_default();
                player.volume_by(-0.05);
            }
            "m" | "M" => player.toggle_mute(),
            "s" | "S" => player.toggle_shuffle(),
            "r" | "R" => player.cycle_repeat(),
            "/" => {
                ev.prevent_default();
                navigate("/search", Default::default());
                util::focus_search();
            }
            "Escape" => player.sheet_open.set(false),
            _ => {}
        }
    });
    on_cleanup(move || handle.remove());
}
