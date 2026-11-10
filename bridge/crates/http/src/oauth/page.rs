// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

//! The login form and error pages: server-rendered HTML (maud escapes
//! every value), no JavaScript, one inline stylesheet allowed by its hash.

use std::sync::LazyLock;

use axum::response::{Html, IntoResponse, Response};
use data_encoding::BASE64;
use http::{HeaderMap, HeaderValue, StatusCode, header};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use sha2::{Digest, Sha256};

use crate::error::no_store;

const CSS: &str = "\
@font-face{font-family:Geist;src:url(/assets/geist-5.3.0.woff2) format('woff2');font-weight:100 900;font-display:swap}\
:root{color-scheme:light dark;--canvas:#f6f6f9;--bg:#fff;--line:#e4e3eb;--line2:#d3d2dd;--hover:#eeedf3;\
--fg:#15152b;--fg2:#46475e;--fg3:#62637a;--acc:#4f46e5;--acc-hi:#4338ca;--acc-fg:#fff;\
--danger:#c2300a;--danger-soft:#fdebe5;--focus:rgb(79 70 229/.18);\
--shadow:0 4px 14px -4px rgb(21 21 43/.12),0 2px 4px rgb(21 21 43/.05)}\
@media (prefers-color-scheme:dark){:root{--canvas:#0a0b14;--bg:#0e0f1a;--line:#23253a;--line2:#323550;\
--hover:#1c1e30;--fg:#ececf5;--fg2:#b6b7cc;--fg3:#9091ab;--acc:#5b54e8;--acc-hi:#4f46e5;--acc-fg:#fff;\
--danger:#ff7a59;--danger-soft:#2e140c;--focus:rgb(139 133 255/.3);\
--shadow:0 6px 18px -6px rgb(0 0 0/.6),0 0 0 1px rgb(255 255 255/.03)}}\
*{box-sizing:border-box}\
body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:16px;\
background:var(--canvas);color:var(--fg);font:13.5px/1.5 Geist,ui-sans-serif,system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;\
-webkit-font-smoothing:antialiased;-moz-osx-font-smoothing:grayscale}\
main{width:100%;max-width:380px;background:var(--bg);border:1px solid var(--line);border-radius:16px;\
box-shadow:var(--shadow);padding:32px 24px;display:flex;flex-direction:column;gap:16px}\
.brand{display:flex;flex-direction:column;align-items:center;gap:12px}\
.brand svg{display:block;width:56px;height:56px}\
.wordmark{font-size:28px;letter-spacing:-.045em;line-height:1;white-space:nowrap}\
.wordmark b{font-weight:750}.wordmark span{color:var(--fg2);font-weight:450}\
h1{margin:0;text-align:center;font-size:18px;font-weight:700;letter-spacing:-.03em;line-height:1.2}\
p{margin:0;text-align:center;color:var(--fg2)}\
form{display:flex;flex-direction:column}\
label{margin:0 0 6px;font-size:12px;font-weight:500;color:var(--fg2)}\
input{width:100%;height:42px;margin:0 0 14px;padding:0 12px;font:inherit;color:var(--fg);background:var(--bg);\
border:1px solid var(--line2);border-radius:9px;outline:none;transition:border-color .12s,box-shadow .12s}\
input:hover{border-color:var(--fg3)}\
input:focus{border-color:var(--acc);box-shadow:0 0 0 4px var(--focus)}\
.actions{display:flex;gap:8px;margin-top:4px}\
button{flex:1;height:42px;padding:0 18px;font:inherit;font-size:14.5px;font-weight:550;cursor:pointer;\
color:var(--fg);background:var(--bg);border:1px solid var(--line2);border-radius:11px;transition:background-color .12s}\
button:hover{background:var(--hover)}\
button.primary{color:var(--acc-fg);background:var(--acc);border-color:transparent}\
button.primary:hover{background:var(--acc-hi)}\
button:focus-visible{outline:none;box-shadow:0 0 0 3px var(--focus)}\
.error{display:flex;gap:8px;align-items:flex-start;margin:0;padding:8px 12px;border-radius:8px;\
font-size:12px;text-align:start;color:var(--danger);background:var(--danger-soft)}\
.error svg{flex-shrink:0;width:16px;height:16px;margin-top:1px}\
.note{font-size:12px;color:var(--fg3)}\
.server{margin:0;padding:8px 12px;border:1px solid var(--line2);border-radius:8px;font-size:12px;\
text-align:start;color:var(--fg);overflow-wrap:anywhere}.server b{display:block;color:var(--fg2);font-weight:550}";

