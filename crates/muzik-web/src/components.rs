use std::{cell::Cell, cell::RefCell, rc::Rc};

use js_sys::{Array, Object, Reflect};
use leptos::prelude::*;
use leptos_router::hooks::use_location;
use muzik_common::{Album, PlaylistSummary, Track};
use send_wrapper::SendWrapper;
use wasm_bindgen::{prelude::Closure, JsCast, JsValue};

use crate::{
    api,
    collection::use_collection,
    gestures::{Dir, Swipe},
    icons::{Icon, Svg},
    overlays::{use_overlays, Anchor, Overlay},
    player::{fmt_time, use_player, Repeat},
    use_auth, util,
};

// ---------- Toasts ----------

#[derive(Clone, Copy)]
pub struct Toasts {
    msg: RwSignal<Option<String>>,
    gen: StoredValue<u64>,
}

impl Toasts {
    pub fn new() -> Self {
        Self {
            msg: RwSignal::new(None),
            gen: StoredValue::new(0),
        }
    }

    pub fn show(&self, text: impl Into<String>) {
        let this = *self;
        this.gen.update_value(|g| *g += 1);
        let mine = this.gen.get_value();
        this.msg.set(Some(text.into()));
        set_timeout(
            move || {
                if this.gen.try_get_value() == Some(mine) {
                    this.msg.try_set(None);
                }
            },
            std::time::Duration::from_millis(1800),
        );
    }
}

pub fn use_toasts() -> Toasts {
    expect_context::<Toasts>()
}

#[component]
pub fn Toast() -> impl IntoView {
    let t = use_toasts();
    view! {
        <div class="toast glass" class:show=move || t.msg.with(Option::is_some) role="status" aria-live="polite">
            {move || t.msg.get().unwrap_or_default()}
        </div>
    }
}

// ---------- Navigation ----------

fn is_active(path: &str, href: &str) -> bool {
    if href == "/" {
        path == "/"
    } else {
        path.starts_with(href)
    }
}

#[component]
fn NavLink(href: &'static str, label: &'static str, icon: Icon) -> impl IntoView {
    let loc = use_location();
    view! {
        <li><a href=href class="nav-link" class:active=move || is_active(&loc.pathname.get(), href)>
            <Svg icon=icon/><span>{label}</span>
        </a></li>
    }
}

/// Desktop sidebar, laid out like a native source list, with the user's
/// playlists under the library.
#[component]
pub fn Sidebar() -> impl IntoView {
    let loc = use_location();
    let auth = use_auth();
    let c = use_collection();
    let ov = use_overlays();
    let name = move || auth.user().map(|u| u.display_name).unwrap_or_default();
    view! {
        <nav class="sidebar" aria-label="Main">
            <a href="/search" class="sidebar-search" class:active=move || is_active(&loc.pathname.get(), "/search")>
                <Svg icon=Icon::Search/><span>"Search"</span><kbd>"/"</kbd>
            </a>
            <ul class="nav">
                <NavLink href="/" label="Home" icon=Icon::Home/>
            </ul>
            <p class="nav-header">"Library"</p>
            <ul class="nav">
                <NavLink href="/albums" label="Albums" icon=Icon::Albums/>
                <NavLink href="/artists" label="Artists" icon=Icon::Artist/>
                <NavLink href="/queue" label="Queue" icon=Icon::Queue/>
            </ul>
            <div class="nav-header with-action">
                <a href="/library">"Playlists"</a>
                <button class="icon-btn subtle mini-btn" title="New Playlist" aria-label="New Playlist"
                    on:click=move |_| ov.open(Overlay::NewPlaylist { track_ids: Vec::new() })>
                    <Svg icon=Icon::Plus/>
                </button>
            </div>
            <ul class="nav sidebar-playlists">
                {move || c.playlists.get().into_iter().map(|p| {
                    let href = format!("/playlist/{}", p.id);
                    let href2 = href.clone();
                    view! {
                        <li><a href=href class="nav-link playlist-link"
                            class:active=move || loc.pathname.get() == href2>
                            <PlaylistCover covers=p.cover_album_ids.clone() size=96/>
                            <span class="playlist-link-name">{p.name.clone()}</span>
                            {(!p.is_owner).then(|| view! { <span class="playlist-link-icon" title="Shared with you"><Svg icon=Icon::People/></span> })}
                        </a></li>
                    }
                }).collect_view()}
            </ul>
            <a href="/settings" class="sidebar-user" class:active=move || is_active(&loc.pathname.get(), "/settings")>
                {move || view! { <Monogram name=name()/> }}
                <span class="sidebar-user-name">{name}</span>
                <Svg icon=Icon::Settings/>
            </a>
        </nav>
    }
}

