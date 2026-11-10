use leptos::{prelude::*, task::spawn_local};
use leptos_router::hooks::{use_params_map, use_query_map};
use muzik_common::{Album, Artist};

use crate::{
    api,
    components::{AlbumCard, Cover, ErrorBox, Loading, Monogram, TrackList},
    icons::{Icon, Svg},
    overlays::{use_overlays, Overlay},
    player::use_player,
    util,
};

const PAGE: i64 = 120;

/// Infinite, paginated list state shared by the albums and artists pages.
struct Paged<T: Send + Sync + 'static> {
    items: RwSignal<Vec<T>>,
    total: RwSignal<Option<i64>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<api::ApiError>>,
}

// Manual impls: signals are `Copy` regardless of `T`.
impl<T: Send + Sync + 'static> Clone for Paged<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: Send + Sync + 'static> Copy for Paged<T> {}

impl<T: Send + Sync + Clone + 'static> Paged<T> {
    fn new() -> Self {
        Self {
            items: RwSignal::new(Vec::new()),
            total: RwSignal::new(None),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
        }
    }

    fn load_more<F, Fut>(self, fetch: F)
    where
        F: FnOnce(i64) -> Fut,
        Fut: std::future::Future<Output = api::ApiResult<muzik_common::Page<T>>> + 'static,
    {
        if self.loading.get_untracked() {
            return;
        }
        let have = self.items.with_untracked(Vec::len) as i64;
        if self.total.get_untracked().is_some_and(|t| have >= t) {
            return;
        }
        self.loading.set(true);
        let fut = fetch(have);
        spawn_local(async move {
            match fut.await {
                Ok(p) => {
                    self.total.try_set(Some(p.total));
                    self.items.try_update(|v| v.extend(p.items));
                }
                Err(e) => {
                    self.error.try_set(Some(e));
                }
            }
            self.loading.try_set(false);
        });
    }
}

