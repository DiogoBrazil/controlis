//! Typed calls into the Tauri backend commands.

use serde::Serialize;
use wasm_bindgen::prelude::*;

use crate::bindings::tauri::{invoke, invoke_js, typed_channel};
use crate::models::{HostInfo, HostUiEvent};

/// Starts the host. Events flow into `on_event` for as long as the returned
/// closure is kept alive.
pub async fn start_host(
    on_event: impl FnMut(HostUiEvent) + 'static,
) -> Result<(HostInfo, Closure<dyn FnMut(JsValue)>), String> {
    let (channel, closure) = typed_channel(on_event);
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &"onEvent".into(), channel.as_ref())
        .map_err(|_| "montando args".to_string())?;
    let info: HostInfo = invoke_js("start_host", args.into()).await?;
    Ok((info, closure))
}

#[derive(Serialize)]
struct ActionArgs<'a> {
    action: &'a str,
}

pub async fn host_command(action: &str) -> Result<(), String> {
    invoke("host_command", &ActionArgs { action }).await
}

pub async fn stop_host() -> Result<(), String> {
    invoke("stop_host", &serde_json::Map::new()).await
}
