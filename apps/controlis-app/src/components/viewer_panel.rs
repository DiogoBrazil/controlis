use leptos::html::Canvas;
use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::bindings::screen::{connect_screen, ScreenHandle};
use crate::models::{MonitorUi, ViewerInfo, ViewerUiEvent};

/// Viewer connection lifecycle.
#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    Connecting,
    Active,
    Failed(String),
}

#[component]
pub fn ViewerPanel() -> impl IntoView {
    let phase = RwSignal::new(Phase::Idle);
    let code_text = RwSignal::new(String::new());
    let addr_override = RwSignal::new(String::new());
    let show_advanced = RwSignal::new(false);
    let status = RwSignal::new(String::new());
    let fingerprint = RwSignal::new(Option::<String>::None);
    let monitors = RwSignal::new(Vec::<MonitorUi>::new());
    let selected_monitor = RwSignal::new(0u32);

    let handle = StoredValue::new_local(Option::<ScreenHandle>::None);
    let event_closure = StoredValue::new_local(Option::<Closure<dyn FnMut(JsValue)>>::None);
    let canvas_ref: NodeRef<Canvas> = NodeRef::new();

    let disconnect = move || {
        if let Some(h) = handle.try_with_value(|h| h.as_ref().map(|h| h.disconnect())) {
            let _ = h;
        }
        handle.set_value(None);
    };

    on_cleanup(disconnect);

    let connect = move |_| {
        let code = code_text.get().trim().to_string();
        if code.is_empty() {
            phase.set(Phase::Failed("Digite o código de acesso.".into()));
            return;
        }
        let manual = addr_override.get().trim().to_string();
        let manual = (!manual.is_empty()).then_some(manual);
        phase.set(Phase::Connecting);

        spawn_local(async move {
            let Some(canvas) = canvas_ref.get_untracked() else {
                phase.set(Phase::Failed("canvas indisponível".into()));
                return;
            };
            let closure = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
                let Ok(event) = serde_wasm_bindgen::from_value::<ViewerUiEvent>(value) else {
                    return;
                };
                match event {
                    ViewerUiEvent::Resized { width_px, height_px } => {
                        status.set(format!("{width_px}x{height_px}"));
                    }
                    ViewerUiEvent::MonitorsUpdated { monitors: list } => monitors.set(list),
                    ViewerUiEvent::Disconnected { reason } => {
                        handle.set_value(None);
                        phase.set(Phase::Failed(format!("Desconectado: {reason}")));
                    }
                    ViewerUiEvent::Error { message } => status.set(format!("Erro: {message}")),
                }
            });

            match connect_screen(&canvas, &code, manual, closure.as_ref().unchecked_ref()).await {
                Ok(h) => {
                    let h: ScreenHandle = h.unchecked_into();
                    if let Ok(info) = serde_wasm_bindgen::from_value::<ViewerInfo>(h.info()) {
                        let primary = info
                            .monitors
                            .iter()
                            .find(|m| m.is_primary)
                            .or_else(|| info.monitors.first());
                        if let Some(primary) = primary {
                            status.set(format!("{}x{}", primary.width_px, primary.height_px));
                            selected_monitor.set(primary.id);
                        }
                        monitors.set(info.monitors);
                        fingerprint.set(info.fingerprint);
                    }
                    handle.set_value(Some(h));
                    event_closure.set_value(Some(closure));
                    phase.set(Phase::Active);
                }
                Err(e) => {
                    let message = e
                        .as_string()
                        .or_else(|| js_sys::JSON::stringify(&e).ok().map(String::from))
                        .unwrap_or_else(|| "erro desconhecido".into());
                    phase.set(Phase::Failed(message));
                }
            }
        });
    };

    let is_active = move || phase.get() == Phase::Active;

    view! {
        <div class="panel viewer-panel" class:viewer-active=is_active>
            <Show when=move || !is_active()>
                <div class="connect-box">
                    <section class="card connect-card">
                        <h2>"Conectar a um computador"</h2>
                        <p class="hint">"Digite o código de acesso exibido no computador host."</p>
                        <input
                            class="code-input"
                            type="text"
                            placeholder="XXXX-XXXX-XXXX-XXXX"
                            prop:value=move || code_text.get()
                            on:input=move |ev| code_text.set(event_target_value(&ev))
                            on:keydown=move |ev| if ev.key() == "Enter" { connect(()) }
                        />
                        <details class="advanced" prop:open=move || show_advanced.get()>
                            <summary on:click=move |_| show_advanced.update(|v| *v = !*v)>
                                "Avançado"
                            </summary>
                            <label class="field">
                                <span>"Endereço manual (IP:porta)"</span>
                                <input
                                    type="text"
                                    placeholder="vazio = usar o do código"
                                    prop:value=move || addr_override.get()
                                    on:input=move |ev| addr_override.set(event_target_value(&ev))
                                />
                            </label>
                            <p class="hint small">
                                "Use apenas se o host tiver várias interfaces e o código apontar para a errada."
                            </p>
                        </details>
                        {move || match phase.get() {
                            Phase::Failed(reason) => view! {
                                <div class="alert alert-error">{reason}</div>
                            }.into_any(),
                            _ => view! { <div></div> }.into_any(),
                        }}
                        <button
                            class="btn btn-primary btn-connect"
                            disabled=move || phase.get() == Phase::Connecting
                            on:click=move |_| connect(())
                        >
                            {move || if phase.get() == Phase::Connecting { "Conectando…" } else { "Conectar" }}
                        </button>
                    </section>
                </div>
            </Show>

            <div class="viewer-stage" class:hidden=move || !is_active()>
                <div class="viewer-toolbar">
                    <span class="status">{move || status.get()}</span>
                    {move || fingerprint.get().map(|fp| view! {
                        <span class="status mono small" title="Impressão digital do host (TOFU)">{fp}</span>
                    })}
                    <Show when=move || { monitors.get().len() > 1 }>
                        <div class="monitor-picker">
                            <For each=move || monitors.get() key=|m| m.id let:monitor>
                                {
                                    let id = monitor.id;
                                    let label = format!("{} ({}x{})", monitor.name, monitor.width_px, monitor.height_px);
                                    view! {
                                        <button
                                            class="btn btn-ghost btn-monitor"
                                            class:selected=move || selected_monitor.get() == id
                                            on:click=move |_| {
                                                selected_monitor.set(id);
                                                handle.with_value(|h| {
                                                    if let Some(h) = h.as_ref() {
                                                        h.select_monitor(id);
                                                    }
                                                });
                                            }
                                        >
                                            {label}
                                        </button>
                                    }
                                }
                            </For>
                        </div>
                    </Show>
                    <button
                        class="btn btn-danger"
                        on:click=move |_| {
                            disconnect();
                            phase.set(Phase::Failed("Você encerrou a sessão".into()));
                        }
                    >
                        "Desconectar"
                    </button>
                </div>
                <div class="canvas-wrap">
                    <canvas node_ref=canvas_ref class="remote-screen" tabindex="0"></canvas>
                </div>
            </div>
        </div>
    }
}
