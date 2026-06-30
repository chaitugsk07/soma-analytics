//! Model view — "Power BI Model view"-style data-model browser.
//!
//! Fetches GET /api/v1/model (Editor+ token required) and renders either:
//!   • Diagram view (default) — SVG relationship diagram with cube boxes + join lines
//!   • List view — one Card per cube showing measures, dimensions, joins, segments

mod model_diagram;
use model_diagram::ModelDiagramView;

use crate::api::{fetch_model, FullCube, FullModel};
use crate::app::AppCtx;
use leptos::prelude::*;
use soma_ui::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Card, CardContent,
    CardHeader, CardTitle, Empty, PageHeader, Spinner,
};

// ── Badge colour helpers ───────────────────────────────────────────────────────

fn dim_type_variant(data_type: &str) -> BadgeVariant {
    match data_type {
        "time" => BadgeVariant::Default,
        "number" => BadgeVariant::Secondary,
        "boolean" => BadgeVariant::Outline,
        _ => BadgeVariant::Outline, // string
    }
}

fn relationship_variant(rel: &str) -> BadgeVariant {
    match rel {
        "one_to_one" => BadgeVariant::Success,
        "one_to_many" => BadgeVariant::Secondary,
        _ => BadgeVariant::Outline, // many_to_one
    }
}

// ── Section sub-components ────────────────────────────────────────────────────

#[component]
fn SectionLabel(#[prop(into)] label: String) -> impl IntoView {
    view! {
        <p class="text-xs font-semibold uppercase tracking-widest text-muted-foreground mb-1.5 mt-3">
            {label}
        </p>
    }
}

// ── Cube card ─────────────────────────────────────────────────────────────────

