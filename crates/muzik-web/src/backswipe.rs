//! Swipe from the left edge to go back, the way Safari does it: the current
//! page follows the finger and slides away to reveal the previous page, which
//! sits underneath with a slight parallax and dimming.
//!
//! The previous page isn't rendered any more, so it is shown from a snapshot:
//! when a navigation removes a page from `<main>`, a `MutationObserver` keeps
//! that (already detached) DOM node and the scroll position it was left at.
//! After going back, the snapshot covers the swap until the real page has
//! rendered and its scroll position is restored, so the hand-off is invisible.

use std::{collections::HashMap, time::Duration};

use leptos::{html, prelude::*};
use leptos_router::hooks::use_location;
use send_wrapper::SendWrapper;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{Element, MutationObserver, MutationObserverInit, TouchEvent};

use crate::gestures::{now, point, LOCK_PX, VELOCITY};

/// Width of the left-edge zone where a back swipe can start.
const EDGE_PX: f64 = 24.0;
/// Fraction of the width past which a release goes back.
const COMMIT: f64 = 0.35;
/// How far left the previous page starts (fraction of the width).
const PARALLAX: f64 = 0.3;
/// Dimming of the previous page at the start of the swipe.
const DIM: f64 = 0.22;
/// Duration of the settle animation (matches the CSS transition).
const SETTLE_MS: u64 = 280;
/// Longest the snapshot may cover the real page after going back.
const COVER_MAX_MS: f64 = 1200.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    /// Edge touch, direction not decided yet.
    Pending,
    Dragging,
    /// Animating to the end (`dx` = width) or back (`dx` = 0).
    Settling,
    /// Navigated back; the snapshot hides the real page while it renders.
    Covering,
}

/// The observer and its callback (kept alive together).
type PageObserver = (MutationObserver, Closure<dyn Fn(js_sys::Array)>);

struct Snapshot {
    node: Element,
    scroll: i32,
}

#[derive(Clone, Copy)]
pub struct BackSwipe {
    phase: RwSignal<Phase>,
    dx: RwSignal<f64>,
    width: StoredValue<f64>,
    start: StoredValue<(f64, f64, f64)>,
    /// In-app history: paths visited, top = current.
    stack: RwSignal<Vec<String>>,
    snaps: StoredValue<HashMap<String, Snapshot>, LocalStorage>,
    /// Last known scroll position of `<main>`.
    scroll: StoredValue<i32>,
    main: NodeRef<html::Main>,
    layer: NodeRef<html::Div>,
    /// False on iOS Safari in a browser tab, which has its own back swipe.
    enabled: bool,
}

fn current_path() -> String {
    let loc = window().location();
    format!(
        "{}{}",
        loc.pathname().unwrap_or_default(),
        loc.search().unwrap_or_default()
    )
}

fn reduced_motion() -> bool {
    window()
        .match_media("(prefers-reduced-motion: reduce)")
        .ok()
        .flatten()
        .is_some_and(|m| m.matches())
}

impl BackSwipe {
    /// Call once in the shell; bind `main` and `layer` to the elements.
    pub fn install() -> Self {
        // `navigator.standalone` only exists on iOS: `false` means a Safari
        // tab (native back swipe), `true` an installed web app (none).
        let ios_tab = js_sys::Reflect::get(&window().navigator(), &JsValue::from_str("standalone"))
            .ok()
            .and_then(|v| v.as_bool())
            == Some(false);
        let this = Self {
            phase: RwSignal::new(Phase::Idle),
            dx: RwSignal::new(0.0),
            width: StoredValue::new(1.0),
            start: StoredValue::new((0.0, 0.0, 0.0)),
            stack: RwSignal::new(Vec::new()),
            snaps: StoredValue::new_local(HashMap::new()),
            scroll: StoredValue::new(0),
            main: NodeRef::new(),
            layer: NodeRef::new(),
            enabled: !ios_tab,
        };
        this.track_history();
        this.observe_pages();
        this
    }

    pub fn main_ref(&self) -> NodeRef<html::Main> {
        self.main
    }

    pub fn layer_ref(&self) -> NodeRef<html::Div> {
        self.layer
    }

    /// Follows in-app navigation as a stack of paths, so the gesture only
    /// goes back to a Muzik page, never out of the app.
    fn track_history(self) {
        let location = use_location();
        Effect::new(move |_| {
            let path = format!("{}{}", location.pathname.get(), location.search.get());
            self.stack.update(|s| {
                let n = s.len();
                if n >= 2 && s[n - 2] == path {
                    s.pop();
                } else if s.last() != Some(&path) {
                    s.push(path);
                }
            });
            self.prune();
        });
    }

