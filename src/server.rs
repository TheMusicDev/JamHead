//! HTTP API. Handlers do one blob read and send the bytes as they are stored.

use crate::datadir::{self, Data};
use crate::mbid;
use crate::store::{Kind, Rel};
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
struct PageParams {
    limit: Option<usize>,
    offset: Option<usize>,
}

/// A parent's child list (for example an artist's release groups) with each child's stored blob
/// pasted in as it is. `total` counts every child the dump lists; children that were not ingested
/// (a `--limit` sample) are left out of the page.
async fn children(
    State(app): State<App>,
    Path(id): Path<String>,
    Query(p): Query<PageParams>,
    (parent, child, rel, field): (Kind, Kind, Rel, &'static str),
) -> Response {
    let Ok(id) = mbid::parse(&id) else {
        return error(StatusCode::BAD_REQUEST, "invalid MBID");
    };
    let (limit, offset) = (p.limit.unwrap_or(25).clamp(1, 100), p.offset.unwrap_or(0));
    let data = app.load();
    let result = (|| -> anyhow::Result<Option<Vec<u8>>> {
        if !data.store.contains(parent, &id)? {
            return Ok(None);
        }
        let (total, ids) = data.store.children(rel, &id, offset, limit)?;
        let mut body =
            format!(r#"{{"total":{total},"offset":{offset},"limit":{limit},"{field}":["#)
                .into_bytes();
        let mut first = true;
        for child_id in ids {
            if let Some(blob) = data.store.get(child, &child_id)? {
                if !first {
                    body.push(b',');
                }
                first = false;
                body.extend_from_slice(&blob);
            }
        }
        body.extend_from_slice(b"]}");
        Ok(Some(body))
    })();
    match result {
        Ok(Some(body)) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
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
    let hits = match mode.as_str() {
        "prefix" | "fuzzy" => {
            let Some(index) = data.names.get(&kind) else {
                return error(StatusCode::NOT_FOUND, "no search index for this entity");
            };
            if mode == "prefix" {
                index.prefix(&q, limit)
            } else {
                index.fuzzy(&q, limit)
            }
        }
        "text" => {
            let Some(index) = data.text.get(&kind) else {
                return error(StatusCode::NOT_FOUND, "no search index for this entity");
            };
            match index.search(&q, limit) {
                Ok(hits) => hits,
                Err(e) => {
                    eprintln!("text search error: {e:#}");
                    return error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
                }
            }
        }
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "mode must be prefix, fuzzy or text",
            )
        }
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
    let lists = [
        (
            Kind::Artist,
            Kind::ReleaseGroup,
            Rel::ArtistReleaseGroups,
            "release-groups",
        ),
        (
            Kind::ReleaseGroup,
            Kind::Release,
            Rel::ReleaseGroupReleases,
            "releases",
        ),
    ];
    for spec in lists {
        r = r.route(
            &format!("/v1/{}/{{id}}/{}", spec.0.api_name(), spec.3),
            get(
                move |s: State<App>, id: Path<String>, q: Query<PageParams>| {
                    children(s, id, q, spec)
                },
            ),
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
