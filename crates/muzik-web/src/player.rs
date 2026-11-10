//! Audio playback: queue, two `<audio>` elements (current + preloaded next) so the
//! next track starts without a network round-trip, Media Session integration.

use std::{cell::Cell, cell::RefCell, rc::Rc};

use js_sys::{Array, Function, Object, Reflect};
use leptos::prelude::*;
use muzik_common::{Quality, Track};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{prelude::Closure, JsCast, JsValue};
use web_sys::HtmlAudioElement;

use crate::api;

/// Repeat mode, cycled by the repeat button: Off → All → One.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Repeat {
    #[default]
    Off,
    /// Wrap around the queue.
    All,
    /// Loop the current song (Next still skips).
    One,
}

impl Repeat {
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// Per-device playback preferences, kept in `localStorage`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub wifi_quality: Quality,
    pub cellular_quality: Quality,
    #[serde(default)]
    pub shuffle: bool,
    #[serde(default)]
    pub repeat: Repeat,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            wifi_quality: Quality::Original,
            cellular_quality: Quality::Normal,
            shuffle: false,
            repeat: Repeat::Off,
        }
    }
}

const SETTINGS_KEY: &str = "muzik.settings";
const VOLUME_KEY: &str = "muzik.volume";

fn load_volume() -> f64 {
    storage()
        .and_then(|s| s.get_item(VOLUME_KEY).ok().flatten())
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map_or(1.0, |v| v.clamp(0.0, 1.0))
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

impl Settings {
    fn load() -> Self {
        storage()
            .and_then(|s| s.get_item(SETTINGS_KEY).ok().flatten())
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let (Some(s), Ok(v)) = (storage(), serde_json::to_string(self)) {
            let _ = s.set_item(SETTINGS_KEY, &v);
        }
    }
}

/// True when the browser reports a cellular connection (Network Information API,
/// not available everywhere; defaults to Wi-Fi).
fn on_cellular() -> bool {
    let Some(w) = web_sys::window() else {
        return false;
    };
    Reflect::get(&w.navigator(), &"connection".into())
        .ok()
        .filter(|c| c.is_object())
        .and_then(|c| Reflect::get(&c, &"type".into()).ok())
        .and_then(|t| t.as_string())
        .is_some_and(|t| t == "cellular")
}

type ActionHandler = Box<dyn Fn(JsValue)>;

struct Engine {
    els: [HtmlAudioElement; 2],
    active: Cell<usize>,
    /// Track id loaded into the inactive element, if any.
    preloaded: Cell<Option<i64>>,
}

impl Engine {
    fn cur(&self) -> &HtmlAudioElement {
        &self.els[self.active.get()]
    }
    fn other(&self) -> &HtmlAudioElement {
        &self.els[1 - self.active.get()]
    }
}

thread_local! {
    static ENGINE: RefCell<Option<Rc<Engine>>> = const { RefCell::new(None) };
}

fn engine() -> Option<Rc<Engine>> {
    ENGINE.with(|e| e.borrow().clone())
}

#[derive(Clone, Copy)]
pub struct Player {
    pub queue: RwSignal<Vec<Track>>,
    pub index: RwSignal<Option<usize>>,
    pub playing: RwSignal<bool>,
    pub buffering: RwSignal<bool>,
    pub position: RwSignal<f64>,
    pub duration: RwSignal<f64>,
    pub settings: RwSignal<Settings>,
    pub sheet_open: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    /// Output volume in 0..=1 (slider position), per device.
    pub volume: RwSignal<f64>,
    pub muted: RwSignal<bool>,
    /// What the queue was started from ("Album Title", "Road Trip"…).
    pub source: RwSignal<Option<String>>,
    /// The album/playlist shown on screen, which becomes `source` when
    /// playback starts from it (see `here`).
    page_source: RwSignal<Option<String>>,
    /// While shuffled: the queue in its original order, restored when shuffle
    /// is turned off.
    unshuffled: StoredValue<Option<Vec<Track>>>,
}

pub fn use_player() -> Player {
    expect_context::<Player>()
}

impl Player {
    /// Creates the player, its audio elements and Media Session handlers. Call once.
    pub fn install() -> Self {
        let p = Self {
            queue: RwSignal::new(Vec::new()),
            index: RwSignal::new(None),
            playing: RwSignal::new(false),
            buffering: RwSignal::new(false),
            position: RwSignal::new(0.0),
            duration: RwSignal::new(0.0),
            settings: RwSignal::new(Settings::load()),
            sheet_open: RwSignal::new(false),
            error: RwSignal::new(None),
            volume: RwSignal::new(load_volume()),
            muted: RwSignal::new(false),
            source: RwSignal::new(None),
            page_source: RwSignal::new(None),
            unshuffled: StoredValue::new(None),
        };

        let make = || -> Option<HtmlAudioElement> {
            let a = HtmlAudioElement::new().ok()?;
            a.set_preload("auto");
            Some(a)
        };
        if let (Some(a), Some(b)) = (make(), make()) {
            let eng = Rc::new(Engine {
                els: [a, b],
                active: Cell::new(0),
                preloaded: Cell::new(None),
            });
            for i in 0..2 {
                p.attach_listeners(&eng, i);
            }
            ENGINE.with(|e| *e.borrow_mut() = Some(eng));
        }
        p.install_media_session();

        Effect::new(move |_| p.settings.get().save());
        Effect::new(move |_| {
            let v = p.volume.get();
            let muted = p.muted.get();
            if let Some(e) = engine() {
                // Squared curve: loudness feels linear along the slider.
                let gain = (v * v).clamp(0.0, 1.0);
                for el in &e.els {
                    el.set_volume(gain);
                    el.set_muted(muted);
                }
            }
            if let Some(s) = storage() {
                let _ = s.set_item(VOLUME_KEY, &format!("{v:.3}"));
            }
        });
        p
    }

    fn attach_listeners(self, eng: &Rc<Engine>, i: usize) {
        let el = &eng.els[i];
        let on = |name: &str, f: Box<dyn Fn(&Engine)>| {
            let weak = Rc::downgrade(eng);
            let cb = Closure::<dyn Fn()>::new(move || {
                if let Some(e) = weak.upgrade() {
                    if e.active.get() == i {
                        f(&e);
                    }
                }
            });
            let _ = el.add_event_listener_with_callback(name, cb.as_ref().unchecked_ref());
            cb.forget();
        };
        on(
            "timeupdate",
            Box::new(move |e| {
                let t = e.cur().current_time();
                self.position.set(t);
                self.maybe_preload(e);
            }),
        );
        on(
            "durationchange",
            Box::new(move |e| {
                let d = e.cur().duration();
                if d.is_finite() {
                    self.duration.set(d);
                }
            }),
        );
        on(
            "playing",
            Box::new(move |_| {
                self.playing.set(true);
                self.buffering.set(false);
                set_playback_state("playing");
            }),
        );
        on(
            "pause",
            Box::new(move |_| {
                self.playing.set(false);
                set_playback_state("paused");
            }),
        );
        on("waiting", Box::new(move |_| self.buffering.set(true)));
        on("ended", Box::new(move |e| self.advance(e)));
        on(
            "error",
            Box::new(move |_| {
                self.buffering.set(false);
                self.playing.set(false);
                self.error
                    .set(Some("This track could not be played.".to_owned()));
            }),
        );
    }

    pub fn current(&self) -> Option<Track> {
        let i = self.index.get()?;
        self.queue.with(|q| q.get(i).cloned())
    }

    fn quality(&self) -> Quality {
        let s = self.settings.get_untracked();
        if on_cellular() {
            s.cellular_quality
        } else {
            s.wifi_quality
        }
    }

    /// Declares the album/playlist the current view shows, so playing from it
    /// labels the queue ("Playing from …"). Cleared when the view goes away.
    pub fn here(&self, name: impl Into<String>) {
        let page = self.page_source;
        page.set(Some(name.into()));
        on_cleanup(move || {
            let _ = page.try_set(None);
        });
    }

    /// Replaces the queue and starts playing at `start`.
    pub fn play_list(&self, tracks: Vec<Track>, start: usize) {
        if tracks.is_empty() {
            return;
        }
        let start = start.min(tracks.len() - 1);
        self.source.set(self.page_source.get_untracked());
        if self.shuffle() {
            // The chosen song plays first, the rest in random order.
            self.unshuffled.set_value(Some(tracks.clone()));
            let mut q = tracks;
            q.swap(0, start);
            shuffle_slice(&mut q[1..]);
            self.queue.set(q);
            self.load_index(0, true);
        } else {
            self.unshuffled.set_value(None);
            self.queue.set(tracks);
            self.load_index(start, true);
        }
    }

    /// Turns shuffle on and plays `tracks` from a random song.
    pub fn play_shuffled(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        self.settings.update(|s| s.shuffle = true);
        let start = random_below(tracks.len());
        self.play_list(tracks, start);
    }

    pub fn shuffle(&self) -> bool {
        self.settings.with_untracked(|s| s.shuffle)
    }

    pub fn repeat(&self) -> Repeat {
        self.settings.with_untracked(|s| s.repeat)
    }

    /// Shuffle on: the songs after the current one are shuffled. Off: the
    /// original order comes back, continuing from the current song. Playback
    /// is never interrupted.
    pub fn toggle_shuffle(&self) {
        let on = !self.shuffle();
        self.settings.update(|s| s.shuffle = on);
        let Some(i) = self.index.get_untracked() else {
            self.unshuffled.set_value(None);
            return;
        };
        if on {
            self.unshuffled.set_value(Some(self.queue.get_untracked()));
            self.queue.update(|q| {
                if i + 1 < q.len() {
                    shuffle_slice(&mut q[i + 1..]);
                }
            });
        } else if let Some(original) = self.unshuffled.try_update_value(Option::take).flatten() {
            let cur = self.queue.with_untracked(|q| q.get(i).map(|t| t.id));
            let at = cur
                .and_then(|id| original.iter().position(|t| t.id == id))
                .unwrap_or(0);
            self.queue.set(original);
            self.index.set(Some(at));
        }
        self.queue_changed();
    }

    /// Off → All → One. "One" loops the playing element natively (gapless).
    pub fn cycle_repeat(&self) {
        let r = self.repeat().cycle();
        self.settings.update(|s| s.repeat = r);
        self.apply_loop();
        self.queue_changed();
    }

    fn apply_loop(&self) {
        if let Some(e) = engine() {
            e.cur().set_loop(self.repeat() == Repeat::One);
            e.other().set_loop(false);
        }
    }

    /// Appends a track to the end of the queue (starts playback if idle).
    pub fn enqueue(&self, track: Track) {
        self.enqueue_many(vec![track]);
    }

    /// Appends tracks to the end of the queue (starts playback if idle).
    pub fn enqueue_many(&self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let idle = self.index.get_untracked().is_none();
        self.unshuffled.update_value(|u| {
            if let Some(u) = u {
                u.extend(tracks.iter().cloned());
            }
        });
        self.queue.update(|q| q.extend(tracks));
        if idle {
            self.load_index(0, true);
        } else {
            self.queue_changed();
        }
    }

    /// Inserts a track right after the current one (starts playback if idle).
    pub fn play_next(&self, track: Track) {
        match self.index.get_untracked() {
            None => self.enqueue(track),
            Some(i) => {
                // Also right after the current song in the original order.
                let cur = self.queue.with_untracked(|q| q.get(i).map(|t| t.id));
                self.unshuffled.update_value(|u| {
                    if let Some(u) = u {
                        let at = cur
                            .and_then(|id| u.iter().position(|t| t.id == id))
                            .map_or(u.len(), |p| p + 1);
                        u.insert(at, track.clone());
                    }
                });
                self.queue.update(|q| q.insert((i + 1).min(q.len()), track));
                self.queue_changed();
            }
        }
    }

    /// Drops the preloaded element when the track after the current one changed.
    fn queue_changed(&self) {
        if let Some(e) = engine() {
            if e.preloaded.get().is_some() && !self.next_is_preloaded(&e) {
                e.preloaded.set(None);
            }
        }
    }

    /// Index of the song after the current one (wraps with Repeat All).
    fn next_index(&self) -> Option<usize> {
        let i = self.index.get_untracked()?;
        let len = self.queue.with_untracked(Vec::len);
        if i + 1 < len {
            Some(i + 1)
        } else if self.repeat() == Repeat::All && len > 0 {
            Some(0)
        } else {
            None
        }
    }

    fn next_track(&self) -> Option<Track> {
        let i = self.next_index()?;
        self.queue.with_untracked(|q| q.get(i).cloned())
    }

    fn next_is_preloaded(&self, e: &Engine) -> bool {
        matches!((self.next_track(), e.preloaded.get()), (Some(t), Some(id)) if t.id == id)
    }

    fn load_index(&self, i: usize, autoplay: bool) {
        let Some(track) = self.queue.with_untracked(|q| q.get(i).cloned()) else {
            return;
        };
        let Some(e) = engine() else { return };
        self.error.set(None);
        self.index.set(Some(i));
        self.position.set(0.0);
        self.duration.set(track.duration_ms as f64 / 1000.0);

        if e.preloaded.get() == Some(track.id) {
            // Swap to the already-buffered element.
            let _ = e.cur().pause();
            e.active.set(1 - e.active.get());
        } else {
            e.cur().set_src(&api::stream_url(track.id, self.quality()));
        }
        e.preloaded.set(None);
        e.other().remove_attribute("src").ok();
        e.other().load();
        self.apply_loop();

        if autoplay {
            self.buffering.set(true);
            self.start(e.cur());
        }
        update_media_metadata(&track);
    }

    /// Preloads the next track once the current one is (almost) fully buffered,
    /// so it doesn't compete for bandwidth with the track being played.
    fn maybe_preload(&self, e: &Engine) {
        if e.preloaded.get().is_some() {
            return;
        }
        let Some(next) = self.next_track() else {
            return;
        };
        let cur = e.cur();
        let dur = cur.duration();
        let buffered = cur.buffered();
        let end = buffered
            .length()
            .checked_sub(1)
            .and_then(|i| buffered.end(i).ok())
            .unwrap_or(0.0);
        let remaining = dur - cur.current_time();
        if (dur.is_finite() && end >= dur - 0.5) || remaining < 30.0 {
            let other = e.other();
            other.set_src(&api::stream_url(next.id, self.quality()));
            other.load();
            e.preloaded.set(Some(next.id));
        }
    }

    fn advance(&self, _e: &Engine) {
        match self.next_index() {
            Some(i) => self.load_index(i, true),
            None => {
                self.playing.set(false);
                self.position.set(0.0);
                set_playback_state("none");
            }
        }
    }

    /// Calls `play()` and handles a rejected promise (autoplay blocked, source
    /// error), so the UI never shows "playing" for silent audio.
    fn start(&self, el: &HtmlAudioElement) {
        let Ok(promise) = el.play() else { return };
        let this = *self;
        let on_err = Closure::<dyn FnMut(JsValue)>::new(move |err: JsValue| {
            this.buffering.set(false);
            this.playing.set(false);
            let name = Reflect::get(&err, &"name".into())
                .ok()
                .and_then(|v| v.as_string());
            // NotAllowedError: the browser wants a tap first; AbortError: a newer load replaced this one.
            if !matches!(name.as_deref(), Some("NotAllowedError" | "AbortError")) {
                this.error.set(Some(
                    "Playback failed. Try a lower quality in Settings.".into(),
                ));
            }
        });
        let _ = promise.catch(&on_err);
        on_err.forget();
    }

    pub fn toggle(&self) {
        let Some(e) = engine() else { return };
        if self.index.get_untracked().is_none() {
            return;
        }
        if e.cur().paused() {
            self.start(e.cur());
        } else {
            let _ = e.cur().pause();
        }
    }

    pub fn next(&self) {
        if let Some(e) = engine() {
            if self.next_track().is_some() {
                self.advance(&e);
            }
        }
    }

    pub fn prev(&self) {
        if self.position.get_untracked() > 3.0 {
            self.seek(0.0);
            return;
        }
        let len = self.queue.with_untracked(Vec::len);
        match self.index.get_untracked() {
            Some(i) if i > 0 => self.load_index(i - 1, true),
            Some(0) if self.repeat() == Repeat::All && len > 1 => self.load_index(len - 1, true),
            _ => self.seek(0.0),
        }
    }

    pub fn jump(&self, i: usize) {
        self.load_index(i, true);
    }

    pub fn seek(&self, secs: f64) {
        if let Some(e) = engine() {
            let d = self.duration.get_untracked();
            let t = if d > 0.0 {
                secs.clamp(0.0, d)
            } else {
                secs.max(0.0)
            };
            e.cur().set_current_time(t);
            self.position.set(t);
        }
    }

    pub fn seek_by(&self, delta: f64) {
        self.seek(self.position.get_untracked() + delta);
    }

    pub fn set_volume(&self, v: f64) {
        self.volume.set(v.clamp(0.0, 1.0));
        if v > 0.0 {
            self.muted.set(false);
        }
    }

    pub fn volume_by(&self, delta: f64) {
        self.set_volume(self.volume.get_untracked() + delta);
    }

    pub fn toggle_mute(&self) {
        if self.volume.get_untracked() <= 0.0 {
            self.volume.set(0.5);
            self.muted.set(false);
        } else {
            self.muted.update(|m| *m = !*m);
        }
    }

    fn install_media_session(self) {
        let Some(ms) = media_session() else { return };
        let Ok(set) = Reflect::get(&ms, &"setActionHandler".into()) else {
            return;
        };
        let Ok(set) = set.dyn_into::<Function>() else {
            return;
        };
        let handlers: [(&str, ActionHandler); 5] = [
            ("play", Box::new(move |_| self.toggle())),
            ("pause", Box::new(move |_| self.toggle())),
            ("previoustrack", Box::new(move |_| self.prev())),
            ("nexttrack", Box::new(move |_| self.next())),
            (
                "seekto",
                Box::new(move |d: JsValue| {
                    if let Some(t) = Reflect::get(&d, &"seekTime".into())
                        .ok()
                        .and_then(|v| v.as_f64())
                    {
                        self.seek(t);
                    }
                }),
            ),
        ];
        for (name, f) in handlers {
            let cb = Closure::<dyn Fn(JsValue)>::new(f);
            // Unsupported actions throw; ignore them.
            let _ = set.call2(&ms, &name.into(), cb.as_ref());
            cb.forget();
        }
    }
}

fn media_session() -> Option<JsValue> {
    let w = web_sys::window()?;
    Reflect::get(&w.navigator(), &"mediaSession".into())
        .ok()
        .filter(|v| v.is_object())
}

fn set_playback_state(state: &str) {
    if let Some(ms) = media_session() {
        let _ = Reflect::set(&ms, &"playbackState".into(), &state.into());
    }
}

fn update_media_metadata(t: &Track) {
    let (Some(ms), Some(w)) = (media_session(), web_sys::window()) else {
        return;
    };
    let Some(ctor) = Reflect::get(&w, &"MediaMetadata".into())
        .ok()
        .and_then(|c| c.dyn_into::<Function>().ok())
    else {
        return;
    };
    let init = Object::new();
    let art = Object::new();
    let _ = Reflect::set(&art, &"src".into(), &api::cover_url(t.album_id, 600).into());
    let _ = Reflect::set(&art, &"sizes".into(), &"600x600".into());
    let _ = Reflect::set(&art, &"type".into(), &"image/webp".into());
    let _ = Reflect::set(&init, &"title".into(), &t.title.as_str().into());
    let _ = Reflect::set(&init, &"artist".into(), &t.artist_name.as_str().into());
    let _ = Reflect::set(&init, &"album".into(), &t.album_title.as_str().into());
    let _ = Reflect::set(&init, &"artwork".into(), &Array::of1(&art));
    if let Ok(md) = Reflect::construct(&ctor, &Array::of1(&init)) {
        let _ = Reflect::set(&ms, &"metadata".into(), &md);
    }
}

fn random_below(n: usize) -> usize {
    ((js_sys::Math::random() * n as f64) as usize).min(n.saturating_sub(1))
}

/// Fisher–Yates.
fn shuffle_slice<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, random_below(i + 1));
    }
}

pub fn fmt_time(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "0:00".into();
    }
    let s = secs as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
