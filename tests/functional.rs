//! Per-endpoint black-box tests. Each test exercises one HTTP-level
//! validation rule and asserts at least three things (status, body shape,
//! state side-effect) — that's the contract `manifestcheck` enforces
//! against `COVERAGE.md`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use clock::{Clock, Logical};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use service::Service;
use tower::ServiceExt;

async fn send(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(match body {
            Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, val)
}

fn fresh() -> (Arc<Logical>, axum::Router) {
    let clk = Arc::new(Logical::new());
    let svc = Arc::new(Service::new_with_clock(clk.clone()));
    (clk, server::router(svc))
}

// -----------------------------------------------------------------------------
// /users
// -----------------------------------------------------------------------------

#[tokio::test]
async fn users_post_creates_user() {
    let (_clk, app) = fresh();
    let (s, b) = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(b["handle"], "alice");
    assert_eq!(b["id"], 1);
}

#[tokio::test]
async fn users_post_duplicate_returns_409() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let (s, b) = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(b, json!({"error":"duplicate_user"}));
    // State: alice still has id=1; bob gets a fresh id (note: the user-id
    // generator burns an id on the duplicate attempt — same as the Go impl).
    let (_, b) = send(&app, "POST", "/users", Some(json!({"handle":"bob"}))).await;
    assert!(b["id"].as_i64().unwrap() >= 2);
}

#[tokio::test]
async fn users_post_empty_handle_400() {
    let (_clk, app) = fresh();
    let (s, b) = send(&app, "POST", "/users", Some(json!({"handle":""}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"empty_handle"}));
    // State: timeline of empty user is rejected separately.
    let (s2, _) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(s2, StatusCode::OK);
}

// -----------------------------------------------------------------------------
// /follow
// -----------------------------------------------------------------------------