/// Where the bridge serves Geist (the app's font), for its own pages. The
/// version is in the path, so it can be cached for good.
pub(crate) const FONT_PATH: &str = "/assets/geist-5.3.0.woff2";

/// Geist Sans, Latin subset (SIL Open Font License 1.1, from
/// `@fontsource-variable/geist` 5.3.0).
static FONT: &[u8] = include_bytes!("../../assets/geist-latin-wght-normal.woff2");

/// `GET /assets/geist-5.3.0.woff2`.
pub(crate) async fn font() -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        FONT,
    )
        .into_response()
}

/// `'sha256-…'` of [`CSS`] for the `style-src` directive.
static CSS_HASH: LazyLock<String> = LazyLock::new(|| {
    format!(
        "'sha256-{}'",
        BASE64.encode(&Sha256::digest(CSS.as_bytes()))
    )
});

/// The page language, from `Accept-Language`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    En,
    Es,
}

impl Lang {
    /// The preferred language among English and Spanish (English if the
    /// header names neither).
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        let Some(value) = headers
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
        else {
            return Self::En;
        };
        let mut best: Option<(u16, Self)> = None;
        for item in value.split(',') {
            let mut parts = item.split(';');
            let tag = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
            let quality = parts
                .find_map(|p| p.trim().strip_prefix("q="))
                .map_or(1000, parse_quality);
            let lang = match tag.split('-').next() {
                Some("es") => Self::Es,
                Some("en") => Self::En,
                _ => continue,
            };
            if quality > 0 && best.is_none_or(|(q, _)| quality > q) {
                best = Some((quality, lang));
            }
        }
        best.map_or(Self::En, |(_, lang)| lang)
    }

    fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
        }
    }
}

/// A `q` value in thousandths (`0.8` → 800); bad values count as 0.
fn parse_quality(text: &str) -> u16 {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits: String = fraction.chars().chain("000".chars()).take(3).collect();
    match (whole, digits.parse::<u16>()) {
        ("1", _) => 1000,
        ("0", Ok(thousandths)) => thousandths,
        _ => 0,
    }
}

/// A message shown above the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormError {
    /// Wrong login or password, or the user doesn't exist (same text, on
    /// purpose).
    WrongCredentials,
    /// Empty or invalid fields.
    Missing,
    /// Too many failed attempts.
    RateLimited,
    /// The mail server didn't answer.
    Unavailable,
}

/// Why no form can be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorPage {
    /// Unknown client or redirect URI, or a malformed request: never
    /// redirected (RFC 6749 §4.1.2.1).
    InvalidRequest,
    /// The form expired, was used already, or its CSRF check failed.
    Expired,
    /// The server can't take more logins now.
    Busy,
    /// Something failed inside the server.
    Internal,
}

struct Text {
    title: &'static str,
    lead: &'static str,
    login: &'static str,
    password: &'static str,
    submit: &'static str,
    cancel: &'static str,
    note: &'static str,
}

fn text(lang: Lang) -> Text {
    match lang {
        Lang::En => Text {
            title: "Sign in",
            lead: "Use the username and password of your email account.",
            login: "Username or email",
            password: "Password",
            submit: "Sign in",
            cancel: "Cancel",
            note: "Your password is only used to connect to your mail server, and is stored encrypted while you are signed in.",
        },
        Lang::Es => Text {
            title: "Iniciar sesión",
            lead: "Usa el usuario y la contraseña de tu cuenta de correo.",
            login: "Usuario o correo",
            password: "Contraseña",
            submit: "Entrar",
            cancel: "Cancelar",
            note: "Tu contraseña solo se usa para conectar con tu servidor de correo y se guarda cifrada mientras tengas la sesión abierta.",
        },
    }
}

