//! Binary entry point. Bootstraps a `Service` and serves on `:8080` (or
//! `$PORT` if set). This is part of the TCB.
use std::sync::Arc;

use service::Service;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);

    let svc = Arc::new(Service::new());

    if std::env::var("SEED_DEMO").as_deref() == Ok("true") {
        seed_demo(&svc);
    }

    // Stream 2 Phase 0: announce the admin-token state. The endpoint
    // handlers re-read ADMIN_TOKEN per request (so secret rotation
    // applies without a restart in environments where it's set in-place).
    // Unlike UI cookies, there is NO per-process random fallback — admin
    // auth must be explicit.
    match std::env::var("ADMIN_TOKEN") {
        Ok(v) if !v.is_empty() => {
            eprintln!("admin endpoints enabled (ADMIN_TOKEN set, {} bytes)", v.len());
        }
        _ => {
            eprintln!("WARNING: ADMIN_TOKEN unset — /_admin/* endpoints will return 503 admin_disabled");
        }
    }

    let app = server::router(svc);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}

/// Tier-4 Phase 1b: pre-populate the in-memory state on startup so the
/// public demo always shows something interesting after a Fly machine
/// restart. The verified core is in-memory by design (see TCB.md);
/// this loader is trusted (it directly invokes Service methods, no
/// spec).
///
/// Anyone hitting the live URL fresh sees alice/bob/carol with sample
/// tweets, alice already following bob — exercises F1/F2/F3/F8.
fn seed_demo(svc: &Service) {
    eprintln!("seed_demo: pre-populating alice, bob, carol + sample tweets");
    for handle in ["alice", "bob", "carol"] {
        if let Err(e) = svc.create_user(handle) {
            eprintln!("seed_demo: create_user({handle}) failed: {e:?}");
        }
    }
    let tweets = [
        ("alice", "verified backend, deployed to fly.io"),
        ("alice", "every method here is gobra/verus-checked or honestly trusted"),
        ("bob", "TLC says 141M states, 0 invariant violations"),
        ("carol", "no follow yet — see /timeline?user=alice — bob's tweet shows up, mine doesn't"),
    ];
    for (author, text) in tweets {
        if let Err(e) = svc.post_tweet(author, text) {
            eprintln!("seed_demo: post_tweet({author},…) failed: {e:?}");
        }
    }
    if let Err(e) = svc.follow("alice", "bob") {
        eprintln!("seed_demo: follow(alice,bob) failed: {e:?}");
    }
    eprintln!("seed_demo: done");
}