#[component]
fn CubeCard(cube: FullCube) -> impl IntoView {
    let source_label = cube
        .sql_table
        .clone()
        .unwrap_or_else(|| "SQL".to_string());
    let has_base_sql = cube.base_sql.is_some();

    let source_badge_variant = if has_base_sql {
        BadgeVariant::Secondary
    } else {
        BadgeVariant::Outline
    };

    let measures = cube.measures.clone();
    let dimensions = cube.dimensions.clone();
    let joins = cube.joins.clone();
    let segments = cube.segments.clone();

    view! {
        <Card class="flex flex-col gap-0 overflow-hidden".to_string()>
            <CardHeader class="pb-3".to_string()>
                <div class="flex items-start justify-between gap-2 flex-wrap">
                    <div>
                        <CardTitle class="text-base font-semibold font-mono".to_string()>
                            {cube.name.clone()}
                        </CardTitle>
                        {cube.title.map(|t| view! {
                            <p class="text-xs text-muted-foreground mt-0.5">{t}</p>
                        })}
                    </div>
                    <div class="flex gap-1.5 flex-wrap items-center shrink-0">
                        <Badge variant=source_badge_variant>
                            <span class="font-mono">{source_label}</span>
                        </Badge>
                        <Badge variant=BadgeVariant::Outline>
                            <span class="font-mono text-xs">{format!("tenant: {}", cube.tenant_column)}</span>
                        </Badge>
                    </div>
                </div>
                {cube.description.map(|d| view! {
                    <p class="text-xs text-muted-foreground mt-1">{d}</p>
                })}
            </CardHeader>

            <CardContent class="pt-0 space-y-0".to_string()>
                // ── Measures ───────────────────────────────────────────────
                {if !measures.is_empty() {
                    let rows = measures.iter().map(|m| {
                        let name = m.name.clone();
                        let agg = m.agg_type.clone();
                        let desc = m.description.clone();
                        view! {
                            <div class="flex items-center justify-between gap-2 py-1 border-b border-border/40 last:border-0">
                                <div class="flex flex-col min-w-0">
                                    <span class="text-sm font-mono truncate">{name}</span>
                                    {desc.map(|d| view! {
                                        <span class="text-xs text-muted-foreground truncate">{d}</span>
                                    })}
                                </div>
                                <Badge variant=BadgeVariant::Default>
                                    {agg}
                                </Badge>
                            </div>
                        }
                    }).collect::<Vec<_>>();
                    view! {
                        <div>
                            <SectionLabel label="Measures" />
                            <div class="space-y-0">{rows}</div>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}

                // ── Dimensions ─────────────────────────────────────────────
                {if !dimensions.is_empty() {
                    let rows = dimensions.iter().map(|d| {
                        let name = d.name.clone();
                        let dtype = d.data_type.clone();
                        let desc = d.description.clone();
                        let variant = dim_type_variant(&d.data_type);
                        view! {
                            <div class="flex items-center justify-between gap-2 py-1 border-b border-border/40 last:border-0">
                                <div class="flex flex-col min-w-0">
                                    <span class="text-sm font-mono truncate">{name}</span>
                                    {desc.map(|d| view! {
                                        <span class="text-xs text-muted-foreground truncate">{d}</span>
                                    })}
                                </div>
                                <Badge variant=variant>
                                    {dtype}
                                </Badge>
                            </div>
                        }
                    }).collect::<Vec<_>>();
                    view! {
                        <div>
                            <SectionLabel label="Dimensions" />
                            <div class="space-y-0">{rows}</div>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}

                // ── Joins ──────────────────────────────────────────────────
                {if !joins.is_empty() {
                    let rows = joins.iter().map(|j| {
                        let target = j.target_cube.clone();
                        let rel = j.relationship.clone();
                        let variant = relationship_variant(&j.relationship);
                        view! {
                            <div class="flex items-center justify-between gap-2 py-1 border-b border-border/40 last:border-0">
                                <span class="text-sm text-muted-foreground font-mono truncate">
                                    {format!("→ {}", target)}
                                </span>
                                <Badge variant=variant>
                                    {rel}
                                </Badge>
                            </div>
                        }
                    }).collect::<Vec<_>>();
                    view! {
                        <div>
                            <SectionLabel label="Joins" />
                            <div class="space-y-0">{rows}</div>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}

                // ── Segments ───────────────────────────────────────────────
                {if !segments.is_empty() {
                    let chips = segments.iter().map(|s| {
                        let name = s.name.clone();
                        view! {
                            <Badge variant=BadgeVariant::Outline>
                                <span class="font-mono">{name}</span>
                            </Badge>
                        }
                    }).collect::<Vec<_>>();
                    view! {
                        <div>
                            <SectionLabel label="Segments" />
                            <div class="flex flex-wrap gap-1.5">{chips}</div>
                        </div>
                    }.into_any()
                } else {
                    ().into_any()
                }}
            </CardContent>
        </Card>
    }
}

// ── List view (existing card grid) ────────────────────────────────────────────

#[component]
fn ModelListView(cubes: Vec<FullCube>) -> impl IntoView {
    if cubes.is_empty() {
        return view! {
            <Empty
                title="No cubes yet".to_string()
                description="Apply a model with `soma-cli apply` or the editor (coming soon).".to_string()
            />
        }
        .into_any();
    }

    let cards = cubes
        .into_iter()
        .map(|cube| view! { <CubeCard cube=cube /> })
        .collect::<Vec<_>>();

    view! {
        <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
            {cards}
        </div>
    }
    .into_any()
}

// ── Toggle button helpers ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Diagram,
    List,
}

// ── Page ──────────────────────────────────────────────────────────────────────

#[component]
pub fn ModelViewPage() -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");
    let model: RwSignal<Option<FullModel>> = RwSignal::new(None);
    let err: RwSignal<Option<String>> = RwSignal::new(None);
    let loading = RwSignal::new(true);
    let view_mode: RwSignal<ViewMode> = RwSignal::new(ViewMode::Diagram);

    Effect::new(move |_| {
        let token = ctx.token.get();
        let base = ctx.api_base.get();
        loading.set(true);
        err.set(None);
        model.set(None);
        leptos::task::spawn_local(async move {
            match fetch_model(&base, &token).await {
                Ok(m) => model.set(Some(m)),
                Err(e) => {
                    let msg = if e.status == 401 || e.status == 403 {
                        format!(
                            "{} — paste a valid Editor+ token in the header",
                            e.status
                        )
                    } else {
                        e.message
                    };
                    err.set(Some(msg));
                }
            }
            loading.set(false);
        });
    });

    view! {
        <div class="space-y-6">
            // ── Header ────────────────────────────────────────────────────
            {move || {
                let cube_count = model.get().as_ref().map(|m| m.cubes.len()).unwrap_or(0);
                let subtitle = if cube_count > 0 {
                    Some(format!("{} cube{}", cube_count, if cube_count == 1 { "" } else { "s" }))
                } else {
                    None
                };
                view! {
                    <PageHeader title="Semantic model".to_string() subtitle=subtitle>
                        <span class="text-xs text-muted-foreground">"GET /api/v1/model"</span>
                    </PageHeader>
                }
            }}

            // ── View toggle ────────────────────────────────────────────────
            <div class="flex items-center gap-1 p-1 rounded-lg bg-muted w-fit">
                <button
                    class=move || {
                        let active = view_mode.get() == ViewMode::Diagram;
                        if active {
                            "px-3 py-1.5 text-sm font-medium rounded-md bg-background text-foreground shadow-sm transition-all"
                        } else {
                            "px-3 py-1.5 text-sm font-medium rounded-md text-muted-foreground hover:text-foreground transition-all"
                        }
                    }
                    on:click=move |_| view_mode.set(ViewMode::Diagram)
                >
                    "Diagram"
                </button>
                <button
                    class=move || {
                        let active = view_mode.get() == ViewMode::List;
                        if active {
                            "px-3 py-1.5 text-sm font-medium rounded-md bg-background text-foreground shadow-sm transition-all"
                        } else {
                            "px-3 py-1.5 text-sm font-medium rounded-md text-muted-foreground hover:text-foreground transition-all"
                        }
                    }
                    on:click=move |_| view_mode.set(ViewMode::List)
                >
                    "List"
                </button>
            </div>

            // ── Error banner ──────────────────────────────────────────────
            {move || err.get().map(|msg| view! {
                <Alert variant=AlertVariant::Destructive>
                    <AlertTitle>"Failed to load model"</AlertTitle>
                    <AlertDescription>{msg}</AlertDescription>
                </Alert>
            })}

            // ── Loading spinner ───────────────────────────────────────────
            {move || loading.get().then(|| view! {
                <div class="flex justify-center py-12">
                    <Spinner />
                </div>
            })}

            // ── Content: diagram or list ───────────────────────────────────
            {move || {
                let Some(m) = model.get() else { return ().into_any(); };

                match view_mode.get() {
                    ViewMode::Diagram => {
                        view! { <ModelDiagramView cubes=m.cubes /> }.into_any()
                    }
                    ViewMode::List => {
                        if m.cubes.is_empty() {
                            return view! {
                                <Empty
                                    title="No cubes yet".to_string()
                                    description="Apply a model with `soma-cli apply` or the editor (coming soon).".to_string()
                                />
                            }.into_any();
                        }
                        view! { <ModelListView cubes=m.cubes /> }.into_any()
                    }
                }
            }}
        </div>
    }
}
