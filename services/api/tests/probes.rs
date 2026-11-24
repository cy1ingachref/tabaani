//! Integration tests for the api service: build the router and probe it.
//!
//! These are the RO-4 acceptance check. They do not need Redis or Postgres;
//! they assert the HTTP surface responds correctly and that readiness reports
//! degraded honestly when a dependency is down.

use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;

const UNREACHABLE_PG: &str = "postgres://bledi:bledi@127.0.0.1:1/bledi";
const UNREACHABLE_REDIS: &str = "redis://127.0.0.1:1";

async fn get(path: &str) -> (u16, String) {
    let state = api::AppState::new(UNREACHABLE_PG, UNREACHABLE_REDIS);
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = api::router(state).oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn healthz_is_ok_without_dependencies() {
    let (status, body) = get("/healthz").await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"status\":\"ok\""), "{body}");
}

#[tokio::test]
async fn version_reports_service_metadata() {
    let (status, body) = get("/version").await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"service\":\"api\""), "{body}");
    assert!(body.contains("\"git_sha\""), "{body}");
}

#[tokio::test]
async fn readyz_is_503_when_dependencies_are_down() {
    // Honest degradation: the api must never claim ready without its deps.
    let (status, body) = get("/readyz").await;
    assert_eq!(status, 503, "{body}");
    assert!(body.contains("degraded"), "{body}");
    assert!(body.contains("redis"), "{body}");
}

#[tokio::test]
async fn ingest_route_exists_and_upgrades_only_over_ws() {
    // A plain GET without the WebSocket upgrade headers must not be treated as
    // a successful connection; the route has to exist though.
    let state = api::AppState::new(UNREACHABLE_PG, UNREACHABLE_REDIS);
    let req = Request::builder()
        .uri("/v1/ingest")
        .body(Body::empty())
        .unwrap();
    let resp = api::router(state).oneshot(req).await.unwrap();
    assert_eq!(
        resp.status().as_u16(),
        400,
        "non-upgrade request should be rejected by the WS extractor"
    );
}

#[tokio::test]
async fn live_route_exists_and_upgrades_only_over_ws() {
    let state = api::AppState::new(UNREACHABLE_PG, UNREACHABLE_REDIS);
    let req = Request::builder()
        .uri("/v1/live")
        .body(Body::empty())
        .unwrap();
    let resp = api::router(state).oneshot(req).await.unwrap();
    assert_eq!(resp.status().as_u16(), 400);
}
