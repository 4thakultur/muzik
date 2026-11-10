use leptos::prelude::*;

use crate::{
    api,
    collection::use_collection,
    components::{AlbumCard, Cover, ErrorBox, Loading, PlaylistCard},
    icons::{Icon, Svg},
    use_auth, util,
};

#[component]
pub fn Home() -> impl IntoView {
    let auth = use_auth();
    let recent = util::fetch(|| api::albums(0, 24, "recent"));
    let shared = util::fetch(api::shared_playlists);
    let c = use_collection();
    let mine = move || c.playlists.get();
    // Shared playlists the user hasn't saved yet.
    let discover = move || match shared.get() {
        Some(Ok(list)) => list.into_iter().filter(|p| !p.following).collect(),
        _ => Vec::new(),
    };
    let greeting = move || {
        let hour = js_sys::Date::new_0().get_hours();
        let part = match hour {
            5..=11 => "Good morning",
            12..=17 => "Good afternoon",
            _ => "Good evening",
        };
        match auth.user() {
            Some(u) => format!("{part}, {}", u.display_name),
            None => part.to_owned(),
        }
    };
    view! {
        <div class="page">
            <div class="wash" aria-hidden="true"></div>
            <h1 class="large-title">{greeting}</h1>
            {move || match recent.get() {
                None => view! { <Loading/> }.into_any(),
                Some(Err(e)) => view! { <ErrorBox error=e/> }.into_any(),
                Some(Ok(p)) if p.items.is_empty() => view! {
                    <div class="empty">
                        <span class="empty-icon"><Svg icon=Icon::Note/></span>
                        <h2>"Your library is empty"</h2>
                        <p>"Copy music into the library folder on the server, then scan it from "
                            <a href="/settings">"Settings"</a>"."</p>
                    </div>
                }.into_any(),
                Some(Ok(p)) => view! {
                    <div class="tiles">
                        {p.items.iter().take(6).map(|a| view! {
                            <a class="tile" href=format!("/album/{}", a.id)>
                                <Cover album_id=a.id has_cover=a.has_cover size=96/>
                                <span class="tile-title">{a.title.clone()}</span>
                            </a>
                        }).collect_view()}
                    </div>
                    <a class="section-link" href="/albums?sort=recent">
                        <h2>"Recently Added"</h2><Svg icon=Icon::ChevronRight/>
                    </a>
                    <div class="shelf">
                        {p.items.into_iter().map(|a| view! { <AlbumCard album=a/> }).collect_view()}
                    </div>
                }.into_any(),
            }}
            {move || {
                let list = mine();
                (!list.is_empty()).then(|| view! {
                    <a class="section-link" href="/library">
                        <h2>"Your Playlists"</h2><Svg icon=Icon::ChevronRight/>
                    </a>
                    <div class="shelf">
                        {list.into_iter().map(|p| view! { <PlaylistCard playlist=p/> }).collect_view()}
                    </div>
                })
            }}
            {move || {
                let list: Vec<_> = discover();
                (!list.is_empty()).then(|| view! {
                    <h2 class="section-title">"Shared on This Server"</h2>
                    <div class="shelf">
                        {list.into_iter().map(|p| view! { <PlaylistCard playlist=p/> }).collect_view()}
                    </div>
                })
            }}
        </div>
    }
}