/// Mobile tab bar: a floating glass capsule with a selection pill that glides
/// between tabs.
#[component]
pub fn TabBar() -> impl IntoView {
    let loc = use_location();
    let tabs: [(&str, &str, Icon); 4] = [
        ("/", "Home", Icon::Home),
        ("/search", "Search", Icon::Search),
        ("/library", "Library", Icon::Library),
        ("/settings", "Settings", Icon::Settings),
    ];
    // Pages reached from the Library tab keep it selected.
    const LIBRARY: [&str; 5] = ["/library", "/albums", "/artists", "/playlist/", "/queue"];
    let active = Memo::new(move |_| {
        let path = loc.pathname.get();
        if LIBRARY.iter().any(|p| path.starts_with(p)) {
            return Some(2);
        }
        tabs.iter().position(|(href, _, _)| is_active(&path, href))
    });
    // Where the pill rests: the active tab, or the last one when none is
    // (artist and album pages), so it fades out in place instead of jumping.
    let pill_at =
        Memo::new(move |prev: Option<&usize>| active.get().or(prev.copied()).unwrap_or(0));
    view! {
        <nav class="tabbar glass" aria-label="Main">
            <span class="tab-pill" class:off=move || active.get().is_none()
                style:transform=move || format!("translate3d({}%, 0, 0)", pill_at.get() * 100)></span>
            {tabs.into_iter().enumerate().map(|(i, (href, label, icon))| view! {
                <a href=href class="tab" class:active=move || active.get() == Some(i)
                    aria-current=move || (active.get() == Some(i)).then_some("page")>
                    <Svg icon=icon/><span>{label}</span>
                </a>
            }).collect_view()}
        </nav>
    }
}

// ---------- Artwork ----------

#[component]
pub fn Cover(album_id: i64, has_cover: bool, #[prop(default = 300)] size: u32) -> impl IntoView {
    if has_cover {
        view! {
            <img class="cover" src=api::cover_url(album_id, size) alt="" loading="lazy" decoding="async"/>
        }
        .into_any()
    } else {
        view! { <div class="cover placeholder"><Svg icon=Icon::Note/></div> }.into_any()
    }
}

/// Cover for a track whose album may have no art. Uses a CSS background so a
/// 404 simply leaves the placeholder visible (no broken-image icon).
#[component]
pub fn TrackCover(album_id: i64, #[prop(default = 300)] size: u32) -> impl IntoView {
    let bg = format!("url(\"{}\")", api::cover_url(album_id, size));
    view! {
        <div class="cover placeholder">
            <Svg icon=Icon::Note/>
            <div class="cover-bg" style:background-image=bg></div>
        </div>
    }
}

/// Initials on a neutral gradient disc (artists and users have no pictures yet).
#[component]
pub fn Monogram(name: String, #[prop(optional)] big: bool) -> impl IntoView {
    let initials: String = name
        .split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case("the"))
        .filter_map(|w| w.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    view! { <span class="monogram" class:big=big aria-hidden="true">{initials}</span> }
}

/// Playlist artwork: a 2×2 mosaic of its first four albums, a single cover
/// when it has fewer, or a placeholder when empty.
#[component]
pub fn PlaylistCover(covers: Vec<i64>, #[prop(default = 300)] size: u32) -> impl IntoView {
    let tile = if size >= 300 { 300 } else { 96 };
    match covers.len() {
        0 => view! { <div class="cover placeholder"><Svg icon=Icon::Playlist/></div> }.into_any(),
        1..=3 => view! { <TrackCover album_id=covers[0] size=size/> }.into_any(),
        _ => view! {
            <div class="cover mosaic">
                {covers.into_iter().take(4).map(|id| view! {
                    <div class="cover-bg" style:background-image=format!("url(\"{}\")", api::cover_url(id, tile))></div>
                }).collect_view()}
            </div>
        }
        .into_any(),
    }
}

pub fn songs_label(n: i64) -> String {
    format!("{n} song{}", if n == 1 { "" } else { "s" })
}

#[component]
pub fn PlaylistCard(playlist: PlaylistSummary) -> impl IntoView {
    let href = format!("/playlist/{}", playlist.id);
    let sub = if playlist.is_owner {
        songs_label(playlist.track_count)
    } else {
        format!("By {}", playlist.owner_name)
    };
    view! {
        <a class="card" href=href>
            <PlaylistCover covers=playlist.cover_album_ids.clone()/>
            <div class="card-title">{playlist.name.clone()}</div>
            <div class="card-sub">{sub}</div>
        </a>
    }
}

#[component]
pub fn AlbumCard(album: Album) -> impl IntoView {
    let href = format!("/album/{}", album.id);
    view! {
        <a class="card" href=href>
            <Cover album_id=album.id has_cover=album.has_cover/>
            <div class="card-title">{album.title.clone()}</div>
            <div class="card-sub">
                {album.artist_name.clone()}
                {album.year.map(|y| format!(" · {y}"))}
            </div>
        </a>
    }
}

// ---------- Tracks ----------

