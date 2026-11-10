use leptos::prelude::*;

use crate::{
    api,
    components::{AlbumCard, ErrorBox, Monogram, PlaylistCard, TrackList},
    icons::{Icon, Svg},
    util,
};

#[component]
pub fn Search() -> impl IntoView {
    let input = RwSignal::new(String::new());
    // Debounced copy of the input that drives the request.
    let query = RwSignal::new(String::new());
    let gen = StoredValue::new(0u64);
    Effect::new(move |_| {
        let v = input.get();
        gen.update_value(|g| *g += 1);
        let mine = gen.get_value();
        set_timeout(
            move || {
                if gen.try_get_value() == Some(mine) {
                    query.try_set(v);
                }
            },
            std::time::Duration::from_millis(120),
        );
    });
    let results = util::fetch(move || {
        let q = query.get();
        async move {
            if q.trim().is_empty() {
                Ok(Default::default())
            } else {
                api::search(&q).await
            }
        }
    });
    util::focus_search();

    view! {
        <div class="page">
            <h1 class="large-title">"Search"</h1>
            <label class="search-field">
                <Svg icon=Icon::Search/>
                <input id="search-input" type="search" placeholder="Artists, Albums, Songs, Playlists"
                    aria-label="Search" autocomplete="off" bind:value=input/>
            </label>
            {move || match results.get() {
                None => ().into_any(),
                Some(Err(e)) => view! { <ErrorBox error=e/> }.into_any(),
                Some(Ok(r)) => {
                    let nothing = r.artists.is_empty() && r.albums.is_empty() && r.tracks.is_empty() && r.playlists.is_empty();
                    if nothing {
                        return if query.get_untracked().trim().is_empty() {
                            view! { <p class="footnote center">"Find anything in your library."</p> }.into_any()
                        } else {
                            view! {
                                <div class="empty">
                                    <h2>"No Results"</h2>
                                    <p>{format!("Nothing matches “{}”.", query.get_untracked().trim())}</p>
                                </div>
                            }.into_any()
                        };
                    }
                    view! {
                        {(!r.tracks.is_empty()).then(|| view! {
                            <h2 class="section-title">"Songs"</h2>
                            <div class="tracks"><TrackList tracks=r.tracks.clone() show_album=true/></div>
                        })}
                        {(!r.artists.is_empty()).then(|| view! {
                            <h2 class="section-title">"Artists"</h2>
                            <ul class="list">
                                {r.artists.iter().map(|a| view! {
                                    <li><a class="list-row" href=format!("/artist/{}", a.id)>
                                        <Monogram name=a.name.clone()/>
                                        <span class="list-text">
                                            <span class="list-title">{a.name.clone()}</span>
                                            <span class="list-sub">"Artist"</span>
                                        </span>
                                        <span class="chevron"><Svg icon=Icon::ChevronRight/></span>
                                    </a></li>
                                }).collect_view()}
                            </ul>
                        })}
                        {(!r.albums.is_empty()).then(|| view! {
                            <h2 class="section-title">"Albums"</h2>
                            <div class="grid">
                                {r.albums.iter().cloned().map(|a| view! { <AlbumCard album=a/> }).collect_view()}
                            </div>
                        })}
                        {(!r.playlists.is_empty()).then(|| view! {
                            <h2 class="section-title">"Playlists"</h2>
                            <div class="grid">
                                {r.playlists.iter().cloned().map(|p| view! { <PlaylistCard playlist=p/> }).collect_view()}
                            </div>
                        })}
                    }.into_any()
                }
            }}
        </div>
    }
}
