//! End-to-end integration test through the axum router.
//!
//! Drives a small "user story" — three users, follow, post, timeline,
//! unfollow — through the same router the production binary uses. No
//! network involved; we use `tower::ServiceExt::oneshot`.

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
    let body = match body {
        Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
        None => Body::empty(),
    };
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(body)
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

#[tokio::test]
async fn full_user_story() {
    let clk = Arc::new(Logical::new());
    let svc = Arc::new(Service::new_with_clock(clk.clone()));
    let app = server::router(svc);

    // Create alice, bob, carol.
    let (s, b) = send(&app, "POST", "/users", Some(json!({"handle": "alice"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(b, json!({"handle": "alice", "id": 1}));

    let (s, b) = send(&app, "POST", "/users", Some(json!({"handle": "bob"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(b, json!({"handle": "bob", "id": 2}));

    let (s, _) = send(&app, "POST", "/users", Some(json!({"handle": "carol"}))).await;
    assert_eq!(s, StatusCode::CREATED);

    // Self-follow rejected.
    let (s, b) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"alice"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b, json!({"error": "self_follow_forbidden"}));

    // Alice follows bob (twice — idempotent).
    let (s, _) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = send(&app, "POST", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Bob posts at ts=1.
    clk.tick();
    let (s, b) = send(&app, "POST", "/tweets", Some(json!({"author":"bob","text":"hi"}))).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(b, json!({"id":1,"author":"bob","text":"hi","created_at":1}));

    // Alice's timeline sees bob's tweet.
    let (s, b) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({"tweets":[{"id":1,"author":"bob","text":"hi","created_at":1}],"next_cursor":null})
    );

    // Alice unfollows bob.
    let (s, _) = send(&app, "DELETE", "/follow", Some(json!({"from":"alice","to":"bob"}))).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // Timeline now empty.
    let (s, b) = send(&app, "GET", "/timeline?user=alice", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b, json!({"tweets":[],"next_cursor":null}));
}
