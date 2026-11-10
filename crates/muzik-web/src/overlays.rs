//! Modal layer, rendered once by the shell: context menus (a popover at the
//! pointer on desktop, an action sheet on phones), the "Add to Playlist"
//! picker, playlist create/edit forms and confirmations.

use leptos::{prelude::*, task::spawn_local};
use leptos_router::{hooks::use_navigate, NavigateOptions};
use muzik_common::{
    AddTracksRequest, CreatePlaylistRequest, PlaylistSummary, Track, UpdatePlaylistRequest,
    PLAYLIST_DESCRIPTION_MAX, PLAYLIST_NAME_MAX,
};
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;

use crate::{
    api,
    collection::{use_collection, Collection},
    components::{PlaylistCover, TrackCover},
    icons::{Icon, Svg},
    player::use_player,
};

/// Where a popover menu opens: below/above a button, or at the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    x: f64,
    top: f64,
    bottom: f64,
    align_right: bool,
}

impl Anchor {
    /// Under the event's target button, right-aligned with it.
    pub fn below(ev: &web_sys::MouseEvent) -> Option<Self> {
        let el = ev.current_target()?.dyn_into::<web_sys::Element>().ok()?;
        let r = el.get_bounding_client_rect();
        Some(Self {
            x: r.right(),
            top: r.top(),
            bottom: r.bottom(),
            align_right: true,
        })
    }

    /// At the pointer (right-click).
    pub fn pointer(ev: &web_sys::MouseEvent) -> Option<Self> {
        let (x, y) = (f64::from(ev.client_x()), f64::from(ev.client_y()));
        Some(Self {
            x,
            top: y,
            bottom: y,
            align_right: false,
        })
    }
}

#[derive(Clone)]
pub enum Overlay {
    TrackMenu {
        track: Track,
        at: Option<Anchor>,
        /// Present when the song is shown inside an editable playlist.
        remove: Option<Callback<()>>,
    },
    PlaylistMenu {
        playlist: PlaylistSummary,
        /// Playable songs, for "Add to Queue" / "Add to Other Playlist".
        tracks: Vec<Track>,
        at: Option<Anchor>,
    },
    AddToPlaylist {
        track_ids: Vec<i64>,
        label: String,
    },
    NewPlaylist {
        track_ids: Vec<i64>,
    },
    EditPlaylist {
        playlist: PlaylistSummary,
    },
    /// A single-choice picker (replaces native `<select>` menus).
    Choice {
        title: String,
        options: Vec<String>,
        selected: usize,
        on_pick: Callback<usize>,
        at: Option<Anchor>,
    },
    Confirm {
        title: String,
        message: String,
        action: String,
        on_confirm: Callback<()>,
    },
}

#[derive(Clone, Copy)]
pub struct Overlays {
    pub current: RwSignal<Option<Overlay>>,
    /// Element focused before opening, refocused on close.
    restore: StoredValue<Option<SendWrapper<web_sys::HtmlElement>>>,
}

pub fn use_overlays() -> Overlays {
    expect_context::<Overlays>()
}

impl Overlays {
    pub fn install() -> Self {
        let o = Self {
            current: RwSignal::new(None),
            restore: StoredValue::new(None),
        };
        provide_context(o);
        o
    }

    pub fn open(&self, overlay: Overlay) {
        if self.current.with_untracked(Option::is_none) {
            let active = document()
                .active_element()
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok());
            self.restore.set_value(active.map(SendWrapper::new));
        }
        self.current.set(Some(overlay));
        focus_first_in_panel();
    }

    pub fn close(&self) {
        if self.current.with_untracked(Option::is_none) {
            return;
        }
        self.current.set(None);
        if let Some(Some(el)) = self.restore.try_update_value(Option::take) {
            if el.is_connected() {
                let _ = el.focus();
            }
        }
    }

    pub fn is_open(&self) -> bool {
        self.current.with_untracked(Option::is_some)
    }
}

/// Moves focus into the newly opened panel (an `[autofocus]` field, else the
/// first button) so keyboard users land inside it.
fn focus_first_in_panel() {
    set_timeout(
        || {
            let doc = document();
            let el = doc
                .query_selector(".overlay [autofocus]")
                .ok()
                .flatten()
                .or_else(|| doc.query_selector(".overlay .panel button").ok().flatten());
            if let Some(el) = el.and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()) {
                let _ = el.focus();
            }
        },
        std::time::Duration::from_millis(20),
    );
}

