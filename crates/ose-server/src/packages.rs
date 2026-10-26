//! Usage bundles stream unchanged files and material previews in a small framed container. The
//! catalog derives from the exported assets, so existing installations need no separate export step.
use axum::{
    body::{Body, Bytes},
    extract::{Path, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::StreamExt;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    hash::{Hash, Hasher},
    io,
    path::{Component, Path as FsPath, PathBuf},
    sync::Arc,
    time::{Instant, SystemTime},
};
use tokio::{io::AsyncReadExt, sync::Mutex};

const TARGET: u64 = 8 * 1024 * 1024;
const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_JSON: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
struct Packages(Arc<Inner>);
struct Inner {
    root: PathBuf,
    previews: crate::previews::Previews,
    catalog: Mutex<Option<(Instant, Arc<Catalog>)>>,
    encoded: std::sync::Mutex<Encoded>,
}
#[derive(Default)]
struct Encoded {
    files: BTreeMap<String, (axum::http::HeaderMap, Bytes)>,
    order: VecDeque<String>,
    bytes: usize,
}
#[derive(Clone)]
struct Entry {
    path: String,
    size: u64,
    modified: SystemTime,
    preview: bool,
    preload: bool,
}
impl Entry {
    fn url(&self) -> String {
        format!(
            "assets/{}{}",
            self.path,
            if self.preview { "?preview=1" } else { "" }
        )
    }
    fn estimate(&self) -> u64 {
        if self.preview {
            self.size.min(64 * 1024)
        } else {
            self.size
        }
    }
}
#[derive(Default, Serialize)]
struct Group {
    depends: Vec<String>,
    chunks: Vec<String>,
    prefetch: Vec<String>,
}
#[derive(Serialize)]
struct Chunk {
    url: String,
    revision: String,
    assets: Vec<String>,
    #[serde(skip)]
    entries: Vec<Entry>,
}
#[derive(Serialize)]
struct Catalog {
    version: u8,
    groups: BTreeMap<String, Group>,
    chunks: BTreeMap<String, Chunk>,
}

pub fn router(root: PathBuf, previews: crate::previews::Previews) -> Router {
    let state = Packages(Arc::new(Inner {
        root,
        previews,
        catalog: Mutex::new(None),
        encoded: std::sync::Mutex::default(),
    }));
    let app = Router::new()
        .route("/packages/index.json", get(index))
        .route(
            "/packages/{id}/{revision}",
            get(bundle)
                .layer(
                    tower_http::compression::CompressionLayer::new()
                        .quality(tower_http::compression::CompressionLevel::Precise(6)),
                )
                .layer(axum::middleware::from_fn_with_state(
                    state.clone(),
                    cached_bundle,
                )),
        )
        .with_state(state.clone());
    // Prepare common compressed bytes once, independently of any remote player's link. Later guests
    // share these immutable buffers rather than recompressing a large model for each connection.
    if tokio::runtime::Handle::try_current().is_ok() {
        let warm = app.clone();
        tokio::spawn(async move {
            use tower::ServiceExt;
            let Ok(catalog) = state.catalog().await else {
                return;
            };
            for name in ["core", "materials-shared", "ksc", "vab"] {
                let Some(group) = catalog.groups.get(name) else {
                    continue;
                };
                for id in &group.prefetch {
                    let chunk = &catalog.chunks[id];
                    let request = axum::http::Request::builder()
                        .uri(chunk.url.trim_start_matches("assets"))
                        .header(header::ACCEPT_ENCODING, "gzip")
                        .body(Body::empty())
                        .unwrap();
                    let response = warm.clone().oneshot(request).await.unwrap();
                    let _ = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024).await;
                }
            }
        });
    }
    app
}

