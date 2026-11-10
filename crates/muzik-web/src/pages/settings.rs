use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use muzik_common::{
    ChangePasswordRequest, CreateUserRequest, InkultSettings, InkultTestResult, Quality, ScanMode,
    ScanPhase, ScanProgress, SecurityPolicy, TotpConfirmRequest, TotpSetup, TotpStatus,
    UpdateInkultSettings, UpdateUserRequest, User,
};
use send_wrapper::SendWrapper;
use wasm_bindgen::{prelude::Closure, JsCast};

use crate::{
    api,
    components::{use_toasts, Monogram},
    icons::{Icon, Svg},
    overlays::{use_overlays, Overlay, Picker},
    player::use_player,
    theme::{parse_hex, use_looks, Theme, ACCENTS},
    use_auth, AuthState,
};

const QUALITIES: [(Quality, &str); 4] = [
    (Quality::Original, "Original"),
    (Quality::High, "High"),
    (Quality::Normal, "Normal"),
    (Quality::Low, "Low"),
];

const QUALITY_LABELS: [&str; 4] = [
    QUALITIES[0].1,
    QUALITIES[1].1,
    QUALITIES[2].1,
    QUALITIES[3].1,
];

#[component]
fn QualityPicker(label: &'static str, cellular: bool) -> impl IntoView {
    let player = use_player();
    let current = move || {
        let s = player.settings.get();
        if cellular {
            s.cellular_quality
        } else {
            s.wifi_quality
        }
    };
    view! {
        <div class="cell">
            <span class="cell-label">{label}</span>
            <Picker title=label options=&QUALITY_LABELS
                selected=Signal::derive(move || {
                    let cur = current();
                    QUALITIES.iter().position(|(q, _)| *q == cur).unwrap_or(0)
                })
                on_pick=Callback::new(move |i: usize| {
                    if let Some((q, _)) = QUALITIES.get(i) {
                        player.settings.update(|s| {
                            if cellular { s.cellular_quality = *q } else { s.wifi_quality = *q }
                        });
                    }
                })/>
        </div>
    }
}

#[component]
fn AppearanceGroup() -> impl IntoView {
    let looks = use_looks();
    let theme = move || looks.appearance.with(|a| a.theme);
    let accent = move || looks.appearance.with(|a| a.accent.clone());
    let set_accent = move |hex: String| {
        if parse_hex(&hex).is_some() {
            looks
                .appearance
                .update(|a| a.accent = hex.to_ascii_lowercase());
        }
    };
    let is_custom = move || {
        let a = accent();
        !ACCENTS.iter().any(|(_, h)| *h == a)
    };
    view! {
        <h3 class="group-header">"Appearance"</h3>
        <div class="group">
            <div class="cell">
                <span class="cell-label">"Theme"</span>
                <div class="segmented" role="group" aria-label="Theme">
                    {[(Theme::System, "Automatic"), (Theme::Light, "Light"), (Theme::Dark, "Dark")]
                        .into_iter()
                        .map(|(t, label)| view! {
                            <button class:active=move || theme() == t aria-pressed=move || (theme() == t).to_string()
                                on:click=move |_| looks.appearance.update(|a| a.theme = t)>{label}</button>
                        })
                        .collect_view()}
                </div>
            </div>
            <div class="cell stacked">
                <span class="cell-label">"Accent Color"</span>
                <div class="swatches" role="radiogroup" aria-label="Accent color">
                    {ACCENTS.into_iter().map(|(name, hex)| view! {
                        <button class="swatch" role="radio" title=name aria-label=name
                            aria-checked=move || (accent() == hex).to_string()
                            style:background-color=hex
                            on:click=move |_| set_accent(hex.to_owned())>
                            {move || (accent() == hex).then(|| view! { <Svg icon=Icon::Check/> })}
                        </button>
                    }).collect_view()}
                    <label class="swatch custom" class:selected=is_custom title="Custom color">
                        <input type="color" aria-label="Custom accent color"
                            prop:value=accent
                            on:input=move |ev| set_accent(event_target_value(&ev))/>
                    </label>
                </div>
                <div class="accent-preview" aria-hidden="true"></div>
            </div>
        </div>
        <p class="group-footer">"The accent tints buttons, highlights and the gradients across Muzik. Saved on this device."</p>
    }
}