fn songs(n: i64) -> String {
    format!("{n} song{}", if n == 1 { "" } else { "s" })
}

struct Item {
    icon: Icon,
    label: String,
    danger: bool,
    run: Box<dyn Fn()>,
}

fn item(icon: Icon, label: impl Into<String>, run: impl Fn() + 'static) -> Item {
    Item {
        icon,
        label: label.into(),
        danger: false,
        run: Box::new(run),
    }
}

const MENU_W: f64 = 250.0;
const CHOICE_W: f64 = 210.0;

/// Inline position (left, top, bottom) for a popover menu `w` px wide.
fn place(at: Option<Anchor>, w: f64) -> (String, String, String) {
    let Some(a) = at else {
        return Default::default();
    };
    let (vw, vh) = web_sys::window()
        .map(|w| {
            let f = |v: Result<wasm_bindgen::JsValue, _>| v.ok().and_then(|v| v.as_f64());
            (
                f(w.inner_width()).unwrap_or(1024.0),
                f(w.inner_height()).unwrap_or(768.0),
            )
        })
        .unwrap_or((1024.0, 768.0));
    let left = if a.align_right { a.x - w } else { a.x };
    let left = left.clamp(8.0, (vw - w - 8.0).max(8.0));
    // Open upwards when the pointer is low on the screen.
    if a.bottom > vh * 0.55 {
        (
            format!("{left:.0}px"),
            String::new(),
            format!("{:.0}px", (vh - a.top + 4.0).max(8.0)),
        )
    } else {
        (
            format!("{left:.0}px"),
            format!("{:.0}px", a.bottom + 4.0),
            String::new(),
        )
    }
}

#[component]
pub fn OverlayHost() -> impl IntoView {
    let ov = use_overlays();
    let handle = window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "Escape" && ov.is_open() {
            ev.prevent_default();
            ov.close();
        }
    });
    on_cleanup(move || handle.remove());

    move || {
        let current = ov.current.get()?;
        Some(match current {
            Overlay::TrackMenu { track, at, remove } => track_menu(track, at, remove),
            Overlay::PlaylistMenu {
                playlist,
                tracks,
                at,
            } => playlist_menu(playlist, tracks, at),
            Overlay::AddToPlaylist { track_ids, label } => {
                dialog(view! { <AddToPlaylist track_ids label/> }.into_any())
            }
            Overlay::NewPlaylist { track_ids } => {
                dialog(view! { <PlaylistForm track_ids/> }.into_any())
            }
            Overlay::EditPlaylist { playlist } => {
                dialog(view! { <PlaylistForm editing=playlist/> }.into_any())
            }
            Overlay::Choice {
                title,
                options,
                selected,
                on_pick,
                at,
            } => choice_menu(title, options, selected, on_pick, at),
            Overlay::Confirm {
                title,
                message,
                action,
                on_confirm,
            } => dialog(
                view! {
                    <div class="panel confirm" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
                        <h2 id="confirm-title">{title}</h2>
                        <p>{message}</p>
                        <div class="panel-actions">
                            <button class="btn" on:click=move |_| ov.close()>"Cancel"</button>
                            <button class="btn danger" on:click=move |_| { ov.close(); on_confirm.run(()); }>{action}</button>
                        </div>
                    </div>
                }
                .into_any(),
            ),
        })
    }
}

/// Dimmed backdrop + centered panel (a bottom sheet on phones).
fn dialog(panel: AnyView) -> AnyView {
    let ov = use_overlays();
    view! {
        <div class="overlay dim" on:click=move |ev| {
            if ev.target() == ev.current_target() { ov.close(); }
        }>
            {panel}
        </div>
    }
    .into_any()
}

fn menu(header: Option<AnyView>, items: Vec<Item>, at: Option<Anchor>) -> AnyView {
    let ov = use_overlays();
    let (left, top, bottom) = place(at, MENU_W);
    view! {
        <div class="overlay menu-backdrop" on:click=move |ev| {
            if ev.target() == ev.current_target() { ov.close(); }
        } on:contextmenu=move |ev| { ev.prevent_default(); ov.close(); }>
            <div class="panel popover" class:anchored=at.is_some() role="menu"
                style:left=left style:top=top style:bottom=bottom>
                {header}
                <div class="menu-items">
                    {items.into_iter().map(|it| {
                        let run = it.run;
                        view! {
                            <button class="menu-item" class:danger=it.danger role="menuitem"
                                on:click=move |_| { ov.close(); run(); }>
                                <Svg icon=it.icon/><span>{it.label}</span>
                            </button>
                        }
                    }).collect_view()}
                </div>
                <button class="menu-cancel" on:click=move |_| ov.close()>"Cancel"</button>
            </div>
        </div>
    }
    .into_any()
}

