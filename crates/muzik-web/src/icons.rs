//! Original glyphs drawn on a 24×24 grid, in the spirit of system symbol sets:
//! filled shapes for primary actions and tabs, rounded strokes elsewhere.

use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Prev,
    Shuffle,
    Repeat,
    RepeatOne,
    Home,
    Search,
    Albums,
    Artist,
    Settings,
    Queue,
    ChevronDown,
    ChevronUpDown,
    ChevronRight,
    Plus,
    Note,
    Check,
    VolumeOff,
    VolumeLow,
    VolumeHigh,
    Heart,
    HeartFill,
    More,
    Playlist,
    PlaylistAdd,
    Library,
    Grip,
    Minus,
    Pencil,
    Trash,
    Copy,
    People,
    Close,
    PlayNext,
}

const HEART: &str = "M12 20.2s-7.6-4.5-8.8-9.7C2.5 7.2 4.6 4.3 7.7 4.3c1.9 0 3.3 1 4.3 2.6 1-1.6 2.4-2.6 4.3-2.6 3.1 0 5.2 2.9 4.5 6.2-1.2 5.2-8.8 9.7-8.8 9.7z";

const SPEAKER: &str = "M3.5 9.6c0-.6.5-1.1 1.1-1.1h2.7l4.1-3.5c.6-.5 1.6-.1 1.6.8v12.4c0 .9-1 1.3-1.6.8l-4.1-3.5H4.6c-.6 0-1.1-.5-1.1-1.1z";

