//! Tier-4 Stream 2 Phase 0 — snapshot contract.
//!
//! Two admin endpoints expose the verified core's full inner state so
//! the cross-impl shadow / diff-test layer (Stream 2 Phase 1+) can
//! capture and replay state across processes:
//!
//! - `POST /_admin/snapshot`     — dump current state as JSON.
//! - `POST /_admin/load-snapshot` — atomically restore state from JSON.
//!
//! # Trust boundary
//!
//! This entire module is in the TCB. Both handlers bypass the verified
//! core's admission checks (F3/F6/F7/F8/F9) — they are the documented
//! escape hatch. Authentication is a constant-time compare of the
//! `X-Admin-Token` request header against the `ADMIN_TOKEN` env var. If
//! the env var is unset the endpoints are disabled (HTTP 503).
//!
//! # Snapshot version compatibility (K9 / K15)
//!
//! Per the converged Tier-4 plan: a consumer at version N accepts
//! snapshots at versions {N-1, N, N+1}. Currently N=1, so versions 1
//! and 2 are accepted. Unknown fields in newer versions are ignored
//! (forward compatibility). Out-of-window → HTTP 422.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Router,
};
use serde_json::{json, Value};

use domain::{Follow, Tweet, User};
use service::{Service, ServiceState};
use store::StoreSnapshot;

/// Snapshot wire-format version produced by this build. Consumer accepts
/// `[CURRENT_SNAPSHOT_VERSION - 1, CURRENT_SNAPSHOT_VERSION,
/// CURRENT_SNAPSHOT_VERSION + 1]`.
pub const CURRENT_SNAPSHOT_VERSION: u32 = 1;

/// Mount the admin routes on `router`.
pub fn mount(router: Router<Arc<Service>>) -> Router<Arc<Service>> {
    router
        .route("/_admin/snapshot", post(snapshot))
        .route("/_admin/load-snapshot", post(load_snapshot))
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Outcome of `check_admin_token`.
enum AuthResult {
    Ok,
    /// `ADMIN_TOKEN` env var is unset — admin endpoints are disabled.
    Disabled,
    /// Token header missing or did not match.
    Forbidden,
}

fn check_admin_token(headers: &HeaderMap) -> AuthResult {
    let expected = match std::env::var("ADMIN_TOKEN") {
        Ok(v) if !v.is_empty() => v,
        _ => return AuthResult::Disabled,
    };
    let provided = headers
        .get("x-admin-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if constant_time_eq(provided.as_bytes(), expected.as_bytes()) {
        AuthResult::Ok
    } else {
        AuthResult::Forbidden
    }
}

/// Constant-time byte comparison. Length is included in the compare so
/// callers can't probe the expected length; mismatched-length inputs
/// always return false but still walk the longer of the two buffers.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let len = a.len().max(b.len());
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..len {
        let av = *a.get(i).unwrap_or(&0);
        let bv = *b.get(i).unwrap_or(&0);
        diff |= av ^ bv;
    }
    diff == 0
}

fn forbidden() -> (StatusCode, axum::Json<Value>) {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(json!({"error": "forbidden"})),
    )
}

fn admin_disabled() -> (StatusCode, axum::Json<Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(json!({"error": "admin_disabled"})),
    )
}

// ---------------------------------------------------------------------------
// /etc/version.json — same source as GET /version
// ---------------------------------------------------------------------------

fn read_version_meta() -> (String, String) {
    let mut git_sha = String::from("dev");
    let mut image_digest = String::from("sha256:dev");
    if let Ok(s) = std::fs::read_to_string("/etc/version.json") {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let Some(g) = v.get("git_sha").and_then(|x| x.as_str()) {
                git_sha = g.to_string();
            }
            if let Some(d) = v.get("image_digest").and_then(|x| x.as_str()) {
                image_digest = d.to_string();
            }
        }
    }
    (git_sha, image_digest)
}

fn captured_at_unix_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// JSON marshaling — verified domain types do NOT derive Serialize, so we
// build them with serde_json::json! and parse by hand.
// ---------------------------------------------------------------------------

fn user_to_json(u: &User) -> Value {
    json!({"id": u.id, "handle": u.handle})
}

fn follow_to_json(f: &Follow) -> Value {
    json!({"from": f.from, "to": f.to})
}

