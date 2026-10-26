//! Transfer large binary assets compressed, including installations without prebuilt gzip copies.
use axum::{
    http::{header, HeaderValue, Uri},
    Router,
};
use std::path::PathBuf;
use tower_http::{
    compression::{CompressionLayer, CompressionLevel},
    services::{ServeDir, ServeFile},
};

pub fn compression() -> CompressionLayer {
    // Precompressed responses pass through unchanged. Fast compression is the fallback for assets
    // exported before gzip sidecars were introduced, keeping CPU costs low on small remote hosts.
    CompressionLayer::new().quality(CompressionLevel::Fastest)
}

fn cache(response: &mut axum::response::Response, value: &'static str) {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    // Precompressed responses bypass CompressionLayer's Vary insertion.
    if !response
        .headers()
        .get_all(header::VARY)
        .iter()
        .any(|value| {
            value
                .to_str()
                .unwrap_or("")
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("accept-encoding"))
        })
    {
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
}

pub fn assets(dir: PathBuf) -> Router {
    let previews = crate::previews::Previews::new(dir.clone());
    Router::new()
        .fallback_service(ServeDir::new(dir).precompressed_gzip())
        .layer(axum::middleware::from_fn_with_state(
            previews,
            crate::previews::preview,
        ))
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                if response.status().is_success() {
                    cache(&mut response, "public, max-age=86400");
                }
                response
            },
        ))
}

fn fingerprinted(path: &str) -> bool {
    let Some((_, suffix)) = path.rsplit_once('-') else {
        return false;
    };
    let Some(hash) = suffix.get(..16) else {
        return false;
    };
    hash.bytes().all(|b| b.is_ascii_hexdigit())
        && matches!(suffix.get(16..), Some(".js" | "_bg.wasm" | ".wasm"))
}

pub fn client(dir: PathBuf) -> Router {
    let index = ServeFile::new(dir.join("index.html")).precompressed_gzip();
    Router::new()
        .fallback_service(ServeDir::new(dir).precompressed_gzip().fallback(index))
        .layer(axum::middleware::map_response(
            |uri: Uri, mut response: axum::response::Response| async move {
                if response.status().is_success() {
                    let content = response
                        .headers()
                        .get(header::CONTENT_TYPE)
                        .and_then(|h| h.to_str().ok())
                        .unwrap_or("");
                    let policy = if fingerprinted(uri.path())
                        && matches!(
                            content,
                            "application/wasm" | "text/javascript" | "application/javascript"
                        ) {
                        "public, max-age=31536000, immutable"
                    } else {
                        // Always revalidate the entry point so a new deployment uses the matching WASM.
                        "no-cache"
                    };
                    cache(&mut response, policy);
                }
                response
            },
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use std::io::{Read, Write};
    use tower::ServiceExt;

    struct Files(PathBuf);
    static NEXT_FILES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ose-web-{}-{}-{}",
                std::process::id(),
                NEXT_FILES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn write(&self, name: &str, data: &[u8]) {
            std::fs::write(self.0.join(name), data).unwrap();
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn gzip(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }
    fn decode(data: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        flate2::read::GzDecoder::new(data)
            .read_to_end(&mut output)
            .unwrap();
        output
    }
    async fn get(
        app: Router,
        path: &str,
        range: Option<&str>,
        encoding: &str,
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .uri(path)
            .header(header::ACCEPT_ENCODING, encoding);
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn old_assets_are_compressed_and_existing_gzip_is_not_encoded_twice() {
        let files = Files::new();
        let raw = b"glTF model vertices and indices ".repeat(4096);
        files.write("scene.glb", &raw);
        let app = assets(files.0.clone()).layer(compression());
        let response = get(app.clone(), "/scene.glb", None, "gzip").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=86400"
        );
        let compressed = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(compressed.len() < raw.len() / 4);
        assert_eq!(decode(&compressed), raw);

        let prepared = gzip(&raw);
        files.write("scene.glb.gz", &prepared);
        let response = get(app, "/scene.glb", None, "gzip").await;
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(response.headers()[header::VARY], "accept-encoding");
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            prepared
        );
    }

    #[tokio::test]
    async fn identity_and_range_requests_keep_the_original_bytes() {
        let files = Files::new();
        let raw = b"glTF model vertices and indices ".repeat(4096);
        files.write("scene.glb", &raw);
        files.write("scene.glb.gz", &gzip(&raw));
        let app = assets(files.0.clone()).layer(compression());
        let response = get(app.clone(), "/scene.glb", None, "identity").await;
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            raw
        );
        let response = get(app, "/scene.glb", Some("bytes=0-3"), "identity").await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert!(!response.headers().contains_key(header::CONTENT_ENCODING));
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            b"glTF"
        );
    }

    #[tokio::test]
    async fn client_revalidates_html_and_keeps_fingerprinted_wasm() {
        let files = Files::new();
        files.write("index.html", &b"<html>client</html>".repeat(100));
        files.write("ose-client-0123456789abcdef_bg.wasm", &vec![0; 4096]);
        let app = client(files.0.clone()).layer(compression());
        let html = get(app.clone(), "/", None, "gzip").await;
        assert_eq!(html.headers()[header::CACHE_CONTROL], "no-cache");
        let wasm = get(app, "/ose-client-0123456789abcdef_bg.wasm", None, "gzip").await;
        assert_eq!(
            wasm.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(wasm.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            decode(&to_bytes(wasm.into_body(), usize::MAX).await.unwrap()),
            vec![0; 4096]
        );
    }

    #[tokio::test]
    async fn previews_load_small_then_originals_and_revalidation_still_work() {
        let files = Files::new();
        std::fs::create_dir(files.0.join("textures")).unwrap();
        let path = files.0.join("textures/material.png");
        image::RgbaImage::from_pixel(1024, 512, image::Rgba([80, 120, 200, 128]))
            .save(&path)
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        let app = assets(files.0.clone()).layer(compression());
        let preview = get(
            app.clone(),
            "/textures/material.png?preview=1",
            None,
            "gzip",
        )
        .await;
        assert_eq!(preview.headers()[header::CONTENT_TYPE], "image/webp");
        assert_eq!(preview.headers()["x-ose-preview"], "1");
        let modified = preview.headers()[header::LAST_MODIFIED].clone();
        let data = to_bytes(preview.into_body(), usize::MAX).await.unwrap();
        let small = image::load_from_memory(&data).unwrap();
        assert_eq!((small.width(), small.height()), (256, 128));
        let again = get(
            app.clone(),
            "/textures/material.png?preview=1",
            None,
            "gzip",
        )
        .await;
        assert_eq!(to_bytes(again.into_body(), usize::MAX).await.unwrap(), data);
        let unchanged = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/textures/material.png?preview=1")
                    .header(header::IF_MODIFIED_SINCE, modified)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
        let full = get(app, "/textures/material.png", None, "gzip").await;
        assert!(!full.headers().contains_key("x-ose-preview"));
        assert_eq!(
            to_bytes(full.into_body(), usize::MAX)
                .await
                .unwrap()
                .as_ref(),
            original
        );
    }
}
