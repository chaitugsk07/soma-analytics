//! Visual Model Editor — create/edit/delete cubes, measures, dimensions, joins.
//!
//! Layout: left sidebar (cube list + data sources), right panel (cube detail).
//! Uses native <select> throughout — soma-ui Select is not usable with reactive
//! on:click closures in CSR wasm (Send + Sync constraint).
//!
//! After every mutation the model is reloaded via fetch_model so the UI stays
//! in sync with the server state.

use crate::api::{
    create_cube, create_data_source, create_dimension, create_measure,
    delete_cube, delete_dimension, delete_measure,
    fetch_model, list_data_sources, DataSourceItem, FullCube, FullModel,
};
use crate::app::AppCtx;
use leptos::prelude::*;
use soma_ui::{
    Alert, AlertDescription, AlertTitle, AlertVariant,
    AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
    AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
    Badge, BadgeVariant,
    Button, ButtonVariant,
    Card, CardContent, CardHeader, CardTitle,
    Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
    Empty, Input, Label, PageHeader, Spinner, Textarea,
};

// ── Shared select style ────────────────────────────────────────────────────────

const SELECT_CLS: &str = "flex h-10 w-full items-center rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground focus:outline-none focus:ring-2 focus:ring-ring/40";

// ── Small inline SVG icons ─────────────────────────────────────────────────────

fn trash_icon() -> impl IntoView {
    view! {
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
            stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="3 6 5 6 21 6" />
            <path d="M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" />
            <path d="M10 11v6M14 11v6" />
            <path d="M9 6V4a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2" />
        </svg>
    }
}

fn plus_icon() -> impl IntoView {
    view! {
        <svg class="mr-1" width="14" height="14" viewBox="0 0 24 24" fill="none"
            stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="5" x2="12" y2="19" />
            <line x1="5" y1="12" x2="19" y2="12" />
        </svg>
    }
}

// ── New data-source dialog ─────────────────────────────────────────────────────

#[component]
fn NewDataSourceDialog(
    open: RwSignal<bool>,
    on_created: Callback<()>,
) -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");
    let name = RwSignal::new(String::new());
    let driver = RwSignal::new("postgres".to_string());
    let err = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(false);

    let on_submit = move |_: web_sys::MouseEvent| {
        let n = name.get();
        let d = driver.get();
        if n.trim().is_empty() {
            err.set(Some("Name is required.".into()));
            return;
        }
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        loading.set(true);
        err.set(None);
        leptos::task::spawn_local(async move {
            match create_data_source(&base, &token, n.trim(), &d).await {
                Ok(_) => {
                    open.set(false);
                    name.set(String::new());
                    on_created.run(());
                }
                Err(e) => err.set(Some(e.message)),
            }
            loading.set(false);
        });
    };

    view! {
        <Dialog open=open>
            <DialogContent>
                <DialogHeader>
                    <DialogTitle>"New data source"</DialogTitle>
                    <DialogDescription>"Connect a new data source."</DialogDescription>
                </DialogHeader>
                <div class="space-y-4 py-2">
                    {move || err.get().map(|m| view! {
                        <Alert variant=AlertVariant::Destructive>
                            <AlertDescription>{m}</AlertDescription>
                        </Alert>
                    })}
                    <div class="space-y-1">
                        <Label>"Name"</Label>
                        <Input value=name placeholder="my-db".to_string() />
                    </div>
                    <div class="space-y-1">
                        <Label>"Driver"</Label>
                        <select
                            class=SELECT_CLS
                            on:change=move |e| driver.set(event_target_value(&e))
                        >
                            <option value="postgres">"postgres"</option>
                            <option value="mysql">"mysql"</option>
                            <option value="sqlite">"sqlite"</option>
                        </select>
                    </div>
                </div>
                <DialogFooter>
                    <Button variant=ButtonVariant::Outline on:click=move |_| open.set(false)>
                        "Cancel"
                    </Button>
                    {move || {
                        let is_loading = loading.get();
                        view! {
                            <Button disabled=is_loading on:click=on_submit>
                                {if is_loading { view! { <Spinner /> }.into_any() }
                                 else { view! { "Create" }.into_any() }}
                            </Button>
                        }
                    }}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}

// ── New cube dialog ────────────────────────────────────────────────────────────

