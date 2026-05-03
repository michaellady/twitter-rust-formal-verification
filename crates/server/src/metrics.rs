//! Tier-4 Phase 4 — Prometheus /metrics + structured JSON request logs.
//!
//! Trust: this whole module is in the TCB. The metrics it emits are
//! observability of the verified core's HTTP boundary; they don't
//! influence verification.
//!
//! Three families:
//! - `http_requests_total{path,method,status}` — request counter
//! - `http_request_duration_seconds{path,method}` — histogram
//! - `f_property_violations_total{f}` — counter, incremented when an
//!   API endpoint returns the JSON error code for an F-property
//!   violation (self_follow_forbidden → F4, unknown_user when posting →
//!   F6, unknown_user when following → F9, etc.). The verified core
//!   ENFORCES the F-properties by REJECTING violating attempts; this
//!   counter measures how often clients try.

use std::time::Instant;

use axum::{
    body::Body,
    extract::{MatchedPath, Request},
    http::{Method, StatusCode},
    middleware::Next,
    response::Response,
};
use once_cell::sync::Lazy;
use prometheus::{
    register_counter_vec, register_histogram_vec, CounterVec, HistogramVec,
    Registry, TextEncoder,
};

pub static REGISTRY: Lazy<Registry> = Lazy::new(Registry::new);

pub static HTTP_REQUESTS: Lazy<CounterVec> = Lazy::new(|| {
    let m = register_counter_vec!(
        "http_requests_total",
        "Total number of HTTP requests",
        &["path", "method", "status"]
    )
    .expect("register http_requests_total");
    REGISTRY.register(Box::new(m.clone())).ok();
    m
});

pub static HTTP_DURATION: Lazy<HistogramVec> = Lazy::new(|| {
    let m = register_histogram_vec!(
        "http_request_duration_seconds",
        "HTTP request duration",
        &["path", "method"],
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]
    )
    .expect("register http_request_duration_seconds");
    REGISTRY.register(Box::new(m.clone())).ok();
    m
});

pub static F_PROPERTY_VIOLATIONS: Lazy<CounterVec> = Lazy::new(|| {
    let m = register_counter_vec!(
        "f_property_violations_total",
        "Times a client attempted an action the verified core REJECTED. \
         The verified core enforces F-properties by rejecting violating \
         attempts; this counter measures the attempt rate, not violations \
         of the verified core itself.",
        &["f"]
    )
    .expect("register f_property_violations_total");
    REGISTRY.register(Box::new(m.clone())).ok();
    m
});

/// Map a JSON error code returned by the API to an F-property label.
/// Called from handlers when emitting an error response. Returns `None`
/// if the error doesn't map to a numbered F-property.
pub fn note_violation(error_code: &str, request_path: &str) {
    let f = match (error_code, request_path) {
        ("self_follow_forbidden", _) => "F4",
        ("unknown_user", p) if p.starts_with("/tweets") => "F6",
        ("unknown_user", p) if p.starts_with("/follow") || p.starts_with("/_ui/") => "F9",
        ("duplicate_user", _) => "F-uniqueness",
        ("empty_handle", _) | ("empty_text", _) => "F-input-shape",
        _ => return,
    };
    F_PROPERTY_VIOLATIONS.with_label_values(&[f]).inc();
}

/// axum middleware: wrap every request, observe duration + status.
pub async fn track(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method: Method = req.method().clone();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());

    let response = next.run(req).await;
    let status = response.status();

    HTTP_DURATION
        .with_label_values(&[&path, method.as_str()])
        .observe(start.elapsed().as_secs_f64());
    HTTP_REQUESTS
        .with_label_values(&[&path, method.as_str(), status.as_str()])
        .inc();

    log_json_line(&method, &path, status, start.elapsed().as_secs_f64());
    response
}

/// Write a single-line JSON log entry to stderr — Fly Machines capture
/// stderr as the app's log stream.
fn log_json_line(method: &Method, path: &str, status: StatusCode, dur_s: f64) {
    let dur_ms = (dur_s * 1000.0).round() as u64;
    eprintln!(
        r#"{{"ts":{ts},"level":"info","msg":"http","method":"{m}","path":"{p}","status":{s},"dur_ms":{d}}}"#,
        ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        m = method,
        p = path,
        s = status.as_u16(),
        d = dur_ms,
    );
}

/// GET /metrics — Prometheus text exposition.
pub async fn render() -> (StatusCode, [(&'static str, &'static str); 1], String) {
    let encoder = TextEncoder::new();
    let mut out = String::new();
    if let Err(e) = encoder.encode_utf8(&REGISTRY.gather(), &mut out) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/plain")],
            format!("error: {e}"),
        );
    }
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        out,
    )
}

#[allow(dead_code)] // exported for completeness; axum's Body type matches
pub type _Body = Body;