    /// Drops snapshots of pages that can't be gone back to.
    fn prune(self) {
        let keep: Vec<String> = self
            .stack
            .with_untracked(|s| s.iter().take(s.len().saturating_sub(1)).cloned().collect());
        self.snaps
            .update_value(|m| m.retain(|path, _| keep.contains(path)));
    }

    /// Tags each page element added to `<main>` with its path, and keeps each
    /// removed one as the snapshot of that path.
    fn observe_pages(self) {
        let observer: StoredValue<Option<SendWrapper<PageObserver>>> = StoredValue::new(None);
        Effect::new(move |_| {
            let Some(main) = self.main.get() else { return };
            if observer.with_value(Option::is_some) {
                return;
            }
            let tag = |el: &Element| {
                if el.get_attribute("data-path").is_none() {
                    let _ = el.set_attribute("data-path", &current_path());
                }
            };
            let children = main.children();
            for i in 0..children.length() {
                if let Some(el) = children.item(i) {
                    tag(&el);
                }
            }
            let cb = Closure::<dyn Fn(js_sys::Array)>::new(move |records: js_sys::Array| {
                for rec in records.iter() {
                    let Ok(rec) = rec.dyn_into::<web_sys::MutationRecord>() else {
                        continue;
                    };
                    let added = rec.added_nodes();
                    for i in 0..added.length() {
                        if let Some(el) = added.item(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                            tag(&el);
                        }
                    }
                    let removed = rec.removed_nodes();
                    for i in 0..removed.length() {
                        let Some(el) = removed.item(i).and_then(|n| n.dyn_into::<Element>().ok())
                        else {
                            continue;
                        };
                        let Some(path) = el.get_attribute("data-path") else {
                            continue;
                        };
                        let scroll = self.scroll.get_value();
                        self.snaps.update_value(|m| {
                            m.insert(path, Snapshot { node: el, scroll });
                        });
                    }
                }
                self.prune();
            });
            let Ok(mo) = MutationObserver::new(cb.as_ref().unchecked_ref()) else {
                return;
            };
            let init = MutationObserverInit::new();
            init.set_child_list(true);
            let _ = mo.observe_with_options(&main, &init);
            observer.set_value(Some(SendWrapper::new((mo, cb))));
        });
        on_cleanup(move || {
            observer.try_with_value(|o| {
                if let Some(o) = o {
                    o.0.disconnect();
                }
            });
        });
    }

    /// `on:scroll` of `<main>`.
    pub fn scrolled(&self) {
        if let Some(main) = self.main.get_untracked() {
            self.scroll.set_value(main.scroll_top());
        }
    }

    fn previous_path(&self) -> Option<String> {
        self.stack
            .with_untracked(|s| s.len().checked_sub(2).and_then(|i| s.get(i).cloned()))
    }

    pub fn touch_start(&self, ev: &TouchEvent) {
        if !self.enabled
            || self.phase.get_untracked() != Phase::Idle
            || ev.touches().length() != 1
            || self.previous_path().is_none()
        {
            return;
        }
        let Some((x, y)) = point(ev) else { return };
        if x > EDGE_PX {
            return;
        }
        // Claim the touch so rows underneath don't start their own swipe.
        ev.stop_propagation();
        self.start.set_value((x, y, now()));
        self.phase.set(Phase::Pending);
    }

    pub fn touch_move(&self, ev: &TouchEvent) {
        let phase = self.phase.get_untracked();
        if !matches!(phase, Phase::Pending | Phase::Dragging) {
            return;
        }
        let Some((x, y)) = point(ev) else { return };
        let (sx, sy, _) = self.start.get_value();
        let (dx, dy) = (x - sx, y - sy);
        if phase == Phase::Pending {
            if dx.abs().max(dy.abs()) <= LOCK_PX {
                ev.stop_propagation();
                return;
            }
            if dx <= dy.abs() {
                // A scroll that happened to start at the edge.
                self.phase.set(Phase::Idle);
                return;
            }
            self.begin_drag();
        }
        ev.prevent_default();
        ev.stop_propagation();
        self.dx.set(dx.max(0.0));
    }

    /// Puts the previous page's snapshot under the current page.
    fn begin_drag(&self) {
        let (Some(main), Some(layer)) = (self.main.get_untracked(), self.layer.get_untracked())
        else {
            return;
        };
        self.width
            .set_value(f64::from(main.client_width()).max(1.0));
        layer.set_inner_html("");
        let snap = self.previous_path().and_then(|p| {
            self.snaps
                .with_value(|m| m.get(&p).map(|s| (s.node.clone(), s.scroll)))
        });
        // Show the layer synchronously so the scroll position can be set.
        let _ = layer.class_list().add_1("shown");
        if let Some((node, scroll)) = snap {
            let _ = layer.append_child(&node);
            layer.set_scroll_top(scroll);
        }
        self.dx.set(0.0);
        self.phase.set(Phase::Dragging);
    }

