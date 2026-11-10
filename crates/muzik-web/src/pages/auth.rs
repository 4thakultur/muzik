use leptos::{ev::SubmitEvent, prelude::*, task::spawn_local};
use muzik_common::{LoginRequest, LoginResponse, SetupRequest, TotpSetup};

use crate::{api, icons::Brand, use_auth, AuthState};

#[component]
pub fn Login() -> impl IntoView {
    let auth = use_auth();
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    // Password accepted, waiting for the authenticator / recovery code.
    let mfa = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    // Inkult accounts can sign in too (an admin connected an Inkult server).
    let inkult = RwSignal::new(false);
    spawn_local(async move {
        if let Ok(s) = api::setup_status().await {
            inkult.try_set(s.inkult_sign_in);
        }
    });

    let handle = move |res: Result<LoginResponse, api::ApiError>| match res {
        Ok(LoginResponse::Ok { user }) => auth.0.set(AuthState::In(user)),
        Ok(LoginResponse::MfaRequired) => {
            password.set(String::new());
            code.set(String::new());
            mfa.set(true);
            busy.set(false);
        }
        Err(e) => {
            // The sign-in is over (expired, or refused after the code was
            // accepted): another code can't help, so go back to the form.
            if matches!(
                e.code.as_str(),
                "challenge_expired"
                    | "inkult_name_taken"
                    | "inkult_account_setup"
                    | "inkult_mfa_required"
            ) {
                mfa.set(false);
                code.set(String::new());
            }
            error.set(Some(e.message));
            busy.set(false);
        }
    };

    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        if mfa.get_untracked() {
            let c = code.get_untracked();
            spawn_local(async move { handle(api::login_2fa(c).await) });
        } else {
            let req = LoginRequest {
                username: username.get_untracked(),
                password: password.get_untracked(),
            };
            spawn_local(async move { handle(api::login(&req).await) });
        }
    };

    let back = move |_| {
        mfa.set(false);
        error.set(None);
        code.set(String::new());
    };

    view! {
        <div class="splash">
            <form class="auth-card" on:submit=submit>
                <Brand big=true/>
                {move || if mfa.get() {
                    view! {
                        <h1 class="auth-title">"Two-Factor Authentication"</h1>
                        <p class="auth-sub">"Enter the 6-digit code from your authenticator app, or one of your recovery codes."</p>
                        <div class="field-group">
                            <input type="text" placeholder="Code" aria-label="Verification code"
                                autocomplete="one-time-code" autocapitalize="characters" spellcheck="false"
                                maxlength="24" required autofocus bind:value=code/>
                        </div>
                    }.into_any()
                } else {
                    view! {
                        <h1 class="auth-title">"Sign in to Muzik"</h1>
                        <p class="auth-sub">
                            {move || if inkult.get() {
                                "Use your Muzik account, or your Inkult handle and password."
                            } else {
                                "Use the account your server admin created for you."
                            }}
                        </p>
                        <div class="field-group">
                            <input type="text" placeholder="Username" aria-label="Username"
                                autocomplete="username" autocapitalize="none" required bind:value=username/>
                            <input type="password" placeholder="Password" aria-label="Password"
                                autocomplete="current-password" required bind:value=password/>
                        </div>
                    }.into_any()
                }}
                {move || error.get().map(|e| view! { <p class="error" role="alert">{e}</p> })}
                <button class="btn primary large" type="submit" disabled=move || busy.get()>
                    {move || if mfa.get() { "Verify" } else { "Sign In" }}
                </button>
                {move || mfa.get().then(|| view! {
                    <button class="btn large" type="button" on:click=back>"Back"</button>
                })}
            </form>
        </div>
    }
}