#[component]
pub fn Settings() -> impl IntoView {
    let auth = use_auth();
    let logout = move |_| {
        spawn_local(async move {
            let _ = api::logout().await;
            auth.0.set(AuthState::LoggedOut);
        });
    };
    view! {
        <div class="page narrow">
            <h1 class="large-title">"Settings"</h1>

            {move || auth.user().map(|u| view! {
                <div class="group">
                    <div class="cell profile">
                        <Monogram name=u.display_name.clone() big=true/>
                        <span class="cell-text">
                            <span class="profile-name">{u.display_name.clone()}</span>
                            <span class="cell-sub">
                                {format!("@{}", u.username)}
                                {u.is_admin.then_some(" · Administrator")}
                                {u.inkult.then_some(" · Inkult Account")}
                            </span>
                        </span>
                    </div>
                </div>
            })}

            {move || if auth.user().is_some_and(|u| u.inkult) {
                view! {
                    <h3 class="group-header">"Security"</h3>
                    <div class="group">
                        <div class="cell">
                            <span class="cell-label">"Two-Factor Authentication"</span>
                            <span class="cell-value">
                                {move || if auth.user().is_some_and(|u| u.totp_enabled) { "On in Inkult" } else { "Off" }}
                            </span>
                        </div>
                    </div>
                    <p class="group-footer">
                        "You sign in with your Inkult account. Change your password and two-factor "
                        "login in Inkult; Muzik uses them from your next sign-in."
                    </p>
                }.into_any()
            } else {
                view! { <SecurityGroup/> }.into_any()
            }}

            <AppearanceGroup/>

            <h3 class="group-header">"Streaming Quality"</h3>
            <div class="group">
                <QualityPicker label="Wi-Fi" cellular=false/>
                <QualityPicker label="Mobile Data" cellular=true/>
            </div>
            <p class="group-footer">
                "Original streams the untouched file. High sends FLAC for lossless sources. "
                "Normal and Low use Opus at 160 and 96 kbps. Applies from the next song."
            </p>

            {move || auth.is_admin().then(|| view! { <ScanPanel/><InkultPanel/><UsersPanel/> })}

            <h3 class="group-header hide-touch">"Keyboard Shortcuts"</h3>
            <div class="group hide-touch">
                <div class="cell"><span class="cell-label">"Play / Pause"</span><span class="keys"><kbd>"Space"</kbd></span></div>
                <div class="cell"><span class="cell-label">"Seek 10 Seconds"</span><span class="keys"><kbd>"←"</kbd><kbd>"→"</kbd></span></div>
                <div class="cell"><span class="cell-label">"Previous / Next"</span><span class="keys"><kbd>"⇧"</kbd><kbd>"←"</kbd><kbd>"→"</kbd></span></div>
                <div class="cell"><span class="cell-label">"Volume"</span><span class="keys"><kbd>"↑"</kbd><kbd>"↓"</kbd></span></div>
                <div class="cell"><span class="cell-label">"Mute"</span><span class="keys"><kbd>"M"</kbd></span></div>
                <div class="cell"><span class="cell-label">"Search"</span><span class="keys"><kbd>"/"</kbd></span></div>
            </div>

            <div class="group spaced">
                <button class="cell action destructive" on:click=logout>"Sign Out"</button>
            </div>
        </div>
    }
}

/// What the Security group is currently asking for.
#[derive(Clone)]
enum SecurityStep {
    Idle,
    ChangePassword,
    /// 2FA setup, step 1: confirm the password.
    SetupPassword,
    /// 2FA setup, step 2: scan the QR code and enter a code.
    SetupScan(TotpSetup),
    /// Freshly generated recovery codes, shown once.
    Codes(Vec<String>),
    Disable,
    Regenerate,
}