fn form_error_text(lang: Lang, error: FormError) -> &'static str {
    match (lang, error) {
        (Lang::En, FormError::WrongCredentials) => "Wrong username or password.",
        (Lang::Es, FormError::WrongCredentials) => "Usuario o contraseña incorrectos.",
        (Lang::En, FormError::Missing) => "Enter your username and password.",
        (Lang::Es, FormError::Missing) => "Escribe tu usuario y tu contraseña.",
        (Lang::En, FormError::RateLimited) => {
            "Too many attempts. Wait a few minutes and try again."
        }
        (Lang::Es, FormError::RateLimited) => {
            "Demasiados intentos. Espera unos minutos y vuelve a probar."
        }
        (Lang::En, FormError::Unavailable) => {
            "The mail server is not answering. Try again in a moment."
        }
        (Lang::Es, FormError::Unavailable) => {
            "El servidor de correo no responde. Prueba otra vez dentro de un momento."
        }
    }
}

fn server_alert_label(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Message from your mail server:",
        Lang::Es => "Mensaje de tu servidor de correo:",
    }
}

fn error_page_text(lang: Lang, page: ErrorPage) -> (&'static str, &'static str) {
    match (lang, page) {
        (Lang::En, ErrorPage::InvalidRequest) => (
            "This sign-in link is not valid",
            "The app that sent you here made an invalid request. Go back to the app and try again.",
        ),
        (Lang::Es, ErrorPage::InvalidRequest) => (
            "Este enlace de inicio de sesión no es válido",
            "La aplicación que te ha traído aquí hizo una petición no válida. Vuelve a la aplicación y prueba otra vez.",
        ),
        (Lang::En, ErrorPage::Expired) => (
            "This sign-in form has expired",
            "Go back to the app and start signing in again.",
        ),
        (Lang::Es, ErrorPage::Expired) => (
            "Este formulario ha caducado",
            "Vuelve a la aplicación y empieza otra vez a iniciar sesión.",
        ),
        (Lang::En, ErrorPage::Busy) => (
            "Too many sign-ins right now",
            "Wait a few minutes and try again.",
        ),
        (Lang::Es, ErrorPage::Busy) => (
            "Demasiados inicios de sesión ahora mismo",
            "Espera unos minutos y prueba otra vez.",
        ),
        (Lang::En, ErrorPage::Internal) => (
            "Something went wrong",
            "The server could not complete the sign-in. Try again later.",
        ),
        (Lang::Es, ErrorPage::Internal) => (
            "Algo ha fallado",
            "El servidor no pudo completar el inicio de sesión. Prueba más tarde.",
        ),
    }
}