/// Single-choice menu: a compact popover with a checkmark on desktop, an
/// action sheet with a title on phones.
fn choice_menu(
    title: String,
    options: Vec<String>,
    selected: usize,
    on_pick: Callback<usize>,
    at: Option<Anchor>,
) -> AnyView {
    let ov = use_overlays();
    let (left, top, bottom) = place(at, CHOICE_W);
    let label = title.clone();
    view! {
        <div class="overlay menu-backdrop" on:click=move |ev| {
            if ev.target() == ev.current_target() { ov.close(); }
        } on:contextmenu=move |ev| { ev.prevent_default(); ov.close(); }>
            <div class="panel popover choice" class:anchored=at.is_some() role="menu" aria-label=label
                style:left=left style:top=top style:bottom=bottom>
                <div class="menu-head choice-head">{title}</div>
                <div class="menu-items">
                    {options.into_iter().enumerate().map(|(i, label)| {
                        let on = i == selected;
                        view! {
                            <button class="menu-item choice-item" class:selected=on role="menuitemradio"
                                aria-checked=on.to_string() autofocus=on
                                on:click=move |_| { ov.close(); if !on { on_pick.run(i); } }>
                                <span class="choice-label">{label}</span>
                                {on.then(|| view! { <Svg icon=Icon::Check/> })}
                            </button>
                        }
                    }).collect_view()}
                </div>
                <button class="menu-cancel" on:click=move |_| ov.close()>"Cancel"</button>
            </div>
        </div>
    }
    .into_any()
}

/// Button showing the current choice; opens a [`Overlay::Choice`] menu.
/// Muzik's replacement for a native `<select>`.
#[component]
pub fn Picker(
    /// Menu title, also the button's accessible name.
    title: &'static str,
    options: &'static [&'static str],
    #[prop(into)] selected: Signal<usize>,
    on_pick: Callback<usize>,
    #[prop(optional)] class: &'static str,
) -> impl IntoView {
    let ov = use_overlays();
    view! {
        <button class=format!("picker {class}") aria-haspopup="menu"
            aria-label=move || format!("{title}: {}", options.get(selected.get()).copied().unwrap_or(""))
            on:click=move |ev| ov.open(Overlay::Choice {
                title: title.to_owned(),
                options: options.iter().map(|o| (*o).to_owned()).collect(),
                selected: selected.get_untracked(),
                on_pick,
                at: Anchor::below(&ev),
            })>
            <span>{move || options.get(selected.get()).copied().unwrap_or("")}</span>
            <Svg icon=Icon::ChevronUpDown/>
        </button>
    }
}

fn go(path: String) -> impl Fn() + 'static {
    let navigate = use_navigate();
    let player = use_player();
    move || {
        player.sheet_open.set(false);
        navigate(&path, NavigateOptions::default());
    }
}

fn track_menu(track: Track, at: Option<Anchor>, remove: Option<Callback<()>>) -> AnyView {
    let ov = use_overlays();
    let player = use_player();
    let c = use_collection();

    let mut items = Vec::new();
    let t = track.clone();
    items.push(item(Icon::PlayNext, "Play Next", move || {
        player.play_next(t.clone());
        c.toast("Playing Next");
    }));
    let t = track.clone();
    items.push(item(Icon::Queue, "Add to Queue", move || {
        player.enqueue(t.clone());
        c.toast("Added to Queue");
    }));
    let (id, title) = (track.id, track.title.clone());
    items.push(item(Icon::PlaylistAdd, "Add to Playlist…", move || {
        ov.open(Overlay::AddToPlaylist {
            track_ids: vec![id],
            label: title.clone(),
        })
    }));
    if let Some(remove) = remove {
        let mut it = item(Icon::Trash, "Remove from This Playlist", move || {
            remove.run(())
        });
        it.danger = true;
        items.push(it);
    }
    items.push(item(
        Icon::Albums,
        "Go to Album",
        go(format!("/album/{}", track.album_id)),
    ));
    items.push(item(
        Icon::Artist,
        "Go to Artist",
        go(format!("/artist/{}", track.album_artist_id)),
    ));

    let header = view! {
        <div class="menu-head">
            <TrackCover album_id=track.album_id size=96/>
            <span class="menu-head-text">
                <span class="menu-head-title">{track.title.clone()}</span>
                <span class="menu-head-sub">{track.artist_name.clone()}</span>
            </span>
        </div>
    }
    .into_any();
    menu(Some(header), items, at)
}