/// One path of a glyph: filled, or stroked with the given width.
#[derive(Clone, Copy)]
enum P {
    Fill(&'static str),
    Stroke(&'static str, &'static str),
}

impl Icon {
    fn paths(self) -> &'static [P] {
        use P::{Fill, Stroke};
        match self {
            Icon::Play => &[Fill("M7.2 4.6c0-1 1.1-1.6 1.9-1.1l11 6.9c.8.5.8 1.7 0 2.2l-11 6.9c-.8.5-1.9-.1-1.9-1.1z")],
            Icon::Pause => &[
                Fill("M6 5.2C6 4.5 6.5 4 7.2 4h2.1c.7 0 1.2.5 1.2 1.2v13.6c0 .7-.5 1.2-1.2 1.2H7.2C6.5 20 6 19.5 6 18.8z"),
                Fill("M13.5 5.2c0-.7.5-1.2 1.2-1.2h2.1c.7 0 1.2.5 1.2 1.2v13.6c0 .7-.5 1.2-1.2 1.2h-2.1c-.7 0-1.2-.5-1.2-1.2z"),
            ],
            Icon::Next => &[
                Fill("M2.5 7c0-.8.9-1.3 1.6-.8l7.3 5c.6.4.6 1.2 0 1.6l-7.3 5c-.7.5-1.6 0-1.6-.8z"),
                Fill("M12 7c0-.8.9-1.3 1.6-.8l7.3 5c.6.4.6 1.2 0 1.6l-7.3 5c-.7.5-1.6 0-1.6-.8z"),
            ],
            Icon::Prev => &[
                Fill("M21.5 7c0-.8-.9-1.3-1.6-.8l-7.3 5c-.6.4-.6 1.2 0 1.6l7.3 5c.7.5 1.6 0 1.6-.8z"),
                Fill("M12 7c0-.8-.9-1.3-1.6-.8l-7.3 5c-.6.4-.6 1.2 0 1.6l7.3 5c.7.5 1.6 0 1.6-.8z"),
            ],
            Icon::Shuffle => &[
                Stroke("M3.5 7h2.8c1.9 0 3 .8 4 2.4l3.4 5.2c1 1.6 2.1 2.4 4 2.4h2.8", "2"),
                Stroke("M3.5 17h2.8c1.3 0 2.2-.4 2.9-1.1M14.8 8.1c.7-.7 1.6-1.1 2.9-1.1h2.8", "2"),
                Stroke("M18 4.3 20.7 7 18 9.7M18 14.3l2.7 2.7-2.7 2.7", "2"),
            ],
            Icon::Home => &[Fill(
                "M10.7 3.5a2 2 0 0 1 2.6 0l7 6c.4.4.7.9.7 1.5V19a2 2 0 0 1-2 2h-3.5a1 1 0 0 1-1-1v-4.5a1 1 0 0 0-1-1h-3a1 1 0 0 0-1 1V20a1 1 0 0 1-1 1H5a2 2 0 0 1-2-2v-8c0-.6.3-1.1.7-1.5z",
            )],
            Icon::Search => &[Stroke("M10.5 3.8a6.7 6.7 0 1 1 0 13.4 6.7 6.7 0 0 1 0-13.4zM15.4 15.4 20.5 20.5", "2.3")],
            Icon::Albums => &[
                Fill("M4 10.5A2.5 2.5 0 0 1 6.5 8h11a2.5 2.5 0 0 1 2.5 2.5v8a2.5 2.5 0 0 1-2.5 2.5h-11A2.5 2.5 0 0 1 4 18.5z"),
                Stroke("M6.5 5h11M8.5 2.5h7", "1.8"),
            ],
            Icon::Artist => &[
                Fill("M15.2 2.8a5 5 0 1 1 0 10 5 5 0 0 1 0-10z"),
                Stroke("M11.2 10.6 4.1 18.3a1 1 0 0 0 0 1.4l.2.2a1 1 0 0 0 1.4 0l7.7-7.1", "2.2"),
            ],
            Icon::Settings => &[
                Stroke("M4 7.5h8.5M18.5 7.5H20M4 16.5h1.5M11.5 16.5H20", "2"),
                Stroke("M15.5 5a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5zM8.5 14a2.5 2.5 0 1 1 0 5 2.5 2.5 0 0 1 0-5z", "2"),
            ],
            Icon::Repeat => &[
                Stroke("M4 11.5V9.5A3.5 3.5 0 0 1 7.5 6H19M16.3 3.3 19 6l-2.7 2.7", "2"),
                Stroke("M20 12.5v2A3.5 3.5 0 0 1 16.5 18H5M7.7 20.7 5 18l2.7-2.7", "2"),
            ],
            Icon::RepeatOne => &[
                Stroke("M4 11.5V9.5A3.5 3.5 0 0 1 7.5 6H19M16.3 3.3 19 6l-2.7 2.7", "2"),
                Stroke("M20 12.5v2A3.5 3.5 0 0 1 16.5 18H5M7.7 20.7 5 18l2.7-2.7", "2"),
                Stroke("M10.9 10.4 12.5 9.3v5.4", "1.8"),
            ],
            Icon::Queue => &[
                Stroke("M9 6.5h11M9 12h11M9 17.5h7", "2.1"),
                Fill("M4.6 5.2a1.3 1.3 0 1 1 0 2.6 1.3 1.3 0 0 1 0-2.6zM4.6 10.7a1.3 1.3 0 1 1 0 2.6 1.3 1.3 0 0 1 0-2.6zM4.6 16.2a1.3 1.3 0 1 1 0 2.6 1.3 1.3 0 0 1 0-2.6z"),
            ],
            Icon::ChevronDown => &[Stroke("M6 9.5l6 6 6-6", "2.4")],
            Icon::ChevronUpDown => &[Stroke("M8 9.5l4-4 4 4M8 14.5l4 4 4-4", "2.2")],
            Icon::ChevronRight => &[Stroke("M9.5 6l6 6-6 6", "2.4")],
            Icon::Plus => &[Stroke("M12 5v14M5 12h14", "2.2")],
            // Beamed eighth notes: tilted oval heads, stems and one slanted beam.
            Icon::Note => &[
                Fill("M8.7 17V6.8c0-.5.3-.9.8-1l8.8-2c.6-.1 1.2.3 1.2 1V15h-1.8V7.1l-7.2 1.6V17z"),
                Fill("M4.63 18.66a3.1 2.3-22 1 1 5.75-2.32 3.1 2.3-22 1 1-5.75 2.32zM13.63 16.66a3.1 2.3-22 1 1 5.75-2.32 3.1 2.3-22 1 1-5.75 2.32z"),
            ],
            Icon::Check => &[Stroke("M5.5 12.5l4.2 4.2 8.8-9.4", "2.6")],
            Icon::VolumeOff => &[Fill(SPEAKER), Stroke("M16.2 9.6l4.8 4.8M21 9.6l-4.8 4.8", "2")],
            Icon::VolumeLow => &[Fill(SPEAKER), Stroke("M15.6 9.3a3.9 3.9 0 0 1 0 5.4", "2")],
            Icon::VolumeHigh => &[
                Fill(SPEAKER),
                Stroke("M15.6 9.3a3.9 3.9 0 0 1 0 5.4M18.4 6.6a7.7 7.7 0 0 1 0 10.8", "2"),
            ],
            Icon::Heart => &[Stroke(HEART, "2")],
            Icon::HeartFill => &[Fill(HEART)],
            Icon::More => &[Fill(
                "M5.5 10.2a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6zM12 10.2a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6zM18.5 10.2a1.8 1.8 0 1 1 0 3.6 1.8 1.8 0 0 1 0-3.6z",
            )],
            // Lines of a list with a note at the end.
            Icon::Playlist => &[
                Stroke("M3.5 6h12M3.5 11h12M3.5 16h6.5", "2.1"),
                Stroke("M18.6 17.5V8.6l2.4-.7", "2"),
                Fill("M16.4 15.1a2.3 2.3 0 1 1 0 4.6 2.3 2.3 0 0 1 0-4.6z"),
            ],
            Icon::PlaylistAdd => &[Stroke("M3.5 6h12M3.5 11h12M3.5 16h7M18 12.5v8M14 16.5h8", "2.1")],
            // Two spines and a leaning one.
            Icon::Library => &[Stroke("M5 4.5v15M10 4.5v15M14.6 5.2l4.6 14.1", "2.3")],
            Icon::Grip => &[Stroke("M5 8h14M5 12h14M5 16h14", "1.9")],
            Icon::Minus => &[Stroke("M7 12h10", "2.6")],
            Icon::Pencil => &[Stroke(
                "M14.6 5.6l3.8 3.8M4.5 19.5l.9-4.2 10.3-10.3a1.7 1.7 0 0 1 2.4 0l1 1a1.7 1.7 0 0 1 0 2.4L8.8 18.6z",
                "2",
            )],
            Icon::Trash => &[Stroke(
                "M4.5 6.5h15M9.5 6.5V5c0-.7.5-1.2 1.2-1.2h2.6c.7 0 1.2.5 1.2 1.2v1.5M6.5 6.5l.9 12.2c.1 1 .9 1.8 1.9 1.8h5.4c1 0 1.8-.8 1.9-1.8l.9-12.2",
                "2",
            )],
            Icon::Copy => &[Stroke(
                "M9 10c0-1.1.9-2 2-2h7c1.1 0 2 .9 2 2v8c0 1.1-.9 2-2 2h-7c-1.1 0-2-.9-2-2zM15 8V6c0-1.1-.9-2-2-2H6c-1.1 0-2 .9-2 2v8c0 1.1.9 2 2 2h3",
                "2",
            )],
            Icon::People => &[Stroke(
                "M9 11a3.4 3.4 0 1 0 0-6.8A3.4 3.4 0 0 0 9 11zM2.8 19.8c.6-3.3 3-5.3 6.2-5.3s5.6 2 6.2 5.3M15.8 4.4a3.4 3.4 0 0 1 0 6.4M18 14.8c1.8.7 2.9 2.3 3.2 5",
                "2",
            )],
            Icon::Close => &[Stroke("M6.5 6.5l11 11M17.5 6.5l-11 11", "2.3")],
            // Queue lines with a play triangle in front.
            Icon::PlayNext => &[
                Stroke("M11 6.5h9.5M11 12h9.5M11 17.5h9.5", "2.1"),
                Fill("M3.5 8.9c0-.7.8-1.1 1.4-.7l3.9 2.9c.5.4.5 1.1 0 1.5L4.9 15.5c-.6.4-1.4 0-1.4-.7z"),
            ],
        }
    }
}

