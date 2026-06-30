//! Explore / Playground page — point-and-click query builder → run → chart + table + SQL.
//!
//! Layout: two-column grid (builder left, results right); single-column on small screens.
//!
//! Uses native <select> elements throughout: soma-ui Select/SelectContent/SelectItem require
//! ChildrenFn (Send + Sync) but view items with on:click closures are !Sync in CSR wasm.

use crate::api::{
    compile_query, fetch_model, run_query, Filter, FilterOp, FullCube, FullModel, Granularity,
    ResultSet, SemanticQuery, TimeDimension,
};
use crate::app::AppCtx;
use leptos::prelude::*;
use soma_ui::{
    Alert, AlertDescription, AlertTitle, AlertVariant, AnalyticsPanel, Badge, BadgeVariant,
    Button, ButtonVariant, Card, CardContent, CardHeader, CardTitle, PageHeader, Spinner,
    SomaColumn, SomaResultSet,
};

// ── Shared select class ───────────────────────────────────────────────────────

const SELECT_CLS: &str = "flex h-10 w-full items-center rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40";
const SELECT_SM_CLS: &str = "flex h-9 w-full items-center rounded-md border border-input bg-background px-3 py-1.5 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40";

// ── Filter row state ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct FilterRow {
    member: RwSignal<String>,
    operator: RwSignal<String>,
    value: RwSignal<String>,
}