// Retain only finished encoded streams. Aborted/truncated downloads cannot populate this cache.
async fn cached_bundle(
    State(state): State<Packages>,
    request: Request,
    next: axum::middleware::Next,
) -> Response {
    let accepts = request
        .headers()
        .get(header::ACCEPT_ENCODING)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| {
            h.split(',').any(|coding| {
                let mut parts = coding.trim().split(';');
                parts.next() == Some("gzip")
                    && !parts.any(|p| {
                        p.trim()
                            .strip_prefix("q=")
                            .is_some_and(|q| q.parse::<f32>().unwrap_or(0.0) == 0.0)
                    })
            })
        });
    if request.method() != axum::http::Method::GET || !accepts {
        return next.run(request).await;
    }
    let key = request.uri().path().to_string();
    let saved = state
        .0
        .encoded
        .lock()
        .ok()
        .and_then(|c| c.files.get(&key).cloned());
    if let Some((headers, bytes)) = saved {
        let mut response = Response::new(Body::from(bytes));
        *response.headers_mut() = headers;
        response
            .headers_mut()
            .insert("x-ose-bundle-cache", "hit".parse().unwrap());
        return response;
    }
    let response = next.run(request).await;
    if response.status() != StatusCode::OK
        || response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_none_or(|h| h != "gzip")
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let headers = parts.headers.clone();
    let stream = futures_util::stream::try_unfold(
        (body.into_data_stream(), Vec::new(), true),
        move |(mut source, mut bytes, mut retain)| {
            let (state, key, headers) = (state.clone(), key.clone(), headers.clone());
            async move {
                match source.next().await {
                    Some(Ok(chunk)) => {
                        if retain && bytes.len() + chunk.len() <= 32 * 1024 * 1024 {
                            bytes.extend_from_slice(&chunk);
                        } else {
                            retain = false;
                            bytes.clear();
                        }
                        Ok::<_, axum::Error>(Some((chunk, (source, bytes, retain))))
                    }
                    Some(Err(error)) => Err(error),
                    None => {
                        if retain {
                            if let Ok(mut cache) = state.0.encoded.lock() {
                                if let Some((_, old)) = cache.files.remove(&key) {
                                    cache.bytes -= old.len();
                                    cache.order.retain(|k| k != &key);
                                }
                                while cache.bytes + bytes.len() > 96 * 1024 * 1024 {
                                    let Some(old) = cache.order.pop_front() else {
                                        break;
                                    };
                                    if let Some((_, body)) = cache.files.remove(&old) {
                                        cache.bytes -= body.len();
                                    }
                                }
                                cache.bytes += bytes.len();
                                cache.order.push_back(key.clone());
                                cache.files.insert(key, (headers, Bytes::from(bytes)));
                            }
                        }
                        Ok(None)
                    }
                }
            }
        },
    );
    Response::from_parts(parts, Body::from_stream(stream))
}
impl Packages {
    async fn catalog(&self) -> io::Result<Arc<Catalog>> {
        let mut cached = self.0.catalog.lock().await;
        if let Some((at, catalog)) = &*cached {
            if at.elapsed().as_secs() < 30 {
                return Ok(catalog.clone());
            }
        }
        let root = self.0.root.clone();
        let catalog = Arc::new(
            tokio::task::spawn_blocking(move || build(&root))
                .await
                .map_err(io::Error::other)??,
        );
        *cached = Some((Instant::now(), catalog.clone()));
        Ok(catalog)
    }
}
async fn index(State(state): State<Packages>) -> Response {
    match state.catalog().await {
        Ok(catalog) => ([(header::CACHE_CONTROL, "no-cache")], Json(&*catalog)).into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
async fn bundle(
    State(state): State<Packages>,
    Path((id, revision)): Path<(String, String)>,
) -> Response {
    let Ok(catalog) = state.catalog().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Some(chunk) = catalog.chunks.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if revision != format!("{}.bin", chunk.revision) {
        return StatusCode::GONE.into_response();
    }
    let stream = futures_util::stream::try_unfold(
        Stream {
            root: state.0.root.clone(),
            previews: state.0.previews.clone(),
            entries: chunk.entries.clone().into(),
            data: None,
            started: false,
            ended: false,
        },
        |mut stream| async move {
            let bytes = stream.next().await?;
            Ok::<_, io::Error>(bytes.map(|bytes| (bytes, stream)))
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "public, max-age=31536000, immutable".parse().unwrap(),
    );
    response
}

enum Data {
    File(tokio::fs::File, u64),
    Preview(Bytes),
}
struct Stream {
    root: PathBuf,
    previews: crate::previews::Previews,
    entries: VecDeque<Entry>,
    data: Option<Data>,
    started: bool,
    ended: bool,
}
impl Stream {
    async fn next(&mut self) -> io::Result<Option<Bytes>> {
        if !self.started {
            self.started = true;
            return Ok(Some(Bytes::from_static(b"OSEPACK1")));
        }
        if let Some(data) = self.data.take() {
            match data {
                Data::Preview(bytes) => return Ok(Some(bytes)),
                Data::File(mut file, left) if left > 0 => {
                    let mut bytes = vec![0; left.min(64 * 1024) as usize];
                    file.read_exact(&mut bytes).await?;
                    self.data = Some(Data::File(file, left - bytes.len() as u64));
                    return Ok(Some(Bytes::from(bytes)));
                }
                _ => {}
            }
        }
        let Some(entry) = self.entries.pop_front() else {
            if self.ended {
                return Ok(None);
            }
            self.ended = true;
            return Ok(Some(Bytes::from_static(&[0, 0, 0, 0])));
        };
        let path = self.root.join(&entry.path);
        let file = tokio::fs::File::open(&path).await?;
        let metadata = file.metadata().await?;
        if metadata.len() != entry.size || metadata.modified()? != entry.modified {
            return Err(io::Error::other("asset changed during bundle generation"));
        }
        let preview = if entry.preview {
            self.previews.get(path, entry.modified, entry.size).await
        } else {
            None
        };
        let (size, transformed) = if let Some(bytes) = preview {
            let size = bytes.len();
            self.data = Some(Data::Preview(bytes));
            (size as u64, true)
        } else {
            self.data = Some(Data::File(file, entry.size));
            (entry.size, false)
        };
        let header = serde_json::to_vec(&serde_json::json!({
            "url":entry.url(), "size":size, "type":if transformed {"image/webp"} else {mime(&entry.path)},
            "preview":transformed, "modified":httpdate::fmt_http_date(entry.modified),
        }))?;
        let mut bytes = (header.len() as u32).to_le_bytes().to_vec();
        bytes.extend(header);
        Ok(Some(bytes.into()))
    }
}
fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "json" => "application/json",
        "glb" => "model/gltf-binary",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    }
}
fn add(root: &FsPath, entries: &mut Vec<Entry>, path: &str, preview: bool, preload: bool) {
    if !FsPath::new(path)
        .components()
        .all(|p| matches!(p, Component::Normal(_)))
    {
        return;
    }
    if let Ok(m) = std::fs::metadata(root.join(path)) {
        if m.is_file() && m.len() <= MAX_FILE {
            if let Ok(modified) = m.modified() {
                entries.push(Entry {
                    path: path.into(),
                    size: m.len(),
                    modified,
                    preview,
                    preload,
                });
            }
        }
    }
}
fn files(root: &FsPath, dir: &str) -> Vec<String> {
    let mut result = Vec::new();
    let Ok(read) = std::fs::read_dir(root.join(dir)) else {
        return result;
    };
    for file in read.flatten() {
        let path = file.path();
        let relative = format!("{dir}/{}", file.file_name().to_string_lossy());
        if path.is_dir() {
            result.extend(files(root, &relative));
        } else if !relative.ends_with(".gz") && !relative.ends_with(".tmp") {
            result.push(relative);
        }
    }
    result.sort();
    result
}
fn json(root: &FsPath, path: &str) -> Option<serde_json::Value> {
    if std::fs::metadata(root.join(path)).ok()?.len() > MAX_JSON {
        return None;
    }
    serde_json::from_slice(&std::fs::read(root.join(path)).ok()?).ok()
}
fn scene(root: &FsPath, name: &str) -> Vec<Entry> {
    use std::io::{Read, Seek, SeekFrom};
    let mut entries = Vec::new();
    add(
        root,
        &mut entries,
        &format!("{name}/scene.json"),
        false,
        true,
    );
    let Some(metadata) = json(root, &format!("{name}/scene.json")) else {
        return entries;
    };
    let Some(model) = metadata["file"].as_str() else {
        return entries;
    };
    add(root, &mut entries, model, false, true);
    let glb = (|| -> Option<serde_json::Value> {
        let mut file = std::fs::File::open(root.join(model)).ok()?;
        let mut header = [0; 20];
        file.read_exact(&mut header).ok()?;
        if &header[..4] != b"glTF" || &header[16..20] != b"JSON" {
            return None;
        }
        let size = u32::from_le_bytes(header[12..16].try_into().ok()?) as u64;
        if size > MAX_JSON {
            return None;
        }
        file.seek(SeekFrom::Start(20)).ok()?;
        let mut bytes = vec![0; size as usize];
        file.read_exact(&mut bytes).ok()?;
        serde_json::from_slice(&bytes).ok()
    })();
    if let Some(images) = glb.as_ref().and_then(|j| j["images"].as_array()) {
        for uri in images.iter().filter_map(|i| i["uri"].as_str()) {
            // This is the same exported-material base used by the renderer.
            if let Some(name) = uri.strip_prefix("../textures/") {
                if !name.contains('/') && name.ends_with(".png") {
                    add(root, &mut entries, &format!("textures/{name}"), true, true);
                }
            }
        }
    }
    entries
}
fn build(root: &FsPath) -> io::Result<Catalog> {
    let mut groups: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let mut core = Vec::new();
    for path in files(root, "ui")
        .into_iter()
        .filter(|p| !p[3..].contains('/') || p.starts_with("ui/fonts/"))
    {
        // Palette thumbnails and large loading artwork stay on demand.
        if std::fs::metadata(root.join(&path)).is_ok_and(|m| m.len() <= 256 * 1024) {
            add(root, &mut core, &path, false, true);
        }
    }
    for path in ["loc/en.json", "audio/sounds.json"] {
        add(root, &mut core, path, false, true);
    }
    groups.insert("core".into(), core);
    groups.insert("mission-control".into(), Vec::new()); // mission data arrives over the game protocol
    for name in ["ksc", "vab"] {
        groups.insert(name.into(), scene(root, name));
    }
    let mut flight = Vec::new();
    for path in [
        "planets/terrain.json",
        "planets/terrain.bin",
        "textures/skybox_spacescape_starfield.png",
    ] {
        add(root, &mut flight, path, false, true);
    }
    groups.insert("flight-shared".into(), flight);
    if let Ok(dirs) = std::fs::read_dir(root.join("planets")) {
        for dir in dirs.flatten().filter(|d| d.path().is_dir()) {
            let body = dir.file_name().to_string_lossy().into_owned();
            if !body
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                continue;
            }
            let mut entries = Vec::new();
            for path in files(root, &format!("planets/{body}")) {
                let file = path.rsplit('/').next().unwrap_or("");
                let essential = file.ends_with(".json")
                    || file == "albedo.jpg"
                    || file == "normal.png"
                    || file == "biome.png"
                    || file == "clouds.png"
                    || file.starts_with("atmo_");
                add(root, &mut entries, &path, false, essential);
            }
            for suffix in ["json", "bin"] {
                add(
                    root,
                    &mut entries,
                    &format!("terrain/{body}.{suffix}"),
                    false,
                    suffix == "json",
                );
            }
            entries.sort_by_key(|e| {
                (
                    !e.preload,
                    !e.path.ends_with(".json"),
                    !e.path.ends_with("planet.json"),
                    e.path.clone(),
                )
            });
            groups.insert(format!("planet-{body}"), entries);
        }
    }
    // A shared texture never creates a dependency on the other scene's large model.
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (group, entries) in &mut groups {
        let mut seen = BTreeSet::new();
        entries.retain(|entry| seen.insert(entry.url()));
        for e in entries.iter().filter(|e| e.preview) {
            owners.entry(e.url()).or_default().insert(group.clone());
        }
    }
    let mut shared = BTreeMap::new();
    let mut shared_users = BTreeSet::new();
    for (group, entries) in &mut groups {
        entries.retain(|entry| {
            if entry.preview && owners.get(&entry.url()).is_some_and(|o| o.len() > 1) {
                shared.insert(entry.url(), entry.clone());
                shared_users.insert(group.clone());
                false
            } else {
                true
            }
        });
    }
    groups.insert("materials-shared".into(), shared.into_values().collect());
    let mut catalog = Catalog {
        version: 1,
        groups: BTreeMap::new(),
        chunks: BTreeMap::new(),
    };
    for (name, entries) in groups {
        let mut group = Group::default();
        if name != "core" {
            group.depends.push("core".into());
        }
        if shared_users.contains(&name) {
            group.depends.push("materials-shared".into());
        }
        if name.starts_with("planet-") {
            group.depends.push("flight-shared".into());
        }
        let mut batches: Vec<Vec<Entry>> = Vec::new();
        let mut batch: Vec<Entry> = Vec::new();
        let mut size = 0;
        for entry in entries {
            let next = entry.estimate();
            if !batch.is_empty()
                && (batch[0].preload != entry.preload
                    || size + next > TARGET && next <= TARGET
                    || size + next > MAX_FILE)
            {
                batches.push(std::mem::take(&mut batch));
                size = 0;
            }
            size += next;
            batch.push(entry);
            if size >= TARGET {
                batches.push(std::mem::take(&mut batch));
                size = 0;
            }
        }
        if !batch.is_empty() {
            batches.push(batch);
        }
        for (i, entries) in batches.into_iter().enumerate() {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            "ose-pack-v1-preview256".hash(&mut hash);
            for e in &entries {
                (e.url(), e.size, e.modified).hash(&mut hash);
            }
            let revision = format!("{:016x}", hash.finish());
            let id = format!("{name}-{i}");
            group.chunks.push(id.clone());
            if entries[0].preload {
                group.prefetch.push(id.clone());
            }
            catalog.chunks.insert(
                id.clone(),
                Chunk {
                    url: format!("assets/packages/{id}/{revision}.bin"),
                    revision,
                    assets: entries.iter().map(Entry::url).collect(),
                    entries,
                },
            );
        }
        catalog.groups.insert(name, group);
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::to_bytes, http::Request};
    use std::io::Read;
    use tower::ServiceExt;

    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ose-packages-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn put(&self, path: &str, bytes: &[u8]) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        fn scene(&self, name: &str, images: &[&str], size: usize) -> Vec<u8> {
            self.put(
                &format!("{name}/scene.json"),
                format!(r#"{{"file":"{name}/{name}.glb"}}"#).as_bytes(),
            );
            let json = serde_json::to_vec(&serde_json::json!({"images":images.iter().map(|i| serde_json::json!({"uri":format!("../textures/{i}")})).collect::<Vec<_>>()})).unwrap();
            let mut bytes = b"glTF".to_vec();
            bytes.extend(2u32.to_le_bytes());
            bytes.extend(0u32.to_le_bytes());
            bytes.extend((json.len() as u32).to_le_bytes());
            bytes.extend(b"JSON");
            bytes.extend(json);
            bytes.resize(bytes.len().max(size), 7);
            self.put(&format!("{name}/{name}.glb"), &bytes);
            bytes
        }
        fn image(&self, path: &str) {
            let img = image::RgbaImage::from_pixel(512, 256, image::Rgba([70, 120, 30, 255]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(img)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            self.put(path, bytes.get_ref());
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn groups_share_materials_without_loading_another_scene_or_planet() {
        let f = Files::new();
        f.put("ui/ICO-Mission-Control.png", b"icon");
        f.scene("ksc", &["common.png", "ksc.png"], 10 * 1024 * 1024);
        f.scene("vab", &["common.png", "vab.png"], 256);
        for name in ["common", "ksc", "vab"] {
            f.put(&format!("textures/{name}.png"), b"image");
        }
        for body in ["Kerbin", "Mun"] {
            f.put(&format!("planets/{body}/planet.json"), b"{}");
            f.put(&format!("planets/{body}/albedo.jpg"), b"map");
            f.put(&format!("planets/{body}/detail_a0.png"), b"detail");
            f.put(&format!("terrain/{body}.bin"), b"terrain");
            f.put(&format!("terrain/{body}.json"), b"{}");
        }
        let c = build(&f.0).unwrap();
        assert!(c.groups["vab"].depends.contains(&"materials-shared".into()));
        assert!(!c.groups["vab"].depends.contains(&"ksc".into()));
        assert!(c.groups["mission-control"].depends.contains(&"core".into()));
        let common: Vec<_> = c
            .chunks
            .values()
            .flat_map(|c| &c.assets)
            .filter(|u| u.contains("common.png"))
            .collect();
        assert_eq!(common.len(), 1);
        let ksc: Vec<_> = c.groups["ksc"]
            .chunks
            .iter()
            .flat_map(|id| &c.chunks[id].assets)
            .collect();
        assert!(!ksc.iter().any(|u| u.contains("vab") || u.contains("Mun")));
        assert!(ksc.contains(&&"assets/ksc/ksc.glb".to_string()));
        let mun = &c.groups["planet-Mun"];
        let warm: Vec<_> = mun
            .prefetch
            .iter()
            .flat_map(|id| &c.chunks[id].assets)
            .collect();
        assert!(warm.iter().any(|u| u.ends_with("planet.json")));
        assert!(warm.iter().any(|u| u.ends_with("terrain/Mun.json")));
        assert!(!warm
            .iter()
            .any(|u| u.contains("detail_a0") || u.ends_with(".bin") || u.contains("Kerbin")));
        assert!(mun.chunks.len() > mun.prefetch.len());
    }

    #[tokio::test]
    async fn compressed_bundles_round_trip_models_and_previews_and_reject_stale_revisions() {
        let f = Files::new();
        let original = f.scene("ksc", &["test.png"], 4 * 1024 * 1024);
        f.image("textures/test.png");
        let app = crate::web::assets(f.0.clone()).layer(crate::web::compression());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/packages/index.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        let index: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let chunk = &index["chunks"]["ksc-0"];
        let url = chunk["url"].as_str().unwrap().trim_start_matches("assets");
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(url)
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let compressed = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let mut body = Vec::new();
        flate2::read::GzDecoder::new(&compressed[..])
            .read_to_end(&mut body)
            .unwrap();
        assert!(compressed.len() < original.len() / 4);
        assert_eq!(&body[..8], b"OSEPACK1");
        let cached = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(url)
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cached.headers()["x-ose-bundle-cache"], "hit");
        assert_eq!(
            to_bytes(cached.into_body(), usize::MAX).await.unwrap(),
            compressed
        );
        let identity = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(url)
                    .header(header::ACCEPT_ENCODING, "gzip;q=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(!identity.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(
            to_bytes(identity.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            body
        );
        let mut at = 8;
        let mut files = BTreeMap::new();
        loop {
            let n = u32::from_le_bytes(body[at..at + 4].try_into().unwrap()) as usize;
            at += 4;
            if n == 0 {
                break;
            }
            let meta: serde_json::Value = serde_json::from_slice(&body[at..at + n]).unwrap();
            at += n;
            let size = meta["size"].as_u64().unwrap() as usize;
            files.insert(
                meta["url"].as_str().unwrap().to_owned(),
                (meta, body[at..at + size].to_vec()),
            );
            at += size;
        }
        assert_eq!(files["assets/ksc/ksc.glb"].1, original);
        let (meta, image) = &files["assets/textures/test.png?preview=1"];
        assert_eq!(meta["preview"], true);
        assert_eq!(meta["type"], "image/webp");
        assert_eq!(image::load_from_memory(image).unwrap().width(), 256);
        let bad = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/packages/ksc-0/old.bin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::GONE);
        let missing = app
            .oneshot(
                Request::builder()
                    .uri("/packages/no-such-package/0.bin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn catalog_changes_with_exports_and_leaves_oversized_files_on_the_individual_loader() {
        let f = Files::new();
        f.scene("vab", &[], 256);
        let a = build(&f.0).unwrap();
        f.put("vab/vab.glb", b"new export");
        let b = build(&f.0).unwrap();
        assert_ne!(a.chunks["vab-0"].revision, b.chunks["vab-0"].revision);
        let file = std::fs::File::create(f.0.join("vab/vab.glb")).unwrap();
        file.set_len(MAX_FILE + 1).unwrap();
        let c = build(&f.0).unwrap();
        assert!(!c
            .chunks
            .values()
            .flat_map(|c| &c.assets)
            .any(|u| u.ends_with("vab.glb")));
    }
}
