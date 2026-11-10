use std::future::Future;

use leptos::prelude::*;
use leptos::task::spawn_local;
use send_wrapper::SendWrapper;
use wasm_bindgen::{prelude::Closure, JsCast};

use crate::api::ApiResult;
use crate::{use_auth, AuthState};

pub type Remote<T> = RwSignal<Option<ApiResult<T>>>;

/// Runs `source` whenever the signals it reads change and stores the latest result.
/// Stale responses are dropped; a 401 sends the user back to the login screen.
pub fn fetch<T, Fut>(source: impl Fn() -> Fut + 'static) -> Remote<T>
where
    T: Send + Sync + 'static,
    Fut: Future<Output = ApiResult<T>> + 'static,
{
    let auth = use_auth();
    let out: Remote<T> = RwSignal::new(None);
    let generation = StoredValue::new(0u64);
    Effect::new(move |_| {
        let fut = source();
        generation.update_value(|g| *g += 1);
        let mine = generation.get_value();
        spawn_local(async move {
            let res = fut.await;
            if generation.try_get_value() != Some(mine) {
                return;
            }
            match &res {
                Err(e) if e.is_unauthorized() => auth.0.set(AuthState::LoggedOut),
                Err(e) if e.needs_totp_setup() => auth.refresh(),
                _ => {}
            }
            out.try_set(Some(res));
        });
    });
    out
}

/// Calls `on_visible` each time the referenced element scrolls into view
/// (used for infinite lists).
pub fn on_visible(node: NodeRef<leptos::html::Div>, on_visible: impl Fn() + 'static) {
    let cb = Closure::<dyn Fn(js_sys::Array)>::new(move |entries: js_sys::Array| {
        let hit = entries.iter().any(|e| {
            e.dyn_into::<web_sys::IntersectionObserverEntry>()
                .is_ok_and(|e| e.is_intersecting())
        });
        if hit {
            on_visible();
        }
    });
    let init = web_sys::IntersectionObserverInit::new();
    init.set_root_margin("600px");
    let Ok(observer) =
        web_sys::IntersectionObserver::new_with_options(cb.as_ref().unchecked_ref(), &init)
    else {
        return;
    };
    let holder = SendWrapper::new((observer, cb));
    let holder = StoredValue::new(Some(holder));
    Effect::new(move |_| {
        if let Some(el) = node.get() {
            holder.with_value(|h| {
                if let Some(h) = h {
                    h.0.observe(&el);
                }
            });
        }
    });
    on_cleanup(move || {
        if let Some(Some(h)) = holder.try_update_value(|h| h.take()) {
            h.0.disconnect();
        }
    });
}

pub fn focus_search() {
    set_timeout(
        || {
            if let Some(el) = document().get_element_by_id("search-input") {
                if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
                    let _ = el.focus();
                }
            }
        },
        std::time::Duration::from_millis(30),
    );
}

pub fn fmt_duration_long(ms: i64) -> String {
    let mins = ms / 60_000;
    if mins >= 60 {
        format!("{} h {} min", mins / 60, mins % 60)
    } else {
        format!("{mins} min")
    }
}

pub fn quality_label(t: &muzik_common::Track) -> Option<String> {
    if !t.lossless {
        return None;
    }
    match (t.bit_depth, t.sample_rate) {
        (Some(b), Some(sr)) if b > 16 || sr > 48_000 => Some(format!("Hi-Res {b}/{}", fmt_khz(sr))),
        _ => Some("Lossless".into()),
    }
}

fn fmt_khz(sr: i64) -> String {
    if sr % 1000 == 0 {
        format!("{}", sr / 1000)
    } else {
        format!("{:.1}", sr as f64 / 1000.0)
    }
}