#[component]
fn NewCubeDialog(
    open: RwSignal<bool>,
    data_sources: Signal<Vec<DataSourceItem>>,
    on_created: Callback<()>,
) -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");
    let name = RwSignal::new(String::new());
    let ds_id = RwSignal::new(String::new());
    let source_mode = RwSignal::new("sql_table".to_string());
    let sql_table = RwSignal::new(String::new());
    let base_sql_val = RwSignal::new(String::new());
    let primary_key = RwSignal::new("id".to_string());
    let tenant_col = RwSignal::new("tenant_id".to_string());
    let title_val = RwSignal::new(String::new());
    let description_val = RwSignal::new(String::new());
    let err = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(false);

    let on_submit = move |_: web_sys::MouseEvent| {
        let n = name.get();
        let ds = ds_id.get();
        let pk = primary_key.get();
        let tc = tenant_col.get();
        if n.trim().is_empty() { err.set(Some("Name is required.".into())); return; }
        if ds.trim().is_empty() { err.set(Some("Pick a data source.".into())); return; }
        if pk.trim().is_empty() { err.set(Some("Primary key is required.".into())); return; }

        let mode = source_mode.get();
        let st = sql_table.get();
        let bs = base_sql_val.get();
        let sql_t = if mode == "sql_table" && !st.trim().is_empty() { Some(st) } else { None };
        let base_s = if mode == "base_sql" && !bs.trim().is_empty() { Some(bs) } else { None };
        let t = title_val.get();
        let d_val = description_val.get();
        let title_opt = if t.trim().is_empty() { None } else { Some(t) };
        let desc_opt = if d_val.trim().is_empty() { None } else { Some(d_val) };

        let base = ctx.api_base.get();
        let token = ctx.token.get();
        loading.set(true);
        err.set(None);
        leptos::task::spawn_local(async move {
            match create_cube(
                &base, &token, &ds, n.trim(),
                title_opt.as_deref(), desc_opt.as_deref(),
                sql_t.as_deref(), base_s.as_deref(),
                pk.trim(), tc.trim(),
            ).await {
                Ok(_) => {
                    open.set(false);
                    name.set(String::new());
                    on_created.run(());
                }
                Err(e) => err.set(Some(e.message)),
            }
            loading.set(false);
        });
    };

    view! {
        <Dialog open=open>
            <DialogContent>
                <DialogHeader>
                    <DialogTitle>"New cube"</DialogTitle>
                    <DialogDescription>"Define a semantic cube over a database table or query."</DialogDescription>
                </DialogHeader>
                <div class="space-y-4 py-2">
                    {move || err.get().map(|m| view! {
                        <Alert variant=AlertVariant::Destructive>
                            <AlertDescription>{m}</AlertDescription>
                        </Alert>
                    })}
                    <div class="space-y-1">
                        <Label>"Name"</Label>
                        <Input value=name placeholder="orders".to_string() />
                    </div>
                    <div class="space-y-1">
                        <Label>"Title (optional)"</Label>
                        <Input value=title_val placeholder="Orders".to_string() />
                    </div>
                    <div class="space-y-1">
                        <Label>"Data source"</Label>
                        <select
                            class=SELECT_CLS
                            on:change=move |e| ds_id.set(event_target_value(&e))
                        >
                            <option value="">"-- pick a data source --"</option>
                            {move || data_sources.get().into_iter().map(|ds| {
                                let id = ds.id.clone();
                                let label = format!("{} ({})", ds.name, ds.driver);
                                view! { <option value=id>{label}</option> }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
                    <div class="space-y-1">
                        <Label>"Source mode"</Label>
                        <select
                            class=SELECT_CLS
                            on:change=move |e| source_mode.set(event_target_value(&e))
                        >
                            <option value="sql_table">"sql_table (table name)"</option>
                            <option value="base_sql">"base_sql (inline SQL)"</option>
                        </select>
                    </div>
                    {move || {
                        let mode = source_mode.get();
                        if mode == "sql_table" {
                            view! {
                                <div class="space-y-1">
                                    <Label>"SQL table (schema.table)"</Label>
                                    <Input value=sql_table placeholder="public.orders".to_string() />
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="space-y-1">
                                    <Label>"Base SQL"</Label>
                                    <Textarea value=base_sql_val placeholder="SELECT * FROM orders WHERE ...".to_string() />
                                </div>
                            }.into_any()
                        }
                    }}
                    <div class="grid grid-cols-2 gap-3">
                        <div class="space-y-1">
                            <Label>"Primary key column"</Label>
                            <Input value=primary_key placeholder="id".to_string() />
                        </div>
                        <div class="space-y-1">
                            <Label>"Tenant column"</Label>
                            <Input value=tenant_col placeholder="tenant_id".to_string() />
                        </div>
                    </div>
                    <div class="space-y-1">
                        <Label>"Description (optional)"</Label>
                        <Textarea value=description_val placeholder="What this cube represents...".to_string() />
                    </div>
                </div>
                <DialogFooter>
                    <Button variant=ButtonVariant::Outline on:click=move |_| open.set(false)>
                        "Cancel"
                    </Button>
                    {move || {
                        let is_loading = loading.get();
                        view! {
                            <Button disabled=is_loading on:click=on_submit>
                                {if is_loading { view! { <Spinner /> }.into_any() }
                                 else { view! { "Create" }.into_any() }}
                            </Button>
                        }
                    }}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}

// ── Add measure dialog ─────────────────────────────────────────────────────────

#[component]
fn AddMeasureDialog(
    open: RwSignal<bool>,
    cube_id: Signal<String>,
    on_created: Callback<()>,
) -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");
    let name = RwSignal::new(String::new());
    let agg_type = RwSignal::new("count".to_string());
    let sql_val = RwSignal::new(String::new());
    let description_val = RwSignal::new(String::new());
    let err = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(false);

    let on_submit = move |_: web_sys::MouseEvent| {
        let n = name.get();
        let agg = agg_type.get();
        let cid = cube_id.get();
        if n.trim().is_empty() { err.set(Some("Name is required.".into())); return; }
        let sql_opt = {
            let s = sql_val.get();
            if s.trim().is_empty() || agg == "count" { None } else { Some(s) }
        };
        let d_val = description_val.get();
        let desc_opt = if d_val.trim().is_empty() { None } else { Some(d_val) };
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        loading.set(true);
        err.set(None);
        leptos::task::spawn_local(async move {
            match create_measure(
                &base, &token, &cid, n.trim(), &agg,
                sql_opt.as_deref(), desc_opt.as_deref(),
            ).await {
                Ok(_) => {
                    open.set(false);
                    name.set(String::new());
                    sql_val.set(String::new());
                    description_val.set(String::new());
                    on_created.run(());
                }
                Err(e) => err.set(Some(e.message)),
            }
            loading.set(false);
        });
    };

    view! {
        <Dialog open=open>
            <DialogContent>
                <DialogHeader>
                    <DialogTitle>"Add measure"</DialogTitle>
                </DialogHeader>
                <div class="space-y-4 py-2">
                    {move || err.get().map(|m| view! {
                        <Alert variant=AlertVariant::Destructive>
                            <AlertDescription>{m}</AlertDescription>
                        </Alert>
                    })}
                    <div class="space-y-1">
                        <Label>"Name"</Label>
                        <Input value=name placeholder="count".to_string() />
                    </div>
                    <div class="space-y-1">
                        <Label>"Aggregation type"</Label>
                        <select
                            class=SELECT_CLS
                            on:change=move |e| agg_type.set(event_target_value(&e))
                        >
                            <option value="count">"count"</option>
                            <option value="count_distinct">"count_distinct"</option>
                            <option value="sum">"sum"</option>
                            <option value="avg">"avg"</option>
                            <option value="min">"min"</option>
                            <option value="max">"max"</option>
                            <option value="number">"number"</option>
                        </select>
                    </div>
                    {move || {
                        if agg_type.get() != "count" {
                            view! {
                                <div class="space-y-1">
                                    <Label>"SQL expression (optional)"</Label>
                                    <Input value=sql_val placeholder="{CUBE}.amount".to_string() />
                                </div>
                            }.into_any()
                        } else {
                            ().into_any()
                        }
                    }}
                    <div class="space-y-1">
                        <Label>"Description (optional)"</Label>
                        <Input value=description_val placeholder="Total number of orders".to_string() />
                    </div>
                </div>
                <DialogFooter>
                    <Button variant=ButtonVariant::Outline on:click=move |_| open.set(false)>
                        "Cancel"
                    </Button>
                    {move || {
                        let is_loading = loading.get();
                        view! {
                            <Button disabled=is_loading on:click=on_submit>
                                {if is_loading { view! { <Spinner /> }.into_any() }
                                 else { view! { "Add" }.into_any() }}
                            </Button>
                        }
                    }}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}

// ── Add dimension dialog ───────────────────────────────────────────────────────

#[component]
fn AddDimensionDialog(
    open: RwSignal<bool>,
    cube_id: Signal<String>,
    on_created: Callback<()>,
) -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");
    let name = RwSignal::new(String::new());
    let sql_val = RwSignal::new(String::new());
    let data_type = RwSignal::new("string".to_string());
    let description_val = RwSignal::new(String::new());
    let err = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(false);

    let on_submit = move |_: web_sys::MouseEvent| {
        let n = name.get();
        let s = sql_val.get();
        let cid = cube_id.get();
        if n.trim().is_empty() { err.set(Some("Name is required.".into())); return; }
        if s.trim().is_empty() { err.set(Some("SQL expression is required.".into())); return; }
        let dt = data_type.get();
        let d_val = description_val.get();
        let desc_opt = if d_val.trim().is_empty() { None } else { Some(d_val) };
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        loading.set(true);
        err.set(None);
        leptos::task::spawn_local(async move {
            match create_dimension(
                &base, &token, &cid, n.trim(), s.trim(), &dt, desc_opt.as_deref(),
            ).await {
                Ok(_) => {
                    open.set(false);
                    name.set(String::new());
                    sql_val.set(String::new());
                    description_val.set(String::new());
                    on_created.run(());
                }
                Err(e) => err.set(Some(e.message)),
            }
            loading.set(false);
        });
    };

    view! {
        <Dialog open=open>
            <DialogContent>
                <DialogHeader>
                    <DialogTitle>"Add dimension"</DialogTitle>
                </DialogHeader>
                <div class="space-y-4 py-2">
                    {move || err.get().map(|m| view! {
                        <Alert variant=AlertVariant::Destructive>
                            <AlertDescription>{m}</AlertDescription>
                        </Alert>
                    })}
                    <div class="space-y-1">
                        <Label>"Name"</Label>
                        <Input value=name placeholder="status".to_string() />
                    </div>
                    <div class="space-y-1">
                        <Label>"SQL expression"</Label>
                        <Input value=sql_val placeholder="{CUBE}.status".to_string() />
                    </div>
                    <div class="space-y-1">
                        <Label>"Type"</Label>
                        <select
                            class=SELECT_CLS
                            on:change=move |e| data_type.set(event_target_value(&e))
                        >
                            <option value="string">"string"</option>
                            <option value="number">"number"</option>
                            <option value="time">"time"</option>
                            <option value="boolean">"boolean"</option>
                        </select>
                    </div>
                    <div class="space-y-1">
                        <Label>"Description (optional)"</Label>
                        <Input value=description_val placeholder="Current order status".to_string() />
                    </div>
                </div>
                <DialogFooter>
                    <Button variant=ButtonVariant::Outline on:click=move |_| open.set(false)>
                        "Cancel"
                    </Button>
                    {move || {
                        let is_loading = loading.get();
                        view! {
                            <Button disabled=is_loading on:click=on_submit>
                                {if is_loading { view! { <Spinner /> }.into_any() }
                                 else { view! { "Add" }.into_any() }}
                            </Button>
                        }
                    }}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}

// ── Add join dialog ────────────────────────────────────────────────────────────

#[component]
fn AddJoinDialog(
    open: RwSignal<bool>,
    cube_id: Signal<String>,
    all_cubes: Signal<Vec<FullCube>>,
    on_created: Callback<()>,
) -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");
    let name = RwSignal::new(String::new());
    let target_cube_id = RwSignal::new(String::new());
    let relationship = RwSignal::new("many_to_one".to_string());
    let sql_val = RwSignal::new(String::new());
    let err = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(false);

    let on_submit = move |_: web_sys::MouseEvent| {
        let n = name.get();
        let tcid = target_cube_id.get();
        let cid = cube_id.get();
        let s = sql_val.get();
        if n.trim().is_empty() { err.set(Some("Name is required.".into())); return; }
        if tcid.trim().is_empty() { err.set(Some("Target cube is required.".into())); return; }
        if s.trim().is_empty() { err.set(Some("SQL ON expression is required.".into())); return; }
        let rel = relationship.get();
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        loading.set(true);
        err.set(None);
        leptos::task::spawn_local(async move {
            match crate::api::create_join(&base, &token, &cid, &tcid, n.trim(), &rel, s.trim()).await {
                Ok(_) => {
                    open.set(false);
                    name.set(String::new());
                    sql_val.set(String::new());
                    on_created.run(());
                }
                Err(e) => err.set(Some(e.message)),
            }
            loading.set(false);
        });
    };

    view! {
        <Dialog open=open>
            <DialogContent>
                <DialogHeader>
                    <DialogTitle>"Add join"</DialogTitle>
                </DialogHeader>
                <div class="space-y-4 py-2">
                    {move || err.get().map(|m| view! {
                        <Alert variant=AlertVariant::Destructive>
                            <AlertDescription>{m}</AlertDescription>
                        </Alert>
                    })}
                    <div class="space-y-1">
                        <Label>"Join name"</Label>
                        <Input value=name placeholder="customers".to_string() />
                    </div>
                    <div class="space-y-1">
                        <Label>"Target cube"</Label>
                        <select
                            class=SELECT_CLS
                            on:change=move |e| target_cube_id.set(event_target_value(&e))
                        >
                            <option value="">"-- pick target cube --"</option>
                            {move || all_cubes.get().into_iter()
                                .filter(|c| c.id != cube_id.get())
                                .map(|c| {
                                    let id = c.id.clone();
                                    let label = c.title.clone().unwrap_or_else(|| c.name.clone());
                                    view! { <option value=id>{label}</option> }
                                }).collect::<Vec<_>>()}
                        </select>
                    </div>
                    <div class="space-y-1">
                        <Label>"Relationship"</Label>
                        <select
                            class=SELECT_CLS
                            on:change=move |e| relationship.set(event_target_value(&e))
                        >
                            <option value="many_to_one">"many_to_one"</option>
                            <option value="one_to_many">"one_to_many"</option>
                            <option value="one_to_one">"one_to_one"</option>
                        </select>
                    </div>
                    <div class="space-y-1">
                        <Label>"SQL ON expression"</Label>
                        <Input value=sql_val placeholder="{CUBE}.customer_id = {customers}.id".to_string() />
                    </div>
                </div>
                <DialogFooter>
                    <Button variant=ButtonVariant::Outline on:click=move |_| open.set(false)>
                        "Cancel"
                    </Button>
                    {move || {
                        let is_loading = loading.get();
                        view! {
                            <Button disabled=is_loading on:click=on_submit>
                                {if is_loading { view! { <Spinner /> }.into_any() }
                                 else { view! { "Add" }.into_any() }}
                            </Button>
                        }
                    }}
                </DialogFooter>
            </DialogContent>
        </Dialog>
    }
}

// ── Delete confirm dialog ──────────────────────────────────────────────────────

#[component]
fn DeleteConfirm(
    open: RwSignal<bool>,
    label: Signal<String>,
    on_confirm: Callback<()>,
) -> impl IntoView {
    view! {
        <AlertDialog open=open>
            <AlertDialogContent>
                <AlertDialogHeader>
                    <AlertDialogTitle>"Are you sure?"</AlertDialogTitle>
                    <AlertDialogDescription>
                        {move || format!("This will permanently delete \"{}\".", label.get())}
                    </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                    <AlertDialogCancel>"Cancel"</AlertDialogCancel>
                    <AlertDialogAction on_click=on_confirm>"Delete"</AlertDialogAction>
                </AlertDialogFooter>
            </AlertDialogContent>
        </AlertDialog>
    }
}

// ── Cube detail panel ──────────────────────────────────────────────────────────

#[component]
fn CubeDetail(
    cube: Signal<Option<FullCube>>,
    all_cubes: Signal<Vec<FullCube>>,
    on_reload: Callback<()>,
    on_cube_deleted: Callback<()>,
) -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");

    // Dialog open signals
    let add_measure_open = RwSignal::new(false);
    let add_dimension_open = RwSignal::new(false);
    let add_join_open = RwSignal::new(false);
    let delete_cube_open = RwSignal::new(false);

    // Child delete state
    let del_meas_open = RwSignal::new(false);
    let del_meas_id = RwSignal::new(String::new());
    let del_meas_name = RwSignal::new(String::new());

    let del_dim_open = RwSignal::new(false);
    let del_dim_id = RwSignal::new(String::new());
    let del_dim_name = RwSignal::new(String::new());

    let del_join_open = RwSignal::new(false);
    let del_join_id = RwSignal::new(String::new());
    let del_join_name = RwSignal::new(String::new());

    let action_err = RwSignal::new(Option::<String>::None);

    // Derived cube_id signal
    let cube_id = Signal::derive(move || cube.get().map(|c| c.id).unwrap_or_default());

    // Delete cube label
    let delete_cube_label = Signal::derive(move || {
        cube.get().map(|c| c.name).unwrap_or_default()
    });

    let on_delete_cube = Callback::new(move |_: ()| {
        let cid = cube_id.get();
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        action_err.set(None);
        leptos::task::spawn_local(async move {
            match delete_cube(&base, &token, &cid).await {
                Ok(()) => on_cube_deleted.run(()),
                Err(e) => action_err.set(Some(e.message)),
            }
        });
    });

    let del_meas_label = Signal::derive(move || del_meas_name.get());
    let on_delete_meas = Callback::new(move |_: ()| {
        let cid = cube_id.get();
        let mid = del_meas_id.get();
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        action_err.set(None);
        leptos::task::spawn_local(async move {
            match delete_measure(&base, &token, &cid, &mid).await {
                Ok(()) => on_reload.run(()),
                Err(e) => action_err.set(Some(e.message)),
            }
        });
    });

    let del_dim_label = Signal::derive(move || del_dim_name.get());
    let on_delete_dim = Callback::new(move |_: ()| {
        let cid = cube_id.get();
        let did = del_dim_id.get();
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        action_err.set(None);
        leptos::task::spawn_local(async move {
            match delete_dimension(&base, &token, &cid, &did).await {
                Ok(()) => on_reload.run(()),
                Err(e) => action_err.set(Some(e.message)),
            }
        });
    });

    let del_join_label = Signal::derive(move || del_join_name.get());
    let on_delete_join = Callback::new(move |_: ()| {
        let cid = cube_id.get();
        let jid = del_join_id.get();
        let base = ctx.api_base.get();
        let token = ctx.token.get();
        action_err.set(None);
        leptos::task::spawn_local(async move {
            match crate::api::delete_join(&base, &token, &cid, &jid).await {
                Ok(()) => on_reload.run(()),
                Err(e) => action_err.set(Some(e.message)),
            }
        });
    });

    view! {
        <div>
            <AddMeasureDialog
                open=add_measure_open
                cube_id=cube_id
                on_created=Callback::new(move |_: ()| on_reload.run(()))
            />
            <AddDimensionDialog
                open=add_dimension_open
                cube_id=cube_id
                on_created=Callback::new(move |_: ()| on_reload.run(()))
            />
            <AddJoinDialog
                open=add_join_open
                cube_id=cube_id
                all_cubes=all_cubes
                on_created=Callback::new(move |_: ()| on_reload.run(()))
            />
            <DeleteConfirm open=delete_cube_open label=delete_cube_label on_confirm=on_delete_cube />
            <DeleteConfirm open=del_meas_open label=del_meas_label on_confirm=on_delete_meas />
            <DeleteConfirm open=del_dim_open label=del_dim_label on_confirm=on_delete_dim />
            <DeleteConfirm open=del_join_open label=del_join_label on_confirm=on_delete_join />

            {move || {
                let Some(c) = cube.get() else {
                    return view! {
                        <div class="flex items-center justify-center h-64 text-muted-foreground text-sm">
                            "Select a cube from the left panel."
                        </div>
                    }.into_any();
                };

                let cube_name = c.name.clone();
                let cube_title = c.title.clone();
                let cube_desc = c.description.clone();
                let source = c.sql_table.clone().unwrap_or_else(|| "base_sql".into());
                let pk = c.primary_key.clone();
                let tc = c.tenant_column.clone();
                let measures = c.measures.clone();
                let dimensions = c.dimensions.clone();
                let joins = c.joins.clone();

                view! {
                    <div class="space-y-4">
                        {move || action_err.get().map(|m| view! {
                            <Alert variant=AlertVariant::Destructive>
                                <AlertTitle>"Error"</AlertTitle>
                                <AlertDescription>{m}</AlertDescription>
                            </Alert>
                        })}

                        // Cube header
                        <Card>
                            <CardHeader>
                                <div class="flex items-start justify-between gap-2">
                                    <div>
                                        <CardTitle class="font-mono text-base".to_string()>
                                            {cube_name.clone()}
                                        </CardTitle>
                                        {cube_title.map(|t| view! {
                                            <p class="text-sm text-muted-foreground">{t}</p>
                                        })}
                                        {cube_desc.map(|d| view! {
                                            <p class="text-xs text-muted-foreground mt-1">{d}</p>
                                        })}
                                    </div>
                                    <Button
                                        variant=ButtonVariant::Destructive
                                        on:click=move |_| delete_cube_open.set(true)
                                    >
                                        " Delete"
                                    </Button>
                                </div>
                                <div class="flex gap-2 flex-wrap mt-2">
                                    <Badge variant=BadgeVariant::Outline>
                                        <span class="font-mono text-xs">{format!("source: {}", source)}</span>
                                    </Badge>
                                    <Badge variant=BadgeVariant::Outline>
                                        <span class="font-mono text-xs">{format!("pk: {}", pk)}</span>
                                    </Badge>
                                    <Badge variant=BadgeVariant::Outline>
                                        <span class="font-mono text-xs">{format!("tenant: {}", tc)}</span>
                                    </Badge>
                                </div>
                            </CardHeader>
                        </Card>

                        // Measures
                        <Card>
                            <CardHeader class="pb-2".to_string()>
                                <div class="flex items-center justify-between">
                                    <CardTitle class="text-sm font-semibold uppercase tracking-wide text-muted-foreground".to_string()>
                                        "Measures"
                                    </CardTitle>
                                    <Button variant=ButtonVariant::Outline on:click=move |_| add_measure_open.set(true)>
                                        {plus_icon()} "Add"
                                    </Button>
                                </div>
                            </CardHeader>
                            <CardContent class="pt-0".to_string()>
                                {if measures.is_empty() {
                                    view! {
                                        <p class="text-xs text-muted-foreground py-2">"No measures yet."</p>
                                    }.into_any()
                                } else {
                                    let rows = measures.into_iter().map(|m| {
                                        let mid = m.id.clone();
                                        let mname = m.name.clone();
                                        let agg = m.agg_type.clone();
                                        let desc = m.description.clone();
                                        let mname2 = mname.clone();
                                        view! {
                                            <div class="flex items-center justify-between gap-2 py-1.5 border-b border-border/40 last:border-0">
                                                <div class="flex flex-col min-w-0">
                                                    <span class="text-sm font-mono truncate">{mname}</span>
                                                    {desc.map(|d| view! {
                                                        <span class="text-xs text-muted-foreground truncate">{d}</span>
                                                    })}
                                                </div>
                                                <div class="flex items-center gap-2 shrink-0">
                                                    <Badge variant=BadgeVariant::Default>{agg}</Badge>
                                                    <button
                                                        type="button"
                                                        class="p-1 text-muted-foreground hover:text-destructive transition-colors"
                                                        on:click=move |_| {
                                                            del_meas_id.set(mid.clone());
                                                            del_meas_name.set(mname2.clone());
                                                            del_meas_open.set(true);
                                                        }
                                                    >
                                                        {trash_icon()}
                                                    </button>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>();
                                    view! { <div>{rows}</div> }.into_any()
                                }}
                            </CardContent>
                        </Card>

                        // Dimensions
                        <Card>
                            <CardHeader class="pb-2".to_string()>
                                <div class="flex items-center justify-between">
                                    <CardTitle class="text-sm font-semibold uppercase tracking-wide text-muted-foreground".to_string()>
                                        "Dimensions"
                                    </CardTitle>
                                    <Button variant=ButtonVariant::Outline on:click=move |_| add_dimension_open.set(true)>
                                        {plus_icon()} "Add"
                                    </Button>
                                </div>
                            </CardHeader>
                            <CardContent class="pt-0".to_string()>
                                {if dimensions.is_empty() {
                                    view! {
                                        <p class="text-xs text-muted-foreground py-2">"No dimensions yet."</p>
                                    }.into_any()
                                } else {
                                    let rows = dimensions.into_iter().map(|d| {
                                        let did = d.id.clone();
                                        let dname = d.name.clone();
                                        let dtype = d.data_type.clone();
                                        let desc = d.description.clone();
                                        let dname2 = dname.clone();
                                        let badge_var = match dtype.as_str() {
                                            "time" => BadgeVariant::Default,
                                            "number" => BadgeVariant::Secondary,
                                            _ => BadgeVariant::Outline,
                                        };
                                        view! {
                                            <div class="flex items-center justify-between gap-2 py-1.5 border-b border-border/40 last:border-0">
                                                <div class="flex flex-col min-w-0">
                                                    <span class="text-sm font-mono truncate">{dname}</span>
                                                    {desc.map(|d| view! {
                                                        <span class="text-xs text-muted-foreground truncate">{d}</span>
                                                    })}
                                                </div>
                                                <div class="flex items-center gap-2 shrink-0">
                                                    <Badge variant=badge_var>{dtype}</Badge>
                                                    <button
                                                        type="button"
                                                        class="p-1 text-muted-foreground hover:text-destructive transition-colors"
                                                        on:click=move |_| {
                                                            del_dim_id.set(did.clone());
                                                            del_dim_name.set(dname2.clone());
                                                            del_dim_open.set(true);
                                                        }
                                                    >
                                                        {trash_icon()}
                                                    </button>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>();
                                    view! { <div>{rows}</div> }.into_any()
                                }}
                            </CardContent>
                        </Card>

                        // Joins
                        <Card>
                            <CardHeader class="pb-2".to_string()>
                                <div class="flex items-center justify-between">
                                    <CardTitle class="text-sm font-semibold uppercase tracking-wide text-muted-foreground".to_string()>
                                        "Joins"
                                    </CardTitle>
                                    <Button variant=ButtonVariant::Outline on:click=move |_| add_join_open.set(true)>
                                        {plus_icon()} "Add"
                                    </Button>
                                </div>
                            </CardHeader>
                            <CardContent class="pt-0".to_string()>
                                {if joins.is_empty() {
                                    view! {
                                        <p class="text-xs text-muted-foreground py-2">"No joins yet."</p>
                                    }.into_any()
                                } else {
                                    let rows = joins.into_iter().map(|j| {
                                        let jid = j.id.clone();
                                        let jname = j.name.clone();
                                        let target = j.target_cube.clone();
                                        let rel = j.relationship.clone();
                                        let jname2 = jname.clone();
                                        view! {
                                            <div class="flex items-center justify-between gap-2 py-1.5 border-b border-border/40 last:border-0">
                                                <div class="flex flex-col min-w-0">
                                                    <span class="text-sm font-mono truncate">
                                                        {format!("{} \u{2192} {}", jname, target)}
                                                    </span>
                                                    <span class="text-xs text-muted-foreground">{rel}</span>
                                                </div>
                                                <button
                                                    type="button"
                                                    class="p-1 text-muted-foreground hover:text-destructive transition-colors shrink-0"
                                                    on:click=move |_| {
                                                        del_join_id.set(jid.clone());
                                                        del_join_name.set(jname2.clone());
                                                        del_join_open.set(true);
                                                    }
                                                >
                                                    {trash_icon()}
                                                </button>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>();
                                    view! { <div>{rows}</div> }.into_any()
                                }}
                            </CardContent>
                        </Card>
                    </div>
                }.into_any()
            }}
        </div>
    }
}

// ── EditorPage ─────────────────────────────────────────────────────────────────

#[component]
pub fn EditorPage() -> impl IntoView {
    let ctx = use_context::<AppCtx>().expect("AppCtx required");

    let model: RwSignal<Option<FullModel>> = RwSignal::new(None);
    let data_sources: RwSignal<Vec<DataSourceItem>> = RwSignal::new(vec![]);
    let model_err: RwSignal<Option<String>> = RwSignal::new(None);
    let model_loading = RwSignal::new(true);
    let selected_cube_id: RwSignal<Option<String>> = RwSignal::new(None);

    let new_ds_open = RwSignal::new(false);
    let new_cube_open = RwSignal::new(false);

    // Reload helper — loads model + data sources in parallel.
    let reload = move || {
        let token = ctx.token.get();
        let base = ctx.api_base.get();
        model_loading.set(true);
        model_err.set(None);
        leptos::task::spawn_local(async move {
            let model_res = fetch_model(&base, &token).await;
            let ds_res = list_data_sources(&base, &token).await;
            match model_res {
                Ok(m) => model.set(Some(m)),
                Err(e) => model_err.set(Some(e.message)),
            }
            if let Ok(ds) = ds_res {
                data_sources.set(ds);
            }
            model_loading.set(false);
        });
    };

    // Reload on token/base change.
    Effect::new(move |_| {
        let _ = ctx.token.get();
        let _ = ctx.api_base.get();
        reload();
    });

    let all_cubes = Signal::derive(move || {
        model.get().map(|m| m.cubes).unwrap_or_default()
    });

    let selected_cube = Signal::derive(move || -> Option<FullCube> {
        let id = selected_cube_id.get()?;
        model.get()?.cubes.into_iter().find(|c| c.id == id)
    });

    let ds_signal = Signal::derive(move || data_sources.get());

    view! {
        <div class="space-y-4">
            <PageHeader title="Model Editor".to_string() subtitle=Some("Visual cube, measure, and dimension editor".to_string())>
                <span class="text-xs text-muted-foreground">"GET /api/v1/model"</span>
            </PageHeader>

            <NewDataSourceDialog
                open=new_ds_open
                on_created=Callback::new(move |_: ()| reload())
            />
            <NewCubeDialog
                open=new_cube_open
                data_sources=ds_signal
                on_created=Callback::new(move |_: ()| reload())
            />

            {move || model_err.get().map(|msg| view! {
                <Alert variant=AlertVariant::Destructive>
                    <AlertTitle>"Could not load model"</AlertTitle>
                    <AlertDescription>{msg}</AlertDescription>
                </Alert>
            })}

            {move || model_loading.get().then(|| view! {
                <div class="flex justify-center py-12"><Spinner /></div>
            })}

            {move || {
                if model_loading.get() { return ().into_any(); }
                view! {
                    <div class="grid grid-cols-1 xl:grid-cols-[280px_1fr] gap-4 items-start">

                        // LEFT: cube list + data sources
                        <div class="space-y-3">
                            <Card>
                                <CardHeader class="pb-2".to_string()>
                                    <div class="flex items-center justify-between">
                                        <CardTitle class="text-sm font-semibold".to_string()>"Cubes"</CardTitle>
                                        <Button variant=ButtonVariant::Outline on:click=move |_| new_cube_open.set(true)>
                                            {plus_icon()} "New"
                                        </Button>
                                    </div>
                                </CardHeader>
                                <CardContent class="pt-0".to_string()>
                                    {move || {
                                        let cubes = all_cubes.get();
                                        if cubes.is_empty() {
                                            return view! {
                                                <Empty
                                                    title="No cubes".to_string()
                                                    description="Create your first cube.".to_string()
                                                />
                                            }.into_any();
                                        }
                                        cubes.into_iter().map(|c| {
                                            let id = c.id.clone();
                                            let label = c.title.clone().unwrap_or_else(|| c.name.clone());
                                            let id_for_check = id.clone();
                                            let is_active = Signal::derive(move || {
                                                selected_cube_id.get().as_deref() == Some(&id_for_check)
                                            });
                                            view! {
                                                <button
                                                    type="button"
                                                    class="w-full text-left px-3 py-2 rounded-md text-sm font-mono transition-colors hover:bg-accent"
                                                    class:bg-accent=is_active
                                                    class:text-accent-foreground=is_active
                                                    on:click=move |_| selected_cube_id.set(Some(id.clone()))
                                                >
                                                    {label}
                                                </button>
                                            }
                                        }).collect::<Vec<_>>().into_any()
                                    }}
                                </CardContent>
                            </Card>

                            <Card>
                                <CardHeader class="pb-2".to_string()>
                                    <div class="flex items-center justify-between">
                                        <CardTitle class="text-sm font-semibold".to_string()>"Data sources"</CardTitle>
                                        <Button variant=ButtonVariant::Outline on:click=move |_| new_ds_open.set(true)>
                                            {plus_icon()} "New"
                                        </Button>
                                    </div>
                                </CardHeader>
                                <CardContent class="pt-0".to_string()>
                                    {move || {
                                        let ds = data_sources.get();
                                        if ds.is_empty() {
                                            return view! {
                                                <p class="text-xs text-muted-foreground py-1">"No data sources yet."</p>
                                            }.into_any();
                                        }
                                        ds.into_iter().map(|d| {
                                            let name = d.name.clone();
                                            let driver = d.driver.clone();
                                            view! {
                                                <div class="flex items-center justify-between py-1.5 border-b border-border/40 last:border-0">
                                                    <span class="text-sm font-mono truncate">{name}</span>
                                                    <Badge variant=BadgeVariant::Outline>
                                                        <span class="text-xs">{driver}</span>
                                                    </Badge>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>().into_any()
                                    }}
                                </CardContent>
                            </Card>
                        </div>

                        // RIGHT: cube detail
                        <div class="min-h-[400px]">
                            <CubeDetail
                                cube=selected_cube
                                all_cubes=all_cubes
                                on_reload=Callback::new(move |_: ()| reload())
                                on_cube_deleted=Callback::new(move |_: ()| {
                                    selected_cube_id.set(None);
                                    reload();
                                })
                            />
                        </div>
                    </div>
                }.into_any()
            }}
        </div>
    }
}
