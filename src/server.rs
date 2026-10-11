//! HTTP API. Handlers do one blob read and send the bytes as they are stored.

use crate::compress::{self, Encoding};
use crate::datadir::{self, Data};
use crate::mbid::{self, Mbid};
use crate::store::{Kind, Rel};
use anyhow::Result;
use arc_swap::ArcSwap;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub struct AppState {
    /// The data version being served. Swapped as a whole on reload; requests that already started
    /// keep their old version alive until they finish.
    data: ArcSwap<Data>,
    root: PathBuf,
    admin_token: Option<String>,
    reload_lock: tokio::sync::Mutex<()>,
}

type App = Arc<AppState>;

// Data only changes when a new version is swapped in, and the ETag contains the version, so a
// short browser/CDN cache plus revalidation by ETag is safe.
const CACHE_CONTROL: &str = "public, max-age=300";

fn error(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn internal_error(e: anyhow::Error) -> Response {
    eprintln!("store error: {e:#}");
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

fn encoding_of(headers: &HeaderMap) -> Encoding {
    let accept = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok());
    Encoding::negotiate(accept.unwrap_or(""))
}

/// True if the client's `If-None-Match` already lists this ETag.
fn client_has(headers: &HeaderMap, etag: &str) -> bool {
    let Some(v) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    v.split(',')
        .any(|t| t.trim().trim_start_matches("W/") == etag)
}

fn json_response(enc: Encoding, etag: &str, body: Vec<u8>) -> Response {
    let mut res = ([(header::CONTENT_TYPE, "application/json")], body).into_response();
    let h = res.headers_mut();
    h.insert(
        header::ETAG,
        HeaderValue::from_str(etag).expect("etag is ascii"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if let Some(name) = enc.header() {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static(name));
    }
    res
}

fn not_modified(etag: &str) -> Response {
    let mut res = StatusCode::NOT_MODIFIED.into_response();
    res.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(etag).expect("etag is ascii"),
    );
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(CACHE_CONTROL),
    );
    res
}

async fn healthz() -> &'static str {
    "ok"
}

async fn meta(State(app): State<App>) -> Response {
    let mut res = Json(&app.data.load().meta).into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

async fn entity(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    kind: Kind,
) -> Response {
    let Ok(id) = mbid::parse(&id) else {
        return error(StatusCode::BAD_REQUEST, "invalid MBID");
    };
    let data = app.data.load();
    let enc = encoding_of(&headers);
    let etag = format!(
        "\"{}-{}{}\"",
        data.meta.data_version,
        mbid::format(&id),
        enc.etag_suffix()
    );
    if client_has(&headers, &etag) {
        return not_modified(&etag);
    }
    match data.store.get(kind, &id, enc) {
        Ok(Some(blob)) => json_response(enc, &etag, blob),
        Ok(None) => error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => internal_error(e),
    }
}

#[derive(Deserialize)]
struct PageParams {
    limit: Option<usize>,
    offset: Option<usize>,
}

/// The bytes that glue stored blobs into a list, once per encoding: [comma, closing "]}"].
/// Compressed forms are separate zstd frames / gzip members, which decode as one stream.
fn glue(enc: Encoding) -> &'static [Vec<u8>; 2] {
    static GLUE: OnceLock<Vec<[Vec<u8>; 2]>> = OnceLock::new();
    &GLUE.get_or_init(|| {
        Encoding::ALL
            .iter()
            .map(|e| {
                [
                    compress::encode(*e, b",").unwrap(),
                    compress::encode(*e, b"]}").unwrap(),
                ]
            })
            .collect()
    })[enc as usize]
}

