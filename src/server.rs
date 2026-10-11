//! HTTP API. Handlers do one blob read and send the bytes as they are stored.

use crate::datadir::{self, Data};
use crate::mbid;
use crate::store::Kind;
use anyhow::Result;
use arc_swap::ArcSwap;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

type App = Arc<ArcSwap<Data>>;

fn error(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

async fn healthz() -> &'static str {
    "ok"
}

async fn meta(State(app): State<App>) -> Response {
    Json(&app.load().meta).into_response()
}

async fn entity(State(app): State<App>, Path(id): Path<String>, kind: Kind) -> Response {
    let Ok(id) = mbid::parse(&id) else {
        return error(StatusCode::BAD_REQUEST, "invalid MBID");
    };
    match app.load().store.get(kind, &id) {
        Ok(Some(blob)) => ([(header::CONTENT_TYPE, "application/json")], blob).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            eprintln!("store error: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    limit: Option<usize>,
    mode: Option<String>,
}

async fn search(
    State(app): State<App>,
    Path(entity): Path<String>,
    Query(p): Query<SearchParams>,
) -> Response {
    let Some(kind) = Kind::from_api_name(&entity) else {
        return error(StatusCode::NOT_FOUND, "unknown entity");
    };
    let Some(q) = p.q.filter(|q| !q.trim().is_empty()) else {
        return error(StatusCode::BAD_REQUEST, "missing q");
    };
    let limit = p.limit.unwrap_or(10).clamp(1, 100);
    let mode = p.mode.unwrap_or_else(|| "prefix".into());
    let data = app.load();
    let Some(index) = data.names.get(&kind) else {
        return error(StatusCode::NOT_FOUND, "no search index for this entity");
    };
    let hits = match mode.as_str() {
        "prefix" => index.prefix(&q, limit),
        "fuzzy" => index.fuzzy(&q, limit),
        _ => return error(StatusCode::BAD_REQUEST, "mode must be prefix or fuzzy"),
    };
    let results: Vec<_> = hits
        .iter()
        .map(|h| {
            let mut r = json!({ "id": mbid::format(&h.id), "name": h.name });
            if !h.detail.is_empty() {
                r["detail"] = json!(h.detail);
            }
            r
        })
        .collect();
    Json(json!({ "query": q, "mode": mode, "count": results.len(), "results": results }))
        .into_response()
}

fn router(app: App) -> Router {
    let mut r = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/meta", get(meta));
    for kind in Kind::ALL {
        r = r.route(
            &format!("/v1/{}/{{id}}", kind.api_name()),
            get(move |s: State<App>, p: Path<String>| entity(s, p, kind)),
        );
    }
    r.route("/v1/search/{entity}", get(search)).with_state(app)
}

pub async fn serve(root: PathBuf, bind: String) -> Result<()> {
    let data = Data::open(&datadir::resolve(&root)?)?;
    eprintln!("serving data version {}", data.meta.data_version);
    let app: App = Arc::new(ArcSwap::from_pointee(data));
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    // Printed on stdout so tests (and scripts) can learn the real port when binding to :0.
    println!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