#[component]
pub fn Albums() -> impl IntoView {
    const SORTS: [(&str, &str); 4] = [
        ("title", "A–Z"),
        ("artist", "Artist"),
        ("year", "Year"),
        ("recent", "Recent"),
    ];
    let initial = use_query_map()
        .with_untracked(|q| q.get("sort"))
        .and_then(|v| SORTS.iter().map(|(k, _)| *k).find(|k| *k == v))
        .unwrap_or("title");
    let sort = RwSignal::new(initial);
    let paged = Paged::<Album>::new();
    let more = move || paged.load_more(move |off| api::albums(off, PAGE, sort.get_untracked()));
    Effect::new(move |_| {
        sort.track();
        paged.items.set(Vec::new());
        paged.total.set(None);
        paged.loading.set(false);
        more();
    });
    let sentinel = NodeRef::new();
    util::on_visible(sentinel, more);

    view! {
        <div class="page">
            <div class="page-head">
                <h1 class="large-title">"Albums"</h1>
                <div class="segmented" role="group" aria-label="Sort albums">
                    {SORTS
                        .into_iter()
                        .map(|(key, label)| view! {
                            <button class:active=move || sort.get() == key aria-pressed=move || (sort.get() == key).to_string()
                                on:click=move |_| sort.set(key)>{label}</button>
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="grid">
                {move || paged.items.get().into_iter().map(|a| view! { <AlbumCard album=a/> }).collect_view()}
            </div>
            {move || paged.error.get().map(|e| view! { <ErrorBox error=e/> })}
            {move || paged.loading.get().then(|| view! { <Loading/> })}
            <div node_ref=sentinel class="sentinel"></div>
        </div>
    }
}

#[component]
pub fn Artists() -> impl IntoView {
    let paged = Paged::<Artist>::new();
    let more = move || paged.load_more(|off| api::artists(off, PAGE));
    more();
    let sentinel = NodeRef::new();
    util::on_visible(sentinel, more);

    view! {
        <div class="page">
            <h1 class="large-title">"Artists"</h1>
            <ul class="list">
                {move || paged.items.get().into_iter().map(|a| view! {
                    <li><a class="list-row" href=format!("/artist/{}", a.id)>
                        <Monogram name=a.name.clone()/>
                        <span class="list-text">
                            <span class="list-title">{a.name.clone()}</span>
                            <span class="list-sub">{format!("{} album{}", a.album_count, if a.album_count == 1 { "" } else { "s" })}</span>
                        </span>
                        <span class="chevron"><Svg icon=Icon::ChevronRight/></span>
                    </a></li>
                }).collect_view()}
            </ul>
            {move || paged.error.get().map(|e| view! { <ErrorBox error=e/> })}
            {move || paged.loading.get().then(|| view! { <Loading/> })}
            <div node_ref=sentinel class="sentinel"></div>
        </div>
    }
}

fn id_param() -> impl Fn() -> i64 + Copy + 'static {
    let params = use_params_map();
    move || {
        params
            .with(|p| p.get("id"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

#[component]
pub fn ArtistPage() -> impl IntoView {
    let id = id_param();
    let data = util::fetch(move || {
        let id = id();
        async move { api::artist(id).await }
    });
    view! {
        <div class="page">
            {move || match data.get() {
                None => view! { <Loading/> }.into_any(),
                Some(Err(e)) => view! { <ErrorBox error=e/> }.into_any(),
                Some(Ok(d)) => view! {
                    <div class="wash" aria-hidden="true"></div>
                    <div class="hero artist">
                        <Monogram name=d.artist.name.clone() big=true/>
                        <div class="hero-text">
                            <h1 class="hero-title">{d.artist.name.clone()}</h1>
                            <p class="hero-sub">{format!("{} album{}", d.albums.len(), if d.albums.len() == 1 { "" } else { "s" })}</p>
                        </div>
                    </div>
                    <h2 class="section-title">"Albums"</h2>
                    <div class="grid">
                        {d.albums.into_iter().map(|a| view! { <AlbumCard album=a/> }).collect_view()}
                    </div>
                }.into_any(),
            }}
        </div>
    }
}

#[component]
pub fn AlbumPage() -> impl IntoView {
    let id = id_param();
    let player = use_player();
    let ov = use_overlays();
    let data = util::fetch(move || {
        let id = id();
        async move { api::album(id).await }
    });
    view! {
        <div class="page">
            {move || match data.get() {
                None => view! { <Loading/> }.into_any(),
                Some(Err(e)) => view! { <ErrorBox error=e/> }.into_any(),
                Some(Ok(d)) => {
                    let a = d.album.clone();
                    player.here(a.title.clone());
                    let tracks = d.tracks.clone();
                    let play_all = StoredValue::new(d.tracks.clone());
                    let multi_disc = tracks.iter().filter_map(|t| t.disc_no).max().unwrap_or(1) > 1;
                    let lossless = tracks.iter().find_map(util::quality_label);
                    view! {
                        <div class="wash" aria-hidden="true"></div>
                        <div class="hero">
                            <div class="hero-cover"><Cover album_id=a.id has_cover=a.has_cover size=600/></div>
                            <div class="hero-text">
                                <h1 class="hero-title">{a.title.clone()}</h1>
                                <a class="hero-artist" href=format!("/artist/{}", a.artist_id)>{a.artist_name.clone()}</a>
                                <p class="hero-sub">
                                    {lossless.map(|b| view! { <span class="badge">{b}</span> })}
                                    {a.year.map(|y| y.to_string()).unwrap_or_else(|| "Album".into())}
                                    {format!(" · {} song{} · {}", a.track_count, if a.track_count == 1 { "" } else { "s" }, util::fmt_duration_long(a.duration_ms))}
                                </p>
                                <div class="hero-actions">
                                    <button class="btn primary" on:click=move |_| player.play_list(play_all.get_value(), 0)>
                                        <Svg icon=Icon::Play/>"Play"
                                    </button>
                                    <button class="btn tinted" on:click=move |_| player.play_shuffled(play_all.get_value())>
                                        <Svg icon=Icon::Shuffle/>"Shuffle"
                                    </button>
                                    <div class="hero-icons">
                                        <button class="icon-btn tinted-icon" title="Add to Playlist" aria-label="Add Album to Playlist"
                                            on:click={
                                                let title = a.title.clone();
                                                move |_| ov.open(Overlay::AddToPlaylist {
                                                    track_ids: play_all.with_value(|ts| ts.iter().map(|t| t.id).collect()),
                                                    label: title.clone(),
                                                })
                                            }>
                                            <Svg icon=Icon::PlaylistAdd/>
                                        </button>
                                    </div>
                                </div>
                            </div>
                        </div>
                        <div class="tracks">
                            {if multi_disc {
                                // (disc number, offset in album, tracks)
                                let mut discs: Vec<(i64, usize, Vec<muzik_common::Track>)> = Vec::new();
                                for (i, t) in tracks.iter().cloned().enumerate() {
                                    let n = t.disc_no.unwrap_or(1);
                                    match discs.last_mut() {
                                        Some((d, _, v)) if *d == n => v.push(t),
                                        _ => discs.push((n, i, vec![t])),
                                    }
                                }
                                // Clicking a track queues the whole album, starting there.
                                discs.into_iter().map(|(n, off, ts)| view! {
                                    <h3 class="disc">{format!("Disc {n}")}</h3>
                                    <TrackList tracks=ts numbered=true queue=tracks.clone() offset=off/>
                                }).collect_view().into_any()
                            } else {
                                view! { <TrackList tracks=tracks numbered=true/> }.into_any()
                            }}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}