fn playlist_menu(p: PlaylistSummary, tracks: Vec<Track>, at: Option<Anchor>) -> AnyView {
    let ov = use_overlays();
    let player = use_player();
    let c = use_collection();
    let navigate = use_navigate();
    let mut items = Vec::new();

    if !tracks.is_empty() {
        let ts = tracks.clone();
        items.push(item(Icon::Queue, "Add to Queue", move || {
            player.enqueue_many(ts.clone());
            c.toast("Added to Queue");
        }));
        let (ids, name) = (
            tracks.iter().map(|t| t.id).collect::<Vec<_>>(),
            p.name.clone(),
        );
        items.push(item(
            Icon::PlaylistAdd,
            "Add to Other Playlist…",
            move || {
                ov.open(Overlay::AddToPlaylist {
                    track_ids: ids.clone(),
                    label: name.clone(),
                })
            },
        ));
    }
    if p.is_owner {
        let pl = p.clone();
        items.push(item(Icon::Pencil, "Edit Details", move || {
            ov.open(Overlay::EditPlaylist {
                playlist: pl.clone(),
            })
        }));
        let (id, public) = (p.id, p.public);
        items.push(item(
            Icon::People,
            if public {
                "Stop Sharing"
            } else {
                "Share with Everyone"
            },
            move || {
                spawn_local(async move {
                    let req = UpdatePlaylistRequest {
                        public: Some(!public),
                        ..Default::default()
                    };
                    match api::update_playlist(id, &req).await {
                        Ok(s) => {
                            c.upsert(s);
                            c.touch(id);
                            c.toast(if public {
                                "Only you can see this playlist"
                            } else {
                                "Shared with everyone on this server"
                            });
                        }
                        Err(e) => c.error(e),
                    }
                });
            },
        ));
    } else if p.public {
        let (id, following) = (p.id, p.following);
        items.push(item(
            if following {
                Icon::HeartFill
            } else {
                Icon::Heart
            },
            if following {
                "Remove from Library"
            } else {
                "Save to Library"
            },
            move || follow(c, id, !following),
        ));
    }
    let nav = navigate.clone();
    let id = p.id;
    items.push(item(Icon::Copy, "Save a Copy", move || {
        let nav = nav.clone();
        spawn_local(async move {
            match api::duplicate_playlist(id).await {
                Ok(s) => {
                    let path = format!("/playlist/{}", s.id);
                    c.upsert(s);
                    c.toast("Copy saved to your library");
                    nav(&path, NavigateOptions::default());
                }
                Err(e) => c.error(e),
            }
        });
    }));
    if p.is_owner {
        let (id, name) = (p.id, p.name.clone());
        let nav = navigate.clone();
        let mut it = item(Icon::Trash, "Delete Playlist", move || {
            let nav = nav.clone();
            ov.open(Overlay::Confirm {
                title: "Delete Playlist?".into(),
                message: format!("“{name}” will be deleted for everyone. This can't be undone."),
                action: "Delete".into(),
                on_confirm: Callback::new(move |()| {
                    let nav = nav.clone();
                    spawn_local(async move {
                        match api::delete_playlist(id).await {
                            Ok(()) => {
                                c.forget(id);
                                c.toast("Playlist deleted");
                                nav("/library", NavigateOptions::default());
                            }
                            Err(e) => c.error(e),
                        }
                    });
                }),
            });
        });
        it.danger = true;
        items.push(it);
    }

    let header = view! {
        <div class="menu-head">
            <PlaylistCover covers=p.cover_album_ids.clone() size=96/>
            <span class="menu-head-text">
                <span class="menu-head-title">{p.name.clone()}</span>
                <span class="menu-head-sub">{format!("{} · {}", p.owner_name, songs(p.track_count))}</span>
            </span>
        </div>
    }
    .into_any();
    menu(Some(header), items, at)
}

/// Saves (or removes) another user's playlist from the library.
pub fn follow(c: Collection, id: i64, on: bool) {
    spawn_local(async move {
        match api::follow_playlist(id, on).await {
            Ok(()) => {
                c.refresh();
                c.touch(id);
                c.toast(if on {
                    "Saved to Your Library"
                } else {
                    "Removed from Your Library"
                });
            }
            Err(e) => c.error(e),
        }
    });
}

