//! Large GLBs/terrain can exceed the browser HTTP cache's per-entry limit. Keep a small persistent
//! copy and revalidate it with Last-Modified before reuse. Cache failures never prevent normal loading.
use futures_util::{
    future::{LocalBoxFuture, Shared},
    FutureExt,
};
use js_sys::Uint8Array;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use wasm_bindgen_futures::{future_to_promise, JsFuture};
use web_sys::{Cache, Headers, Request, RequestCache, RequestInit, Response, ResponseInit};

const NAME: &str = "ose-large-assets-v1";
const SIZE: &str = "x-ose-decoded-bytes";
const MIN_ENTRY: usize = 4 * 1024 * 1024;
const MAX_ENTRY: usize = 64 * 1024 * 1024;
const MAX_BYTES: usize = 256 * 1024 * 1024;

async fn open() -> Option<Cache> {
    let storage = web_sys::window()?.caches().ok()?;
    JsFuture::from(storage.open(NAME))
        .await
        .ok()?
        .dyn_into()
        .ok()
}

async fn cached(cache: &Cache, url: &str) -> Option<Response> {
    JsFuture::from(cache.match_with_str(url))
        .await
        .ok()?
        .dyn_into()
        .ok()
}

async fn download(
    url: &str,
    old: Option<&Response>,
    background: bool,
) -> Result<Response, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let options = RequestInit::new();
    let _ = js_sys::Reflect::set(
        &options,
        &"priority".into(),
        &if background { "low" } else { "high" }.into(),
    );
    if let Some(date) = old.and_then(|r| r.headers().get("last-modified").ok().flatten()) {
        let headers = Headers::new()?;
        headers.set("if-modified-since", &date)?;
        options.set_headers_headers(&headers);
        // We handle the 304 against our stored body, including files too large for the HTTP cache.
        options.set_cache(RequestCache::NoStore);
    }
    JsFuture::from(window.fetch_with_str_and_init(url, &options))
        .await?
        .dyn_into()
}

async fn store(
    cache: &Cache,
    url: &str,
    response: &Response,
    buffer: &JsValue,
    size: usize,
) -> Result<(), JsValue> {
    let headers = Headers::new_with_headers(&response.headers())?;
    // Fetch has already decompressed the body; the stored response describes its decoded bytes.
    headers.delete("content-encoding")?;
    headers.delete("content-length")?;
    headers.delete("vary")?;
    headers.set(SIZE, &size.to_string())?;
    let init = ResponseInit::new();
    init.set_headers_headers(&headers);
    let saved = Response::new_with_opt_buffer_source_and_init(Some(buffer.unchecked_ref()), &init)?;

    // Web Locks serialize admission/eviction across tabs. If unavailable, skip persistence and use
    // the normal download, rather than making cache support a requirement for playing.
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let navigator = js_sys::Reflect::get(&window, &"navigator".into())?;
    let locks = js_sys::Reflect::get(&navigator, &"locks".into())?;
    let request: js_sys::Function = js_sys::Reflect::get(&locks, &"request".into())?.dyn_into()?;
    let cache = cache.clone();
    let url = url.to_owned();
    let write = Closure::<dyn FnMut(JsValue) -> js_sys::Promise>::new(move |_| {
        let (cache, url, saved) = (cache.clone(), url.clone(), Clone::clone(&saved));
        future_to_promise(async move {
            store_locked(&cache, &url, &saved, size).await?;
            Ok(JsValue::UNDEFINED)
        })
    });
    let promise = request
        .call2(&locks, &NAME.into(), write.as_ref())?
        .dyn_into::<js_sys::Promise>()?;
    JsFuture::from(promise).await?;
    Ok(())
}

async fn store_locked(
    cache: &Cache,
    url: &str,
    saved: &Response,
    size: usize,
) -> Result<(), JsValue> {
    let absolute = Request::new_with_str(url)?.url();
    let keys = js_sys::Array::from(&JsFuture::from(cache.keys()).await?);
    let mut entries = Vec::new();
    let mut total = size;
    for key in keys.iter() {
        let request: Request = key.dyn_into()?;
        if request.url() == absolute {
            continue;
        }
        if let Some(stored) = cached(cache, &request.url()).await {
            let bytes = stored
                .headers()
                .get(SIZE)?
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(MAX_ENTRY);
            total = total.saturating_add(bytes);
            entries.push((request, bytes));
        }
    }
    for (request, bytes) in entries {
        if total <= MAX_BYTES {
            break;
        }
        JsFuture::from(cache.delete_with_request(&request)).await?;
        total = total.saturating_sub(bytes);
    }
    JsFuture::from(cache.put_with_str(url, saved)).await?;
    Ok(())
}

#[derive(Clone)]
struct Asset {
    buffer: JsValue,
    preview: bool,
}
type Transfer = Shared<LocalBoxFuture<'static, Result<Asset, String>>>;
struct Flight {
    transfer: Transfer,
    priority: Option<Rc<crate::asset_queue::ModelPriority>>,
    consumed: Rc<std::cell::Cell<bool>>,
}
#[derive(Default)]
struct Downloads {
    flights: HashMap<String, Flight>,
    ready: VecDeque<(String, Asset)>,
    bytes: usize,
}
thread_local! { static DOWNLOADS: RefCell<Downloads> = RefCell::default(); }
const MENU_BYTES: usize = 128 * 1024 * 1024;

