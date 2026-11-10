//! Touch swipe tracking: follows the finger along one locked axis, then decides
//! with a distance-or-velocity threshold. The view springs back via CSS
//! transitions when `dragging` turns false.

use leptos::prelude::*;
use web_sys::TouchEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    Undecided,
    X,
    Y,
}

#[derive(Clone, Copy)]
pub struct Swipe {
    pub dx: RwSignal<f64>,
    pub dy: RwSignal<f64>,
    pub dragging: RwSignal<bool>,
    start: StoredValue<(f64, f64, f64)>,
    axis: StoredValue<Axis>,
    /// Which axes this element reacts to.
    horizontal: bool,
    vertical: bool,
}

pub(crate) const LOCK_PX: f64 = 8.0;
const DIST_PX: f64 = 80.0;
pub(crate) const VELOCITY: f64 = 0.45; // px per ms

pub(crate) fn now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

pub(crate) fn point(ev: &TouchEvent) -> Option<(f64, f64)> {
    let t = ev
        .touches()
        .get(0)
        .or_else(|| ev.changed_touches().get(0))?;
    Some((f64::from(t.client_x()), f64::from(t.client_y())))
}

impl Swipe {
    pub fn new(horizontal: bool, vertical: bool) -> Self {
        Self {
            dx: RwSignal::new(0.0),
            dy: RwSignal::new(0.0),
            dragging: RwSignal::new(false),
            start: StoredValue::new((0.0, 0.0, 0.0)),
            axis: StoredValue::new(Axis::Undecided),
            horizontal,
            vertical,
        }
    }

    pub fn start(&self, ev: &TouchEvent) {
        if ev.touches().length() != 1 {
            return;
        }
        if let Some((x, y)) = point(ev) {
            self.start.set_value((x, y, now()));
            self.axis.set_value(Axis::Undecided);
            self.dragging.set(true);
        }
    }

    pub fn moved(&self, ev: &TouchEvent) {
        if !self.dragging.get_untracked() {
            return;
        }
        let Some((x, y)) = point(ev) else { return };
        let (sx, sy, _) = self.start.get_value();
        let (dx, dy) = (x - sx, y - sy);
        let mut axis = self.axis.get_value();
        if axis == Axis::Undecided && dx.abs().max(dy.abs()) > LOCK_PX {
            axis = if dx.abs() > dy.abs() {
                Axis::X
            } else {
                Axis::Y
            };
            self.axis.set_value(axis);
        }
        match axis {
            Axis::X if self.horizontal => {
                ev.prevent_default();
                self.dx.set(dx);
            }
            Axis::Y if self.vertical => {
                ev.prevent_default();
                self.dy.set(dy);
            }
            _ => {}
        }
    }

    /// Ends the gesture and returns the direction if it passed the threshold.
    pub fn end(&self, ev: &TouchEvent) -> Option<Dir> {
        if !self.dragging.get_untracked() {
            return None;
        }
        self.dragging.set(false);
        let (_, _, t0) = self.start.get_value();
        let dt = (now() - t0).max(1.0);
        let (dx, dy) = (self.dx.get_untracked(), self.dy.get_untracked());
        self.dx.set(0.0);
        self.dy.set(0.0);
        let _ = point(ev);
        let passed = |d: f64| d.abs() > DIST_PX || (d.abs() > 24.0 && d.abs() / dt > VELOCITY);
        match self.axis.get_value() {
            Axis::X if self.horizontal && passed(dx) => {
                Some(if dx < 0.0 { Dir::Left } else { Dir::Right })
            }
            Axis::Y if self.vertical && passed(dy) => {
                Some(if dy < 0.0 { Dir::Up } else { Dir::Down })
            }
            _ => None,
        }
    }

    /// CSS transform following the finger (with resistance past the threshold).
    pub fn transform(&self) -> String {
        let damp = |d: f64| {
            if d.abs() <= DIST_PX {
                d
            } else {
                d.signum() * (DIST_PX + (d.abs() - DIST_PX) * 0.35)
            }
        };
        format!(
            "translate3d({:.1}px, {:.1}px, 0)",
            damp(self.dx.get()),
            damp(self.dy.get()).max(if self.vertical { -40.0 } else { 0.0 })
        )
    }

    /// Vertical-only transform for a dismissable sheet: follows the finger 1:1
    /// downwards, with a short rubber band upwards.
    pub fn sheet_transform(&self) -> String {
        let dy = self.dy.get();
        let y = if dy >= 0.0 {
            dy
        } else {
            -(dy.abs().powf(0.7)).min(30.0)
        };
        format!("translate3d(0, {y:.1}px, 0)")
    }
}
