//! Warm compressed scene data while the player uses the main menu. No geometry parsing or GPU work.
use js_sys::Uint8Array;
use std::{cell::RefCell, collections::HashSet};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

#[derive(Default)]
struct State {
    busy: bool,
    done: bool,
}
thread_local! { static STATE: RefCell<State> = RefCell::default(); }

pub fn tick(menu: bool, canvas: &web_sys::HtmlCanvasElement) {
    let (start, done) = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let start = menu && !state.busy && !state.done;
        if start {
            state.busy = true;
        }
        (start, state.done)
    });
    crate::asset_queue::warming(menu && !done);
    let status = if done {
        "ready"
    } else if menu {
        "loading"
    } else {
        "paused"
    };
    if canvas.get_attribute("data-ose-preload").as_deref() != Some(status) {
        canvas.set_attribute("data-ose-preload", status).ok();
    }
    if start {
        wasm_bindgen_futures::spawn_local(async {
            let done = warm().await;
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.busy = false;
                state.done = done;
            });
        });
    }
}

async fn warm() -> bool {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    // Scene models first: their compressed bodies are considerably smaller than the source GLBs.
    for scene in ["ksc", "vab"] {
        let _permit = crate::asset_queue::background(false).await;
        if !crate::asset_queue::is_warming() {
            return false;
        }
        let Some(url) = scene_url(scene).await else {
            continue;
        };
        let _permit = crate::asset_queue::background(false).await;
        if !crate::asset_queue::is_warming() {
            return false;
        }
        if let Ok(buffer) = crate::asset_cache::prefetch(&url).await {
            for image in material_images(&buffer) {
                if seen.insert(image.clone()) {
                    images.push(image);
                }
            }
        }
    }
    for batch in images.chunks(4) {
        let results = futures_util::future::join_all(batch.iter().map(|image| async move {
            let _permit = crate::asset_queue::background(true).await;
            if !crate::asset_queue::is_warming() {
                return false;
            }
            crate::asset_cache::prefetch_preview(&format!("{image}?preview=1")).await == Ok(true)
        }))
        .await;
        if !crate::asset_queue::is_warming() {
            return false;
        }
        // Old servers may return full originals for preview URLs. Stop after this bounded batch.
        if results.contains(&false) {
            break;
        }
    }
    true
}

async fn scene_url(scene: &str) -> Option<String> {
    let response =
        JsFuture::from(web_sys::window()?.fetch_with_str(&format!("assets/{scene}/scene.json")))
            .await
            .ok()?;
    let response: web_sys::Response = response.dyn_into().ok()?;
    if !response.ok() {
        return None;
    }
    let text = JsFuture::from(response.text().ok()?)
        .await
        .ok()?
        .as_string()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(format!("assets/{}", json.get("file")?.as_str()?))
}

fn material_images(buffer: &wasm_bindgen::JsValue) -> Vec<String> {
    let bytes = Uint8Array::new(buffer);
    if bytes.length() < 20 {
        return Vec::new();
    }
    let header = bytes.subarray(0, 20).to_vec();
    if &header[..4] != b"glTF" || &header[16..20] != b"JSON" {
        return Vec::new();
    }
    let len = u32::from_le_bytes(header[12..16].try_into().unwrap());
    if len > 4 * 1024 * 1024 || len > bytes.length() - 20 {
        return Vec::new();
    }
    let json: serde_json::Value =
        match serde_json::from_slice(&bytes.subarray(20, 20 + len).to_vec()) {
            Ok(json) => json,
            Err(_) => return Vec::new(),
        };
    json.get("images")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|image| image.get("uri")?.as_str())
        .map(|uri| crate::render::resolve_uri("assets/parts/", uri))
        .filter(|url| url.starts_with("assets/textures/") && url.ends_with(".png"))
        .collect()
}
