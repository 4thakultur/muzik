//! Library hub and playlist pages.

use leptos::{prelude::*, task::spawn_local};
use leptos_router::hooks::use_params_map;
use muzik_common::{PlaylistEntry, PlaylistSummary, Track};
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;

use crate::{
    api,
    collection::use_collection,
    components::{songs_label, ErrorBox, Loading, PlaylistCover, TrackRow},
    icons::{Icon, Svg},
    overlays::{follow, use_overlays, Anchor, Overlay, Picker},
    player::{fmt_time, use_player},
    util,
};

// ---------- Library hub ----------

#[derive(Clone, Copy, PartialEq)]
enum LibSort {
    Recent,
    Name,
}

/// The Library tab: Albums, Artists, Queue, then every playlist.
#[component]
pub fn LibraryHub() -> impl IntoView {
    let c = use_collection();
    let ov = use_overlays();
    let sort = RwSignal::new(LibSort::Recent);
    let sorted = move || {
        let mut list = c.playlists.get();
        if sort.get() == LibSort::Name {
            list.sort_by_key(|p| p.name.to_lowercase());
        }
        list
    };

    view! {
        <div class="page narrow">
            <h1 class="large-title">"Library"</h1>
            <ul class="list hub">
                <HubLink href="/albums" label="Albums" icon=Icon::Albums/>
                <HubLink href="/artists" label="Artists" icon=Icon::Artist/>
                <HubLink href="/queue" label="Queue" icon=Icon::Queue/>
            </ul>
            <div class="section-head">
                <h2>"Playlists"</h2>
                <div class="segmented" role="group" aria-label="Sort playlists">
                    {[(LibSort::Recent, "Recents"), (LibSort::Name, "A–Z")].into_iter().map(|(k, label)| view! {
                        <button class:active=move || sort.get() == k aria-pressed=move || (sort.get() == k).to_string()
                            on:click=move |_| sort.set(k)>{label}</button>
                    }).collect_view()}
                </div>
            </div>
            <ul class="list">
                <li><button class="list-row new-row" on:click=move |_| ov.open(Overlay::NewPlaylist { track_ids: Vec::new() })>
                    <span class="row-art"><span class="cover new-cover"><Svg icon=Icon::Plus/></span></span>
                    <span class="list-text"><span class="list-title">"New Playlist…"</span></span>
                </button></li>
                {move || sorted().into_iter().map(|p| view! { <PlaylistRow playlist=p/> }).collect_view()}
            </ul>
            {move || c.playlists.with(Vec::is_empty).then(|| view! {
                <p class="footnote hub-note">"Playlists you create or save from others appear here."</p>
            })}
        </div>
    }
}

#[component]
fn HubLink(href: &'static str, label: &'static str, icon: Icon) -> impl IntoView {
    view! {
        <li><a class="list-row" href=href>
            <span class="row-icon"><Svg icon=icon/></span>
            <span class="list-text"><span class="list-title">{label}</span></span>
            <span class="chevron"><Svg icon=Icon::ChevronRight/></span>
        </a></li>
    }
}

#[component]
pub fn PlaylistRow(playlist: PlaylistSummary) -> impl IntoView {
    let p = playlist;
    let mut sub = vec![if p.is_owner {
        "Playlist".to_owned()
    } else {
        p.owner_name.clone()
    }];
    if p.collaborative {
        sub.push("Collaborative".into());
    } else if p.public && p.is_owner {
        sub.push("Shared".into());
    }
    sub.push(songs_label(p.track_count));
    view! {
        <li><a class="list-row" href=format!("/playlist/{}", p.id)>
            <span class="row-art"><PlaylistCover covers=p.cover_album_ids.clone() size=96/></span>
            <span class="list-text">
                <span class="list-title">{p.name.clone()}</span>
                <span class="list-sub">{sub.join(" · ")}</span>
            </span>
            <span class="chevron"><Svg icon=Icon::ChevronRight/></span>
        </a></li>
    }
}

// ---------- Playlist ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Custom,
    Title,
    Artist,
    Album,
    Added,
}

impl Sort {
    const ALL: [(Sort, &'static str); 5] = [
        (Sort::Custom, "Custom Order"),
        (Sort::Title, "Title"),
        (Sort::Artist, "Artist"),
        (Sort::Album, "Album"),
        (Sort::Added, "Recently Added"),
    ];
}

const SORT_LABELS: [&str; 5] = [
    Sort::ALL[0].1,
    Sort::ALL[1].1,
    Sort::ALL[2].1,
    Sort::ALL[3].1,
    Sort::ALL[4].1,
];

/// An entry being dragged to a new position.
#[derive(Clone, Copy, PartialEq)]
struct Drag {
    entry_id: i64,
    from: usize,
    to: usize,
    start_y: f64,
    pointer_y: f64,
    start_scroll: f64,
    row_h: f64,
}

impl Drag {
    fn dy(&self) -> f64 {
        self.pointer_y - self.start_y + (main_scroll_top() - self.start_scroll)
    }