fn tweet_to_json(t: &Tweet) -> Value {
    json!({
        "id": t.id,
        "author": t.author,
        "text": t.text,
        "created_at": t.created_at,
    })
}

fn state_to_json(s: &ServiceState) -> Value {
    json!({
        "clock_now": s.clock_now,
        "id_counter_users": s.id_counter_users,
        "id_counter_tweets": s.id_counter_tweets,
        "users": s.store.users.iter().map(user_to_json).collect::<Vec<_>>(),
        "follows": s.store.follows.iter().map(follow_to_json).collect::<Vec<_>>(),
        "tweets": s.store.tweets.iter().map(tweet_to_json).collect::<Vec<_>>(),
    })
}

#[derive(Debug)]
enum ParseError {
    MissingField(&'static str),
    InvalidType(&'static str),
}

impl ParseError {
    fn message(&self) -> String {
        match self {
            ParseError::MissingField(f) => format!("missing field: {f}"),
            ParseError::InvalidType(f) => format!("invalid type for field: {f}"),
        }
    }
}

fn parse_user(v: &Value) -> Result<User, ParseError> {
    let id = v.get("id").and_then(|x| x.as_i64()).ok_or(ParseError::InvalidType("users[].id"))?;
    let handle = v
        .get("handle")
        .and_then(|x| x.as_str())
        .ok_or(ParseError::InvalidType("users[].handle"))?
        .to_string();
    Ok(User { id, handle })
}

fn parse_follow(v: &Value) -> Result<Follow, ParseError> {
    let from = v
        .get("from")
        .and_then(|x| x.as_str())
        .ok_or(ParseError::InvalidType("follows[].from"))?
        .to_string();
    let to = v
        .get("to")
        .and_then(|x| x.as_str())
        .ok_or(ParseError::InvalidType("follows[].to"))?
        .to_string();
    Ok(Follow { from, to })
}

fn parse_tweet(v: &Value) -> Result<Tweet, ParseError> {
    let id = v.get("id").and_then(|x| x.as_i64()).ok_or(ParseError::InvalidType("tweets[].id"))?;
    let author = v
        .get("author")
        .and_then(|x| x.as_str())
        .ok_or(ParseError::InvalidType("tweets[].author"))?
        .to_string();
    let text = v
        .get("text")
        .and_then(|x| x.as_str())
        .ok_or(ParseError::InvalidType("tweets[].text"))?
        .to_string();
    let created_at = v
        .get("created_at")
        .and_then(|x| x.as_i64())
        .ok_or(ParseError::InvalidType("tweets[].created_at"))?;
    Ok(Tweet { id, author, text, created_at })
}

fn parse_state(state: &Value) -> Result<ServiceState, ParseError> {
    let clock_now = state
        .get("clock_now")
        .and_then(|x| x.as_i64())
        .ok_or(ParseError::MissingField("state.clock_now"))?;
    let id_counter_users = state
        .get("id_counter_users")
        .and_then(|x| x.as_i64())
        .ok_or(ParseError::MissingField("state.id_counter_users"))?;
    let id_counter_tweets = state
        .get("id_counter_tweets")
        .and_then(|x| x.as_i64())
        .ok_or(ParseError::MissingField("state.id_counter_tweets"))?;
    let users = state
        .get("users")
        .and_then(|x| x.as_array())
        .ok_or(ParseError::MissingField("state.users"))?
        .iter()
        .map(parse_user)
        .collect::<Result<Vec<_>, _>>()?;
    let follows = state
        .get("follows")
        .and_then(|x| x.as_array())
        .ok_or(ParseError::MissingField("state.follows"))?
        .iter()
        .map(parse_follow)
        .collect::<Result<Vec<_>, _>>()?;
    let tweets = state
        .get("tweets")
        .and_then(|x| x.as_array())
        .ok_or(ParseError::MissingField("state.tweets"))?
        .iter()
        .map(parse_tweet)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ServiceState {
        clock_now,
        id_counter_users,
        id_counter_tweets,
        store: StoreSnapshot { users, follows, tweets },
    })
}

// ---------------------------------------------------------------------------
// POST /_admin/snapshot
// ---------------------------------------------------------------------------

async fn snapshot(
    State(svc): State<Arc<Service>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    match check_admin_token(&headers) {
        AuthResult::Disabled => return admin_disabled().into_response(),
        AuthResult::Forbidden => return forbidden().into_response(),
        AuthResult::Ok => {}
    }
    let state = svc.snapshot_state();
    let (git_sha, image_digest) = read_version_meta();
    let body = json!({
        "snapshot_version": CURRENT_SNAPSHOT_VERSION,
        "captured_at_unix_nanos": captured_at_unix_nanos().to_string(),
        "git_sha": git_sha,
        "image_digest": image_digest,
        "state": state_to_json(&state),
    });
    (StatusCode::OK, axum::Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// POST /_admin/load-snapshot
// ---------------------------------------------------------------------------

fn supported_versions() -> [u32; 2] {
    // Per K9/K15: accept N-1, N, N+1. N=1, so N-1=0 isn't a real version
    // and is excluded. Effective accepted set today: {1, 2}.
    [CURRENT_SNAPSHOT_VERSION, CURRENT_SNAPSHOT_VERSION + 1]
}

fn version_unsupported(received: u32) -> (StatusCode, axum::Json<Value>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        axum::Json(json!({
            "error": "snapshot_version_unsupported",
            "supported": supported_versions(),
            "received": received,
        })),
    )
}

fn malformed_json(reason: impl ToString) -> (StatusCode, axum::Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        axum::Json(json!({"error": "malformed_json", "reason": reason.to_string()})),
    )
}

async fn load_snapshot(
    State(svc): State<Arc<Service>>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    match check_admin_token(&headers) {
        AuthResult::Disabled => return admin_disabled().into_response(),
        AuthResult::Forbidden => return forbidden().into_response(),
        AuthResult::Ok => {}
    }

    let v: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => return malformed_json(e).into_response(),
    };