#[component]
pub fn TrackList(
    tracks: Vec<Track>,
    #[prop(default = false)] show_album: bool,
    #[prop(default = false)] numbered: bool,
    /// What gets queued when a row is clicked (defaults to `tracks`); `offset` is the
    /// position of the first displayed track in it.
    #[prop(optional)]
    queue: Option<Vec<Track>>,
    #[prop(default = 0)] offset: usize,
) -> impl IntoView {
    let all = StoredValue::new(queue.unwrap_or_else(|| tracks.clone()));
    tracks
        .into_iter()
        .enumerate()
        .map(|(i, t)| view! { <TrackRow track=t index=offset + i all=all show_album numbered/> })
        .collect_view()
}

/// Three bouncing bars shown in place of the track number of the current track.
#[component]
pub fn NowPlayingBars() -> impl IntoView {
    let player = use_player();
    view! {
        <span class="eq" class:paused=move || !player.playing.get() aria-label="Now playing">
            <i></i><i></i><i></i>
        </span>
    }
}

/// A song row: tap to play, swipe right to queue, ⋯ (or right-click) for
/// more actions.
#[component]
pub fn TrackRow(
    track: Track,
    index: usize,
    all: StoredValue<Vec<Track>>,
    show_album: bool,
    numbered: bool,
    /// Replaces the default "play `all` from `index`" action.
    #[prop(optional)]
    on_play: Option<Callback<()>>,
    /// Offers "Remove from This Playlist" in the menu.
    #[prop(optional)]
    on_remove: Option<Callback<()>>,
    /// Extra trailing controls (playlist editing).
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let player = use_player();
    let toasts = use_toasts();
    let ov = use_overlays();
    let swipe = Swipe::new(true, false);
    let id = track.id;
    let is_current = move || player.current().is_some_and(|c| c.id == id);
    let queued = track.clone();
    let enqueue = move || {
        player.enqueue(queued.clone());
        toasts.show("Added to Queue");
    };
    let play = move || match on_play {
        Some(cb) => cb.run(()),
        None => player.play_list(all.get_value(), index),
    };
    let menu_track = track.clone();
    let open_menu = move |at: Option<Anchor>| {
        ov.open(Overlay::TrackMenu {
            track: menu_track.clone(),
            at,
            remove: on_remove,
        })
    };
    let open_menu2 = open_menu.clone();
    let badge = util::quality_label(&track);
    let num = if numbered {
        track.track_no.map(|n| n.to_string()).unwrap_or_default()
    } else {
        (index + 1).to_string()
    };
    let sub = if show_album {
        format!("{} · {}", track.artist_name, track.album_title)
    } else {
        track.artist_name.clone()
    };

    view! {
        <div class="track-row-wrap">
            <div class="swipe-hint"><Svg icon=Icon::Queue/><span>"Add to Queue"</span></div>
            <div
                class="track-row"
                class:with-art=show_album
                class:current=is_current
                class:dragging=move || swipe.dragging.get()
                style:transform=move || swipe.transform()
                on:touchstart=move |ev| swipe.start(&ev)
                on:touchmove=move |ev| swipe.moved(&ev)
                on:touchend=move |ev| {
                    if swipe.end(&ev) == Some(Dir::Right) {
                        enqueue();
                    }
                }
                on:touchcancel=move |ev| { swipe.end(&ev); }
                on:contextmenu=move |ev| {
                    ev.prevent_default();
                    open_menu(Anchor::pointer(&ev));
                }
            >
                <button class="track-main" on:click=move |_| play() aria-label=format!("Play {}", track.title)>
                    {if show_album {
                        view! {
                            <span class="track-art">
                                <TrackCover album_id=track.album_id size=96/>
                                {move || is_current().then(|| view! { <span class="track-art-eq"><NowPlayingBars/></span> })}
                            </span>
                        }.into_any()
                    } else {
                        view! {
                            <span class="track-num">
                                {move || if is_current() {
                                    view! { <NowPlayingBars/> }.into_any()
                                } else {
                                    view! { <span>{num.clone()}</span> }.into_any()
                                }}
                            </span>
                        }.into_any()
                    }}
                    <span class="track-text">
                        <span class="track-title">{track.title.clone()}</span>
                        <span class="track-sub">
                            {badge.map(|b| view! { <span class="badge">{b}</span> })}
                            {sub}
                        </span>
                    </span>
                    <span class="track-dur">{fmt_time(track.duration_ms as f64 / 1000.0)}</span>
                </button>
                <button class="icon-btn subtle row-more" title="More" aria-label=format!("More actions for {}", track.title)
                    on:click=move |ev| open_menu2(Anchor::below(&ev))>
                    <Svg icon=Icon::More/>
                </button>
                {children.map(|c| c())}
            </div>
        </div>
    }
}

// ---------- Player ----------

