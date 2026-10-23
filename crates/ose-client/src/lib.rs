//! Browser client: rendering, input, UI and snapshot interpolation only (ADR-0002). Game logic is native.

#![forbid(unsafe_code)]

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Client {
    /// Latest server tick we have a snapshot for.
    server_tick: ose_data::Tick,
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Client {
        console_error_panic_hook::set_once();
        Client {
            server_tick: ose_data::Tick::default(),
        }
    }

    /// Record a server time update; returns universal time in seconds.
    pub fn on_time_state(&mut self, tick: u64) -> f64 {
        self.server_tick = ose_data::Tick(tick);
        self.server_tick.seconds()
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id("status") {
            el.set_text_content(Some("OpenSpaceExplorer client loaded"));
        }
    }
}