#[component]
fn SecurityGroup() -> impl IntoView {
    let toasts = use_toasts();
    let auth = use_auth();
    let status = RwSignal::new(None::<TotpStatus>);
    let step = RwSignal::new(SecurityStep::Idle);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    // Form fields (cleared whenever the step changes).
    let current = RwSignal::new(String::new());
    let new_pw = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());

    let load = move || {
        spawn_local(async move {
            if let Ok(s) = api::totp_status().await {
                status.try_set(Some(s));
            }
        });
    };
    load();

    let go = move |next: SecurityStep| {
        for f in [current, new_pw, confirm, code] {
            f.set(String::new());
        }
        error.set(None);
        busy.set(false);
        step.set(next);
    };
    let set_totp_flag = move |on: bool| {
        auth.0.update(|s| {
            if let AuthState::In(u) = s {
                u.totp_enabled = on;
            }
        });
    };
    let fail = move |e: api::ApiError| {
        error.set(Some(e.message));
        busy.set(false);
    };

    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        error.set(None);
        let (pw, c) = (current.get_untracked(), code.get_untracked());
        match step.get_untracked() {
            SecurityStep::ChangePassword => {
                if new_pw.get_untracked() != confirm.get_untracked() {
                    error.set(Some("Passwords don't match.".into()));
                    return;
                }
                busy.set(true);
                let req = ChangePasswordRequest {
                    current_password: pw,
                    new_password: new_pw.get_untracked(),
                };
                spawn_local(async move {
                    match api::change_password(&req).await {
                        Ok(()) => {
                            toasts.show("Password changed. Other devices were signed out.");
                            go(SecurityStep::Idle);
                        }
                        Err(e) => fail(e),
                    }
                });
            }
            SecurityStep::SetupPassword => {
                busy.set(true);
                spawn_local(async move {
                    match api::totp_setup(pw).await {
                        Ok(setup) => go(SecurityStep::SetupScan(setup)),
                        Err(e) => fail(e),
                    }
                });
            }
            SecurityStep::SetupScan(_) => {
                busy.set(true);
                spawn_local(async move {
                    match api::totp_enable(c).await {
                        Ok(rc) => {
                            set_totp_flag(true);
                            load();
                            go(SecurityStep::Codes(rc.codes));
                        }
                        Err(e) => fail(e),
                    }
                });
            }
            SecurityStep::Disable => {
                busy.set(true);
                let req = TotpConfirmRequest {
                    password: pw,
                    code: c,
                };
                spawn_local(async move {
                    match api::totp_disable(&req).await {
                        Ok(()) => {
                            set_totp_flag(false);
                            load();
                            toasts.show("Two-factor authentication is off");
                            go(SecurityStep::Idle);
                        }
                        Err(e) => fail(e),
                    }
                });
            }
            SecurityStep::Regenerate => {
                busy.set(true);
                let req = TotpConfirmRequest {
                    password: pw,
                    code: c,
                };
                spawn_local(async move {
                    match api::totp_recovery_codes(&req).await {
                        Ok(rc) => {
                            load();
                            go(SecurityStep::Codes(rc.codes));
                        }
                        Err(e) => fail(e),
                    }
                });
            }
            SecurityStep::Idle | SecurityStep::Codes(_) => {}
        }
    };

    let enabled = move || status.with(|s| s.as_ref().is_some_and(|s| s.enabled));
    // Required by an admin: it can't be turned off from here.
    let enforced = move || auth.user().is_some_and(|u| u.totp_enforced);
    let idle = move || matches!(step.get(), SecurityStep::Idle);

    let password_field = move |label: &'static str, sig: RwSignal<String>, auto: &'static str| {
        view! {
            <label class="cell"><span class="cell-label">{label}</span>
                <input class="cell-input" type="password" autocomplete=auto placeholder="Required"
                    required bind:value=sig/></label>
        }
    };
    let code_field = move || {
        view! {
            <label class="cell"><span class="cell-label">"Code"</span>
                <input class="cell-input" type="text" autocomplete="one-time-code" inputmode="text"
                    autocapitalize="characters" spellcheck="false" maxlength="16"
                    placeholder="123456 or recovery code" required bind:value=code/></label>
        }
    };
    let actions = move |label: &'static str, destructive: bool| {
        view! {
            {move || error.get().map(|e| view! { <div class="cell"><p class="error" role="alert">{e}</p></div> })}
            <button class="cell action" class:destructive=destructive type="submit" disabled=move || busy.get()>{label}</button>
            <button class="cell action" type="button" on:click=move |_| go(SecurityStep::Idle)>"Cancel"</button>
        }
    };

    view! {
        <h3 class="group-header">"Security"</h3>
        <div class="group">
            <button class="cell action" disabled=move || !idle() on:click=move |_| go(SecurityStep::ChangePassword)>
                "Change Password"
            </button>
            <div class="cell">
                <span class="cell-text">
                    <span class="cell-label">"Two-Factor Authentication"</span>
                    {move || status.get().filter(|s| s.enabled).map(|s| view! {
                        <span class="cell-sub">{format!("{} recovery codes left", s.recovery_codes_left)}</span>
                    })}
                </span>
                <span class="cell-value">{move || if enabled() { "On" } else { "Off" }}</span>
            </div>
            {move || if enabled() {
                view! {
                    <button class="cell action" disabled=move || !idle() on:click=move |_| go(SecurityStep::Regenerate)>"New Recovery Codes"</button>
                    {move || (!enforced()).then(|| view! {
                        <button class="cell action" disabled=move || !idle() on:click=move |_| go(SecurityStep::Disable)>"Turn Off Two-Factor Authentication"</button>
                    })}
                }.into_any()
            } else {
                view! {
                    <button class="cell action" disabled=move || !idle() on:click=move |_| go(SecurityStep::SetupPassword)>"Set Up Authenticator App"</button>
                }.into_any()
            }}
        </div>
        <p class="group-footer">
            "With two-factor authentication, signing in also asks for a code from an authenticator app "
            "(such as Apple Passwords, Google Authenticator, 1Password or Aegis)."
            {move || enforced().then_some(" Your administrator requires it for your account.")}
        </p>

        {move || match step.get() {
            SecurityStep::Idle => ().into_any(),
            SecurityStep::ChangePassword => view! {
                <h3 class="group-header">"Change Password"</h3>
                <form class="group" on:submit=submit>
                    {password_field("Current", current, "current-password")}
                    {password_field("New", new_pw, "new-password")}
                    {password_field("Confirm", confirm, "new-password")}
                    {actions("Change Password", false)}
                </form>
                <p class="group-footer">"At least 8 characters. Other devices will be signed out."</p>
            }.into_any(),
            SecurityStep::SetupPassword => view! {
                <h3 class="group-header">"Set Up Two-Factor Authentication"</h3>
                <form class="group" on:submit=submit>
                    {password_field("Password", current, "current-password")}
                    {actions("Continue", false)}
                </form>
                <p class="group-footer">"Confirm your password to continue."</p>
            }.into_any(),
            SecurityStep::SetupScan(setup) => {
                let qr = format!(
                    "data:image/svg+xml;charset=utf-8,{}",
                    String::from(js_sys::encode_uri_component(&setup.qr_svg))
                );
                let grouped = setup.secret.as_bytes().chunks(4)
                    .map(|c| String::from_utf8_lossy(c).into_owned())
                    .collect::<Vec<_>>()
                    .join(" ");
                view! {
                    <h3 class="group-header">"Scan with Your Authenticator App"</h3>
                    <form class="group" on:submit=submit>
                        <div class="cell stacked totp-setup">
                            <img class="totp-qr" src=qr width="200" height="200"
                                alt="QR code containing the two-factor setup key"/>
                            <span class="cell-sub">"Can't scan? Enter this setup key:"</span>
                            <code class="totp-secret">{grouped}</code>
                        </div>
                        <label class="cell"><span class="cell-label">"Code"</span>
                            <input class="cell-input" type="text" autocomplete="one-time-code" inputmode="numeric"
                                maxlength="7" placeholder="6-digit code" required bind:value=code/></label>
                        {actions("Turn On", false)}
                    </form>
                    <p class="group-footer">"Enter the 6-digit code the app shows to finish. Other devices will be signed out."</p>
                }.into_any()
            }
            SecurityStep::Codes(codes) => {
                let text = codes.join("\n");
                let copy = move |_| {
                    let text = text.clone();
                    let ok = window().navigator().clipboard().write_text(&text);
                    spawn_local(async move {
                        if wasm_bindgen_futures::JsFuture::from(ok).await.is_ok() {
                            toasts.show("Recovery codes copied");
                        } else {
                            toasts.show("Could not copy, please write them down");
                        }
                    });
                };
                view! {
                    <h3 class="group-header">"Recovery Codes"</h3>
                    <div class="group">
                        <div class="cell stacked">
                            <ul class="recovery-codes">
                                {codes.into_iter().map(|c| view! { <li><code>{c}</code></li> }).collect_view()}
                            </ul>
                        </div>
                        <button class="cell action" type="button" on:click=copy>"Copy Codes"</button>
                        <button class="cell action" type="button" on:click=move |_| go(SecurityStep::Idle)>"I Saved Them"</button>
                    </div>
                    <p class="group-footer">
                        "Each code signs you in once if you lose your phone. Store them somewhere safe: "
                        "they won't be shown again, and any older codes no longer work."
                    </p>
                }.into_any()
            }
            SecurityStep::Disable => view! {
                <h3 class="group-header">"Turn Off Two-Factor Authentication"</h3>
                <form class="group" on:submit=submit>
                    {password_field("Password", current, "current-password")}
                    {code_field()}
                    {actions("Turn Off", true)}
                </form>
            }.into_any(),
            SecurityStep::Regenerate => view! {
                <h3 class="group-header">"New Recovery Codes"</h3>
                <form class="group" on:submit=submit>
                    {password_field("Password", current, "current-password")}
                    {code_field()}
                    {actions("Generate", false)}
                </form>
                <p class="group-footer">"Your current recovery codes will stop working."</p>
            }.into_any(),
        }}
    }
}