/// The app icon (the same as the web app's favicon), inline so the CSP
/// needs no `img-src`.
const APP_ICON: &str = r##"<svg viewBox="0 0 64 64" aria-hidden="true"><defs><linearGradient id="aib" x1="0" y1="0" x2="0" y2="64" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#1B1B3A"/><stop offset="1" stop-color="#0B0C1A"/></linearGradient><radialGradient id="aih" cx="0.5" cy="0.5" r="0.6"><stop offset="0" stop-color="#6E63FF" stop-opacity=".34"/><stop offset=".55" stop-color="#3B2FD9" stop-opacity=".1"/><stop offset="1" stop-color="#3B2FD9" stop-opacity="0"/></radialGradient><linearGradient id="aig" x1="18" y1="46" x2="46" y2="18" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#6E63FF"/><stop offset="1" stop-color="#4FD3F5"/></linearGradient></defs><rect width="64" height="64" rx="16" fill="url(#aib)"/><rect width="64" height="64" rx="16" fill="url(#aih)"/><rect x=".5" y=".5" width="63" height="63" rx="15.5" fill="none" stroke="#fff" stroke-opacity=".08"/><g transform="translate(5 5) scale(.84375)"><g fill="none" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"><path d="M32.0 16.0C32.9 15.6 35.4 14.1 37.4 13.7C39.4 13.2 41.8 13.0 44.1 13.3C46.4 13.6 49.0 14.3 51.3 15.5C53.6 16.7 56.9 19.5 58.0 20.4" stroke="#6E63FF"/><path d="M45.9 24.0C46.5 24.6 48.8 26.2 50.0 27.7C51.2 29.2 52.3 31.1 53.0 33.1C53.7 35.1 54.1 37.5 54.2 39.8C54.2 42.2 53.2 46.0 53.1 47.2" stroke="#886BFF"/><path d="M45.9 40.0C45.7 41.0 45.8 43.9 45.2 45.9C44.5 47.8 43.5 50.0 42.1 51.8C40.7 53.7 38.8 55.5 36.7 56.9C34.5 58.4 30.3 59.8 29.1 60.3" stroke="#A274FF"/><path d="M32.0 48.0C31.1 48.3 28.7 49.5 26.8 49.7C24.9 50.0 22.7 50.0 20.6 49.6C18.5 49.2 16.2 48.5 14.2 47.3C12.1 46.1 9.2 43.4 8.3 42.6" stroke="#83A0FA"/><path d="M18.1 40.0C17.4 39.4 14.8 38.0 13.4 36.5C12.0 34.9 10.7 33.0 9.8 30.9C8.9 28.7 8.2 26.2 8.1 23.6C7.9 21.0 8.8 16.7 8.9 15.3" stroke="#69BAF7"/><path d="M18.1 24.0C18.3 23.1 18.5 20.4 19.3 18.6C20.0 16.8 21.0 14.9 22.4 13.3C23.9 11.7 25.6 10.1 27.7 8.9C29.7 7.7 33.5 6.6 34.7 6.1" stroke="#4FD3F5"/></g><path fill-rule="evenodd" fill="url(#aig)" d="M19.5 32a12.5 12.5 0 1 0 25 0a12.5 12.5 0 1 0 -25 0Z M27 30.8a6.2 6.2 0 1 0 12.4 0a6.2 6.2 0 1 0 -12.4 0Z"/><circle cx="33.2" cy="30.8" r="6.2" fill="#0E1240"/><circle cx="35.99" cy="28.01" r="2.1" fill="#9BEBFF"/></g></svg>"##;

/// A warning triangle (lucide "triangle-alert") for form errors.
fn warning_icon() -> Markup {
    html! {
        svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
            stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" {
            path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" {}
            path d="M12 9v4" {}
            path d="M12 17h.01" {}
        }
    }
}

fn layout(lang: Lang, title: &str, body: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang=(lang.code()) {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="referrer" content="no-referrer";
                meta name="robots" content="noindex";
                title { (title) " · Nebula Mail" }
                style { (PreEscaped(CSS)) }
            }
            body {
                main {
                    div.brand {
                        (PreEscaped(APP_ICON))
                        span.wordmark { b { "nebula" } " " span { "mail" } }
                    }
                    (body)
                }
            }
        }
    }
}

/// What the login form needs.
pub(crate) struct LoginForm<'a> {
    pub(crate) lang: Lang,
    /// The pending request id (hidden field).
    pub(crate) pending: &'a str,
    /// The login typed before, to fill in again after an error.
    pub(crate) login: &'a str,
    pub(crate) error: Option<FormError>,
    /// The mail server's `[ALERT]` with the refusal (plain text, escaped
    /// here like every value), RFC 3501 §7.1, RFC 9051 §7.1.
    pub(crate) alert: Option<&'a str>,
    /// Origin of the redirect URI, allowed in `form-action`.
    pub(crate) redirect_origin: &'a str,
}

fn html_response(status: StatusCode, markup: &Markup, csp: &str) -> Response {
    let mut response = (status, Html(markup.clone().into_string())).into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(csp) {
        headers.insert(header::CONTENT_SECURITY_POLICY, value);
    }
    no_store(response)
}