    pub fn touch_end(&self, ev: &TouchEvent) {
        match self.phase.get_untracked() {
            Phase::Pending => {
                self.phase.set(Phase::Idle);
                return;
            }
            Phase::Dragging => {}
            _ => return,
        }
        ev.stop_propagation();
        let (_, _, t0) = self.start.get_value();
        let dt = (now() - t0).max(1.0);
        let dx = self.dx.get_untracked();
        let w = self.width.get_value();
        let commit = dx > w * COMMIT || (dx > 24.0 && dx / dt > VELOCITY);
        self.settle(commit);
    }

    pub fn touch_cancel(&self) {
        match self.phase.get_untracked() {
            Phase::Pending => self.phase.set(Phase::Idle),
            Phase::Dragging => self.settle(false),
            _ => {}
        }
    }

    fn settle(self, commit: bool) {
        self.phase.set(Phase::Settling);
        self.dx
            .set(if commit { self.width.get_value() } else { 0.0 });
        let wait = Duration::from_millis(if reduced_motion() { 0 } else { SETTLE_MS });
        set_timeout(
            move || {
                if commit {
                    self.go_back();
                } else {
                    self.finish();
                }
            },
            wait,
        );
    }

    fn go_back(self) {
        let Some(target) = self.previous_path() else {
            self.finish();
            return;
        };
        let scroll = self
            .snaps
            .with_value(|m| m.get(&target).map_or(0, |s| s.scroll));
        self.phase.set(Phase::Covering);
        self.dx.set(0.0);
        if let Ok(h) = window().history() {
            let _ = h.back();
        }
        self.await_page(target, scroll, now());
    }

    /// Waits until the real previous page is in place (as tall as its
    /// snapshot, or a timeout), restores its scroll, then uncovers it.
    fn await_page(self, target: String, scroll: i32, since: f64) {
        let (Some(main), Some(layer)) = (self.main.get_untracked(), self.layer.get_untracked())
        else {
            self.finish();
            return;
        };
        let arrived = current_path() == target
            && main
                .first_element_child()
                .and_then(|el| el.get_attribute("data-path"))
                .is_some_and(|p| p == target);
        let tall_enough = main.scroll_height() + 4 >= layer.scroll_height();
        if arrived && (tall_enough || now() - since > COVER_MAX_MS) {
            main.set_scroll_top(scroll);
            self.scroll.set_value(main.scroll_top());
            // One more frame for lazy/virtualized content at that position.
            set_timeout(move || self.finish(), Duration::from_millis(48));
        } else if now() - since > COVER_MAX_MS * 2.0 {
            self.finish();
        } else {
            set_timeout(
                move || self.await_page(target, scroll, since),
                Duration::from_millis(32),
            );
        }
    }

    fn finish(&self) {
        if let Some(layer) = self.layer.get_untracked() {
            let _ = layer.class_list().remove_1("shown");
            layer.set_inner_html("");
        }
        self.dx.set(0.0);
        self.phase.set(Phase::Idle);
    }

    // ----- view bindings -----

    pub fn settling(&self) -> bool {
        self.phase.get() == Phase::Settling
    }

    pub fn covering(&self) -> bool {
        self.phase.get() == Phase::Covering
    }

    /// The live page is moved aside (casts a shadow on the previous one).
    pub fn lifted(&self) -> bool {
        matches!(self.phase.get(), Phase::Dragging | Phase::Settling)
    }

    fn progress(&self) -> f64 {
        (self.dx.get() / self.width.get_value()).clamp(0.0, 1.0)
    }

    pub fn main_transform(&self) -> String {
        if self.lifted() {
            format!("translate3d({:.1}px, 0, 0)", self.dx.get())
        } else {
            String::new()
        }
    }

    pub fn layer_transform(&self) -> String {
        if self.lifted() {
            let x = -PARALLAX * self.width.get_value() * (1.0 - self.progress());
            format!("translate3d({x:.1}px, 0, 0)")
        } else {
            String::new()
        }
    }

    pub fn layer_filter(&self) -> String {
        if self.lifted() {
            format!("brightness({:.3})", 1.0 - DIM * (1.0 - self.progress()))
        } else {
            String::new()
        }
    }
}