#[component]
fn ScanPanel() -> impl IntoView {
    let toasts = use_toasts();
    let progress = RwSignal::new(None::<ScanProgress>);

    spawn_local(async move {
        if let Ok(p) = api::scan_status().await {
            progress.try_set(Some(p));
        }
    });

    // Live progress over server-sent events.
    if let Ok(es) = web_sys::EventSource::new("/api/scan/events") {
        let cb = Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |ev: web_sys::MessageEvent| {
            if let Some(Ok(p)) = ev
                .data()
                .as_string()
                .map(|s| serde_json::from_str::<ScanProgress>(&s))
            {
                progress.try_set(Some(p));
            }
        });
        let _ = es.add_event_listener_with_callback("progress", cb.as_ref().unchecked_ref());
        let holder = SendWrapper::new((es, cb));
        on_cleanup(move || holder.0.close());
    }

    let running = move || {
        progress.with(|p| {
            p.as_ref().is_some_and(|p| {
                matches!(
                    p.phase,
                    ScanPhase::Walking | ScanPhase::Reading | ScanPhase::Finalizing
                )
            })
        })
    };
    let start = move |mode: ScanMode| {
        spawn_local(async move {
            match api::start_scan(mode).await {
                Ok(()) => toasts.show("Scan started"),
                Err(e) => toasts.show(e.message),
            }
        });
    };

    view! {
        <h3 class="group-header">"Library"</h3>
        <div class="group">
            <button class="cell action" disabled=running on:click=move |_| start(ScanMode::Incremental)>"Scan for Changes"</button>
            <button class="cell action" disabled=running on:click=move |_| start(ScanMode::Full)>"Full Rescan"</button>
            {move || progress.get().map(|p| {
                let pct = if p.files_to_read > 0 {
                    (p.files_read as f64 / p.files_to_read as f64 * 100.0).min(100.0)
                } else if running() { 0.0 } else { 100.0 };
                let phase = match p.phase {
                    ScanPhase::Idle => "Idle",
                    ScanPhase::Walking => "Looking for files…",
                    ScanPhase::Reading => "Reading tags…",
                    ScanPhase::Finalizing => "Finishing…",
                    ScanPhase::Done => "Up to Date",
                    ScanPhase::Failed => "Scan Failed",
                };
                view! {
                    <div class="cell stacked">
                        <div class="scan-head">
                            <span class="cell-label">{phase}</span>
                            <span class="cell-value">{format!("{} files", p.files_seen)}</span>
                        </div>
                        {running().then(|| view! {
                            <div class="meter"><div style:width=format!("{pct:.1}%")></div></div>
                        })}
                        <span class="cell-sub">
                            {format!("{} added · {} updated · {} removed · {} errors", p.added, p.updated, p.removed, p.errors)}
                        </span>
                        {p.message.map(|m| view! { <span class="error">{m}</span> })}
                    </div>
                }
            })}
        </div>
        <p class="group-footer">"To add music, copy files into the library folder on the server, then scan for changes."</p>
    }
}