#[component]
pub fn Svg(icon: Icon) -> impl IntoView {
    view! {
        <svg class="icon" viewBox="0 0 24 24" aria-hidden="true" stroke-linecap="round" stroke-linejoin="round">
            {icon.paths().iter().map(|p| {
                let (d, fill, stroke, width) = match *p {
                    P::Fill(d) => (d, "currentColor", "none", "0"),
                    P::Stroke(d, w) => (d, "none", "currentColor", w),
                };
                view! { <path d=d fill=fill stroke=stroke stroke-width=width></path> }
            }).collect_view()}
        </svg>
    }
}

/// The Muzik mark: an app-icon squircle filled with the accent gradient and an
/// "M" drawn as five equalizer bars.
#[component]
pub fn Logo(#[prop(optional)] big: bool) -> impl IntoView {
    view! {
        <span class="logo" class:big=big aria-hidden="true">
            <svg viewBox="0 0 24 24">
                <g fill="currentColor">
                    <rect x="4.2" y="6" width="2.6" height="12" rx="1.3"></rect>
                    <rect x="7.95" y="8.5" width="2.6" height="5.5" rx="1.3"></rect>
                    <rect x="10.7" y="11" width="2.6" height="5" rx="1.3"></rect>
                    <rect x="13.45" y="8.5" width="2.6" height="5.5" rx="1.3"></rect>
                    <rect x="17.2" y="6" width="2.6" height="12" rx="1.3"></rect>
                </g>
            </svg>
        </span>
    }
}

/// App icon, centered (auth and loading screens).
#[component]
pub fn Brand(#[prop(optional)] big: bool) -> impl IntoView {
    view! {
        <div class="brand" class:big=big>
            <Logo big=big/>
        </div>
    }
}
