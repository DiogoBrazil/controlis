use leptos::prelude::*;

use crate::app::Screen;

#[component]
pub fn Home(screen: RwSignal<Screen>) -> impl IntoView {
    view! {
        <div class="home">
            <div class="home-hero">
                <h1>"Acesso remoto simples e seguro"</h1>
                <p class="home-sub">"Escolha como esta máquina vai participar."</p>
            </div>
            <div class="home-cards">
                <button class="mode-card" on:click=move |_| screen.set(Screen::Host)>
                    <div class="mode-icon">"🖥"</div>
                    <div class="mode-title">"Permitir controle"</div>
                    <div class="mode-desc">
                        "Gere um código de acesso e deixe alguém de confiança ver e controlar esta tela."
                    </div>
                    <div class="mode-tag">"Host"</div>
                </button>
                <button class="mode-card" on:click=move |_| screen.set(Screen::Viewer)>
                    <div class="mode-icon">"👁"</div>
                    <div class="mode-title">"Controlar outro"</div>
                    <div class="mode-desc">
                        "Digite o código exibido no outro computador para ver e controlar a tela dele."
                    </div>
                    <div class="mode-tag">"Viewer"</div>
                </button>
            </div>
        </div>
    }
}