impl FilterRow {
    fn new() -> Self {
        Self {
            member: RwSignal::new(String::new()),
            operator: RwSignal::new("equals".to_string()),
            value: RwSignal::new(String::new()),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn parse_filter_op(s: &str) -> FilterOp {
    match s {
        "not_equals" => FilterOp::NotEquals,
        "contains" => FilterOp::Contains,
        "gt" => FilterOp::Gt,
        "gte" => FilterOp::Gte,
        "lt" => FilterOp::Lt,
        "lte" => FilterOp::Lte,
        _ => FilterOp::Equals,
    }
}

fn parse_granularity(s: &str) -> Granularity {
    match s {
        "week" => Granularity::Week,
        "month" => Granularity::Month,
        "quarter" => Granularity::Quarter,
        "year" => Granularity::Year,
        _ => Granularity::Day,
    }
}

fn agg_badge_variant(agg: &str) -> BadgeVariant {
    match agg {
        "count" | "count_distinct" => BadgeVariant::Default,
        "sum" | "avg" => BadgeVariant::Secondary,
        _ => BadgeVariant::Outline,
    }
}

// ── CheckItem sub-component ───────────────────────────────────────────────────

#[component]
fn CheckItem(
    #[prop(into)] label: String,
    #[prop(optional_no_strip)] desc: Option<String>,
    badge: (String, BadgeVariant),
    checked: RwSignal<bool>,
) -> impl IntoView {
    let (badge_text, badge_variant) = badge;
    view! {
        <label class="flex items-center gap-2 cursor-pointer group py-0.5">
            <button
                type="button"
                role="checkbox"
                aria-checked=move || if checked.get() { "true" } else { "false" }
                class="relative flex h-5 w-5 shrink-0 items-center justify-center rounded border hover:border-ring/60 transition-colors"
                class:bg-primary=move || checked.get()
                class:border-primary=move || checked.get()
                class:border-input=move || !checked.get()
                on:click=move |_| checked.update(|v| *v = !*v)
            >
                <Show when=move || checked.get()>
                    <svg class="text-primary-foreground" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="20 6 9 17 4 12" />
                    </svg>
                </Show>
            </button>
            <div class="flex flex-1 items-center justify-between gap-2 min-w-0">
                <div class="flex flex-col min-w-0">
                    <span class="text-sm font-mono truncate">{label}</span>
                    {desc.map(|d| view! { <span class="text-xs text-muted-foreground truncate">{d}</span> })}
                </div>
                <Badge variant=badge_variant>{badge_text}</Badge>
            </div>
        </label>
    }
}

// ── Page ──────────────────────────────────────────────────────────────────────

#[component]
pub fn ExplorePage() -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");

    // ── Model data ────────────────────────────────────────────────────────────
    let model: RwSignal<Option<FullModel>> = RwSignal::new(None);
    let model_err: RwSignal<Option<String>> = RwSignal::new(None);
    let model_loading = RwSignal::new(true);

    Effect::new(move |_| {
        let token = ctx.token.get();
        let base = ctx.api_base.get();
        model_loading.set(true);
        model_err.set(None);
        model.set(None);
        leptos::task::spawn_local(async move {
            match fetch_model(&base, &token).await {
                Ok(m) => model.set(Some(m)),
                Err(e) => model_err.set(Some(e.message)),
            }
            model_loading.set(false);
        });
    });

    // ── Builder state ─────────────────────────────────────────────────────────
    let selected_cube: RwSignal<String> = RwSignal::new(String::new());

    // Per-member checked states: Vec<(name, RwSignal<bool>)>
    let measure_checks: RwSignal<Vec<(String, RwSignal<bool>)>> = RwSignal::new(vec![]);
    let dimension_checks: RwSignal<Vec<(String, RwSignal<bool>)>> = RwSignal::new(vec![]);

    let time_member: RwSignal<String> = RwSignal::new(String::new());
    let granularity: RwSignal<String> = RwSignal::new("day".to_string());
    let date_from: RwSignal<String> = RwSignal::new(String::new());
    let date_to: RwSignal<String> = RwSignal::new(String::new());
    let filter_rows: RwSignal<Vec<FilterRow>> = RwSignal::new(vec![]);
    let limit: RwSignal<String> = RwSignal::new("100".to_string());

    // ── Results state ─────────────────────────────────────────────────────────
    let result: RwSignal<Option<ResultSet>> = RwSignal::new(None);
    let sql_result: RwSignal<Option<(String, usize)>> = RwSignal::new(None);
    let query_err: RwSignal<Option<String>> = RwSignal::new(None);
    let loading = RwSignal::new(false);
    let chart_type: RwSignal<String> = RwSignal::new("table".to_string());

    // ── Derived: current cube ─────────────────────────────────────────────────
    let current_cube = Signal::derive(move || -> Option<FullCube> {
        let cube_name = selected_cube.get();
        if cube_name.is_empty() {
            return None;
        }
        model.get()?.cubes.into_iter().find(|c| c.name == cube_name)
    });

    // ── On cube change ────────────────────────────────────────────────────────
    let on_cube_select = move |name: String| {
        selected_cube.set(name.clone());
        time_member.set(String::new());
        granularity.set("day".to_string());
        date_from.set(String::new());
        date_to.set(String::new());
        filter_rows.set(vec![]);
        result.set(None);
        sql_result.set(None);
        query_err.set(None);

        if let Some(m) = model.get() {
            if let Some(cube) = m.cubes.into_iter().find(|c| c.name == name) {
                measure_checks.set(
                    cube.measures
                        .iter()
                        .map(|m| (m.name.clone(), RwSignal::new(false)))
                        .collect(),
                );
                dimension_checks.set(
                    cube.dimensions
                        .iter()
                        .map(|d| (d.name.clone(), RwSignal::new(false)))
                        .collect(),
                );
            }
        }
    };

    // ── Build SemanticQuery ───────────────────────────────────────────────────
    let build_query = move || -> Option<SemanticQuery> {
        let cube = selected_cube.get();
        if cube.is_empty() {
            return None;
        }
        let measures = measure_checks
            .get()
            .into_iter()
            .filter(|(_, sig)| sig.get())
            .map(|(n, _)| format!("{cube}.{n}"))
            .collect::<Vec<_>>();
        let dimensions = dimension_checks
            .get()
            .into_iter()
            .filter(|(_, sig)| sig.get())
            .map(|(n, _)| format!("{cube}.{n}"))
            .collect::<Vec<_>>();
        let filters = filter_rows
            .get()
            .into_iter()
            .filter(|f| !f.member.get().is_empty())
            .map(|f| Filter {
                member: format!("{cube}.{}", f.member.get()),
                operator: parse_filter_op(&f.operator.get()),
                values: {
                    let v = f.value.get();
                    if v.is_empty() { vec![] } else { vec![v] }
                },
            })
            .collect::<Vec<_>>();
        let time_dimension = {
            let td = time_member.get();
            if td.is_empty() {
                None
            } else {
                let from = date_from.get();
                let to = date_to.get();
                let date_range = if !from.is_empty() && !to.is_empty() {
                    Some([from, to])
                } else {
                    None
                };
                Some(TimeDimension {
                    member: format!("{cube}.{td}"),
                    granularity: parse_granularity(&granularity.get()),
                    date_range,
                })
            }
        };
        let limit_val = limit.get().parse::<u32>().ok();
        Some(SemanticQuery {
            cube,
            measures,
            dimensions,
            filters,
            segments: vec![],
            time_dimension,
            order: vec![],
            limit: limit_val,
            offset: None,
        })
    };

    // ── Run / ShowSQL handlers ────────────────────────────────────────────────
    let on_run = move |_| {
        let Some(query) = build_query() else { return };
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        loading.set(true);
        result.set(None);
        sql_result.set(None);
        query_err.set(None);
        leptos::task::spawn_local(async move {
            match run_query(&base, &token, &query).await {
                Ok(rs) => result.set(Some(rs)),
                Err(e) => query_err.set(Some(e.message)),
            }
            loading.set(false);
        });
    };

    let on_show_sql = move |_| {
        let Some(query) = build_query() else { return };
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        loading.set(true);
        sql_result.set(None);
        query_err.set(None);
        leptos::task::spawn_local(async move {
            match compile_query(&base, &token, &query).await {
                Ok(cr) => sql_result.set(Some((cr.sql, cr.param_count))),
                Err(e) => query_err.set(Some(e.message)),
            }
            loading.set(false);
        });
    };

    view! {
        <div class="space-y-4">
            <PageHeader title="Explore".to_string() subtitle=Some("Query builder & playground".to_string())>
                <span class="text-xs text-muted-foreground">"POST /api/v1/query"</span>
            </PageHeader>

            // ── Model load error ──────────────────────────────────────────────
            {move || model_err.get().map(|msg| view! {
                <Alert variant=AlertVariant::Destructive>
                    <AlertTitle>"Could not load model"</AlertTitle>
                    <AlertDescription>{msg}</AlertDescription>
                </Alert>
            })}

            // ── Two-column layout ─────────────────────────────────────────────
            <div class="grid grid-cols-1 xl:grid-cols-[380px_1fr] gap-4 items-start">

                // ── LEFT: Builder panel ───────────────────────────────────────
                <Card class="flex flex-col gap-0".to_string()>
                    <CardHeader class="pb-3".to_string()>
                        <CardTitle class="text-sm font-semibold".to_string()>"Query builder"</CardTitle>
                    </CardHeader>
                    <CardContent class="space-y-4 pt-0".to_string()>

                        // ── Cube selector ─────────────────────────────────────
                        <div class="space-y-1">
                            <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">"Cube"</p>
                            {move || {
                                if model_loading.get() {
                                    return view! {
                                        <div class="flex items-center gap-2 py-2">
                                            <Spinner />
                                            <span class="text-xs text-muted-foreground">"Loading model..."</span>
                                        </div>
                                    }.into_any();
                                }
                                let Some(m) = model.get() else {
                                    return view! {
                                        <p class="text-xs text-muted-foreground py-2">"No model loaded."</p>
                                    }.into_any();
                                };
                                let cube_options = m.cubes.into_iter().map(|c| {
                                    let name = c.name.clone();
                                    let label = c.title.clone().unwrap_or_else(|| c.name.clone());
                                    view! { <option value=name>{label}</option> }
                                }).collect::<Vec<_>>();
                                view! {
                                    <select
                                        class=SELECT_CLS
                                        on:change=move |e| on_cube_select(event_target_value(&e))
                                    >
                                        <option value="">"-- pick a cube --"</option>
                                        {cube_options}
                                    </select>
                                }.into_any()
                            }}
                        </div>

                        // ── Measures ──────────────────────────────────────────
                        {move || {
                            let checks = measure_checks.get();
                            if checks.is_empty() { return ().into_any(); }
                            let Some(cube) = current_cube.get() else { return ().into_any(); };
                            view! {
                                <div class="space-y-1">
                                    <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">"Measures"</p>
                                    <div class="space-y-0.5">
                                        {checks.into_iter().map(|(name, sig)| {
                                            let measure = cube.measures.iter().find(|m| m.name == name).cloned();
                                            let agg = measure.as_ref().map(|m| m.agg_type.clone()).unwrap_or_default();
                                            let desc = measure.and_then(|m| m.description.clone());
                                            let badge_var = agg_badge_variant(&agg);
                                            view! {
                                                <CheckItem
                                                    label=name
                                                    desc=desc
                                                    badge=(agg, badge_var)
                                                    checked=sig
                                                />
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            }.into_any()
                        }}

                        // ── Dimensions ────────────────────────────────────────
                        {move || {
                            let checks = dimension_checks.get();
                            if checks.is_empty() { return ().into_any(); }
                            let Some(cube) = current_cube.get() else { return ().into_any(); };
                            view! {
                                <div class="space-y-1">
                                    <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">"Dimensions"</p>
                                    <div class="space-y-0.5">
                                        {checks.into_iter().map(|(name, sig)| {
                                            let dim = cube.dimensions.iter().find(|d| d.name == name).cloned();
                                            let dtype = dim.as_ref().map(|d| d.data_type.clone()).unwrap_or_default();
                                            let desc = dim.and_then(|d| d.description.clone());
                                            let badge_var = match dtype.as_str() {
                                                "time" => BadgeVariant::Default,
                                                "number" => BadgeVariant::Secondary,
                                                _ => BadgeVariant::Outline,
                                            };
                                            view! {
                                                <CheckItem
                                                    label=name
                                                    desc=desc
                                                    badge=(dtype, badge_var)
                                                    checked=sig
                                                />
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            }.into_any()
                        }}

                        // ── Time grain (only shown when cube has time dims) ────
                        {move || {
                            let Some(cube) = current_cube.get() else { return ().into_any(); };
                            let time_dims: Vec<String> = cube.dimensions.into_iter()
                                .filter(|d| d.data_type == "time")
                                .map(|d| d.name.clone())
                                .collect();
                            if time_dims.is_empty() { return ().into_any(); }
                            let td_options = time_dims.into_iter().map(|name| {
                                view! { <option value=name.clone()>{name.clone()}</option> }
                            }).collect::<Vec<_>>();
                            view! {
                                <div class="space-y-2 border-t border-border/40 pt-3">
                                    <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">"Time grain (optional)"</p>
                                    <select
                                        class=SELECT_SM_CLS
                                        on:change=move |e| time_member.set(event_target_value(&e))
                                    >
                                        <option value="">"-- none --"</option>
                                        {td_options}
                                    </select>
                                    <Show when=move || !time_member.get().is_empty()>
                                        <select
                                            class=SELECT_SM_CLS
                                            on:change=move |e| granularity.set(event_target_value(&e))
                                        >
                                            <option value="day">"Day"</option>
                                            <option value="week">"Week"</option>
                                            <option value="month">"Month"</option>
                                            <option value="quarter">"Quarter"</option>
                                            <option value="year">"Year"</option>
                                        </select>
                                        <div class="grid grid-cols-2 gap-2">
                                            <div class="space-y-1">
                                                <p class="text-xs text-muted-foreground">"From"</p>
                                                <input
                                                    type="date"
                                                    class="flex h-9 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40"
                                                    prop:value=move || date_from.get()
                                                    on:input=move |e| date_from.set(event_target_value(&e))
                                                />
                                            </div>
                                            <div class="space-y-1">
                                                <p class="text-xs text-muted-foreground">"To"</p>
                                                <input
                                                    type="date"
                                                    class="flex h-9 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40"
                                                    prop:value=move || date_to.get()
                                                    on:input=move |e| date_to.set(event_target_value(&e))
                                                />
                                            </div>
                                        </div>
                                    </Show>
                                </div>
                            }.into_any()
                        }}

                        // ── Filters ───────────────────────────────────────────
                        {move || {
                            let Some(cube) = current_cube.get() else { return ().into_any(); };
                            let dim_names: Vec<String> = cube.dimensions.iter().map(|d| d.name.clone()).collect();
                            let rows = filter_rows.get();
                            view! {
                                <div class="space-y-2 border-t border-border/40 pt-3">
                                    <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">"Filters"</p>
                                    {rows.into_iter().enumerate().map(|(i, row)| {
                                        let member_options = dim_names.iter().map(|n| {
                                            let n = n.clone();
                                            view! { <option value=n.clone()>{n.clone()}</option> }
                                        }).collect::<Vec<_>>();
                                        view! {
                                            <div class="flex gap-1.5 items-center flex-wrap">
                                                <div class="flex-1 min-w-0">
                                                    <select
                                                        class="flex h-10 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40"
                                                        on:change=move |e| row.member.set(event_target_value(&e))
                                                    >
                                                        <option value="">"field..."</option>
                                                        {member_options}
                                                    </select>
                                                </div>
                                                <div class="w-20 shrink-0">
                                                    <select
                                                        class="flex h-10 w-full rounded-md border border-input bg-background px-2 text-xs text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40"
                                                        on:change=move |e| row.operator.set(event_target_value(&e))
                                                    >
                                                        <option value="equals">"="</option>
                                                        <option value="not_equals">"!="</option>
                                                        <option value="contains">"~"</option>
                                                        <option value="gt">">"</option>
                                                        <option value="gte">">="</option>
                                                        <option value="lt">"&lt;"</option>
                                                        <option value="lte">"&lt;="</option>
                                                    </select>
                                                </div>
                                                <input
                                                    type="text"
                                                    class="flex h-10 w-24 shrink-0 rounded-md border border-input bg-background px-3 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40"
                                                    placeholder="value"
                                                    prop:value=move || row.value.get()
                                                    on:input=move |e| row.value.set(event_target_value(&e))
                                                />
                                                <button
                                                    type="button"
                                                    class="shrink-0 p-1 text-muted-foreground hover:text-destructive transition-colors"
                                                    on:click=move |_| {
                                                        filter_rows.update(|v| { v.remove(i); });
                                                    }
                                                >
                                                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                        <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
                                                    </svg>
                                                </button>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                    <Button variant=ButtonVariant::Outline on:click=move |_| {
                                        filter_rows.update(|v| v.push(FilterRow::new()));
                                    }>
                                        <svg class="mr-1.5" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                            <line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" />
                                        </svg>
                                        "Add filter"
                                    </Button>
                                </div>
                            }.into_any()
                        }}

                        // ── Limit ─────────────────────────────────────────────
                        <div class="space-y-1 border-t border-border/40 pt-3">
                            <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">"Limit"</p>
                            <input
                                type="number"
                                min="1"
                                max="10000"
                                class="flex h-10 w-full rounded-md border border-input bg-background px-3 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40"
                                prop:value=move || limit.get()
                                on:input=move |e| limit.set(event_target_value(&e))
                            />
                        </div>

                        // ── Action buttons ────────────────────────────────────
                        <div class="flex gap-2 border-t border-border/40 pt-3">
                            {move || {
                                let disabled = selected_cube.get().is_empty() || loading.get();
                                let is_loading = loading.get();
                                view! {
                                    <>
                                        <Button disabled=disabled on:click=on_run>
                                            {if is_loading {
                                                view! { <Spinner /> }.into_any()
                                            } else {
                                                view! { "Run" }.into_any()
                                            }}
                                        </Button>
                                        <Button variant=ButtonVariant::Outline disabled=disabled on:click=on_show_sql>
                                            "Show SQL"
                                        </Button>
                                    </>
                                }
                            }}
                        </div>

                    </CardContent>
                </Card>

                // ── RIGHT: Results panel ──────────────────────────────────────
                <Card class="flex flex-col gap-0 min-h-[400px]".to_string()>
                    <CardHeader class="pb-3".to_string()>
                        <div class="flex items-center justify-between gap-3 flex-wrap">
                            <CardTitle class="text-sm font-semibold".to_string()>"Results"</CardTitle>
                            <select
                                class="h-9 rounded-md border border-input bg-background px-2 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40"
                                on:change=move |e| chart_type.set(event_target_value(&e))
                            >
                                <option value="table">"Table"</option>
                                <option value="bar">"Bar"</option>
                                <option value="line">"Line"</option>
                                <option value="area">"Area"</option>
                                <option value="pie">"Pie"</option>
                                <option value="number">"Number"</option>
                            </select>
                        </div>
                    </CardHeader>
                    <CardContent class="pt-0 flex-1 space-y-4".to_string()>

                        // ── Query error ───────────────────────────────────────
                        {move || query_err.get().map(|msg| view! {
                            <Alert variant=AlertVariant::Destructive>
                                <AlertTitle>"Query failed"</AlertTitle>
                                <AlertDescription>{msg}</AlertDescription>
                            </Alert>
                        })}

                        // ── Loading ───────────────────────────────────────────
                        {move || loading.get().then(|| view! {
                            <div class="flex justify-center py-12"><Spinner /></div>
                        })}

                        // ── Empty state ───────────────────────────────────────
                        {move || {
                            let has_result = result.get().is_some();
                            let has_sql = sql_result.get().is_some();
                            let is_loading = loading.get();
                            let has_err = query_err.get().is_some();
                            if !has_result && !has_sql && !is_loading && !has_err {
                                view! {
                                    <div class="flex flex-col items-center justify-center py-16 text-center gap-2">
                                        <svg class="text-muted-foreground/40" width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                                            <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
                                        </svg>
                                        <p class="text-sm text-muted-foreground">"Pick a cube and measures, then Run."</p>
                                    </div>
                                }.into_any()
                            } else {
                                ().into_any()
                            }
                        }}

                        // ── Chart + meta ──────────────────────────────────────
                        {move || {
                            let Some(rs) = result.get() else { return ().into_any(); };
                            let soma_rs = SomaResultSet {
                                columns: rs.columns.iter().map(|c| SomaColumn {
                                    name: c.name.clone(),
                                    data_type: c.data_type.clone(),
                                }).collect(),
                                rows: rs.rows.clone(),
                            };
                            let cube_name = selected_cube.get();
                            let ct = chart_type.get();
                            let cache = rs.meta.cache.clone();
                            let row_count = rs.meta.row_count;
                            view! {
                                <div class="space-y-4">
                                    <p class="text-xs text-muted-foreground tabular-nums">
                                        {format!("rows: {} - cache: {}", row_count, cache)}
                                    </p>
                                    <AnalyticsPanel
                                        title=format!("{} report", cube_name)
                                        chart_type=ct
                                        result=soma_rs
                                    />
                                </div>
                            }.into_any()
                        }}

                        // ── SQL result ────────────────────────────────────────
                        {move || {
                            let Some((sql, param_count)) = sql_result.get() else { return ().into_any(); };
                            view! {
                                <div class="space-y-2">
                                    <div class="flex items-center gap-2">
                                        <p class="text-xs font-medium text-muted-foreground uppercase tracking-wide">"Generated SQL"</p>
                                        <Badge variant=BadgeVariant::Outline>
                                            {format!("{} param{}", param_count, if param_count == 1 { "" } else { "s" })}
                                        </Badge>
                                    </div>
                                    <pre class="overflow-auto rounded-md bg-muted/50 border border-border p-4 text-xs font-mono text-foreground whitespace-pre-wrap break-words max-h-96">
                                        {sql}
                                    </pre>
                                </div>
                            }.into_any()
                        }}

                    </CardContent>
                </Card>

            </div>
        </div>
    }
}
