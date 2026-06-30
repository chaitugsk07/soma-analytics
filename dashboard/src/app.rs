//! App shell: router, sidebar, header with token + api-base inputs.

use crate::pages::{EditorPage, ExplorePage, ModelViewPage};
use leptos::prelude::*;
use leptos_router::{
    components::{FlatRoutes, Route, Router},
    hooks::use_location,
    path,
};
use soma_ui::{Input, Sidebar, SidebarItem, ThemeToggle, STYLES};

fn local_storage_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok()?)
        .and_then(|s| s.get_item(key).ok()?)
}

fn local_storage_set(key: &str, value: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok()?) {
        let _ = storage.set_item(key, value);
    }
}

// ── Shared context ─────────────────────────────────────────────────────────────

/// Signals threaded via context so pages can read token + api_base without prop drilling.
#[derive(Clone, Copy)]
pub struct AppCtx {
    pub token: RwSignal<String>,
    pub api_base: RwSignal<String>,
}

fn sidebar_items() -> Vec<SidebarItem> {
    vec![
        SidebarItem {
            label: "Model".to_string(),
            href: "/".to_string(),
            icon: Some(soma_ui::icons::icondata::LuDatabase),
        },
        SidebarItem {
            label: "Explore".to_string(),
            href: "/explore".to_string(),
            icon: Some(soma_ui::icons::icondata::LuSearch),
        },
        SidebarItem {
            label: "Editor".to_string(),
            href: "/edit".to_string(),
            icon: Some(soma_ui::icons::icondata::LuPencil),
        },
    ]
}

#[component]
fn AppShell(children: Children) -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx must be provided");
    let location = use_location();
    let active_path = Signal::derive(move || location.pathname.get());

    let brand = view! {
        <span class="font-heading font-bold text-lg text-foreground tracking-tight">
            "soma-analytics"
        </span>
    }
    .into_any();

    view! {
        <div class="flex h-screen bg-background overflow-hidden">
            <Sidebar
                items=sidebar_items()
                active_path=active_path
                brand=brand
            />
            <div class="flex flex-col flex-1 overflow-hidden">
                // Top bar
                <header class="flex items-center justify-between px-4 h-auto min-h-14 py-2 border-b border-border bg-card shrink-0 gap-4 flex-wrap">
                    <div class="flex items-center gap-2">
                        <span class="font-heading font-semibold text-foreground text-sm">"soma-analytics"</span>
                    </div>
                    <div class="flex items-center gap-2 flex-wrap">
                        <div class="w-56">
                            <Input
                                value=ctx.api_base
                                placeholder="API base URL".to_string()
                                on:change=move |e| {
                                    let v = event_target_value(&e);
                                    local_storage_set("soma_analytics_api", &v);
                                    ctx.api_base.set(v);
                                }
                            />
                        </div>
                        <div class="w-56">
                            <Input
                                input_type="password".to_string()
                                value=ctx.token
                                placeholder="admin token".to_string()
                                on:change=move |e| {
                                    let v = event_target_value(&e);
                                    local_storage_set("soma_analytics_token", &v);
                                    ctx.token.set(v);
                                }
                            />
                        </div>
                        <ThemeToggle />
                    </div>
                </header>
                // Page content
                <main class="flex-1 overflow-auto p-6">
                    {children()}
                </main>
            </div>
        </div>
    }
}

#[component]
pub fn App() -> impl IntoView {
    let token = RwSignal::new(local_storage_get("soma_analytics_token").unwrap_or_default());
    let api_base = RwSignal::new(
        local_storage_get("soma_analytics_api")
            .unwrap_or_else(|| "http://localhost:8090".to_string()),
    );
    provide_context(AppCtx { token, api_base });

    view! {
        <style>{STYLES}</style>
        <Router>
            <AppShell>
                <FlatRoutes fallback=|| view! { <div class="text-muted-foreground">"Page not found"</div> }>
                    <Route path=path!("/") view=ModelViewPage />
                    <Route path=path!("/explore") view=ExplorePage />
                    <Route path=path!("/edit") view=EditorPage />
                </FlatRoutes>
            </AppShell>
        </Router>
    }
}