/// The login form.
pub(crate) fn login_form(status: StatusCode, form: &LoginForm<'_>) -> Response {
    let t = text(form.lang);
    let body = html! {
        h1 { (t.title) }
        p { (t.lead) }
        @if let Some(error) = form.error {
            div.error role="alert" { (warning_icon()) span { (form_error_text(form.lang, error)) } }
        }
        @if let Some(alert) = form.alert {
            div.server role="alert" { b { (server_alert_label(form.lang)) } (alert) }
        }
        form method="post" action="/oauth/authorize" {
            input type="hidden" name="pending" value=(form.pending);
            label for="login" { (t.login) }
            input #login type="text" name="login" value=(form.login) autocomplete="username"
                autocapitalize="none" spellcheck="false" required autofocus;
            label for="password" { (t.password) }
            input #password type="password" name="password" autocomplete="current-password" required;
            div.actions {
                button type="submit" name="action" value="deny" formnovalidate { (t.cancel) }
                button.primary type="submit" name="action" value="login" { (t.submit) }
            }
        }
        p.note { (t.note) }
    };
    let csp = format!(
        "default-src 'none'; style-src {}; font-src 'self'; form-action 'self' {}; frame-ancestors 'none'; base-uri 'none'",
        *CSS_HASH, form.redirect_origin
    );
    html_response(status, &layout(form.lang, t.title, &body), &csp)
}

/// An error page (never a redirect).
pub(crate) fn error_page(lang: Lang, page: ErrorPage) -> Response {
    let (title, detail) = error_page_text(lang, page);
    let status = match page {
        ErrorPage::InvalidRequest => StatusCode::BAD_REQUEST,
        ErrorPage::Expired => StatusCode::FORBIDDEN,
        ErrorPage::Busy => StatusCode::SERVICE_UNAVAILABLE,
        ErrorPage::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body = html! {
        h1 { (title) }
        p { (detail) }
    };
    let csp = format!(
        "default-src 'none'; style-src {}; font-src 'self'; form-action 'none'; frame-ancestors 'none'; base-uri 'none'",
        *CSS_HASH
    );
    html_response(status, &layout(lang, title, &body), &csp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(value: &str) -> Lang {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_str(value).unwrap(),
        );
        Lang::from_headers(&headers)
    }

    #[test]
    fn picks_the_preferred_language() {
        assert_eq!(lang("es-ES,es;q=0.9,en;q=0.8"), Lang::Es);
        assert_eq!(lang("en-US,en;q=0.9,es;q=0.8"), Lang::En);
        assert_eq!(lang("fr-FR,fr;q=0.9,es;q=0.5"), Lang::Es);
        assert_eq!(lang("de"), Lang::En);
        assert_eq!(lang("es;q=0, en;q=0.1"), Lang::En);
        assert_eq!(lang("en;q=0.5, es;q=0.7"), Lang::Es);
        assert_eq!(Lang::from_headers(&HeaderMap::new()), Lang::En);
        assert_eq!(parse_quality("0.85"), 850);
        assert_eq!(parse_quality("1.0"), 1000);
        assert_eq!(parse_quality("x"), 0);
    }

    #[tokio::test]
    async fn values_are_escaped_and_csp_matches_the_style() {
        let response = login_form(
            StatusCode::OK,
            &LoginForm {
                lang: Lang::En,
                pending: "abc",
                login: "\"><script>alert(1)</script>",
                error: Some(FormError::WrongCredentials),
                alert: Some("Locked <b>now</b> & \"later\""),
                redirect_origin: "https://mail.example.com",
            },
        );
        let csp = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(csp.contains(&*CSS_HASH));
        assert!(csp.contains("form-action 'self' https://mail.example.com"));
        assert!(!csp.contains("script"));
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("Message from your mail server:"));
        assert!(body.contains("Locked &lt;b&gt;now&lt;/b&gt; &amp; &quot;later&quot;"));
        let markup = layout(Lang::En, "x", &html! { input value="\"><script>"; });
        assert!(!markup.into_string().contains("<script>"));
    }
}