fn progress_pct(player: crate::player::Player) -> f64 {
    let d = player.duration.get();
    if d > 0.0 {
        (player.position.get() / d * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Seek bar. `stacked` puts the times under the bar (now-playing sheet), with
/// the remaining time on the right and the quality badge in the middle.
#[component]
fn Progress(#[prop(optional)] stacked: bool) -> impl IntoView {
    let player = use_player();
    let bar = view! {
        <div class="bar">
            <div class="bar-track"><div class="bar-fill" style:width=move || format!("{:.3}%", progress_pct(player))></div></div>
            <input type="range" min="0" step="0.1" aria-label="Seek"
                prop:max=move || player.duration.get().max(0.0)
                prop:value=move || player.position.get()
                on:input=move |ev| {
                    if let Ok(v) = event_target_value(&ev).parse::<f64>() {
                        player.seek(v);
                    }
                }/>
        </div>
    };
    if stacked {
        view! {
            <div class="progress stacked">
                {bar}
                <div class="times">
                    <span class="time">{move || fmt_time(player.position.get())}</span>
                    <span class="times-mid">
                        {move || player.current().and_then(|t| util::quality_label(&t)).map(|b| view! { <span class="badge">{b}</span> })}
                    </span>
                    <span class="time">{move || format!("-{}", fmt_time((player.duration.get() - player.position.get()).max(0.0)))}</span>
                </div>
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class="progress">
                <span class="time">{move || fmt_time(player.position.get())}</span>
                {bar}
                <span class="time">{move || fmt_time(player.duration.get())}</span>
            </div>
        }
        .into_any()
    }
}

/// Play/pause. The default is a filled circle; `plain` is a bare glyph.
#[component]
fn PlayButton(#[prop(optional)] plain: bool, #[prop(optional)] big: bool) -> impl IntoView {
    let player = use_player();
    view! {
        <button class=if plain { "glyph-btn" } else { "play-btn" } class:big=big
            aria-label=move || if player.playing.get() { "Pause" } else { "Play" }
            on:click=move |ev| { ev.stop_propagation(); player.toggle(); }>
            {move || if player.playing.get() {
                view! { <Svg icon=Icon::Pause/> }
            } else {
                view! { <Svg icon=Icon::Play/> }
            }}
        </button>
    }
}

/// Shuffle toggle (highlighted with a dot while on).
#[component]
fn ShuffleButton() -> impl IntoView {
    let player = use_player();
    let on = move || player.settings.with(|s| s.shuffle);
    view! {
        <button class="glyph-btn mode-btn" class:on=on
            aria-pressed=move || on().to_string()
            aria-label="Shuffle" title=move || if on() { "Shuffle: On" } else { "Shuffle: Off" }
            on:click=move |ev| { ev.stop_propagation(); player.toggle_shuffle(); }>
            <Svg icon=Icon::Shuffle/>
        </button>
    }
}

/// Repeat: Off → All → One (shows a "1" for One).
#[component]
fn RepeatButton() -> impl IntoView {
    let player = use_player();
    let mode = move || player.settings.with(|s| s.repeat);
    let label = move || match mode() {
        Repeat::Off => "Repeat: Off",
        Repeat::All => "Repeat: All",
        Repeat::One => "Repeat: One",
    };
    view! {
        <button class="glyph-btn mode-btn" class:on=move || mode() != Repeat::Off
            aria-label=label title=label
            on:click=move |ev| { ev.stop_propagation(); player.cycle_repeat(); }>
            {move || if mode() == Repeat::One {
                view! { <Svg icon=Icon::RepeatOne/> }
            } else {
                view! { <Svg icon=Icon::Repeat/> }
            }}
        </button>
    }
}

/// Desktop player bar.
#[component]
pub fn PlayerBar() -> impl IntoView {
    let player = use_player();
    view! {
        <footer class="playerbar glass" class:idle=move || player.index.get().is_none()>
            <div class="pb-track">
                {move || match player.current() {
                    Some(t) => view! {
                        <button class="pb-art" on:click=move |_| player.sheet_open.set(true) aria-label="Open Now Playing">
                            <TrackCover album_id=t.album_id size=96/>
                        </button>
                        <div class="pb-text">
                            <a class="pb-title" href=format!("/album/{}", t.album_id)><Marquee text=t.title.clone()/></a>
                            <a class="pb-artist" href=format!("/artist/{}", t.album_artist_id)><Marquee text=t.artist_name.clone()/></a>
                        </div>
                    }.into_any(),
                    None => view! {
                        <div class="pb-art"><div class="cover placeholder"><Svg icon=Icon::Note/></div></div>
                        <div class="pb-text"><span class="pb-artist">"Not Playing"</span></div>
                    }.into_any(),
                }}
            </div>
            <div class="pb-center">
                <div class="pb-controls">
                    <ShuffleButton/>
                    <button class="glyph-btn" aria-label="Previous" on:click=move |_| player.prev()><Svg icon=Icon::Prev/></button>
                    <PlayButton/>
                    <button class="glyph-btn" aria-label="Next" on:click=move |_| player.next()><Svg icon=Icon::Next/></button>
                    <RepeatButton/>
                </div>
                <Progress/>
            </div>
            <div class="pb-right">
                {move || player.current().and_then(|t| util::quality_label(&t)).map(|b| view! { <span class="badge">{b}</span> })}
                <a class="icon-btn subtle" href="/queue" aria-label="Queue" title="Queue"><Svg icon=Icon::Queue/></a>
                <Volume/>
            </div>
        </footer>
    }
}

/// Desktop volume: mute toggle + slider (↑/↓ and `m` from the keyboard).
#[component]
fn Volume() -> impl IntoView {
    let player = use_player();
    let level = move || {
        if player.muted.get() {
            0.0
        } else {
            player.volume.get()
        }
    };
    view! {
        <div class="volume">
            <button class="icon-btn subtle"
                aria-label=move || if level() == 0.0 { "Unmute" } else { "Mute" }
                title=move || if level() == 0.0 { "Unmute" } else { "Mute" }
                on:click=move |_| player.toggle_mute()>
                {move || {
                    let v = level();
                    let icon = if v == 0.0 { Icon::VolumeOff } else if v < 0.5 { Icon::VolumeLow } else { Icon::VolumeHigh };
                    view! { <Svg icon=icon/> }
                }}
            </button>
            <div class="bar volume-bar"
                on:wheel=move |ev: leptos::ev::WheelEvent| {
                    ev.prevent_default();
                    player.volume_by(if ev.delta_y() > 0.0 { -0.05 } else { 0.05 });
                }>
                <div class="bar-track"><div class="bar-fill" style:width=move || format!("{:.1}%", level() * 100.0)></div></div>
                <input type="range" min="0" max="1" step="0.01" aria-label="Volume"
                    prop:value=level
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<f64>() {
                            player.set_volume(v);
                        }
                    }/>
            </div>
        </div>
    }
}

/// Mobile mini-player: swipe left/right for next/previous, up to expand.
#[component]
pub fn MiniPlayer() -> impl IntoView {
    let player = use_player();
    let swipe = Swipe::new(true, true);
    view! {
        <div class="mini glass" class:idle=move || player.index.get().is_none()
            class:dragging=move || swipe.dragging.get()
            style:transform=move || swipe.transform()
            on:touchstart=move |ev| swipe.start(&ev)
            on:touchmove=move |ev| swipe.moved(&ev)
            on:touchend=move |ev| match swipe.end(&ev) {
                Some(Dir::Left) => player.next(),
                Some(Dir::Right) => player.prev(),
                Some(Dir::Up) => player.sheet_open.set(true),
                _ => {}
            }
            on:touchcancel=move |ev| { swipe.end(&ev); }
        >
            <button class="mini-open" aria-label="Open Now Playing" on:click=move |_| player.sheet_open.set(true)>
                {move || player.current().map(|t| view! {
                    <TrackCover album_id=t.album_id size=96/>
                    <span class="mini-text">
                        <span class="mini-title"><Marquee text=t.title.clone()/></span>
                        <span class="mini-artist">{t.artist_name.clone()}</span>
                    </span>
                })}
            </button>
            <PlayButton plain=true/>
            <button class="glyph-btn" aria-label="Next" on:click=move |ev| { ev.stop_propagation(); player.next(); }>
                <Svg icon=Icon::Next/>
            </button>
            <div class="mini-progress"><div style:width=move || format!("{:.3}%", progress_pct(player))></div></div>
        </div>
    }
}

/// Full-screen "now playing" sheet. The player fills the first screen, with
/// the "Up Next" card (header + first song) peeking at the bottom; scrolling (or swiping up on the artwork)
/// reveals the rest of the queue and suggestions. Swipe down anywhere from the
/// top of the screen to the bottom of the artwork to close; swipe the artwork
/// sideways for next/previous. The artwork shrinks back while paused.
#[component]
pub fn NowPlaying() -> impl IntoView {
    let player = use_player();
    let sheet = Swipe::new(false, true);
    let art = Swipe::new(true, false);
    let ov = use_overlays();
    let scroller = NodeRef::<leptos::html::Div>::new();
    let upnext = NodeRef::<leptos::html::Div>::new();
    let close = move || player.sheet_open.set(false);
    // Always open on the player, not where the list was left.
    Effect::new(move |_| {
        if player.sheet_open.get() {
            if let Some(el) = scroller.get_untracked() {
                el.set_scroll_top(0);
            }
        }
    });
    let reveal_list = move || {
        if let (Some(el), Some(list)) = (scroller.get_untracked(), upnext.get_untracked()) {
            let opts = web_sys::ScrollToOptions::new();
            opts.set_top(f64::from(list.offset_top()) - 8.0);
            opts.set_behavior(if reduced_motion() {
                web_sys::ScrollBehavior::Instant
            } else {
                web_sys::ScrollBehavior::Smooth
            });
            el.scroll_to_with_scroll_to_options(&opts);
        }
    };
    view! {
        <section class="sheet" class:open=move || player.sheet_open.get()
            class:paused=move || !player.playing.get()
            class:dragging=move || sheet.dragging.get()
            style:transform=move || if player.sheet_open.get() { sheet.sheet_transform() } else { String::new() }
            aria-hidden=move || (!player.sheet_open.get()).to_string()
            aria-label="Now Playing"
        >
            <div class="sheet-bg">
                {move || player.current().map(|t| view! {
                    <div class="cover-bg" style:background-image=format!("url(\"{}\")", api::cover_url(t.album_id, 96))></div>
                })}
            </div>
            <div class="sheet-scroll" node_ref=scroller>
                <div class="sheet-player">
                    // Everything from the top edge down to the artwork is a drag handle:
                    // down closes, up scrolls to the list. Touches on the artwork reach
                    // both trackers; each locks to its own axis.
                    <div class="sheet-drag"
                        on:touchstart=move |ev| sheet.start(&ev)
                        on:touchmove=move |ev| sheet.moved(&ev)
                        on:touchend=move |ev| match sheet.end(&ev) {
                            Some(Dir::Down) => close(),
                            Some(Dir::Up) => reveal_list(),
                            _ => {}
                        }
                        on:touchcancel=move |ev| { sheet.end(&ev); }>
                        <header class="sheet-head">
                            <button class="icon-btn" aria-label="Close Now Playing" title="Close" on:click=move |_| close()>
                                <Svg icon=Icon::ChevronDown/>
                            </button>
                            <div class="sheet-context">
                                {move || match player.source.get() {
                                    Some(s) => view! {
                                        <span class="sheet-context-kicker">"Playing from"</span>
                                        <span class="sheet-context-name">{s}</span>
                                    }.into_any(),
                                    None => view! { <span class="sheet-context-name">"Now Playing"</span> }.into_any(),
                                }}
                            </div>
                            <button class="icon-btn" aria-label="More" title="More"
                                disabled=move || player.current().is_none()
                                on:click=move |ev| if let Some(t) = player.current() {
                                    ov.open(Overlay::TrackMenu { track: t, at: Anchor::below(&ev), remove: None });
                                }>
                                <Svg icon=Icon::More/>
                            </button>
                        </header>
                        {move || player.current().map(|t| view! {
                            <div class="sheet-art"
                                class:dragging=move || art.dragging.get()
                                style:transform=move || art.transform()
                                on:touchstart=move |ev| art.start(&ev)
                                on:touchmove=move |ev| art.moved(&ev)
                                on:touchend=move |ev| match art.end(&ev) {
                                    Some(Dir::Left) => player.next(),
                                    Some(Dir::Right) => player.prev(),
                                    _ => {}
                                }
                                on:touchcancel=move |ev| { art.end(&ev); }>
                                <TrackCover album_id=t.album_id size=600/>
                            </div>
                        })}
                    </div>
                    {move || player.current().map(|t| {
                        let fmt = format!(
                            "{}{}",
                            t.codec.to_uppercase(),
                            t.bitrate_kbps.filter(|_| !t.lossless).map(|b| format!(" · {b} kbps")).unwrap_or_default()
                        );
                        view! {
                            <div class="sheet-meta">
                                <div class="sheet-meta-text">
                                    <h2><Marquee text=t.title.clone()/></h2>
                                    <a href=format!("/artist/{}", t.album_artist_id) on:click=move |_| close()>
                                        <Marquee text=t.artist_name.clone()/>
                                    </a>
                                    <span class="sheet-format">{fmt}</span>
                                </div>
                                    </div>
                        }
                    })}
                    <Progress stacked=true/>
                    <div class="sheet-controls">
                        <ShuffleButton/>
                        <button class="glyph-btn big" aria-label="Previous" on:click=move |_| player.prev()><Svg icon=Icon::Prev/></button>
                        <PlayButton plain=true big=true/>
                        <button class="glyph-btn big" aria-label="Next" on:click=move |_| player.next()><Svg icon=Icon::Next/></button>
                        <RepeatButton/>
                    </div>
                    {move || player.error.get().map(|e| view! { <p class="sheet-error">{e}</p> })}
                </div>
                <div class="sheet-lists" node_ref=upnext>
                    <UpNext on_navigate=Callback::new(move |()| close())/>
                    <Suggestions/>
                </div>
            </div>
        </section>
    }
}

/// Most queued tracks listed in the sheet; the rest are on the Queue page.
const UP_NEXT_MAX: usize = 30;
/// Suggestions shown at once (the server sends extra to cover queued ones).
const SUGGESTIONS_SHOWN: usize = 8;

/// A compact row in the now-playing sheet: art, title, artist, then `trail`.
#[component]
fn SheetRow(
    track: Track,
    #[prop(into)] label: String,
    on_click: Callback<()>,
    trail: Icon,
) -> impl IntoView {
    view! {
        <button class="sheet-row" aria-label=label on:click=move |_| on_click.run(())>
            <span class="track-art"><TrackCover album_id=track.album_id size=96/></span>
            <span class="track-text">
                <span class="track-title">{track.title.clone()}</span>
                <span class="track-sub">{track.artist_name.clone()}</span>
            </span>
            <span class="sheet-row-trail"><Svg icon=trail/></span>
        </button>
    }
}

/// The rest of the queue: the album or playlist being played, plus anything
/// added after it. Tapping a song jumps to it.
#[component]
fn UpNext(on_navigate: Callback<()>) -> impl IntoView {
    let player = use_player();
    let rest = Memo::new(move |_| {
        let cur = player.index.get();
        player.queue.with(|q| match cur {
            Some(i) => q
                .iter()
                .cloned()
                .enumerate()
                .skip(i + 1)
                .collect::<Vec<_>>(),
            None => Vec::new(),
        })
    });
    view! {
        <section class="sheet-card" aria-label="Up Next">
            <header class="sheet-card-head">
                <h3><Svg icon=Icon::Queue/>"Up Next"</h3>
                {move || (rest.with(Vec::len) > UP_NEXT_MAX).then(|| view! {
                    <a class="sheet-link" href="/queue" on:click=move |_| on_navigate.run(())>
                        {move || format!("Show All {}", rest.with(Vec::len))}
                    </a>
                })}
            </header>
            {move || {
                let items = rest.get();
                if items.is_empty() {
                    return view! { <p class="sheet-empty">"Nothing else is queued."</p> }.into_any();
                }
                view! {
                    <div class="sheet-rows">
                        {items.into_iter().take(UP_NEXT_MAX).map(|(i, t)| {
                            let label = format!("Play {}", t.title);
                            view! {
                                <SheetRow track=t label trail=Icon::Play
                                    on_click=Callback::new(move |()| player.jump(i))/>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </section>
    }
}

/// Songs from the library similar to the current one (genre, artist, era),
/// not already queued. Tapping one adds it to the end of the queue.
#[component]
fn Suggestions() -> impl IntoView {
    let player = use_player();
    let toasts = use_toasts();
    let refresh = RwSignal::new(0u32);
    let seed = Memo::new(move |_| player.current().map(|t| t.id));
    // Only fetched while the sheet is open, once per song (or refresh).
    let data = util::fetch(move || {
        let id = seed.get().filter(|_| player.sheet_open.get());
        let _ = refresh.get();
        async move {
            match id {
                Some(id) => api::suggestions(id, 24).await,
                None => Ok(Vec::new()),
            }
        }
    });
    let shown = Memo::new(move |_| {
        let Some(Ok(ts)) = data.get() else {
            return Vec::new();
        };
        player.queue.with(|q| {
            ts.into_iter()
                .filter(|t| !q.iter().any(|x| x.id == t.id))
                .take(SUGGESTIONS_SHOWN)
                .collect::<Vec<_>>()
        })
    });
    view! {
        <section class="sheet-card" aria-label="Suggestions">
            <header class="sheet-card-head">
                <h3><Svg icon=Icon::Note/>"Suggestions"</h3>
                <button class="icon-btn" aria-label="New Suggestions" title="New Suggestions"
                    on:click=move |_| refresh.update(|n| *n += 1)>
                    <Svg icon=Icon::Shuffle/>
                </button>
            </header>
            {move || {
                let items = shown.get();
                if items.is_empty() {
                    let msg = match data.get() {
                        None => "",
                        Some(Err(_)) => "Couldn't load suggestions.",
                        Some(Ok(_)) => "No suggestions right now.",
                    };
                    return view! { <p class="sheet-empty">{msg}</p> }.into_any();
                }
                view! {
                    <div class="sheet-rows">
                        {items.into_iter().map(|t| {
                            let label = format!("Add {} to the queue", t.title);
                            let queued = t.clone();
                            view! {
                                <SheetRow track=t label trail=Icon::Plus
                                    on_click=Callback::new(move |()| {
                                        player.enqueue(queued.clone());
                                        toasts.show("Added to Queue");
                                    })/>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
        </section>
    }
}

// ---------- Marquee ----------

/// Space between the text and its repeated copy while scrolling, in px (the
/// `.marquee-copy` padding in CSS must match).
const MARQUEE_GAP: f64 = 40.0;
const MARQUEE_REST_MS: f64 = 3000.0;
const MARQUEE_SPEED: f64 = 32.0; // px per second

struct MarqueeState {
    observer: web_sys::ResizeObserver,
    _cb: Closure<dyn Fn()>,
    anim: Rc<RefCell<Option<JsValue>>>,
}

fn reduced_motion() -> bool {
    web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .is_some_and(|m| m.matches())
}

fn cancel_anim(anim: &RefCell<Option<JsValue>>) {
    if let Some(a) = anim.borrow_mut().take() {
        if let Ok(f) =
            Reflect::get(&a, &"cancel".into()).and_then(|f| f.dyn_into::<js_sys::Function>())
        {
            let _ = f.call0(&a);
        }
    }
}

fn keyframe(x: f64, offset: f64, easing: Option<&str>) -> JsValue {
    let k = Object::new();
    let _ = Reflect::set(
        &k,
        &"transform".into(),
        &format!("translate3d({x:.1}px, 0, 0)").into(),
    );
    let _ = Reflect::set(&k, &"offset".into(), &offset.into());
    if let Some(e) = easing {
        let _ = Reflect::set(&k, &"easing".into(), &e.into());
    }
    k.into()
}

/// Single-line text. When it doesn't fit, it rests for 3 s, then scrolls
/// smoothly left (followed by a copy of itself) back to the start, and loops.
/// Plain ellipsis when it fits or when reduced motion is requested.
#[component]
pub fn Marquee(#[prop(into)] text: String) -> impl IntoView {
    let outer = NodeRef::<leptos::html::Span>::new();
    let track = NodeRef::<leptos::html::Span>::new();
    let first = NodeRef::<leptos::html::Span>::new();
    let scrolling = RwSignal::new(false);
    let state: StoredValue<Option<SendWrapper<MarqueeState>>> = StoredValue::new(None);

    Effect::new(move |_| {
        let (Some(o), Some(t), Some(f)) = (outer.get(), track.get(), first.get()) else {
            return;
        };
        if state.with_value(Option::is_some) {
            return;
        }
        let anim = Rc::new(RefCell::new(None::<JsValue>));
        let last = Rc::new(Cell::new(0.0_f64));
        let anim2 = anim.clone();
        let o2: web_sys::HtmlElement = o.clone().into();
        let measure = move || {
            let avail = f64::from(o2.client_width());
            let text_w = f64::from(f.offset_width());
            let overflow = avail > 0.0 && text_w > avail + 1.0 && !reduced_motion();
            if !overflow {
                cancel_anim(&anim2);
                last.set(0.0);
                scrolling.try_set(false);
                return;
            }
            let dist = (text_w + MARQUEE_GAP).round();
            if anim2.borrow().is_some() && last.get() == dist {
                return;
            }
            cancel_anim(&anim2);
            last.set(dist);
            scrolling.try_set(true);
            let travel = (dist / MARQUEE_SPEED * 1000.0).max(1500.0);
            let total = MARQUEE_REST_MS + travel;
            let frames = Array::of3(
                &keyframe(0.0, 0.0, None),
                &keyframe(
                    0.0,
                    MARQUEE_REST_MS / total,
                    Some("cubic-bezier(0.4, 0, 0.6, 1)"),
                ),
                &keyframe(-dist, 1.0, None),
            );
            let opts = Object::new();
            let _ = Reflect::set(&opts, &"duration".into(), &total.into());
            let _ = Reflect::set(&opts, &"iterations".into(), &f64::INFINITY.into());
            let animate = Reflect::get(&t, &"animate".into())
                .ok()
                .and_then(|f| f.dyn_into::<js_sys::Function>().ok());
            if let Some(animate) = animate {
                if let Ok(a) = animate.call2(&t, &frames, &opts) {
                    *anim2.borrow_mut() = Some(a);
                }
            }
        };
        // ResizeObserver reports once on observe, then on every size change
        // (window resize, mini-player shown on rotation, ...).
        let cb = Closure::<dyn Fn()>::new(measure);
        let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&o);
        state.set_value(Some(SendWrapper::new(MarqueeState {
            observer,
            _cb: cb,
            anim,
        })));
    });
    on_cleanup(move || {
        if let Some(Some(s)) = state.try_update_value(Option::take) {
            s.observer.disconnect();
            cancel_anim(&s.anim);
        }
    });

    view! {
        <span class="marquee" class:scrolling=move || scrolling.get() node_ref=outer>
            <span class="marquee-track" node_ref=track>
                <span node_ref=first>{text.clone()}</span>
                <span class="marquee-copy" aria-hidden="true">{text}</span>
            </span>
        </span>
    }
}

// ---------- Misc ----------

/// Activity indicator: eight spokes rotating in steps.
#[component]
pub fn Spinner() -> impl IntoView {
    view! {
        <svg class="spinner" viewBox="0 0 24 24" role="progressbar" aria-label="Loading">
            {(0..8).map(|i| view! {
                <rect x="11" y="1.5" width="2" height="6" rx="1" fill="currentColor"
                    transform=format!("rotate({} 12 12)", i * 45)
                    opacity=format!("{:.2}", 0.25 + 0.75 * f64::from(i) / 7.0)></rect>
            }).collect_view()}
        </svg>
    }
}

#[component]
pub fn Loading() -> impl IntoView {
    view! { <div class="loading"><Spinner/></div> }
}

#[component]
pub fn ErrorBox(error: api::ApiError) -> impl IntoView {
    view! { <div class="error-box" role="alert">{error.message}</div> }
}
