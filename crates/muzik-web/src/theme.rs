//! Appearance: light / dark (true-black OLED) theme and a single accent color,
//! stored per device in `localStorage`. The accent is exposed to CSS as
//! `--accent`; every gradient in the stylesheet is derived from it.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{prelude::Closure, JsCast};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Preset accents (name, hex). Each keeps ≥ 3:1 contrast with white text and
/// stays readable on both the white and the black theme.
pub const ACCENTS: [(&str, &str); 8] = [
    ("Rose", "#ff2d55"),
    ("Red", "#ff3b30"),
    ("Orange", "#f06a00"),
    ("Green", "#28a745"),
    ("Teal", "#0f97a8"),
    ("Blue", "#0a84ff"),
    ("Indigo", "#5e5ce6"),
    ("Purple", "#af52de"),
];

pub const DEFAULT_ACCENT: &str = ACCENTS[0].1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    pub theme: Theme,
    pub accent: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            accent: DEFAULT_ACCENT.to_owned(),
        }
    }
}

const KEY: &str = "muzik.appearance";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

impl Appearance {
    fn load() -> Self {
        let mut a: Self = storage()
            .and_then(|s| s.get_item(KEY).ok().flatten())
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
        if parse_hex(&a.accent).is_none() {
            a.accent = DEFAULT_ACCENT.to_owned();
        }
        a
    }

    fn save(&self) {
        if let (Some(s), Ok(v)) = (storage(), serde_json::to_string(self)) {
            let _ = s.set_item(KEY, &v);
        }
    }
}

/// Parses `#rrggbb` into 0–255 channels.
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some((c(0)?, c(2)?, c(4)?))
}

/// Text color for content drawn on top of the accent (WCAG relative luminance).
fn on_accent(rgb: (u8, u8, u8)) -> &'static str {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(rgb.0) + 0.7152 * lin(rgb.1) + 0.0722 * lin(rgb.2);
    if l > 0.45 {
        "#000000"
    } else {
        "#ffffff"
    }
}

fn system_dark() -> Option<web_sys::MediaQueryList> {
    web_sys::window()?
        .match_media("(prefers-color-scheme: dark)")
        .ok()
        .flatten()
}

#[derive(Clone, Copy)]
pub struct Looks {
    pub appearance: RwSignal<Appearance>,
    system_is_dark: RwSignal<bool>,
}

pub fn use_looks() -> Looks {
    expect_context::<Looks>()
}

impl Looks {
    /// Provides the context and keeps `<html>` in sync. Call once at startup.
    pub fn install() -> Self {
        let mq = system_dark();
        let looks = Self {
            appearance: RwSignal::new(Appearance::load()),
            system_is_dark: RwSignal::new(mq.as_ref().is_some_and(|m| m.matches())),
        };
        if let Some(mq) = mq {
            let sig = looks.system_is_dark;
            let cb = Closure::<dyn Fn(web_sys::MediaQueryListEvent)>::new(
                move |e: web_sys::MediaQueryListEvent| sig.set(e.matches()),
            );
            let _ = mq.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
            cb.forget();
        }
        provide_context(looks);

        Effect::new(move |_| {
            let a = looks.appearance.get();
            a.save();
            looks.apply(&a);
        });
        looks
    }

    pub fn is_dark(&self) -> bool {
        match self.appearance.with(|a| a.theme) {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => self.system_is_dark.get(),
        }
    }

    fn apply(&self, a: &Appearance) {
        let Some(root) = document().document_element() else {
            return;
        };
        let dark = self.is_dark();
        let _ = root.set_attribute("data-theme", if dark { "dark" } else { "light" });
        if let Ok(el) = root.dyn_into::<web_sys::HtmlElement>() {
            let rgb = parse_hex(&a.accent).unwrap_or((255, 45, 85));
            let style = el.style();
            let _ = style.set_property("--accent", &a.accent);
            let _ = style.set_property("--on-accent", on_accent(rgb));
        }
        if let Ok(Some(meta)) = document().query_selector("meta[name=theme-color]") {
            let _ = meta.set_attribute("content", if dark { "#000000" } else { "#ffffff" });
        }
    }
}
