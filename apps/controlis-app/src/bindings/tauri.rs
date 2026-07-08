//! Minimal typed bridge to `window.__TAURI__` (exposed via `withGlobalTauri`).

use serde::de::DeserializeOwned;
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// `window.__TAURI__.core.invoke`.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = invoke, catch)]
    pub async fn invoke_raw(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;

    /// `window.__TAURI__.core.Channel` — ordered backend→frontend streaming.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"], js_name = Channel)]
    pub type TauriChannel;

    #[wasm_bindgen(constructor, js_namespace = ["window", "__TAURI__", "core"], js_class = Channel)]
    pub fn new() -> TauriChannel;

    #[wasm_bindgen(method, setter, js_name = onmessage)]
    pub fn set_onmessage(this: &TauriChannel, cb: &js_sys::Function);
}

fn error_text(err: JsValue) -> String {
    err.as_string()
        .or_else(|| js_sys::JSON::stringify(&err).ok().map(String::from))
        .unwrap_or_else(|| "erro desconhecido".into())
}

/// Invokes a Tauri command with typed arguments and response decoding.
pub async fn invoke<T, A>(cmd: &str, args: &A) -> Result<T, String>
where
    T: DeserializeOwned,
    A: Serialize,
{
    let args = serde_wasm_bindgen::to_value(args).map_err(|e| e.to_string())?;
    match invoke_raw(cmd, args).await {
        Ok(value) => serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string()),
        Err(err) => Err(error_text(err)),
    }
}

/// Invokes a command whose args include non-serde values (e.g. channels),
/// passed as a prebuilt JS object.
pub async fn invoke_js<T: DeserializeOwned>(cmd: &str, args: JsValue) -> Result<T, String> {
    match invoke_raw(cmd, args).await {
        Ok(value) => serde_wasm_bindgen::from_value(value).map_err(|e| e.to_string()),
        Err(err) => Err(error_text(err)),
    }
}

/// Creates a Tauri channel whose messages are decoded into `T` and handed to
/// `on_message`. Returns the channel (to put into invoke args) and the closure
/// that must be kept alive for the channel's lifetime.
pub fn typed_channel<T: DeserializeOwned + 'static>(
    mut on_message: impl FnMut(T) + 'static,
) -> (TauriChannel, Closure<dyn FnMut(JsValue)>) {
    let channel = TauriChannel::new();
    let closure = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
        match serde_wasm_bindgen::from_value::<T>(value) {
            Ok(message) => on_message(message),
            Err(e) => web_sys::console::warn_1(&format!("evento inválido: {e}").into()),
        }
    });
    channel.set_onmessage(closure.as_ref().unchecked_ref());
    (channel, closure)
}