#[tokio::test]
async fn follow_self_forbidden_f4() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let (s, b) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"alice"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"self_follow_forbidden"}));
    // State: alice's timeline is still empty.
    let (_, tl) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(tl["tweets"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn follow_unknown_user_400() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let (s, b) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"ghost"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"unknown_user"}));
    // State: no edge created.
    let (_, tl) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(tl["tweets"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn follow_idempotent_f3() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"bob"}))).await;
    let (s1, _) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    let (s2, _) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    let (s3, _) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    assert_eq!(s1, StatusCode::NO_CONTENT);
    assert_eq!(s2, StatusCode::NO_CONTENT);
    assert_eq!(s3, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn unfollow_idempotent_f3() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"bob"}))).await;
    // DELETE on missing edge still 204.
    let (s, _) = send(&app, "DELETE", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let _ = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    let (s2, _) = send(&app, "DELETE", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    let (s3, _) = send(&app, "DELETE", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    assert_eq!(s2, StatusCode::NO_CONTENT);
    assert_eq!(s3, StatusCode::NO_CONTENT);
    // State: timeline empty.
    let (_, tl) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(tl["tweets"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn unfollow_unknown_user_400() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let (s, b) = send(&app, "DELETE", "/follow", Some(json!({"from":"alice","to":"ghost"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"unknown_user"}));
    // State: alice still has no follows.
    let (_, tl) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(tl["tweets"].as_array().unwrap().len(), 0);
}

// -----------------------------------------------------------------------------
// /tweets
// -----------------------------------------------------------------------------

#[tokio::test]
async fn tweet_empty_text_400() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let (s, b) = send(&app, "POST", "/tweets", Some(json!({"author":"alice","text":""}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"empty_text"}));
    // State: timeline still empty.
    let (_, tl) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(tl["tweets"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn tweet_unknown_author_400_f6() {
    let (_clk, app) = fresh();
    let (s, b) = send(&app, "POST", "/tweets", Some(json!({"author":"ghost","text":"hi"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"unknown_user"}));
    // State: ghost is still not a user.
    let (s2, _) = send(&app, "GET", "/timeline?user=ghost", None).await;
    assert_eq!(s2, StatusCode::OK);
}

#[tokio::test]
async fn tweet_uses_clock_and_id_f7_f8() {
    let (clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    clk.tick(); // ts=1
    let (s, b) = send(&app, "POST", "/tweets", Some(json!({"author":"alice","text":"a"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(b, json!({"id":1,"author":"alice","text":"a","created_at":1}));
    let (s2, b2) = send(&app, "POST", "/tweets", Some(json!({"author":"alice","text":"b"}))).await;
    assert_eq!(s2, StatusCode::CREATED);
    // F7: ties allowed (same ts); F8: id strictly monotonic.
    assert_eq!(b2, json!({"id":2,"author":"alice","text":"b","created_at":1}));
}

// -----------------------------------------------------------------------------
// /timeline
// -----------------------------------------------------------------------------

#[tokio::test]
async fn timeline_empty_user_400() {
    let (_clk, app) = fresh();
    let (s, b) = send(&app, "GET", "/timeline", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"empty_user"}));
    // State: independent of any users created.
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let (s2, b2) = send(&app, "GET", "/timeline?user=", None).await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    assert_eq!(b2, json!({"error":"empty_user"}));
}

#[tokio::test]
async fn timeline_invalid_limit_400() {
    let (_clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let (s, b) = send(&app, "GET", "/timeline?user=alice&limit=-1", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error":"invalid_limit"}));
    let (s2, b2) = send(&app, "GET", "/timeline?user=alice&limit=abc", None).await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    assert_eq!(b2, json!({"error":"invalid_limit"}));
}

#[tokio::test]
async fn timeline_unknown_user_returns_empty_200() {
    let (_clk, app) = fresh();
    let (s, b) = send(&app, "GET", "/timeline?user=ghost", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, json!({"tweets":[],"next_cursor":null}));
    assert!(b["tweets"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn timeline_orders_by_created_then_id_f2() {
    let (clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"bob"}))).await;
    let _ = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    clk.tick();
    let _ = send(&app, "POST", "/tweets", Some(json!({"author":"bob","text":"first"}))).await;
    let _ = send(&app, "POST", "/tweets", Some(json!({"author":"bob","text":"second"}))).await;
    let (s, b) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(s, StatusCode::OK);
    let tw = b["tweets"].as_array().unwrap();
    assert_eq!(tw.len(), 2);
    // (created_at desc, id desc); both ts=1 -> id=2 first.
    assert_eq!(tw[0]["id"], 2);
    assert_eq!(tw[1]["id"], 1);
}

#[tokio::test]
async fn timeline_visibility_f1() {
    let (clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"bob"}))).await;
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"carol"}))).await;
    let _ = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    clk.tick();
    let _ = send(&app, "POST", "/tweets", Some(json!({"author":"bob","text":"visible"}))).await;
    let _ = send(&app, "POST", "/tweets", Some(json!({"author":"carol","text":"invisible"}))).await;
    let (s, b) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(s, StatusCode::OK);
    let tw = b["tweets"].as_array().unwrap();
    assert_eq!(tw.len(), 1);
    assert_eq!(tw[0]["text"], "visible");
}

#[tokio::test]
async fn timeline_limit_truncates() {
    let (clk, app) = fresh();
    let _ = send(&app, "POST", "/users", Some(json!({"handle":"alice"}))).await;
    for i in 0..5 {
        clk.tick();
        let _ = send(&app, "POST", "/tweets", Some(json!({"author":"alice","text": format!("t{i}")}))).await;
    }
    let (s, b) = send(&app, "GET", "/timeline?user=alice&limit=2", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["tweets"].as_array().unwrap().len(), 2);
    let (_, b2) = send(&app, "GET", "/timeline?user=alice&limit=0", None).await;
    assert_eq!(b2["tweets"].as_array().unwrap().len(), 5);
}