#[component]
fn AddToPlaylist(track_ids: Vec<i64>, label: String) -> impl IntoView {
    let ov = use_overlays();
    let c = use_collection();
    let ids = StoredValue::new(track_ids);
    let filter = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    // Set when every song was already in the chosen playlist: ask "Add anyway?".
    let confirm: RwSignal<Option<PlaylistSummary>> = RwSignal::new(None);

    let add = move |p: PlaylistSummary, allow_duplicates: bool| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let req = AddTracksRequest {
                track_ids: ids.get_value(),
                position: None,
                allow_duplicates,
            };
            let res = api::add_to_playlist(p.id, &req).await;
            busy.try_set(false);
            match res {
                Ok(r) if r.added == 0 && r.duplicates > 0 => {
                    confirm.try_set(Some(p));
                }
                Ok(r) => {
                    let name = r.playlist.name.clone();
                    c.upsert(r.playlist);
                    c.touch(p.id);
                    ov.close();
                    c.toast(if r.duplicates > 0 {
                        format!(
                            "Added {} to {name} ({} already there)",
                            songs(r.added),
                            r.duplicates
                        )
                    } else {
                        format!("Added to {name}")
                    });
                }
                Err(e) => c.error(e),
            }
        });
    };

    let n = ids.with_value(Vec::len);
    let list = move || {
        let q = filter.get().to_lowercase();
        c.editable()
            .into_iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q))
            .map(|p| {
                let p2 = p.clone();
                view! {
                    <li><button class="pick-row" disabled=move || busy.get() on:click=move |_| add(p2.clone(), false)>
                        <PlaylistCover covers=p.cover_album_ids.clone() size=96/>
                        <span class="list-text">
                            <span class="list-title">{p.name.clone()}</span>
                            <span class="list-sub">
                                {if p.is_owner { songs(p.track_count) } else { format!("{} · {}", p.owner_name, songs(p.track_count)) }}
                            </span>
                        </span>
                    </button></li>
                }
            })
            .collect_view()
    };

    view! {
        <div class="panel picker" role="dialog" aria-modal="true" aria-labelledby="picker-title">
            <header class="panel-head">
                <div class="panel-head-text">
                    <h2 id="picker-title">"Add to Playlist"</h2>
                    <p class="panel-sub">{if n == 1 { label } else { format!("{label} · {}", songs(n as i64)) }}</p>
                </div>
                <button class="icon-btn subtle" aria-label="Close" on:click=move |_| ov.close()><Svg icon=Icon::Close/></button>
            </header>
            {move || match confirm.get() {
                Some(p) => {
                    let p2 = p.clone();
                    view! {
                        <div class="panel-body confirm">
                            <h3>"Already Added"</h3>
                            <p>{if n == 1 {
                                format!("This song is already in “{}”.", p.name)
                            } else {
                                format!("These songs are already in “{}”.", p.name)
                            }}</p>
                            <div class="panel-actions">
                                <button class="btn" on:click=move |_| confirm.set(None)>"Don't Add"</button>
                                <button class="btn primary" disabled=move || busy.get()
                                    on:click=move |_| add(p2.clone(), true)>"Add Anyway"</button>
                            </div>
                        </div>
                    }.into_any()
                }
                None => view! {
                    <div class="panel-body">
                        {(c.editable().len() > 6).then(|| view! {
                            <label class="search-field small">
                                <Svg icon=Icon::Search/>
                                <input type="search" placeholder="Find a playlist" aria-label="Find a playlist"
                                    autocomplete="off" bind:value=filter/>
                            </label>
                        })}
                        <ul class="list pick-list">
                            <li><button class="pick-row new" on:click=move |_| {
                                ov.open(Overlay::NewPlaylist { track_ids: ids.get_value() })
                            }>
                                <span class="cover new-cover"><Svg icon=Icon::Plus/></span>
                                <span class="list-text"><span class="list-title">"New Playlist…"</span></span>
                            </button></li>
                            {list}
                        </ul>
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

/// Create (with optional songs) or edit a playlist's details and sharing.
#[component]
fn PlaylistForm(
    #[prop(optional)] track_ids: Vec<i64>,
    #[prop(optional)] editing: Option<PlaylistSummary>,
) -> impl IntoView {
    let ov = use_overlays();
    let c = use_collection();
    let navigate = use_navigate();
    let is_edit = editing.is_some();
    let original = StoredValue::new(editing.clone());
    let name = RwSignal::new(
        editing
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| c.next_name()),
    );
    let description = RwSignal::new(
        editing
            .as_ref()
            .map(|p| p.description.clone())
            .unwrap_or_default(),
    );
    let public = RwSignal::new(editing.as_ref().is_some_and(|p| p.public));
    let collaborative = RwSignal::new(editing.as_ref().is_some_and(|p| p.collaborative));
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let ids = StoredValue::new(track_ids);

    let submit = move || {
        if busy.get_untracked() || name.with_untracked(|n| n.trim().is_empty()) {
            return;
        }
        busy.set(true);
        error.set(None);
        let navigate = navigate.clone();
        spawn_local(async move {
            let (name, description, public, collaborative) = (
                name.get_untracked(),
                description.get_untracked(),
                public.get_untracked(),
                collaborative.get_untracked() && public.get_untracked(),
            );
            let res = match original.get_value() {
                Some(p) => {
                    let req = UpdatePlaylistRequest {
                        name: Some(name),
                        description: Some(description),
                        public: Some(public),
                        collaborative: Some(collaborative),
                    };
                    api::update_playlist(p.id, &req).await
                }
                None => {
                    let req = CreatePlaylistRequest {
                        name,
                        description: Some(description),
                        public,
                        collaborative,
                        track_ids: ids.get_value(),
                    };
                    api::create_playlist(&req).await
                }
            };
            busy.try_set(false);
            match res {
                Ok(p) => {
                    let (id, pname) = (p.id, p.name.clone());
                    c.upsert(p);
                    c.touch(id);
                    ov.close();
                    if is_edit {
                        return;
                    }
                    if ids.with_value(Vec::is_empty) {
                        navigate(&format!("/playlist/{id}"), NavigateOptions::default());
                    } else {
                        c.toast(format!("Added to {pname}"));
                    }
                }
                Err(e) if e.is_unauthorized() => c.error(e),
                Err(e) => {
                    error.try_set(Some(e.message));
                }
            }
        });
    };
    let title = if is_edit {
        "Edit Details"
    } else {
        "New Playlist"
    };
    view! {
        <form class="panel form" role="dialog" aria-modal="true" aria-labelledby="form-title"
            on:submit=move |ev| { ev.prevent_default(); submit(); }>
            <header class="panel-head">
                <button type="button" class="link-btn" on:click=move |_| ov.close()>"Cancel"</button>
                <h2 id="form-title">{title}</h2>
                <button type="submit" class="link-btn strong"
                    disabled=move || busy.get() || name.with(|n| n.trim().is_empty())>
                    {if is_edit { "Save" } else { "Create" }}
                </button>
            </header>
            <div class="panel-body">
                {move || original.with_value(|p| p.as_ref().map(|p| view! {
                    <div class="form-cover"><PlaylistCover covers=p.cover_album_ids.clone() size=300/></div>
                }))}
                <div class="group form-group">
                    <input class="form-input title" type="text" aria-label="Playlist name" placeholder="Playlist Name"
                        maxlength=PLAYLIST_NAME_MAX.to_string() autofocus autocomplete="off"
                        bind:value=name
                        on:focus=move |ev| {
                            if let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                el.select();
                            }
                        }/>
                    <textarea class="form-input" rows="3" aria-label="Description" placeholder="Description (optional)"
                        maxlength=PLAYLIST_DESCRIPTION_MAX.to_string()
                        prop:value=move || description.get()
                        on:input=move |ev| description.set(event_target_value(&ev))></textarea>
                </div>
                <div class="group">
                    <label class="cell">
                        <span class="cell-text">
                            <span>"Share with Everyone"</span>
                            <span class="cell-sub">"Everyone on this server can find, play and save it."</span>
                        </span>
                        <input type="checkbox" class="switch" role="switch" bind:checked=public/>
                    </label>
                    <label class="cell" class:dimmed=move || !public.get()>
                        <span class="cell-text">
                            <span>"Collaborative"</span>
                            <span class="cell-sub">"Others can add, remove and reorder songs."</span>
                        </span>
                        <input type="checkbox" class="switch" role="switch"
                            disabled=move || !public.get()
                            prop:checked=move || collaborative.get() && public.get()
                            on:change=move |ev| collaborative.set(event_target_checked(&ev))/>
                    </label>
                </div>
                {move || error.get().map(|e| view! { <p class="error" role="alert">{e}</p> })}
            </div>
        </form>
    }
}
