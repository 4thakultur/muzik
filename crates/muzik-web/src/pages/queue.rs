use leptos::prelude::*;
use muzik_common::Track;

use crate::{
    components::{NowPlayingBars, TrackCover},
    icons::{Icon, Svg},
    overlays::{use_overlays, Overlay},
    player::{fmt_time, use_player},
};

#[component]
fn QueueRow(track: Track, index: usize, #[prop(optional)] current: bool) -> impl IntoView {
    let player = use_player();
    view! {
        <div class="track-row-wrap">
            <div class="track-row with-art" class:current=current>
                <button class="track-main" on:click=move |_| player.jump(index)>
                    <span class="track-art">
                        <TrackCover album_id=track.album_id size=96/>
                        {current.then(|| view! { <span class="track-art-eq"><NowPlayingBars/></span> })}
                    </span>
                    <span class="track-text">
                        <span class="track-title">{track.title.clone()}</span>
                        <span class="track-sub">{track.artist_name.clone()}" · "{track.album_title.clone()}</span>
                    </span>
                    <span class="track-dur">{fmt_time(track.duration_ms as f64 / 1000.0)}</span>
                </button>
            </div>
        </div>
    }
}

#[component]
pub fn Queue() -> impl IntoView {
    let player = use_player();
    let ov = use_overlays();
    view! {
        <div class="page narrow">
            <div class="page-head">
                <h1 class="large-title">"Queue"</h1>
                {move || player.queue.with(|q| !q.is_empty()).then(|| view! {
                    <button class="btn small tinted" on:click=move |_| ov.open(Overlay::NewPlaylist {
                        track_ids: player.queue.with_untracked(|q| q.iter().map(|t| t.id).collect()),
                    })>
                        <Svg icon=Icon::PlaylistAdd/>"Save as Playlist"
                    </button>
                })}
            </div>
            {move || {
                let q = player.queue.get();
                let Some(cur) = player.index.get().filter(|i| *i < q.len()) else {
                    return view! {
                        <div class="empty">
                            <h2>"Nothing queued"</h2>
                            <p>"Swipe a song to the right, or use +, to play it next."</p>
                        </div>
                    }.into_any();
                };
                let up_next: Vec<_> = q.iter().cloned().enumerate().skip(cur + 1).collect();
                view! {
                    <h2 class="section-title">"Now Playing"</h2>
                    <div class="tracks"><QueueRow track=q[cur].clone() index=cur current=true/></div>
                    <h2 class="section-title">"Playing Next"</h2>
                    {if up_next.is_empty() {
                        view! { <p class="footnote">"Nothing else in the queue."</p> }.into_any()
                    } else {
                        view! {
                            <div class="tracks">
                                {up_next.into_iter().map(|(i, t)| view! { <QueueRow track=t index=i/> }).collect_view()}
                            </div>
                        }.into_any()
                    }}
                }.into_any()
            }}
        </div>
    }
}