// Keep prefetched ArrayBuffers outside WASM memory. Geometry is parsed only when its scene opens.
async fn shared(url: &str, background: bool) -> Result<Asset, String> {
    let ready = DOWNLOADS.with(|downloads| {
        let mut d = downloads.borrow_mut();
        let index = d.ready.iter().position(|(key, _)| key == url)?;
        if background {
            return Some(d.ready[index].1.clone());
        }
        let (_, asset) = d.ready.remove(index)?;
        d.bytes -= Uint8Array::new(&asset.buffer).length() as usize;
        Some(asset)
    });
    if let Some(asset) = ready {
        return Ok(asset);
    }
    let (transfer, consumed) = DOWNLOADS.with(|downloads| {
        let mut d = downloads.borrow_mut();
        let flight = d.flights.entry(url.to_owned()).or_insert_with(|| {
            let priority = url
                .ends_with(".glb")
                .then(|| Rc::new(crate::asset_queue::ModelPriority::new(background)));
            let path = url.to_owned();
            Flight {
                transfer: load(path, background).boxed_local().shared(),
                priority,
                consumed: Rc::new(std::cell::Cell::new(false)),
            }
        });
        if !background {
            flight.consumed.set(true);
            if let Some(priority) = &flight.priority {
                priority.promote();
            }
        }
        (flight.transfer.clone(), flight.consumed.clone())
    });
    let result = transfer.await;
    DOWNLOADS.with(|downloads| {
        let mut d = downloads.borrow_mut();
        // Another waiter may already have removed this completed transfer.
        if d.flights
            .get(url)
            .is_some_and(|f| Rc::ptr_eq(&f.consumed, &consumed))
        {
            d.flights.remove(url);
            if !consumed.get() {
                if let Ok(asset) = &result {
                    let size = Uint8Array::new(&asset.buffer).length() as usize;
                    if size <= MENU_BYTES {
                        while d.bytes + size > MENU_BYTES {
                            if let Some((_, old)) = d.ready.pop_front() {
                                d.bytes -= Uint8Array::new(&old.buffer).length() as usize;
                            }
                        }
                        d.bytes += size;
                        d.ready.push_back((url.to_owned(), asset.clone()));
                    }
                }
            }
        }
    });
    result
}

pub async fn prefetch(url: &str) -> Result<JsValue, String> {
    Ok(shared(url, true).await?.buffer)
}
pub async fn prefetch_preview(url: &str) -> Result<bool, String> {
    let asset = shared(url, true).await?;
    // The server returns already-small source images directly, without the preview marker.
    Ok(asset.preview || Uint8Array::new(&asset.buffer).length() <= 256 * 1024)
}
pub async fn bytes(url: &str) -> Result<Vec<u8>, String> {
    Ok(Uint8Array::new(&shared(url, false).await?.buffer).to_vec())
}
pub async fn preview(url: &str) -> Result<Response, String> {
    let asset = shared(url, false).await?;
    let init = ResponseInit::new();
    let headers = Headers::new().map_err(|e| format!("{e:?}"))?;
    if asset.preview {
        headers
            .set("x-ose-preview", "1")
            .map_err(|e| format!("{e:?}"))?;
    }
    init.set_headers_headers(&headers);
    Response::new_with_opt_buffer_source_and_init(Some(asset.buffer.unchecked_ref()), &init)
        .map_err(|e| format!("{e:?}"))
}

async fn load(url: String, background: bool) -> Result<Asset, String> {
    let url = url.as_str();
    let eligible = url.starts_with("assets/") && (url.ends_with(".glb") || url.ends_with(".bin"));
    let cache = if eligible { open().await } else { None };
    let old = match &cache {
        Some(cache) => cached(cache, url).await,
        None => None,
    };
    let response = download(url, old.as_ref(), background)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if response.status() == 304 {
        if let Some(old) = old {
            let buffer = JsFuture::from(old.array_buffer().map_err(|e| format!("{e:?}"))?)
                .await
                .map_err(|e| format!("{e:?}"))?;
            return Ok(Asset {
                buffer,
                preview: false,
            });
        }
    }
    if !response.ok() {
        return Err(format!("HTTP {}", response.status()));
    }
    let buffer = JsFuture::from(response.array_buffer().map_err(|e| format!("{e:?}"))?)
        .await
        .map_err(|e| format!("{e:?}"))?;
    let array = Uint8Array::new(&buffer);
    let size = array.length() as usize;
    if response.headers().has("x-ose-package").unwrap_or(false) {
        // The service worker already persists each versioned package member. Avoid a second large copy.
        if let Some(cache) = &cache {
            let _ = JsFuture::from(cache.delete_with_str(url)).await;
        }
    } else if (MIN_ENTRY..=MAX_ENTRY).contains(&size) {
        if let Some(cache) = cache {
            let _ = store(&cache, url, &response, &buffer, size).await;
        }
    }
    let preview = response
        .headers()
        .get("x-ose-preview")
        .ok()
        .flatten()
        .as_deref()
        == Some("1");
    Ok(Asset { buffer, preview })
}
