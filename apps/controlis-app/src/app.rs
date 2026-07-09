use leptos::prelude::*;

use crate::components::home::Home;
use crate::components::host_panel::HostPanel;
use crate::components::viewer_panel::ViewerPanel;

/// Which screen the app is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    Host,
    Viewer,
}

#[component]
pub fn App() -> impl IntoView {
    let screen = RwSignal::new(Screen::Home);

    view! {
        <div class="app-shell">
            <header class="app-header">
                <div class="brand" on:click=move |_| screen.set(Screen::Home)>
                    <span class="brand-mark">"⌖"</span>
                    <span class="brand-name">"Controlis"</span>
                </div>
                <div class="header-mode">
                    {move || match screen.get() {
                        Screen::Home => "".to_string(),
                        Screen::Host => "Modo Host".to_string(),
                        Screen::Viewer => "Modo Viewer".to_string(),
                    }}
                </div>
                <div class="header-actions">
                    <Show when=move || screen.get() != Screen::Home>
                        <button class="btn btn-ghost" on:click=move |_| screen.set(Screen::Home)>
                            "← Início"
                        </button>
                    </Show>
                </div>
            </header>
            <main class="app-main">
                {move || match screen.get() {
                    Screen::Home => view! { <Home screen=screen/> }.into_any(),
                    Screen::Host => view! { <HostPanel/> }.into_any(),
                    Screen::Viewer => view! { <ViewerPanel/> }.into_any(),
                }}
            </main>
        </div>
    }
}
