//! HTTP handlers — the only file in the TCB that touches axum.
//!
//! Every handler:
//!   1. Decodes the request via `axum::Json` / `axum::extract::Query`.
//!   2. Calls one verified `service::Service` method.
//!   3. Encodes the response with `axum::Json` and an explicit status code.
//!
//! The error-code mapping mirrors the Go impl byte-for-byte so the
//! conformance suite passes against either implementation.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use service::{Service, ServiceError};

use crate::metrics;

/// Construct the axum router around `svc`.
pub fn router(svc: Arc<Service>) -> Router {
    let api = Router::new()
        .route("/users", post(create_user))
        .route("/follow", post(follow).delete(unfollow))
        .route("/tweets", post(post_tweet))
        .route("/timeline", get(timeline))
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/metrics", get(metrics::render));
    let with_ui = crate::ui::mount(api);
    crate::admin::mount(with_ui)
        .layer(middleware::from_fn(metrics::track))
        .with_state(svc)
}

// -----------------------------------------------------------------------------
// GET /healthz — liveness/readiness probe used by Fly's load balancer
// (Tier-4 phase 1)
// -----------------------------------------------------------------------------

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

// -----------------------------------------------------------------------------
// GET /version — image provenance (Tier-4 phase 1; baked into image at build time)
//
// Reads `/etc/version.json` written by the build-image-main job in
// verify.yml. On rollback the previous image is re-pulled, so /version
// automatically reports the rolled-back release's digest — no env-var
// sync required (K21 fix from the converged Tier-4 plan).
// -----------------------------------------------------------------------------

#[derive(Serialize)]
struct VersionResp {
    git_sha: String,
    image_digest: String,
    process_uptime_seconds: u64,
    snapshot_version: u32,
}

async fn version() -> impl IntoResponse {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(std::time::Instant::now);
    let uptime = start.elapsed().as_secs();

    // Defaults: dev/local runs without a baked image.
    let mut git_sha = String::from("dev");
    let mut image_digest = String::from("sha256:dev");

    if let Ok(s) = std::fs::read_to_string("/etc/version.json") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(g) = v.get("git_sha").and_then(|x| x.as_str()) {
                git_sha = g.to_string();
            }
            if let Some(d) = v.get("image_digest").and_then(|x| x.as_str()) {
                image_digest = d.to_string();
            }
        }
    }

    Json(VersionResp {
        git_sha,
        image_digest,
        process_uptime_seconds: uptime,
        snapshot_version: 1,
    })
}

#[derive(Serialize)]
struct ErrBody {
    error: &'static str,
}

fn err(status: StatusCode, code: &'static str) -> (StatusCode, Json<ErrBody>) {
    (status, Json(ErrBody { error: code }))
}

fn map_err(e: ServiceError) -> (StatusCode, &'static str) {
    match e {
        ServiceError::EmptyHandle => (StatusCode::BAD_REQUEST, "empty_handle"),
        ServiceError::EmptyText => (StatusCode::BAD_REQUEST, "empty_text"),
        ServiceError::SelfFollow => (StatusCode::BAD_REQUEST, "self_follow_forbidden"),
        ServiceError::UnknownUser => (StatusCode::BAD_REQUEST, "unknown_user"),
        ServiceError::DuplicateUser => (StatusCode::CONFLICT, "duplicate_user"),
    }
}

// -----------------------------------------------------------------------------
// POST /users
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateUserReq {
    handle: String,
}

#[derive(Serialize)]
struct UserResp {
    handle: String,
    id: i64,
}

async fn create_user(
    State(svc): State<Arc<Service>>,
    body: Option<Json<CreateUserReq>>,
) -> impl IntoResponse {
    let Json(req) = match body {
        Some(b) => b,
        None => return err(StatusCode::BAD_REQUEST, "invalid_json").into_response(),
    };
    match svc.create_user(&req.handle) {
        Ok(u) => (StatusCode::CREATED, Json(UserResp { handle: u.handle, id: u.id }))
            .into_response(),
        Err(e) => {
            let (status, code) = map_err(e);
            metrics::note_violation(code, "/users");
            err(status, code).into_response()
        }
    }
}

// -----------------------------------------------------------------------------
// POST /follow, DELETE /follow
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct FollowReq {
    from: String,
    to: String,
}

async fn follow(
    State(svc): State<Arc<Service>>,
    body: Option<Json<FollowReq>>,
) -> impl IntoResponse {
    let Json(req) = match body {
        Some(b) => b,
        None => return err(StatusCode::BAD_REQUEST, "invalid_json").into_response(),
    };
    match svc.follow(&req.from, &req.to) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let (status, code) = map_err(e);
            metrics::note_violation(code, "/follow");
            err(status, code).into_response()
        }
    }
}

async fn unfollow(
    State(svc): State<Arc<Service>>,
    body: Option<Json<FollowReq>>,
) -> impl IntoResponse {
    let Json(req) = match body {
        Some(b) => b,
        None => return err(StatusCode::BAD_REQUEST, "invalid_json").into_response(),
    };
    match svc.unfollow(&req.from, &req.to) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let (status, code) = map_err(e);
            metrics::note_violation(code, "/follow");
            err(status, code).into_response()
        }
    }
}

// -----------------------------------------------------------------------------
// POST /tweets
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct TweetReq {
    author: String,
    text: String,
}

#[derive(Serialize)]
struct TweetResp {
    id: i64,
    author: String,
    text: String,
    created_at: i64,
}

async fn post_tweet(
    State(svc): State<Arc<Service>>,
    body: Option<Json<TweetReq>>,
) -> impl IntoResponse {
    let Json(req) = match body {
        Some(b) => b,
        None => return err(StatusCode::BAD_REQUEST, "invalid_json").into_response(),
    };
    match svc.post_tweet(&req.author, &req.text) {
        Ok(t) => (
            StatusCode::CREATED,
            Json(TweetResp {
                id: t.id,
                author: t.author,
                text: t.text,
                created_at: t.created_at,
            }),
        )
            .into_response(),
        Err(e) => {
            let (status, code) = map_err(e);
            metrics::note_violation(code, "/tweets");
            err(status, code).into_response()
        }
    }
}

// -----------------------------------------------------------------------------
// GET /timeline
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct TimelineQuery {
    user: Option<String>,
    limit: Option<String>,
}

#[derive(Serialize)]
struct TimelineResp {
    tweets: Vec<TweetResp>,
    next_cursor: Option<String>,
}

async fn timeline(
    State(svc): State<Arc<Service>>,
    Query(q): Query<TimelineQuery>,
) -> impl IntoResponse {
    let user = q.user.unwrap_or_default();
    if user.is_empty() {
        return err(StatusCode::BAD_REQUEST, "empty_user").into_response();
    }
    let limit = match q.limit.as_deref() {
        Some(s) if !s.trim().is_empty() => match s.trim().parse::<i64>() {
            Ok(v) if v >= 0 => v as usize,
            _ => return err(StatusCode::BAD_REQUEST, "invalid_limit").into_response(),
        },
        _ => 0,
    };
    let tw = svc.home_timeline(&user, limit);
    let resp = TimelineResp {
        tweets: tw
            .into_iter()
            .map(|t| TweetResp {
                id: t.id,
                author: t.author,
                text: t.text,
                created_at: t.created_at,
            })
            .collect(),
        next_cursor: None,
    };
    (StatusCode::OK, Json(resp)).into_response()
}
