//! Bridge to `public/js/screen.js`, which owns the frame loop and raw input
//! capture on the remote-screen canvas.

use wasm_bindgen::prelude::*;

// wasm-bindgen packages this JS module as a snippet; its absolute imports
// resolve against the copied public assets at runtime.
#[wasm_bindgen(module = "/public/js/screen.js")]
extern "C" {
    /// Connects the viewer session and wires the canvas. Resolves once the
    /// host accepts; the returned handle controls the session.
    #[wasm_bindgen(js_name = connectScreen, catch)]
    pub async fn connect_screen(
        canvas: &web_sys::HtmlCanvasElement,
        code: &str,
        addr_override: Option<String>,
        on_event: &JsValue,
    ) -> Result<JsValue, JsValue>;

    /// Handle returned by `connectScreen`.
    pub type ScreenHandle;

    #[wasm_bindgen(method, getter)]
    pub fn info(this: &ScreenHandle) -> JsValue;

    #[wasm_bindgen(method, js_name = selectMonitor)]
    pub fn select_monitor(this: &ScreenHandle, id: u32);

    #[wasm_bindgen(method)]
    pub fn disconnect(this: &ScreenHandle);

    #[wasm_bindgen(method)]
    pub fn dispose(this: &ScreenHandle);
}
