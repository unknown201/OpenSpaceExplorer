//! Browser client. Phase 0: proves the WASM toolchain by ticking the shared sim in the page.

#![forbid(unsafe_code)]

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Client {
    universe: ose_sim::Universe,
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Client {
        console_error_panic_hook::set_once();
        Client {
            universe: ose_sim::Universe::new(),
        }
    }

    pub fn step(&mut self) -> f64 {
        self.universe.step();
        self.universe.tick().seconds()
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