#[component]
fn UsersPanel() -> impl IntoView {
    let toasts = use_toasts();
    let auth = use_auth();
    let users = RwSignal::new(Vec::<User>::new());
    let reload = move || {
        spawn_local(async move {
            match api::users().await {
                Ok(u) => {
                    users.try_set(u);
                }
                Err(e) => toasts.show(e.message),
            }
        });
    };
    reload();

    // Server-wide 2FA policy.
    let policy = RwSignal::new(None::<SecurityPolicy>);
    spawn_local(async move {
        match api::security_policy().await {
            Ok(p) => {
                policy.try_set(Some(p));
            }
            Err(e) => toasts.show(e.message),
        }
    });
    let required_for_all = move || policy.with(|p| p.as_ref().is_some_and(|p| p.require_totp));
    let ov = use_overlays();
    let apply_policy = move |require_totp: bool| {
        spawn_local(async move {
            match api::set_security_policy(&SecurityPolicy { require_totp }).await {
                Ok(p) => {
                    policy.try_set(Some(p));
                    // Keep the admin's own "required" state in sync.
                    auth.0.update(|s| {
                        if let AuthState::In(u) = s {
                            u.totp_enforced = require_totp || u.totp_required;
                        }
                    });
                    toasts.show(if require_totp {
                        "Two-factor authentication is now required for everyone"
                    } else {
                        "Two-factor authentication is now optional"
                    });
                    reload();
                }
                Err(e) => toasts.show(e.message),
            }
        });
    };
    let toggle_policy = move |ev: leptos::ev::MouseEvent| {
        // The switch only moves once the server agrees.
        ev.prevent_default();
        if required_for_all() {
            apply_policy(false);
            return;
        }
        let missing =
            users.with_untracked(|us| us.iter().filter(|u| !u.disabled && !u.totp_enabled).count());
        ov.open(Overlay::Confirm {
            title: "Require Two-Factor Authentication?".into(),
            message: match missing {
                0 => "Every user already uses two-factor authentication. New users will have to set it up on first sign-in.".into(),
                1 => "1 user doesn't use it yet and will have to set it up before they can use Muzik again.".into(),
                n => format!("{n} users don't use it yet and will have to set it up before they can use Muzik again."),
            },
            action: "Require".into(),
            on_confirm: Callback::new(move |()| apply_policy(true)),
        });
    };

    let username = RwSignal::new(String::new());
    let display = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let is_admin = RwSignal::new(false);
    let require_2fa = RwSignal::new(false);
    let via_inkult = RwSignal::new(false);
    let create = move |ev: SubmitEvent| {
        ev.prevent_default();
        let d = display.get_untracked();
        let inkult = via_inkult.get_untracked();
        let req = CreateUserRequest {
            username: username.get_untracked(),
            display_name: (!d.trim().is_empty()).then_some(d),
            password: if inkult {
                String::new()
            } else {
                password.get_untracked()
            },
            inkult,
            is_admin: is_admin.get_untracked(),
            totp_required: require_2fa.get_untracked(),
        };
        spawn_local(async move {
            match api::create_user(&req).await {
                Ok(u) => {
                    toasts.show(format!("Created {}", u.username));
                    username.set(String::new());
                    display.set(String::new());
                    password.set(String::new());
                    is_admin.set(false);
                    require_2fa.set(false);
                    via_inkult.set(false);
                    reload();
                }
                Err(e) => toasts.show(e.message),
            }
        });
    };
    let update = move |id: i64, req: UpdateUserRequest, done: &'static str| {
        spawn_local(async move {
            match api::update_user(id, &req).await {
                Ok(_) => {
                    if !done.is_empty() {
                        toasts.show(done);
                    }
                    reload();
                }
                Err(e) => toasts.show(e.message),
            }
        });
    };
    let blank = || UpdateUserRequest {
        display_name: None,
        disabled: None,
        is_admin: None,
        password: None,
        reset_totp: false,
        totp_required: None,
        inkult: None,
    };
    let set_disabled = move |id: i64, disabled: bool| {
        update(
            id,
            UpdateUserRequest {
                disabled: Some(disabled),
                ..blank()
            },
            "",
        );
    };
    let set_required = move |id: i64, required: bool| {
        update(
            id,
            UpdateUserRequest {
                totp_required: Some(required),
                ..blank()
            },
            if required {
                "Two-factor authentication required"
            } else {
                "Two-factor authentication is optional for them"
            },
        );
    };
    let reset_totp = move |u: User| {
        let id = u.id;
        let after = if u.totp_enforced {
            "will have to set up two-factor authentication again on their next sign-in"
        } else {
            "will be able to sign in with just their password"
        };
        ov.open(Overlay::Confirm {
            title: "Reset Two-Factor Authentication?".into(),
            message: format!(
                "@{} {after} and will be signed out everywhere. \
                 Only do this if you're sure it's really them asking.",
                u.username
            ),
            action: "Reset".into(),
            on_confirm: Callback::new(move |()| {
                update(
                    id,
                    UpdateUserRequest {
                        reset_totp: true,
                        ..blank()
                    },
                    "Two-factor authentication reset",
                )
            }),
        });
    };
    let link_inkult = move |u: User| {
        let id = u.id;
        ov.open(Overlay::Confirm {
            title: "Sign In with Inkult?".into(),
            message: format!(
                "@{0} will sign in with the Inkult account named \"{0}\" instead of a Muzik password. \
                 Their Muzik password and two-factor authentication are removed and they are signed \
                 out everywhere. Their playlists stay.",
                u.username
            ),
            action: "Use Inkult".into(),
            on_confirm: Callback::new(move |()| {
                update(
                    id,
                    UpdateUserRequest {
                        inkult: Some(true),
                        ..blank()
                    },
                    "They now sign in with Inkult",
                )
            }),
        });
    };
    // User whose password is being reset, and the new password.
    let resetting = RwSignal::new(None::<i64>);
    let new_password = RwSignal::new(String::new());
    let save_password = move |ev: SubmitEvent| {
        ev.prevent_default();
        let Some(id) = resetting.get_untracked() else {
            return;
        };
        update(
            id,
            UpdateUserRequest {
                password: Some(new_password.get_untracked()),
                ..blank()
            },
            "Password reset. They were signed out everywhere.",
        );
        resetting.set(None);
        new_password.set(String::new());
    };
    let me = move || auth.user().map(|u| u.id);

    view! {
        <h3 class="group-header">"Two-Factor Authentication"</h3>
        <div class="group">
            <label class="cell"><span class="cell-label">"Require for Everyone"</span>
                <input class="switch" type="checkbox" role="switch"
                    disabled=move || policy.with(|p| p.is_none())
                    prop:checked=required_for_all
                    on:click=toggle_policy/></label>
        </div>
        <p class="group-footer">
            {move || if required_for_all() {
                "Everyone must use an authenticator app. Users without one are asked to set it up before they can use Muzik, and nobody can turn it off."
            } else {
                "When off, you can still require it for specific users below. Turn it on for your own account first."
            }}
        </p>

        <h3 class="group-header">"Users"</h3>
        <div class="group">
            {move || users.get().into_iter().map(|u| {
                let id = u.id;
                let disabled = u.disabled;
                let required = u.totp_required;
                let other = me() != Some(id);
                let inkult = u.inkult;
                let u2 = u.clone();
                let u3 = u.clone();
                view! {
                    <div class="cell" class:dimmed=disabled>
                        <Monogram name=u.display_name.clone()/>
                        <span class="cell-text">
                            <span class="cell-label">{u.display_name.clone()}</span>
                            <span class="cell-sub">
                                {format!("@{}", u.username)}
                                {u.is_admin.then_some(" · Admin")}
                                {u.inkult.then_some(" · Inkult")}
                                {if u.totp_enabled {
                                    Some(" · 2FA")
                                } else if u.totp_enforced {
                                    Some(" · 2FA Setup Pending")
                                } else {
                                    None
                                }}
                                {u.disabled.then_some(" · Disabled")}
                            </span>
                        </span>
                        {other.then(|| view! {
                            <span class="user-actions">
                                // An Inkult account's 2FA is reset in Inkult.
                                {(u.totp_enabled && !inkult).then(|| view! {
                                    <button class="btn small" on:click=move |_| reset_totp(u2.clone())>"Reset 2FA"</button>
                                })}
                                {(!inkult).then(|| view! {
                                    <button class="btn small" title="Sign in with the Inkult account of the same name"
                                        on:click=move |_| link_inkult(u3.clone())>"Use Inkult"</button>
                                })}
                                // Per-user requirement (moot while the server requires it for everyone).
                                {move || (!required_for_all()).then(|| {
                                    view! {
                                        <button class="btn small" class:tinted=required aria-pressed=required.to_string()
                                            title=if required { "Two-factor authentication is required for this user" } else { "Make two-factor authentication mandatory for this user" }
                                            on:click=move |_| set_required(id, !required)>
                                            {if required { "2FA Required" } else { "Require 2FA" }}
                                        </button>
                                    }
                                })}
                                <button class="btn small" aria-expanded=move || (resetting.get() == Some(id)).to_string()
                                    on:click=move |_| {
                                        new_password.set(String::new());
                                        resetting.update(|r| *r = if *r == Some(id) { None } else { Some(id) });
                                    }>{if inkult { "Set Password" } else { "Password" }}</button>
                                <button class="btn small" class:tinted=disabled on:click=move |_| set_disabled(id, !disabled)>
                                    {if disabled { "Enable" } else { "Disable" }}
                                </button>
                            </span>
                        })}
                    </div>
                    {move || (resetting.get() == Some(id)).then(|| view! {
                        <form class="cell" on:submit=save_password>
                            <span class="cell-label">"New Password"</span>
                            <input class="cell-input" type="password" autocomplete="new-password"
                                placeholder="At least 8 characters" required minlength="8" bind:value=new_password/>
                            <button class="btn small tinted" type="submit">"Reset"</button>
                        </form>
                        {inkult.then(|| view! {
                            <p class="cell cell-sub">"They will sign in with this Muzik password instead of Inkult."</p>
                        })}
                    })}
                }
            }).collect_view()}
        </div>

        <h3 class="group-header">"Add User"</h3>
        <form class="group" on:submit=create>
            <label class="cell"><span class="cell-label">"Username"</span>
                <input class="cell-input" type="text" autocomplete="off" autocapitalize="none" placeholder="Required" required bind:value=username/></label>
            <label class="cell"><span class="cell-label">"Display Name"</span>
                <input class="cell-input" type="text" autocomplete="off" placeholder="Optional" bind:value=display/></label>
            <label class="cell"><span class="cell-label">"Sign In with Inkult"</span>
                <input class="switch" type="checkbox" role="switch" bind:checked=via_inkult/></label>
            {move || (!via_inkult.get()).then(|| view! {
                <label class="cell"><span class="cell-label">"Password"</span>
                    <input class="cell-input" type="password" autocomplete="new-password" placeholder="Required" required bind:value=password/></label>
            })}
            <label class="cell"><span class="cell-label">"Administrator"</span>
                <input class="switch" type="checkbox" role="switch" bind:checked=is_admin/></label>
            {move || (!required_for_all()).then(|| view! {
                <label class="cell"><span class="cell-label">"Require Two-Factor Authentication"</span>
                    <input class="switch" type="checkbox" role="switch" bind:checked=require_2fa/></label>
            })}
            <button class="cell action" type="submit">"Create User"</button>
        </form>
        <p class="group-footer">
            {move || if via_inkult.get() {
                "Inkult users get their account at their first sign-in. Add one here only to set it up beforehand, e.g. as an administrator: use their Inkult handle as the username."
            } else {
                "They can change their password from Settings once signed in."
            }}
        </p>
    }
}