    let version = match v.get("snapshot_version").and_then(|x| x.as_u64()) {
        Some(n) if n <= u32::MAX as u64 => n as u32,
        _ => return malformed_json("missing or invalid snapshot_version").into_response(),
    };
    if !supported_versions().contains(&version) {
        return version_unsupported(version).into_response();
    }

    let state_v = match v.get("state") {
        Some(s) => s,
        None => return malformed_json("missing state").into_response(),
    };

    let new_state = match parse_state(state_v) {
        Ok(s) => s,
        Err(e) => return malformed_json(e.message()).into_response(),
    };

    svc.load_state(new_state);

    (
        StatusCode::OK,
        axum::Json(json!({"ok": true, "snapshot_version": version})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn supported_versions_includes_current_and_next() {
        let v = supported_versions();
        assert!(v.contains(&CURRENT_SNAPSHOT_VERSION));
        assert!(v.contains(&(CURRENT_SNAPSHOT_VERSION + 1)));
    }

    #[test]
    fn parse_state_round_trip() {
        let svc = Service::new();
        svc.create_user("alice").unwrap();
        svc.create_user("bob").unwrap();
        svc.follow("alice", "bob").unwrap();
        svc.post_tweet("alice", "hi").unwrap();
        let s1 = svc.snapshot_state();
        let json = state_to_json(&s1);
        let parsed = parse_state(&json).unwrap();
        assert_eq!(parsed, s1);
    }

    #[test]
    fn parse_state_rejects_missing_clock_now() {
        let v: Value = json!({
            "id_counter_users": 0,
            "id_counter_tweets": 0,
            "users": [],
            "follows": [],
            "tweets": [],
        });
        let err = parse_state(&v).unwrap_err();
        assert!(err.message().contains("clock_now"));
    }

    #[test]
    fn parse_state_rejects_invalid_user_type() {
        let v: Value = json!({
            "clock_now": 0,
            "id_counter_users": 0,
            "id_counter_tweets": 0,
            "users": [{"id": "not-an-int", "handle": "alice"}],
            "follows": [],
            "tweets": [],
        });
        assert!(parse_state(&v).is_err());
    }

    #[test]
    fn parse_state_ignores_unknown_fields() {
        // Forward compat (K9/K15): unknown top-level fields under `state`
        // are silently ignored.
        let v: Value = json!({
            "clock_now": 0,
            "id_counter_users": 0,
            "id_counter_tweets": 0,
            "users": [],
            "follows": [],
            "tweets": [],
            "future_field": "ignored",
            "another": {"nested": true},
        });
        assert!(parse_state(&v).is_ok());
    }
}
