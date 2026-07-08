use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api;
use crate::models::{HostInfo, HostUiEvent};

const MAX_LOG_LINES: usize = 200;

/// A pending connection request awaiting the user's decision.
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    peer_addr: String,
    fingerprint: Option<String>,
}

#[component]
pub fn HostPanel() -> impl IntoView {
    let info = RwSignal::new(Option::<HostInfo>::None);
    let code = RwSignal::new(Option::<String>::None);
    let pending = RwSignal::new(Option::<Pending>::None);
    let session_peer = RwSignal::new(Option::<String>::None);
    let log = RwSignal::new(Vec::<String>::new());
    let error = RwSignal::new(Option::<String>::None);
    let copied = RwSignal::new(false);

    let push_log = move |line: String| {
        log.update(|log| {
            log.insert(0, line);
            log.truncate(MAX_LOG_LINES);
        });
    };

    // The event closure must outlive the host session; tying it to the
    // component keeps events flowing until the panel unmounts.
    let event_closure = StoredValue::new_local(None);

    Effect::new(move |_| {
        spawn_local(async move {
            let result = api::start_host(move |event| match event {
                HostUiEvent::CodeReady { code: c } => {
                    code.set(Some(c));
                    session_peer.set(None);
                    pending.set(None);
                }
                HostUiEvent::ApprovalRequested { peer_addr, fingerprint } => {
                    pending.set(Some(Pending { peer_addr, fingerprint }));
                }
                HostUiEvent::SessionStarted { peer_addr } => {
                    pending.set(None);
                    session_peer.set(Some(peer_addr));
                }
                HostUiEvent::SessionEnded { reason } => {
                    session_peer.set(None);
                    push_log(format!("Sessão encerrada: {reason}"));
                }
                HostUiEvent::Log { line } => push_log(line),
                HostUiEvent::Error { message } => push_log(format!("Erro: {message}")),
                HostUiEvent::Stopped => {}
            })
            .await;
            match result {
                Ok((host_info, closure)) => {
                    event_closure.set_value(Some(closure));
                    info.set(Some(host_info));
                }
                Err(e) => error.set(Some(format!("Falha ao iniciar o host: {e}"))),
            }
        });
    });

    on_cleanup(move || {
        spawn_local(async move {
            let _ = api::stop_host().await;
        });
    });

    let copy_code = move |_| {
        if let Some(code) = code.get() {
            if let Some(window) = web_sys::window() {
                let _ = window.navigator().clipboard().write_text(&code);
                copied.set(true);
            }
        }
    };

    view! {
        <div class="panel host-panel">
            <Show when=move || session_peer.get().is_some()>
                <div class="session-banner">
                    <span class="session-dot"></span>
                    <span class="session-text">
                        {move || format!("Sessão remota ativa — {}", session_peer.get().unwrap_or_default())}
                    </span>
                    <button
                        class="btn btn-danger"
                        on:click=move |_| spawn_local(async { let _ = api::host_command("stop").await; })
                    >
                        "Encerrar sessão"
                    </button>
                </div>
            </Show>

            <Show when=move || error.get().is_some()>
                <div class="alert alert-error">{move || error.get().unwrap_or_default()}</div>
            </Show>

            <section class="card code-card">
                <h2>"Código de acesso"</h2>
                <p class="hint">"Dite este código para quem vai controlar este computador. Ele já inclui o endereço desta máquina."</p>
                <div class="access-code">
                    {move || match code.get() {
                        Some(code) => view! {
                            <span class="code-text">{code}</span>
                            <button class="btn btn-ghost btn-copy" on:click=copy_code>
                                {move || if copied.get() { "Copiado ✓" } else { "Copiar" }}
                            </button>
                        }.into_any(),
                        None => view! { <span class="code-waiting">"gerando…"</span> }.into_any(),
                    }}
                </div>
                <p class="hint waiting-hint">
                    {move || if session_peer.get().is_some() {
                        "Conectado.".to_string()
                    } else if code.get().is_some() {
                        "Aguardando conexão…".to_string()
                    } else {
                        String::new()
                    }}
                </p>
            </section>

            <details class="advanced tech-details">
                <summary>"Detalhes técnicos"</summary>
                <div class="host-facts">
                    {move || info.get().map(|i| view! {
                        <div class="fact"><span class="fact-label">"Porta"</span><span class="fact-value">{i.port}</span></div>
                        <div class="fact"><span class="fact-label">"Backend"</span><span class="fact-value">{i.backend_label}</span></div>
                        <div class="fact fact-wide">
                            <span class="fact-label">"Impressão digital do certificado (conferência opcional por voz)"</span>
                            <span class="fact-value mono small">{i.fingerprint}</span>
                        </div>
                    })}
                </div>
                <div class="log-block">
                    <span class="fact-label">"Registro"</span>
                    <div class="log-lines">
                        <For each=move || log.get().into_iter().enumerate() key=|(i, _)| *i let:entry>
                            <div class="log-line">{entry.1}</div>
                        </For>
                    </div>
                </div>
            </details>

            <Show when=move || pending.get().is_some()>
                <div class="modal-backdrop">
                    <div class="modal">
                        <h3>"Solicitação de conexão"</h3>
                        <p>
                            {move || format!("{} quer ver e controlar esta tela.", pending.get().map(|p| p.peer_addr).unwrap_or_default())}
                        </p>
                        {move || pending.get().and_then(|p| p.fingerprint).map(|fp| view! {
                            <p class="mono small dim">{fp}</p>
                        })}
                        <div class="modal-actions">
                            <button
                                class="btn btn-primary"
                                on:click=move |_| spawn_local(async { let _ = api::host_command("approve").await; })
                            >
                                "Aceitar"
                            </button>
                            <button
                                class="btn btn-ghost"
                                on:click=move |_| spawn_local(async { let _ = api::host_command("reject").await; })
                            >
                                "Recusar"
                            </button>
                        </div>
                    </div>
                </div>
            </Show>

        </div>
    }
}