/// Inkult connector: while on, every Inkult account is a Muzik user.
///
/// Turning it on or changing the address decides where Inkult users'
/// passwords are sent, so the server asks the admin to confirm with their
/// password (`reauth_required`); the panel then shows a confirmation step and
/// sends the change again with it.
#[component]
fn InkultPanel() -> impl IntoView {
    let toasts = use_toasts();
    let auth = use_auth();
    // What the server has; the form edits a copy.
    let saved = RwSignal::new(None::<InkultSettings>);
    let enabled = RwSignal::new(false);
    let url = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    // Last connection test: the address tested and its outcome.
    let status = RwSignal::new(None::<(String, Result<InkultTestResult, String>)>);
    // A change waiting for the admin's password (and toast to show after).
    let confirm = RwSignal::new(None::<(InkultSettings, &'static str)>);
    let password = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let confirm_error = RwSignal::new(None::<String>);

    let show = move |s: InkultSettings| {
        enabled.set(s.enabled);
        url.set(s.url.clone());
        saved.try_set(Some(s));
    };
    spawn_local(async move {
        match api::inkult_settings().await {
            Ok(s) => show(s),
            Err(e) => toasts.show(e.message),
        }
    });

    let close_confirm = move || {
        confirm.set(None);
        password.set(String::new());
        code.set(String::new());
        confirm_error.set(None);
    };
    let save = move |next: InkultSettings, done: &'static str| {
        busy.set(true);
        let confirming = confirm.get_untracked().is_some();
        let body = UpdateInkultSettings {
            enabled: next.enabled,
            url: next.url.clone(),
            password: confirming.then(|| password.get_untracked()),
            code: confirming
                .then(|| code.get_untracked())
                .filter(|c| !c.trim().is_empty()),
        };
        spawn_local(async move {
            match api::set_inkult_settings(&body).await {
                Ok(s) => {
                    close_confirm();
                    show(s);
                    toasts.show(done);
                }
                // Ask for the password, keeping the form as typed.
                Err(e) if e.code == "reauth_required" => {
                    confirm_error.set(None);
                    confirm.set(Some((next, done)));
                }
                Err(e)
                    if confirming
                        && matches!(e.code.as_str(), "wrong_password" | "invalid_code") =>
                {
                    confirm_error.set(Some(e.message));
                }
                Err(e) => {
                    close_confirm();
                    // Put the form back to what the server has.
                    if let Some(s) = saved.get_untracked() {
                        show(s);
                    }
                    toasts.show(e.message);
                }
            }
            busy.set(false);
        });
    };
    let current = move || InkultSettings {
        enabled: enabled.get_untracked(),
        url: url.get_untracked(),
        ..Default::default()
    };
    let toggle_enabled = move |ev: leptos::ev::MouseEvent| {
        // The switch only moves once the server agrees.
        ev.prevent_default();
        let on = !enabled.get_untracked();
        save(
            InkultSettings {
                enabled: on,
                ..current()
            },
            if on {
                "Inkult sign-in is on"
            } else {
                "Inkult sign-in is off"
            },
        );
    };
    let submit_url = move |ev: SubmitEvent| {
        ev.prevent_default();
        save(current(), "Inkult address saved");
    };
    let submit_confirm = move |ev: SubmitEvent| {
        ev.prevent_default();
        if let Some((next, done)) = confirm.get_untracked() {
            save(next, done);
        }
    };
    let test = move |_| {
        status.set(None);
        busy.set(true);
        let u = url.get_untracked();
        spawn_local(async move {
            let r = api::test_inkult(u.clone()).await.map_err(|e| e.message);
            status.try_set(Some((u, r)));
            busy.set(false);
        });
    };
    let same_url =
        |a: &str, b: &str| a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/');
    let dirty = move || saved.with(|s| s.as_ref().is_some_and(|s| !same_url(&s.url, &url.get())));
    let loading = move || saved.with(|s| s.is_none());
    // Plain http:// off the local network, as the server judged the address
    // shown (from the last test of it, or the saved settings).
    let insecure = move || {
        let u = url.get();
        let tested = status.with(|t| match t {
            Some((tu, Ok(r))) if same_url(tu, &u) => Some(r.insecure),
            _ => None,
        });
        tested
            .or_else(|| {
                saved.with(|s| {
                    s.as_ref()
                        .filter(|s| same_url(&s.url, &u))
                        .map(|s| s.insecure)
                })
            })
            .unwrap_or(false)
    };
    let needs_code = move || auth.user().is_some_and(|u| u.totp_enabled && !u.inkult);

    // Separate libraries: saved on its own, no confirmation needed (it only
    // narrows what Inkult users see).
    let separate = move || saved.with(|s| s.as_ref().is_some_and(|s| s.separate_libraries));
    let index = move || saved.with(|s| s.as_ref().and_then(|s| s.library_index.clone()));
    let toggle_separate = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        let on = !separate();
        busy.set(true);
        spawn_local(async move {
            match api::set_inkult_libraries(on).await {
                Ok(s) => {
                    show(s);
                    toasts.show(if on {
                        "Each Inkult user now hears only their own songs"
                    } else {
                        "Everyone hears the whole library"
                    });
                }
                Err(e) => toasts.show(e.message),
            }
            busy.set(false);
        });
    };

    view! {
        <h3 class="group-header">"Inkult"</h3>
        <form class="group" on:submit=submit_url>
            <label class="cell"><span class="cell-label">"Server Address"</span>
                <input class="cell-input" type="url" inputmode="url" autocomplete="off" autocapitalize="none"
                    spellcheck="false" placeholder="http://inkult:9876" bind:value=url/></label>
            {move || status.get().map(|(_, r)| match r {
                Ok(r) => view! { <div class="cell"><span class="cell-sub">{format!("Connected to Inkult {}", r.version)}</span></div> }.into_any(),
                Err(e) => view! { <div class="cell"><p class="error" role="alert">{e}</p></div> }.into_any(),
            })}
            {move || insecure().then(|| view! {
                <div class="cell"><p class="warning-note" role="note">
                    "This address uses http:// outside your local network. Inkult passwords and codes would "
                    "travel unencrypted, and anyone on the way could read them or sign in as any Inkult user. "
                    "Use an https:// address."
                </p></div>
            })}
            <button class="cell action" type="button" on:click=test
                disabled=move || busy.get() || url.with(|u| u.trim().is_empty())>"Test Connection"</button>
            {move || dirty().then(|| view! {
                <button class="cell action" type="submit" disabled=move || busy.get() || confirm.with(Option::is_some)>"Save Address"</button>
            })}
            <label class="cell"><span class="cell-label">"Sign In with Inkult"</span>
                <input class="switch" type="checkbox" role="switch"
                    disabled=move || busy.get() || loading() || confirm.with(Option::is_some)
                    prop:checked=move || enabled.get()
                    on:click=toggle_enabled/></label>
        </form>
        {move || confirm.with(Option::is_some).then(|| view! {
            <h3 class="group-header">"Confirm It's You"</h3>
            <form class="group" on:submit=submit_confirm>
                <label class="cell"><span class="cell-label">"Your Password"</span>
                    <input class="cell-input" type="password" autocomplete="current-password" placeholder="Required"
                        required autofocus bind:value=password/></label>
                {move || needs_code().then(|| view! {
                    <label class="cell"><span class="cell-label">"Code"</span>
                        <input class="cell-input" type="text" autocomplete="one-time-code" inputmode="text"
                            autocapitalize="characters" spellcheck="false" maxlength="16"
                            placeholder="123456 or recovery code" required bind:value=code/></label>
                })}
                {move || confirm_error.get().map(|e| view! { <div class="cell"><p class="error" role="alert">{e}</p></div> })}
                <button class="cell action" type="submit" disabled=move || busy.get()>"Confirm"</button>
                <button class="cell action" type="button" on:click=move |_| {
                    close_confirm();
                    if let Some(s) = saved.get_untracked() {
                        show(s);
                    }
                }>"Cancel"</button>
            </form>
            <p class="group-footer">
                "Muzik sends the password of every Inkult user who signs in to this address, so changing it needs your password."
            </p>
        })}
        <p class="group-footer">
            "When on, every Inkult user can use Muzik: they sign in with their Inkult handle and password "
            "(and two-factor login), and their Muzik account is created the first time. "
            "If Muzik reaches Inkult by a host name (e.g. inkult in Docker), add it to INKULT_ALLOWED_HOSTS on the Inkult server."
        </p>
        <div class="group">
            <label class="cell"><span class="cell-label">"Separate Libraries"</span>
                <input class="switch" type="checkbox" role="switch"
                    disabled=move || busy.get() || loading() || (index().is_none() && !separate())
                    prop:checked=separate
                    on:click=toggle_separate/></label>
            {move || index().map(|i| {
                let found = format!("{} of {} songs from Inkult found in the library", i.matched, i.files);
                view! {
                    <div class="cell"><span class="cell-sub">{found}</span></div>
                    {i.error.map(|e| view! { <div class="cell"><p class="error" role="alert">{e}</p></div> })}
                }
            })}
        </div>
        <p class="group-footer">
            {move || if index().is_some() {
                "When on, each Inkult user only hears the songs they own in Inkult, plus songs added to the \
                 music folder by hand. Inkult captains, Muzik admins and accounts with a Muzik password hear everything."
            } else {
                "Needs Inkult's library.json: mount Inkult's data folder read-only and set MUZIK_INKULT_LIBRARY."
            }}
        </p>
    }
}