    fn target(&self, n: usize) -> usize {
        let steps = (self.dy() / self.row_h).round();
        (self.from as f64 + steps).clamp(0.0, n.saturating_sub(1) as f64) as usize
    }
}

fn main_el() -> Option<web_sys::HtmlElement> {
    document()
        .get_element_by_id("main")
        .and_then(|e| e.dyn_into().ok())
}

fn main_scroll_top() -> f64 {
    main_el().map_or(0.0, |m| f64::from(m.scroll_top()))
}

fn plain(s: &str) -> String {
    s.to_lowercase()
}

#[component]
pub fn PlaylistPage() -> impl IntoView {
    let params = use_params_map();
    let id = move || {
        params
            .with(|p| p.get("id"))
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let c = use_collection();
    let ov = use_overlays();
    let player = use_player();

    // Refetch when this playlist changes elsewhere (picker, menu, edit form).
    let rev = Memo::new(move |prev: Option<&u64>| {
        let (pid, n) = c.touched.get();
        if pid == id() {
            n
        } else {
            prev.copied().unwrap_or(0)
        }
    });
    let data = util::fetch(move || {
        let id = id();
        let _ = rev.get();
        async move { api::playlist(id).await }
    });

    let info = RwSignal::new(None::<PlaylistSummary>);
    let entries = RwSignal::new(Vec::<PlaylistEntry>::new());
    let error = RwSignal::new(None::<api::ApiError>);
    Effect::new(move |_| match data.get() {
        Some(Ok(d)) => {
            info.set(Some(d.playlist));
            entries.set(d.entries);
            error.set(None);
        }
        Some(Err(e)) => error.set(Some(e)),
        None => {}
    });
    // A different playlist: forget the previous one while loading.
    Effect::new(move |prev: Option<i64>| {
        let id = id();
        if prev.is_some_and(|p| p != id) {
            info.set(None);
            entries.set(Vec::new());
        }
        id
    });

    let sort = RwSignal::new(Sort::Custom);
    let filter = RwSignal::new(String::new());
    let editing = RwSignal::new(false);
    let drag = RwSignal::new(None::<Drag>);

    let can_edit = move || info.with(|p| p.as_ref().is_some_and(|p| p.can_edit));
    let can_reorder =
        move || can_edit() && sort.get() == Sort::Custom && filter.with(String::is_empty);

    // Entries in display order (sorted / filtered), with their index in `entries`.
    let shown = Memo::new(move |_| {
        let q = plain(filter.get().trim());
        let mut rows: Vec<(usize, PlaylistEntry)> = entries
            .get()
            .into_iter()
            .enumerate()
            .filter(|(_, e)| {
                q.is_empty()
                    || plain(&e.title).contains(&q)
                    || plain(&e.artist_name).contains(&q)
                    || plain(&e.album_title).contains(&q)
            })
            .collect();
        match sort.get() {
            Sort::Custom => {}
            Sort::Title => rows.sort_by_key(|(_, e)| plain(&e.title)),
            Sort::Artist => {
                rows.sort_by_key(|(_, e)| (plain(&e.artist_name), plain(&e.album_title)))
            }
            Sort::Album => rows.sort_by_key(|(_, e)| {
                (
                    plain(&e.album_title),
                    e.track.as_ref().and_then(|t| t.track_no),
                )
            }),
            Sort::Added => rows.sort_by_key(|(i, e)| (std::cmp::Reverse(e.added_at), *i)),
        }
        rows
    });
    // Playable songs in display order: what Play / Shuffle / a row tap queue.
    let playable = Memo::new(move |_| {
        shown
            .get()
            .into_iter()
            .filter_map(|(_, e)| e.track)
            .collect::<Vec<Track>>()
    });

    let after_change = move || {
        c.refresh();
        if let Some(p) = info.get_untracked() {
            let n = entries.with_untracked(Vec::len) as i64;
            info.set(Some(PlaylistSummary {
                track_count: n,
                ..p
            }));
        }
    };

    let remove = move |entry_id: i64| {
        let pid = id();
        let before = entries.get_untracked();
        entries.update(|v| v.retain(|e| e.entry_id != entry_id));
        after_change();
        spawn_local(async move {
            match api::remove_entry(pid, entry_id).await {
                Ok(()) => {
                    let name = info.with_untracked(|p| p.as_ref().map(|p| p.name.clone()));
                    c.toast(format!("Removed from {}", name.unwrap_or_default()));
                }
                Err(e) => {
                    entries.set(before);
                    after_change();
                    c.error(e);
                }
            }
        });
    };

    let finish_drag = move || {
        let Some(d) = drag.get_untracked() else {
            return;
        };
        drag.set(None);
        let n = entries.with_untracked(Vec::len);
        let to = d.to.min(n.saturating_sub(1));
        if to == d.from || d.from >= n {
            return;
        }
        let pid = id();
        let before = entries.get_untracked();
        entries.update(|v| {
            let e = v.remove(d.from);
            v.insert(to, e);
        });
        spawn_local(async move {
            if let Err(e) = api::move_entry(pid, d.entry_id, to as i64).await {
                entries.set(before);
                c.error(e);
            } else {
                c.refresh();
            }
        });
    };

    // While dragging near the top or bottom edge, scroll the page.
    let scroller: StoredValue<Option<SendWrapper<IntervalHandle>>> = StoredValue::new(None);
    let stop_scroller = move || {
        if let Some(Some(h)) = scroller.try_update_value(Option::take) {
            h.take().clear();
        }
    };
    let start_scroller = move || {
        stop_scroller();
        let handle = set_interval_with_handle(
            move || {
                let (Some(d), Some(main)) = (drag.get_untracked(), main_el()) else {
                    return;
                };
                let rect = main.get_bounding_client_rect();
                let vh = window()
                    .inner_height()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(800.0);
                let mobile = window()
                    .inner_width()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .is_some_and(|w| w < 900.0);
                let top = rect.top() + 70.0;
                let bottom = rect.bottom().min(vh) - if mobile { 200.0 } else { 130.0 };
                let speed = if d.pointer_y < top {
                    -((top - d.pointer_y) / 4.0).clamp(2.0, 18.0)
                } else if d.pointer_y > bottom {
                    ((d.pointer_y - bottom) / 4.0).clamp(2.0, 18.0)
                } else {
                    return;
                };
                main.set_scroll_top(main.scroll_top() + speed as i32);
                // Re-evaluate the target under the (now scrolled) pointer.
                drag.update(|d| {
                    if let Some(d) = d {
                        d.to = d.target(entries.with_untracked(Vec::len));
                    }
                });
            },
            std::time::Duration::from_millis(16),
        );
        if let Ok(h) = handle {
            scroller.set_value(Some(SendWrapper::new(h)));
        }
    };
    on_cleanup(stop_scroller);

    // Vertical offset of row `i` while something is dragged.
    let offset_of = move |entry_id: i64, i: usize| -> String {
        drag.with(|d| match d {
            Some(d) if d.entry_id == entry_id => format!("translate3d(0, {:.1}px, 0)", d.dy()),
            Some(d) if d.from < i && i <= d.to => format!("translate3d(0, {:.1}px, 0)", -d.row_h),
            Some(d) if d.to <= i && i < d.from => format!("translate3d(0, {:.1}px, 0)", d.row_h),
            _ => String::new(),
        })
    };

    let hero = move || {
        info.get().map(|p| {
            player.here(p.name.clone());
            let p2 = p.clone();
            let mut meta = vec![p.owner_name.clone(), songs_label(p.track_count)];
            if p.duration_ms > 0 {
                meta.push(util::fmt_duration_long(p.duration_ms));
            }
            let kind = if p.collaborative {
                "Collaborative Playlist"
            } else if p.public {
                "Shared Playlist"
            } else {
                "Playlist"
            };
            view! {
                <div class="hero">
                    <div class="hero-cover"><PlaylistCover covers=p.cover_album_ids.clone() size=600/></div>
                    <div class="hero-text">
                        <p class="hero-kind">{kind}</p>
                        <h1 class="hero-title">{p.name.clone()}</h1>
                        {(!p.description.is_empty()).then(|| view! { <p class="hero-desc">{p.description.clone()}</p> })}
                        <p class="hero-sub">{meta.join(" · ")}</p>
                        <div class="hero-actions">
                            <button class="btn primary" disabled=move || playable.with(Vec::is_empty)
                                on:click=move |_| player.play_list(playable.get_untracked(), 0)>
                                <Svg icon=Icon::Play/>"Play"
                            </button>
                            <button class="btn tinted" disabled=move || playable.with(Vec::is_empty)
                                on:click=move |_| player.play_shuffled(playable.get_untracked())>
                                <Svg icon=Icon::Shuffle/>"Shuffle"
                            </button>
                            <div class="hero-icons">
                                {if p.is_owner {
                                    let p3 = p.clone();
                                    view! {
                                        <button class="icon-btn tinted-icon" title="Edit Details" aria-label="Edit Details"
                                            on:click=move |_| ov.open(Overlay::EditPlaylist { playlist: p3.clone() })>
                                            <Svg icon=Icon::Pencil/>
                                        </button>
                                    }.into_any()
                                } else {
                                    let (pid, following) = (p.id, p.following);
                                    view! {
                                        <button class="icon-btn tinted-icon save-btn" class:on=following
                                            aria-pressed=following.to_string()
                                            title=if following { "Remove from Library" } else { "Save to Library" }
                                            aria-label=if following { "Remove from Library" } else { "Save to Library" }
                                            on:click=move |_| follow(c, pid, !following)>
                                            {if following { view! { <Svg icon=Icon::HeartFill/> } } else { view! { <Svg icon=Icon::Heart/> } }}
                                        </button>
                                    }.into_any()
                                }}
                                <button class="icon-btn tinted-icon" title="More" aria-label="More"
                                    on:click=move |ev| ov.open(Overlay::PlaylistMenu {
                                        playlist: p2.clone(),
                                        tracks: playable.get_untracked(),
                                        at: Anchor::below(&ev),
                                    })>
                                    <Svg icon=Icon::More/>
                                </button>
                            </div>
                        </div>
                    </div>
                </div>
            }
        })
    };

    let toolbar = move || {
        let n = entries.with(Vec::len);
        (n > 0).then(|| view! {
            <div class="list-toolbar">
                <label class="search-field small">
                    <Svg icon=Icon::Search/>
                    <input type="search" placeholder="Find in playlist" aria-label="Find in playlist"
                        autocomplete="off" bind:value=filter/>
                </label>
                <Picker title="Sort By" class="sort-menu" options=&SORT_LABELS
                    selected=Signal::derive(move || {
                        let cur = sort.get();
                        Sort::ALL.iter().position(|(s, _)| *s == cur).unwrap_or(0)
                    })
                    on_pick=Callback::new(move |i: usize| {
                        if let Some((s, _)) = Sort::ALL.get(i) { sort.set(*s); }
                    })/>
                {move || can_edit().then(|| view! {
                    <button class="btn small" class:tinted=move || editing.get()
                        aria-pressed=move || editing.get().to_string()
                        on:click=move |_| editing.update(|e| *e = !*e)>
                        {move || if editing.get() { "Done" } else { "Edit" }}
                    </button>
                })}
            </div>
        })
    };

    let rows = move || {
        let rows = shown.get();
        let reorder = can_reorder();
        let editable = can_edit();
        let n = rows.len();
        if n == 0 {
            return if entries.with(Vec::is_empty) {
                view! {
                    <div class="empty">
                        <span class="empty-icon"><Svg icon=Icon::Playlist/></span>
                        <h2>"This playlist is empty"</h2>
                        <p>{if editable {
                            "Add songs from any album or search result with ⋯ → Add to Playlist."
                        } else {
                            "Songs added to it will appear here."
                        }}</p>
                    </div>
                }
                .into_any()
            } else {
                view! { <p class="footnote center">"No songs match."</p> }.into_any()
            };
        }
        let all = StoredValue::new(playable.get_untracked());
        let mut play_index = 0usize;
        rows.into_iter()
            .enumerate()
            .map(|(i, (_, e))| {
                let entry_id = e.entry_id;
                let remove_cb = editable.then(|| Callback::new(move |()| remove(entry_id)));
                let remove_label = format!("Remove {}", e.title);
                let controls = move || {
                    view! {
                        {editable.then(|| view! {
                            <button class="row-remove" title="Remove" aria-label=remove_label
                                on:click=move |_| remove(entry_id)>
                                <Svg icon=Icon::Minus/>
                            </button>
                        })}
                        {reorder.then(|| view! {
                            <span class="drag-handle" role="button" tabindex="0" aria-label="Reorder (use arrow keys)" title="Drag to reorder"
                                on:touchstart=move |ev| ev.stop_propagation()
                                on:pointerdown=move |ev: web_sys::PointerEvent| {
                                    if ev.button() != 0 { return; }
                                    ev.prevent_default();
                                    let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) else { return };
                                    let _ = el.set_pointer_capture(ev.pointer_id());
                                    let row_h = el.closest(".reorder-item").ok().flatten()
                                        .and_then(|r| r.dyn_into::<web_sys::HtmlElement>().ok())
                                        .map_or(56.0, |r| f64::from(r.offset_height()));
                                    let y = f64::from(ev.client_y());
                                    drag.set(Some(Drag {
                                        entry_id, from: i, to: i, start_y: y, pointer_y: y,
                                        start_scroll: main_scroll_top(), row_h,
                                    }));
                                    start_scroller();
                                }
                                on:pointermove=move |ev: web_sys::PointerEvent| {
                                    let n = entries.with_untracked(Vec::len);
                                    drag.update(|d| if let Some(d) = d {
                                        d.pointer_y = f64::from(ev.client_y());
                                        d.to = d.target(n);
                                    });
                                }
                                on:pointerup=move |_| { stop_scroller(); finish_drag(); }
                                on:pointercancel=move |_| { stop_scroller(); drag.set(None); }
                                on:keydown=move |ev: web_sys::KeyboardEvent| {
                                    let delta: i64 = match ev.key().as_str() {
                                        "ArrowUp" => -1,
                                        "ArrowDown" => 1,
                                        _ => return,
                                    };
                                    ev.prevent_default();
                                    ev.stop_propagation();
                                    let to = (i as i64 + delta).clamp(0, n as i64 - 1) as usize;
                                    if to != i {
                                        drag.set(Some(Drag { entry_id, from: i, to, start_y: 0.0, pointer_y: 0.0, start_scroll: 0.0, row_h: 0.0 }));
                                        finish_drag();
                                        focus_handle(to);
                                    }
                                }>
                                <Svg icon=Icon::Grip/>
                            </span>
                        })}
                    }
                };
                let inner = match e.track.clone() {
                    Some(t) => {
                        let at = play_index;
                        play_index += 1;
                        let on_play = Callback::new(move |()| player.play_list(all.get_value(), at));
                        match remove_cb {
                            Some(r) => view! {
                                <TrackRow track=t index=at all show_album=true numbered=false on_play on_remove=r>
                                    {controls()}
                                </TrackRow>
                            }.into_any(),
                            None => view! {
                                <TrackRow track=t index=at all show_album=true numbered=false on_play>
                                    {controls()}
                                </TrackRow>
                            }.into_any(),
                        }
                    }
                    None => view! { <UnavailableRow entry=e.clone()>{controls()}</UnavailableRow> }.into_any(),
                };
                view! {
                    <div class="reorder-item" data-index=i
                        class:lifted=move || drag.with(|d| d.is_some_and(|d| d.entry_id == entry_id))
                        style:transform=move || offset_of(entry_id, i)>
                        {inner}
                    </div>
                }
            })
            .collect_view()
            .into_any()
    };

    view! {
        <div class="page">
            <div class="wash" aria-hidden="true"></div>
            {move || match (info.with(Option::is_some), error.get()) {
                (_, Some(e)) => view! { <ErrorBox error=e/> }.into_any(),
                (false, None) => view! { <Loading/> }.into_any(),
                (true, None) => view! {
                    {hero}
                    {toolbar}
                    <div class="tracks playlist-tracks"
                        class:editing=move || editing.get()
                        class:reorderable=can_reorder
                        class:is-dragging=move || drag.with(Option::is_some)>
                        {rows}
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

/// Keeps keyboard focus on the handle of a row that just moved.
fn focus_handle(index: usize) {
    set_timeout(
        move || {
            let sel = format!(".reorder-item[data-index=\"{index}\"] .drag-handle");
            if let Some(el) = document()
                .query_selector(&sel)
                .ok()
                .flatten()
                .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let _ = el.focus();
            }
        },
        std::time::Duration::from_millis(30),
    );
}

/// A playlist entry whose file is no longer in the library.
#[component]
fn UnavailableRow(entry: PlaylistEntry, children: Children) -> impl IntoView {
    view! {
        <div class="track-row-wrap">
            <div class="track-row with-art unavailable" title="This song is no longer in the library">
                <div class="track-main">
                    <span class="track-art"><div class="cover placeholder"><Svg icon=Icon::Note/></div></span>
                    <span class="track-text">
                        <span class="track-title">{entry.title.clone()}</span>
                        <span class="track-sub"><span class="badge">"Unavailable"</span>{format!("{} · {}", entry.artist_name, entry.album_title)}</span>
                    </span>
                    <span class="track-dur">{fmt_time(entry.duration_ms as f64 / 1000.0)}</span>
                </div>
                {children()}
            </div>
        </div>
    }
}
