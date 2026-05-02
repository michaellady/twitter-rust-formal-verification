//! Replays `specs/conformance.jsonl` byte-identically through the axum
//! router. The Go impl runs the same JSONL through *its* router. Both
//! must produce structurally-identical responses (status + JSON body).
//!
//! Strategy:
//!   - Drive a deterministic `Logical` clock by ticking it forward to the
//!     `expected.body.created_at` of each `POST /tweets` step before the
//!     request is sent.
//!   - Compare expected and actual JSON via `serde_json::Value` round-trip
//!     (normalizes key order and number representation).

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use clock::{Clock, Logical};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::Value;
use service::Service;
use tower::ServiceExt;

#[derive(Deserialize)]
struct ConfLine {
    step: u32,
    name: String,
    request: ConfReq,
    expected: ConfExpected,
}

#[derive(Deserialize)]
struct ConfReq {
    method: String,
    path: String,
    #[serde(default)]
    body: Option<Value>,
}

#[derive(Deserialize)]
struct ConfExpected {
    status: u16,
    #[serde(default)]
    body: Option<Value>,
}

fn spec_path() -> Option<PathBuf> {
    // Probe well-known locations relative to the workspace root.
    let candidates = [
        // workspace root + specs/
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("specs").join("conformance.jsonl"),
        // fallback: cwd
        PathBuf::from("specs").join("conformance.jsonl"),
    ];
    for p in &candidates {
        if p.exists() {
            return Some(p.clone());
        }
    }
    None
}

fn set_clock_to(clk: &Logical, target: i64) {
    while clk.now() < target {
        clk.tick();
    }
}

#[tokio::test]
async fn conformance_replay() {
    let Some(path) = spec_path() else {
        eprintln!("specs/conformance.jsonl not found (submodule not initialized) — skipping");
        return;
    };
    let raw = std::fs::read_to_string(&path).expect("read spec");

    let clk = Arc::new(Logical::new());
    let svc = Arc::new(Service::new_with_clock(clk.clone()));
    let app = server::router(svc);

    for (lineno, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let spec: ConfLine = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {}: bad JSONL: {e}: {line}", lineno + 1));

        // Drive the clock for PostTweet.
        if spec.request.method == "POST" && spec.request.path == "/tweets" {
            if let Some(body) = &spec.expected.body {
                if let Some(ts) = body.get("created_at").and_then(|v| v.as_i64()) {
                    set_clock_to(&clk, ts);
                }
            }
        }

        // Build the request.
        let body_bytes = match &spec.request.body {
            Some(v) => serde_json::to_vec(v).unwrap(),
            None => Vec::new(),
        };
        let req = Request::builder()
            .method(spec.request.method.as_str())
            .uri(&spec.request.path)
            .header("content-type", "application/json")
            .body(Body::from(body_bytes))
            .expect("build req");
        let resp = app.clone().oneshot(req).await.expect("router");

        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();

        // Status check.
        let want_status =
            StatusCode::from_u16(spec.expected.status).expect("valid status in spec");
        assert_eq!(
            status, want_status,
            "step {} {}: status mismatch (body={:?})",
            spec.step,
            spec.name,
            String::from_utf8_lossy(&bytes)
        );

        // Body check (when expected).
        if let Some(want_body) = spec.expected.body {
            // 204 with expected body: not in spec, but be defensive.
            let got: Value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                    panic!(
                        "step {} {}: body not JSON: {e}: {}",
                        spec.step,
                        spec.name,
                        String::from_utf8_lossy(&bytes)
                    )
                })
            };
            // Normalize via re-encode (canonical key order via serde_json::Value).
            assert_eq!(
                got, want_body,
                "step {} {}: body mismatch\n  want: {}\n  got:  {}",
                spec.step,
                spec.name,
                serde_json::to_string(&want_body).unwrap(),
                serde_json::to_string(&got).unwrap(),
            );
        } else if want_status == StatusCode::NO_CONTENT {
            assert!(
                bytes.is_empty(),
                "step {} {}: 204 should have empty body, got {:?}",
                spec.step,
                spec.name,
                String::from_utf8_lossy(&bytes)
            );
        }
    }
}
