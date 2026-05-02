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
    let app = server::router(svc);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .expect("bind");
    eprintln!("listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}
