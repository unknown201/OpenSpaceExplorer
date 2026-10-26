//! Small material previews for cold remote loads; full-resolution originals stay byte-for-byte intact.
use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use std::{
    collections::HashMap,
    path::{Component, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tokio::sync::Semaphore;

const BUDGET: usize = 32 * 1024 * 1024;
#[derive(Clone)]
pub struct Previews(Arc<Inner>);
struct Inner {
    root: PathBuf,
    work: Semaphore,
    cache: Mutex<Cache>,
}
#[derive(Default)]
struct Cache {
    bytes: usize,
    files: HashMap<PathBuf, Entry>,
}
struct Entry {
    modified: SystemTime,
    size: u64,
    body: Bytes,
}
impl Previews {
    pub fn new(root: PathBuf) -> Self {
        Self(Arc::new(Inner {
            root,
            work: Semaphore::new(2),
            cache: Mutex::default(),
        }))
    }
    async fn get(&self, path: PathBuf, modified: SystemTime, size: u64) -> Option<Bytes> {
        {
            let cache = self.0.cache.lock().ok()?;
            if let Some(entry) = cache
                .files
                .get(&path)
                .filter(|e| e.modified == modified && e.size == size)
            {
                return Some(entry.body.clone());
            }
        }
        let _permit = self.0.work.acquire().await.ok()?;
        let source = path.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            let mut reader = image::ImageReader::open(source).ok()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode().ok()?;
            if image.width() <= 256 && image.height() <= 256 {
                return None;
            }
            let small = image.thumbnail(256, 256).to_rgba8();
            let mut bytes = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
                .encode(
                    &small,
                    small.width(),
                    small.height(),
                    image::ExtendedColorType::Rgba8,
                )
                .ok()?;
            Some(Bytes::from(bytes))
        })
        .await
        .ok()??;
        let mut cache = self.0.cache.lock().ok()?;
        if cache.bytes + bytes.len() > BUDGET {
            cache.files.clear();
            cache.bytes = 0;
        }
        if let Some(old) = cache.files.remove(&path) {
            cache.bytes -= old.body.len();
        }
        cache.bytes += bytes.len();
        cache.files.insert(
            path,
            Entry {
                modified,
                size,
                body: bytes.clone(),
            },
        );
        Some(bytes)
    }
}

pub async fn preview(State(state): State<Previews>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let wants = request
        .uri()
        .query()
        .is_some_and(|q| q.split('&').any(|p| p == "preview=1"));
    let relative = path.strip_prefix("/textures/");
    if request.method() != axum::http::Method::GET
        || !wants
        || !relative.is_some_and(|p| {
            p.ends_with(".png") && std::path::Path::new(p).components().count() == 1
        })
    {
        return next.run(request).await;
    }
    let relative = relative.unwrap();
    if !std::path::Path::new(relative)
        .components()
        .all(|p| matches!(p, Component::Normal(_)))
    {
        return next.run(request).await;
    }
    let source = state.0.root.join("textures").join(relative);
    let metadata = match tokio::fs::metadata(&source).await {
        Ok(m) => m,
        Err(_) => return next.run(request).await,
    };
    let modified = match metadata.modified() {
        Ok(m) => m,
        Err(_) => return next.run(request).await,
    };
    let Some(bytes) = state.get(source, modified, metadata.len()).await else {
        return next.run(request).await;
    };
    let fresh = request
        .headers()
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| httpdate::parse_http_date(h).ok())
        .is_some_and(|date| {
            modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                <= date
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
        });
    let mut response = Response::new(if fresh {
        Body::empty()
    } else {
        Body::from(bytes)
    });
    if fresh {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
    }
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("image/webp"));
    response
        .headers_mut()
        .insert("x-ose-preview", HeaderValue::from_static("1"));
    response.headers_mut().insert(
        header::LAST_MODIFIED,
        HeaderValue::from_str(&httpdate::fmt_http_date(modified)).unwrap(),
    );
    response
}
