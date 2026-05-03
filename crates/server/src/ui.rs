//! Tier-4 Phase 2 — HTMX UI for the verified twitter clone.
//!
//! All routes here live in the TCB. The UI is a thin server-rendered
//! view: every form posts to a handler that calls into the verified
//! service core, then re-renders the relevant fragment.
//!
//! Actor identity (per converged plan K20+K23):
//! - `/` shows a handle picker. Submitting it POSTs to `/_session` which
//!   sets a cookie `acting_handle=<handle>.<hmac>` (URL-safe).
//! - The HMAC uses `UI_COOKIE_HMAC_KEY` env var. If unset, falls back to
//!   a per-process random key — fine for local dev, NOT durable for prod.
//!   In prod the key is set as a Fly secret (see DEPLOY.md).
//! - Every UI write reads the cookie's handle and uses it as the actor.
//! - This is NOT auth — anyone can claim any handle. Honest framing in
//!   TCB.md and DEPLOY.md.

use std::sync::{Arc, OnceLock};

use axum::{
    extract::{Form, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use service::{Service, ServiceError};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const COOKIE_NAME: &str = "acting_handle";

/// Mount UI routes on the existing router.
///
/// UI follow/unfollow live under `/_ui/` to avoid colliding with the
/// JSON `POST /follow` API route (which takes `{from, to}` in the body).
/// The UI variants take actor from cookie + `to` from form.
pub fn mount(router: Router<Arc<Service>>) -> Router<Arc<Service>> {
    router
        .route("/", get(home_or_login))
        .route("/_session", post(set_session))
        .route("/_session/clear", post(clear_session))
        .route("/u/:handle", get(profile))
        .route("/compose", post(compose))
        .route("/_ui/follow", post(ui_follow))
        .route("/_ui/unfollow", post(ui_unfollow))
        .route("/register", post(ui_register))
}

// ---------------------------------------------------------------------------
// HMAC cookie signing — Tier-4 Phase 2 K23
// ---------------------------------------------------------------------------

fn signing_key() -> &'static [u8] {
    static KEY: OnceLock<Vec<u8>> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(hex_key) = std::env::var("UI_COOKIE_HMAC_KEY") {
            if let Ok(bytes) = hex::decode(&hex_key) {
                if bytes.len() >= 16 {
                    return bytes;
                }
            }
            eprintln!("UI_COOKIE_HMAC_KEY set but invalid (need >=16 hex bytes); using ephemeral key");
        }
        // Dev fallback: per-process random. Local-only.
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = ((std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
                .wrapping_mul((i as u128).wrapping_add(1)))
                & 0xff) as u8;
        }
        k.to_vec()
    })
}

fn sign(handle: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(signing_key()).expect("hmac");
    mac.update(handle.as_bytes());
    let tag = mac.finalize().into_bytes();
    format!("{}.{}", handle, hex::encode(&tag[..16]))
}