type ListSpec = (Kind, Kind, Rel, &'static str);

/// A parent's child list (for example an artist's release groups) with each child's stored blob
/// pasted in as it is, in whatever encoding the client accepts, so nothing is parsed or compressed
/// per request. `total` counts every child the dump lists; children that were not ingested
/// (a `--limit` sample) are left out of the page.
async fn children(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(p): Query<PageParams>,
    (parent, child, rel, field): ListSpec,
) -> Response {
    let Ok(id) = mbid::parse(&id) else {
        return error(StatusCode::BAD_REQUEST, "invalid MBID");
    };
    let (limit, offset) = (p.limit.unwrap_or(25).clamp(1, 100), p.offset.unwrap_or(0));
    let data = app.data.load();
    let enc = encoding_of(&headers);
    let etag = format!(
        "\"{}-{}-{field}-{limit}-{offset}{}\"",
        data.meta.data_version,
        mbid::format(&id),
        enc.etag_suffix()
    );
    if client_has(&headers, &etag) {
        return not_modified(&etag);
    }
    let result = (|| -> Result<Option<Vec<u8>>> {
        if !data.store.contains(parent, &id)? {
            return Ok(None);
        }
        let (total, ids): (usize, Vec<Mbid>) = data.store.children(rel, &id, offset, limit)?;
        let head = format!(r#"{{"total":{total},"offset":{offset},"limit":{limit},"{field}":["#);
        let mut body = compress::encode(enc, head.as_bytes())?;
        let [comma, close] = glue(enc);
        let mut first = true;
        for child_id in ids {
            if let Some(blob) = data.store.get(child, &child_id, enc)? {
                if !first {
                    body.extend_from_slice(comma);
                }
                first = false;
                body.extend_from_slice(&blob);
            }
        }
        body.extend_from_slice(close);
        Ok(Some(body))
    })();
    match result {
        Ok(Some(body)) => json_response(enc, &etag, body),
        Ok(None) => error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => internal_error(e),
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
    let data = app.data.load();
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
        .route("/v1/meta", get(meta))
        .route("/admin/reload", post(admin_reload));
    for kind in Kind::ALL {
        r = r.route(
            &format!("/v1/{}/{{id}}", kind.api_name()),
            get(move |s: State<App>, h: HeaderMap, p: Path<String>| entity(s, h, p, kind)),
        );
    }
    let lists: [ListSpec; 2] = [
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
                move |s: State<App>, h: HeaderMap, id: Path<String>, q: Query<PageParams>| {
                    children(s, h, id, q, spec)
                },
            ),
        );
    }
    r.route("/v1/search/{entity}", get(search)).with_state(app)
}

/// Switch to whatever `current` points at now, if that is not what is already served.
/// Returns the new version, or None when nothing changed. The old version keeps serving
/// until the new one has opened successfully, and requests already in flight finish on it.
async fn reload(app: &App) -> Result<Option<String>> {
    let _one_at_a_time = app.reload_lock.lock().await;
    let dir = datadir::resolve(&app.root)?;
    if dir == app.data.load().dir {
        return Ok(None);
    }
    let opened = tokio::task::spawn_blocking(move || Data::open(&dir)).await??;
    let version = opened.meta.data_version.clone();
    app.data.store(Arc::new(opened));
    eprintln!("switched to data version {version}");
    Ok(Some(version))
}

/// Constant-time comparison, so response timing does not leak the token.
fn same_token(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// `POST /admin/reload` with `Authorization: Bearer <token>`. Off unless a token is configured.
async fn admin_reload(State(app): State<App>, headers: HeaderMap) -> Response {
    let Some(token) = &app.admin_token else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !given.is_some_and(|g| same_token(g, token)) {
        return error(StatusCode::UNAUTHORIZED, "missing or wrong token");
    }
    match reload(&app).await {
        Ok(Some(v)) => Json(json!({ "reloaded": true, "data_version": v })).into_response(),
        Ok(None) => {
            Json(json!({ "reloaded": false, "data_version": app.data.load().meta.data_version }))
                .into_response()
        }
        Err(e) => internal_error(e),
    }
}

pub struct ServeOptions {
    pub root: PathBuf,
    pub bind: String,
    /// Seconds between checks of the `current` link. 0 turns the check off.
    pub watch_secs: u64,
    pub admin_token: Option<String>,
}

pub async fn serve(opts: ServeOptions) -> Result<()> {
    let data = Data::open(&datadir::resolve(&opts.root)?)?;
    eprintln!("serving data version {}", data.meta.data_version);
    let app: App = Arc::new(AppState {
        data: ArcSwap::from_pointee(data),
        root: opts.root,
        admin_token: opts.admin_token.filter(|t| !t.is_empty()),
        reload_lock: tokio::sync::Mutex::new(()),
    });

    if opts.watch_secs > 0 {
        let app = app.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(opts.watch_secs));
            let mut last_error = String::new();
            loop {
                tick.tick().await;
                match reload(&app).await {
                    Ok(_) => last_error.clear(),
                    // Keep serving the old version; only say it once per distinct problem.
                    Err(e) if e.to_string() != last_error => {
                        last_error = e.to_string();
                        eprintln!("reload failed, still serving the old version: {e:#}");
                    }
                    Err(_) => {}
                }
            }
        });
    }
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    {
        let app = app.clone();
        tokio::spawn(async move {
            while hangup.recv().await.is_some() {
                match reload(&app).await {
                    Ok(Some(_)) => {}
                    Ok(None) => eprintln!("SIGHUP: already serving the newest version"),
                    Err(e) => eprintln!("reload failed, still serving the old version: {e:#}"),
                }
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(&opts.bind).await?;
    // Printed on stdout so tests (and scripts) can learn the real port when binding to :0.
    println!("listening on http://{}", listener.local_addr()?);
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    axum::serve(listener, router(app))
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        })
        .await?;
    Ok(())
}
