//! `tabaani` api service: ingest (pings up) and live feed (vehicle states down).
//!
//! Scope here: HTTP + WebSocket, Redis Streams as the queue, and a live-state
//! store in Redis. The matcher worker that turns pings into vehicle state is a
//! separate binary (`services/matcher`).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde_json::json;
use tower_http::trace::TraceLayer;

pub mod pb;
pub mod state;
mod ws;

pub use pb::*;
pub use state::StoredVehicle;

const BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Git SHA baked in at build time; falls back to "dev" for local builds.
pub fn git_sha() -> &'static str {
    option_env!("GIT_SHA").unwrap_or("dev")
}

fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    redis_url: String,
    database_url: String,
    started_at: chrono::DateTime<chrono::Utc>,
}

impl AppState {
    pub fn new(database_url: impl Into<String>, redis_url: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(Inner {
                database_url: database_url.into(),
                redis_url: redis_url.into(),
                started_at: chrono::Utc::now(),
            }),
        }
    }

    pub fn uptime_s(&self) -> i64 {
        (chrono::Utc::now() - self.inner.started_at)
            .num_seconds()
            .max(0)
    }

    pub fn database_url(&self) -> &str {
        &self.inner.database_url
    }

    pub fn redis_url(&self) -> &str {
        &self.inner.redis_url
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/version", get(version))
        .route("/v1/ingest", get(ws::ingest_handler))
        .route("/v1/live", get(ws::live_handler))
        .route("/v1/stops/{id}/arrivals", get(arrivals))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// Readiness: dependencies reachable. Cheap probes only.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let mut checks = serde_json::Map::new();
    let mut ok = true;

    match ping_postgres(state.database_url()).await {
        Ok(()) => {
            checks.insert("postgres".into(), json!({ "ok": true }));
        }
        Err(e) => {
            ok = false;
            tracing::warn!(error = %e, "readiness probe: postgres unreachable");
            checks.insert(
                "postgres".into(),
                json!({ "ok": false, "error": e.to_string() }),
            );
        }
    };
    match ping_redis(state.redis_url()).await {
        Ok(()) => {
            checks.insert("redis".into(), json!({ "ok": true }));
        }
        Err(e) => {
            ok = false;
            tracing::warn!(error = %e, "readiness probe: redis unreachable");
            checks.insert(
                "redis".into(),
                json!({ "ok": false, "error": e.to_string() }),
            );
        }
    };

    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(json!({
            "status": if ok { "ready" } else { "degraded" },
            "checks": checks,
        })),
    )
}

async fn version() -> impl IntoResponse {
    Json(json!({
        "service": "api",
        "version": BUILD_VERSION,
        "git_sha": git_sha(),
        "build_profile": build_profile(),
        "uptime_s": 0,
    }))
}

/// Next arrivals at a stop. Until Postgres is wired (Phase 1b) this derives
/// them from the live feed in Redis so the shape of the API is real and the
/// numbers are honest about being schedule-derived.
async fn arrivals(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match arrivals_inner(&state, &id).await {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "stop_id": id, "error": e.to_string() })),
        ),
    }
}

async fn arrivals_inner(state: &AppState, id: &str) -> anyhow::Result<serde_json::Value> {
    let client = redis::Client::open(state.redis_url())?;
    let mut conn = client.get_multiplexed_async_connection().await?;
    // Live vehicles currently known, newest first.
    let ids: Vec<String> = redis::cmd("SMEMBERS")
        .arg("veh:ids")
        .query_async(&mut conn)
        .await
        .unwrap_or_default();

    let mut out = Vec::new();
    for vid in ids.into_iter().take(50) {
        let key = format!("veh:{vid}");
        let json: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .ok()
            .flatten();
        if let Some(json) = json {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                out.push(v);
            }
        }
    }
    Ok(json!({ "stop_id": id, "vehicles": out, "basis": "live-redis" }))
}

async fn ping_postgres(url: &str) -> anyhow::Result<()> {
    // A cheap `SELECT 1` over a short-lived connection: readiness must not
    // depend on the matcher pool, just on the database answering.
    let (client, conn) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .map_err(|e| anyhow::anyhow!("postgres connect failed ({}): {e}", redact_pg(url)))?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::warn!(error = %e, "postgres probe connection error");
        }
    });
    let row = tokio::time::timeout(Duration::from_secs(3), client.query_one("SELECT 1", &[]))
        .await
        .map_err(|_| anyhow::anyhow!("postgres probe timed out"))?
        .map_err(|e| anyhow::anyhow!("postgres probe query failed: {e}"))?;
    let v: i32 = row.get(0);
    anyhow::ensure!(v == 1, "unexpected postgres probe result");
    Ok(())
}

/// Mask the password in a Postgres URL before logging it.
fn redact_pg(url: &str) -> String {
    if let Some(rest) = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))
    {
        if let Some((userinfo, host)) = rest.split_once('@') {
            if let Some((user, _pw)) = userinfo.split_once(':') {
                return format!("postgres://{user}:***@{host}");
            }
        }
    }
    url.to_string()
}

async fn ping_redis(url: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(url)?;
    let mut conn = tokio::time::timeout(
        Duration::from_secs(2),
        client.get_multiplexed_async_connection(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("redis connect timeout"))??;
    let pong: String = redis::cmd("PING").query_async(&mut conn).await?;
    anyhow::ensure!(pong.eq_ignore_ascii_case("PONG"), "unexpected redis probe");
    Ok(())
}