fn verify(cookie: &str) -> Option<String> {
    let (handle, tag) = cookie.rsplit_once('.')?;
    if handle.is_empty() || tag.len() != 32 {
        return None;
    }
    let expected = sign(handle);
    let (_, expected_tag) = expected.rsplit_once('.')?;
    if !constant_time_eq(tag.as_bytes(), expected_tag.as_bytes()) {
        return None;
    }
    Some(handle.to_string())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn current_actor(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie_header.split(';') {
        let kv = part.trim();
        if let Some(rest) = kv.strip_prefix(&format!("{COOKIE_NAME}=")) {
            return verify(rest);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Page rendering helpers
// ---------------------------------------------------------------------------

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(title: &str, actor: Option<&str>, body: &str) -> Html<String> {
    let title = html_escape(title);
    let actor_chip = match actor {
        Some(h) => format!(
            r#"<span class="actor">acting as <strong>@{h}</strong> ·
              <form method="post" action="/_session/clear" style="display:inline">
                <button class="link">switch</button>
              </form></span>"#,
            h = html_escape(h)
        ),
        None => r#"<span class="actor"><a href="/">log in</a></span>"#.into(),
    };
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title} · twitter-formal-rust</title>
<script src="https://unpkg.com/htmx.org@2.0.4" integrity="sha384-HGfztofotfshcF7+8n44JQL2oJmowVChPTg48S+jvZoztPfvwD79OC/LTtG6dMp+" crossorigin="anonymous"></script>
<style>
  :root {{ color-scheme: light dark; }}
  * {{ box-sizing: border-box; }}
  body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif;
         max-width: 640px; margin: 0 auto; padding: 1rem;
         line-height: 1.5; color: #111; background: #fafafa; }}
  @media (prefers-color-scheme: dark) {{ body {{ color: #eee; background: #111; }}
    .tweet, .compose, .panel {{ background: #1a1a1a; border-color: #333; }}
    a {{ color: #6cb6ff; }} }}
  header {{ display: flex; justify-content: space-between; align-items: baseline;
            border-bottom: 1px solid #ccc; padding-bottom: .5rem; margin-bottom: 1rem; }}
  header h1 {{ margin: 0; font-size: 1.2rem; }}
  header h1 a {{ color: inherit; text-decoration: none; }}
  .actor {{ font-size: .85rem; color: #666; }}
  .verified-badge {{ display: inline-block; background: #2b6cb0; color: white;
                     padding: 1px 6px; border-radius: 3px; font-size: .7rem;
                     vertical-align: middle; }}
  .tweet {{ border: 1px solid #ddd; border-radius: 6px; padding: .75rem 1rem;
            margin-bottom: .5rem; background: white; }}
  .tweet-meta {{ font-size: .8rem; color: #666; margin-bottom: .25rem; }}
  .tweet-meta a {{ color: inherit; text-decoration: none; }}
  .tweet-meta a:hover {{ text-decoration: underline; }}
  .compose, .panel {{ border: 1px solid #ddd; border-radius: 6px; padding: 1rem;
                       margin-bottom: 1rem; background: white; }}
  .compose textarea, .panel input[type=text] {{ width: 100%; padding: .5rem;
    border: 1px solid #ccc; border-radius: 4px; font: inherit; }}
  .compose textarea {{ min-height: 4em; resize: vertical; }}
  button {{ padding: .4rem .9rem; border: 1px solid #2b6cb0; background: #2b6cb0;
            color: white; border-radius: 4px; cursor: pointer; font: inherit; }}
  button:hover {{ background: #244e85; }}
  button.link {{ background: none; color: #2b6cb0; border: none; padding: 0;
                  text-decoration: underline; cursor: pointer; }}
  button.secondary {{ background: white; color: #2b6cb0; }}
  .error {{ color: #b00; padding: .5rem; border: 1px solid #f5b5b5;
            border-radius: 4px; background: #fff5f5; margin-bottom: 1rem; }}
  .empty {{ color: #888; text-align: center; padding: 2rem 0; }}
  footer {{ font-size: .8rem; color: #888; margin-top: 2rem; padding-top: 1rem;
            border-top: 1px solid #ccc; }}
  footer a {{ color: inherit; }}
</style>
</head>
<body>
<header>
  <h1><a href="/">twitter-formal-rust <span class="verified-badge">verified</span></a></h1>
  {actor_chip}
</header>
{body}
<footer>
  Verified backend (Verus + TLC). UI is in TCB. <a href="/version">/version</a> · <a href="/healthz">/healthz</a> · <a href="https://github.com/michaellady/twitter-rust-formal-verification">source</a>
</footer>
</body>
</html>"#
    ))
}

fn render_tweets(tweets: &[domain::Tweet]) -> String {
    if tweets.is_empty() {
        return r#"<p class="empty">No tweets to show. Follow someone or post.</p>"#.into();
    }
    tweets
        .iter()
        .map(|t| {
            format!(
                r#"<article class="tweet">
                     <div class="tweet-meta"><a href="/u/{a}">@{a}</a> · #{id}</div>
                     <div>{body}</div>
                   </article>"#,
                a = html_escape(&t.author),
                id = t.id,
                body = html_escape(&t.text),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ErrorParam {
    error: Option<String>,
}

async fn home_or_login(
    State(svc): State<Arc<Service>>,
    headers: HeaderMap,
    Query(q): Query<ErrorParam>,
) -> Response {
    let actor = current_actor(&headers);
    let error_html = q
        .error
        .as_deref()
        .map(|e| format!(r#"<div class="error">{}</div>"#, html_escape(e)))
        .unwrap_or_default();
    match actor {
        None => {
            let body = format!(
                r#"{error_html}
                   <div class="panel">
                     <h2>Pick a handle</h2>
                     <p>This is a verified-backend demo. There is no auth — pick any handle and you become it.</p>
                     <form method="post" action="/_session">
                       <input type="text" name="handle" required pattern="[a-z0-9_]+"
                              minlength="1" maxlength="32" placeholder="alice"
                              autocapitalize="off" autocorrect="off" spellcheck="false" autofocus>
                       <p style="margin-top:.5rem;font-size:.85rem;color:#666">Try <code>alice</code>, <code>bob</code>, or <code>carol</code> — they're seeded. Or pick something new and click "create handle" below.</p>
                       <button type="submit">log in as this handle</button>
                       <button type="submit" name="register" value="1" class="secondary" formaction="/register">create handle &amp; log in</button>
                     </form>
                   </div>
                "#
            );
            page("log in", None, &body).into_response()
        }
        Some(handle) => {
            let tweets = svc.home_timeline(&handle, 50);
            let body = format!(
                r#"{error_html}
                   <div class="compose">
                     <form method="post" action="/compose">
                       <textarea name="text" required maxlength="280" placeholder="What's verified?"></textarea>
                       <div style="margin-top:.5rem;display:flex;justify-content:space-between;align-items:center">
                         <span style="font-size:.85rem;color:#666">Posting as <strong>@{h}</strong></span>
                         <button type="submit">post</button>
                       </div>
                     </form>
                   </div>
                   <h2 style="margin:1rem 0 .5rem">home timeline</h2>
                   {tweets}
                "#,
                h = html_escape(&handle),
                tweets = render_tweets(&tweets)
            );
            page("home", Some(&handle), &body).into_response()
        }
    }
}

#[derive(Deserialize)]
struct SessionForm {
    handle: String,
}

fn cookie_header_for(handle: &str) -> String {
    format!(
        "{COOKIE_NAME}={signed}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000",
        signed = sign(handle)
    )
}

async fn set_session(Form(form): Form<SessionForm>) -> Response {
    let handle = form.handle.trim().to_string();
    if handle.is_empty() {
        return Redirect::to("/?error=empty+handle").into_response();
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, cookie_header_for(&handle).parse().unwrap());
    (StatusCode::SEE_OTHER, headers, [(header::LOCATION, "/")]).into_response()
}

async fn clear_session() -> Response {
    let mut headers = HeaderMap::new();
    let clear = format!("{COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax");
    headers.insert(header::SET_COOKIE, clear.parse().unwrap());
    (StatusCode::SEE_OTHER, headers, [(header::LOCATION, "/")]).into_response()
}

async fn ui_register(
    State(svc): State<Arc<Service>>,
    Form(form): Form<SessionForm>,
) -> Response {
    let handle = form.handle.trim().to_string();
    if handle.is_empty() {
        return Redirect::to("/?error=empty+handle").into_response();
    }
    match svc.create_user(&handle) {
        Ok(_) | Err(ServiceError::DuplicateUser) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::SET_COOKIE, cookie_header_for(&handle).parse().unwrap());
            (StatusCode::SEE_OTHER, headers, [(header::LOCATION, "/")]).into_response()
        }
        Err(e) => Redirect::to(&format!(
            "/?error={}",
            urlencoding::encode(&format!("{e:?}"))
        ))
        .into_response(),
    }
}

#[derive(Deserialize)]
struct ComposeForm {
    text: String,
}

async fn compose(
    State(svc): State<Arc<Service>>,
    headers: HeaderMap,
    Form(form): Form<ComposeForm>,
) -> Response {
    let Some(actor) = current_actor(&headers) else {
        return Redirect::to("/").into_response();
    };
    match svc.post_tweet(&actor, form.text.trim()) {
        Ok(_) => Redirect::to("/").into_response(),
        Err(e) => Redirect::to(&format!(
            "/?error={}",
            urlencoding::encode(&format!("{e:?}"))
        ))
        .into_response(),
    }
}

async fn profile(
    State(svc): State<Arc<Service>>,
    headers: HeaderMap,
    Path(handle): Path<String>,
) -> Response {
    let actor = current_actor(&headers);
    let tweets = svc.home_timeline(&handle, 50);
    let own_tweets: Vec<_> = tweets.into_iter().filter(|t| t.author == handle).collect();

    let follow_btn = match &actor {
        Some(a) if a == &handle => String::new(),
        Some(_) => format!(
            r#"<form method="post" action="/_ui/follow" style="display:inline">
                 <input type="hidden" name="to" value="{h}">
                 <button>follow</button>
               </form>
               <form method="post" action="/_ui/unfollow" style="display:inline;margin-left:.5rem">
                 <input type="hidden" name="to" value="{h}">
                 <button class="secondary">unfollow</button>
               </form>"#,
            h = html_escape(&handle)
        ),
        None => String::new(),
    };

    let body = format!(
        r#"<div class="panel">
             <h2 style="margin-top:0">@{h}</h2>
             {follow_btn}
           </div>
           <h2 style="margin:1rem 0 .5rem">@{h}'s tweets</h2>
           {tweets}"#,
        h = html_escape(&handle),
        follow_btn = follow_btn,
        tweets = render_tweets(&own_tweets),
    );
    page(&format!("@{handle}"), actor.as_deref(), &body).into_response()
}

#[derive(Deserialize)]
struct FollowForm {
    to: String,
}

async fn ui_follow(
    State(svc): State<Arc<Service>>,
    headers: HeaderMap,
    Form(form): Form<FollowForm>,
) -> Response {
    let Some(actor) = current_actor(&headers) else {
        return Redirect::to("/").into_response();
    };
    match svc.follow(&actor, form.to.trim()) {
        Ok(_) => Redirect::to(&format!("/u/{}", form.to)).into_response(),
        Err(e) => Redirect::to(&format!(
            "/u/{}?error={}",
            form.to,
            urlencoding::encode(&format!("{e:?}"))
        ))
        .into_response(),
    }
}

async fn ui_unfollow(
    State(svc): State<Arc<Service>>,
    headers: HeaderMap,
    Form(form): Form<FollowForm>,
) -> Response {
    let Some(actor) = current_actor(&headers) else {
        return Redirect::to("/").into_response();
    };
    let _ = svc.unfollow(&actor, form.to.trim());
    Redirect::to(&format!("/u/{}", form.to)).into_response()
}