#[component]
pub fn Setup() -> impl IntoView {
    let auth = use_auth();
    let username = RwSignal::new(String::new());
    let display = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if password.get_untracked() != confirm.get_untracked() {
            error.set(Some("Passwords don't match.".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        let d = display.get_untracked();
        let req = SetupRequest {
            username: username.get_untracked(),
            display_name: (!d.trim().is_empty()).then_some(d),
            password: password.get_untracked(),
        };
        spawn_local(async move {
            match api::setup(&req).await {
                Ok(u) => auth.0.set(AuthState::In(u)),
                Err(e) => {
                    error.set(Some(e.message));
                    busy.set(false);
                }
            }
        });
    };

    view! {
        <div class="splash">
            <form class="auth-card" on:submit=submit>
                <Brand big=true/>
                <h1 class="auth-title">"Welcome to Muzik"</h1>
                <p class="auth-sub">"Create the administrator account for this server."</p>
                <div class="field-group">
                    <input type="text" placeholder="Username" aria-label="Username"
                        autocomplete="username" autocapitalize="none" required bind:value=username/>
                    <input type="text" placeholder="Display name (optional)" aria-label="Display name"
                        autocomplete="name" bind:value=display/>
                </div>
                <div class="field-group">
                    <input type="password" placeholder="Password" aria-label="Password"
                        autocomplete="new-password" required bind:value=password/>
                    <input type="password" placeholder="Confirm password" aria-label="Confirm password"
                        autocomplete="new-password" required bind:value=confirm/>
                </div>
                {move || error.get().map(|e| view! { <p class="error" role="alert">{e}</p> })}
                <button class="btn primary large" type="submit" disabled=move || busy.get()>"Create Account"</button>
            </form>
        </div>
    }
}

/// Steps of the mandatory 2FA enrollment.
#[derive(Clone)]
enum Enroll {
    Password,
    Scan(TotpSetup),
    Codes(Vec<String>),
}

/// Shown instead of the app when an admin made 2FA mandatory for this account
/// and it isn't set up yet (the server refuses everything else meanwhile).
#[component]
pub fn RequiredTotpSetup() -> impl IntoView {
    let auth = use_auth();
    let step = RwSignal::new(Enroll::Password);
    // Feedback for "Copy Codes" (toasts only render inside the app shell).
    let copied = RwSignal::new(None::<&'static str>);
    let password = RwSignal::new(String::new());
    let code = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    let fail = move |e: api::ApiError| {
        if e.is_unauthorized() {
            auth.0.set(AuthState::LoggedOut);
            return;
        }
        error.set(Some(e.message));
        busy.set(false);
    };
    let go = move |next: Enroll| {
        password.set(String::new());
        code.set(String::new());
        error.set(None);
        busy.set(false);
        step.set(next);
    };

    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        error.set(None);
        match step.get_untracked() {
            Enroll::Password => {
                busy.set(true);
                let pw = password.get_untracked();
                spawn_local(async move {
                    match api::totp_setup(pw).await {
                        Ok(setup) => go(Enroll::Scan(setup)),
                        Err(e) => fail(e),
                    }
                });
            }
            Enroll::Scan(_) => {
                busy.set(true);
                let c = code.get_untracked();
                spawn_local(async move {
                    match api::totp_enable(c).await {
                        Ok(rc) => go(Enroll::Codes(rc.codes)),
                        Err(e) => fail(e),
                    }
                });
            }
            // Recovery codes saved: the account is now allowed in.
            Enroll::Codes(_) => auth.refresh(),
        }
    };

    let logout = move |_| {
        spawn_local(async move {
            let _ = api::logout().await;
            auth.0.set(AuthState::LoggedOut);
        });
    };

    // Inkult accounts get their 2FA from Inkult: nothing to enroll here.
    if auth.user().is_some_and(|u| u.inkult) {
        return view! {
            <div class="splash">
                <div class="auth-card">
                    <Brand big=true/>
                    <h1 class="auth-title">"Two-Factor Authentication Required"</h1>
                    <p class="auth-sub">
                        "Your administrator requires two-factor authentication. You sign in with your "
                        "Inkult account, so turn on two-factor login in Inkult, then sign in here again."
                    </p>
                    <button class="btn primary large" type="button" on:click=logout>"Sign Out"</button>
                </div>
            </div>
        }
        .into_any();
    }

    view! {
        <div class="splash">
            <form class="auth-card" on:submit=submit>
                <Brand big=true/>
                {move || match step.get() {
                    Enroll::Password => view! {
                        <h1 class="auth-title">"Set Up Two-Factor Authentication"</h1>
                        <p class="auth-sub">
                            "Your administrator requires a code from an authenticator app (such as Apple Passwords, "
                            "Google Authenticator, 1Password or Aegis) when you sign in. Confirm your password to start."
                        </p>
                        <div class="field-group">
                            <input type="password" placeholder="Password" aria-label="Password"
                                autocomplete="current-password" required autofocus bind:value=password/>
                        </div>
                    }.into_any(),
                    Enroll::Scan(setup) => {
                        let qr = format!(
                            "data:image/svg+xml;charset=utf-8,{}",
                            String::from(js_sys::encode_uri_component(&setup.qr_svg))
                        );
                        let grouped = setup.secret.as_bytes().chunks(4)
                            .map(|c| String::from_utf8_lossy(c).into_owned())
                            .collect::<Vec<_>>()
                            .join(" ");
                        view! {
                            <h1 class="auth-title">"Scan the QR Code"</h1>
                            <p class="auth-sub">"Add Muzik to your authenticator app, then enter the 6-digit code it shows."</p>
                            <div class="totp-setup enroll">
                                <img class="totp-qr" src=qr width="200" height="200"
                                    alt="QR code containing the two-factor setup key"/>
                                <span class="auth-sub">"Can't scan? Enter this setup key:"</span>
                                <code class="totp-secret">{grouped}</code>
                            </div>
                            <div class="field-group">
                                <input type="text" placeholder="6-digit code" aria-label="Verification code"
                                    autocomplete="one-time-code" inputmode="numeric" maxlength="7"
                                    required autofocus bind:value=code/>
                            </div>
                        }.into_any()
                    }
                    Enroll::Codes(codes) => {
                        let text = codes.join("\n");
                        let copy = move |_| {
                            let ok = window().navigator().clipboard().write_text(&text);
                            spawn_local(async move {
                                copied.set(Some(
                                    if wasm_bindgen_futures::JsFuture::from(ok).await.is_ok() {
                                        "Copied"
                                    } else {
                                        "Could not copy, please write them down"
                                    },
                                ));
                            });
                        };
                        view! {
                            <h1 class="auth-title">"Save Your Recovery Codes"</h1>
                            <p class="auth-sub">
                                "Each code signs you in once if you lose your phone. "
                                "Store them somewhere safe: they won't be shown again."
                            </p>
                            <ul class="recovery-codes">
                                {codes.into_iter().map(|c| view! { <li><code>{c}</code></li> }).collect_view()}
                            </ul>
                            <button class="btn large" type="button" on:click=copy aria-live="polite">
                                {move || copied.get().unwrap_or("Copy Codes")}
                            </button>
                        }.into_any()
                    }
                }}
                {move || error.get().map(|e| view! { <p class="error" role="alert">{e}</p> })}
                <button class="btn primary large" type="submit" disabled=move || busy.get()>
                    {move || match step.get() {
                        Enroll::Password => "Continue",
                        Enroll::Scan(_) => "Turn On",
                        Enroll::Codes(_) => "I Saved Them",
                    }}
                </button>
                {move || (!matches!(step.get(), Enroll::Codes(_))).then(|| view! {
                    <button class="btn large" type="button" on:click=logout>"Sign Out"</button>
                })}
            </form>
        </div>
    }
    .into_any()
}
