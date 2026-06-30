//! SVG relationship diagram for the semantic model — Power BI Model View style.
//!
//! Renders each cube as a box, with relationship lines between joined cubes.
//! Layout: deterministic grid (ceil(sqrt(n)) columns), fixed box width, variable height.

use crate::api::FullCube;
use leptos::prelude::*;

// ── Layout constants ──────────────────────────────────────────────────────────

const BOX_WIDTH: f64 = 220.0;
const HEADER_HEIGHT: f64 = 52.0; // name + source line
const ROW_HEIGHT: f64 = 22.0;
const MAX_VISIBLE_FIELDS: usize = 6;
const GAP_H: f64 = 80.0; // horizontal gap between boxes
const GAP_V: f64 = 60.0; // vertical gap between boxes
const MARGIN: f64 = 40.0;
const CORNER_RADIUS: f64 = 6.0;

// ── Box geometry ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct BoxLayout {
    name: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl BoxLayout {
    fn center_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
    fn center_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
    fn right(&self) -> f64 {
        self.x + self.width
    }
    fn bottom(&self) -> f64 {
        self.y + self.height
    }
}

fn box_height(cube: &FullCube) -> f64 {
    let total_fields = cube.measures.len() + cube.dimensions.len();
    let visible = total_fields.min(MAX_VISIBLE_FIELDS);
    // section label rows: +1 if has measures, +1 if has dimensions
    let section_rows = if !cube.measures.is_empty() { 1 } else { 0 }
        + if !cube.dimensions.is_empty() { 1 } else { 0 };
    let has_more = total_fields > MAX_VISIBLE_FIELDS;
    let extra = if has_more { 1 } else { 0 };
    HEADER_HEIGHT + (visible as f64 + section_rows as f64 + extra as f64) * ROW_HEIGHT + 12.0
}

fn compute_layouts(cubes: &[FullCube]) -> Vec<BoxLayout> {
    let n = cubes.len();
    if n == 0 {
        return vec![];
    }
    let cols = (n as f64).sqrt().ceil() as usize;
    cubes
        .iter()
        .enumerate()
        .map(|(i, cube)| {
            let col = i % cols;
            let row = i / cols;
            let x = MARGIN + col as f64 * (BOX_WIDTH + GAP_H);
            let y = MARGIN + row as f64 * (box_height(cube) + GAP_V);
            BoxLayout {
                name: cube.name.clone(),
                x,
                y,
                width: BOX_WIDTH,
                height: box_height(cube),
            }
        })
        .collect()
}

fn total_svg_size(layouts: &[BoxLayout]) -> (f64, f64) {
    if layouts.is_empty() {
        return (400.0, 300.0);
    }
    let w = layouts
        .iter()
        .map(|b| b.right())
        .fold(0.0_f64, f64::max)
        + MARGIN;
    let h = layouts
        .iter()
        .map(|b| b.bottom())
        .fold(0.0_f64, f64::max)
        + MARGIN;
    (w, h)
}

// ── Edge routing: nearest-edge midpoint ───────────────────────────────────────

/// Returns (x1,y1) on src edge and (x2,y2) on tgt edge, picking horizontal
/// or vertical attachment based on which axis has more separation.
fn edge_points(src: &BoxLayout, tgt: &BoxLayout) -> (f64, f64, f64, f64) {
    let sc = (src.center_x(), src.center_y());
    let tc = (tgt.center_x(), tgt.center_y());
    let dx = tc.0 - sc.0;
    let dy = tc.1 - sc.1;

    // Decide which side of src to exit from and which side of tgt to enter
    let (x1, y1) = if dx.abs() >= dy.abs() {
        // horizontal dominant
        if dx > 0.0 {
            (src.right(), sc.1)
        } else {
            (src.x, sc.1)
        }
    } else {
        if dy > 0.0 {
            (sc.0, src.bottom())
        } else {
            (sc.0, src.y)
        }
    };

    let (x2, y2) = if dx.abs() >= dy.abs() {
        if dx > 0.0 {
            (tgt.x, tc.1)
        } else {
            (tgt.right(), tc.1)
        }
    } else {
        if dy > 0.0 {
            (tc.0, tgt.y)
        } else {
            (tc.0, tgt.bottom())
        }
    };

    (x1, y1, x2, y2)
}

fn cardinality_label(rel: &str) -> &'static str {
    match rel {
        "many_to_one" => "∗──1",
        "one_to_many" => "1──∗",
        "one_to_one" => "1──1",
        _ => "──",
    }
}

// ── Field row helpers ─────────────────────────────────────────────────────────

/// Collect (glyph, name) pairs for the visible field rows.
fn field_rows(cube: &FullCube) -> (Vec<(String, String)>, Vec<(String, String)>, usize) {
    let measure_rows: Vec<(String, String)> = cube
        .measures
        .iter()
        .map(|m| ("Σ".to_string(), m.name.clone()))
        .collect();
    let dim_rows: Vec<(String, String)> = cube
        .dimensions
        .iter()
        .map(|d| {
            let glyph = match d.data_type.as_str() {
                "time" => "T".to_string(),
                "number" => "#".to_string(),
                "boolean" => "B".to_string(),
                _ => "A".to_string(),
            };
            (glyph, d.name.clone())
        })
        .collect();
    let total = measure_rows.len() + dim_rows.len();
    let hidden = total.saturating_sub(MAX_VISIBLE_FIELDS);
    (measure_rows, dim_rows, hidden)
}

// ── SVG sub-components ────────────────────────────────────────────────────────

/// A single cube box rendered as SVG elements.
#[component]
fn CubeBox(
    cube: FullCube,
    layout: BoxLayout,
    hovered: RwSignal<Option<String>>,
    is_highlighted: Signal<bool>,
    is_dimmed: Signal<bool>,
) -> impl IntoView {
    let name = cube.name.clone();
    let title = cube.title.clone();
    let source = cube
        .sql_table
        .clone()
        .unwrap_or_else(|| "SQL".to_string());

    let (measure_rows, dim_rows, hidden_count) = field_rows(&cube);

    let x = layout.x;
    let y = layout.y;
    let w = layout.width;
    let h = layout.height;

    let name_hover = name.clone();
    let name_leave = name.clone();

    // Field rows: measures first, then dims, capped at MAX_VISIBLE_FIELDS
    let mut all_rows: Vec<(String, String, bool)> = vec![]; // (glyph, text, is_section_label)
    if !measure_rows.is_empty() {
        all_rows.push(("".to_string(), "MEASURES".to_string(), true));
        for (g, n) in &measure_rows {
            all_rows.push((g.clone(), n.clone(), false));
        }
    }
    if !dim_rows.is_empty() {
        all_rows.push(("".to_string(), "DIMENSIONS".to_string(), true));
        for (g, n) in &dim_rows {
            all_rows.push((g.clone(), n.clone(), false));
        }
    }

    // Count visible non-section rows
    let mut visible_rows: Vec<(String, String, bool)> = vec![];
    let mut field_count = 0usize;
    for row in &all_rows {
        if row.2 {
            // section label — always include if there's space
            visible_rows.push(row.clone());
        } else {
            if field_count < MAX_VISIBLE_FIELDS {
                visible_rows.push(row.clone());
                field_count += 1;
            }
        }
    }

    let rows_view: Vec<_> = visible_rows
        .iter()
        .enumerate()
        .map(|(i, (glyph, text, is_section))| {
            let row_y = y + HEADER_HEIGHT + i as f64 * ROW_HEIGHT + ROW_HEIGHT / 2.0 + 4.0;
            let text_x = x + 12.0;
            if *is_section {
                let section_text = text.clone();
                view! {
                    <text
                        x=format!("{:.1}", text_x)
                        y=format!("{:.1}", row_y)
                        font-size="9"
                        font-weight="600"
                        letter-spacing="0.08em"
                        fill="var(--muted-foreground, #888)"
                        font-family="system-ui,sans-serif"
                    >
                        {section_text}
                    </text>
                }
                .into_any()
            } else {
                let g = glyph.clone();
                let t = text.clone();
                view! {
                    <text
                        x=format!("{:.1}", text_x)
                        y=format!("{:.1}", row_y)
                        font-size="11"
                        fill="var(--foreground, #222)"
                        font-family="monospace,ui-monospace"
                    >
                        <tspan fill="var(--muted-foreground, #888)" font-size="10">{g}" "</tspan>
                        {t}
                    </text>
                }
                .into_any()
            }
        })
        .collect();

    let more_label = if hidden_count > 0 {
        let more_y = y + HEADER_HEIGHT + visible_rows.len() as f64 * ROW_HEIGHT + ROW_HEIGHT / 2.0 + 4.0;
        Some(view! {
            <text
                x=format!("{:.1}", x + 12.0)
                y=format!("{:.1}", more_y)
                font-size="10"
                fill="var(--muted-foreground, #888)"
                font-family="system-ui,sans-serif"
                font-style="italic"
            >
                {format!("+{} more", hidden_count)}
            </text>
        })
    } else {
        None
    };

    let display_name = title.unwrap_or_else(|| name.clone());

    view! {
        <g
            on:mouseenter=move |_| hovered.set(Some(name_hover.clone()))
            on:mouseleave=move |_| {
                let current = hovered.get_untracked();
                if current.as_deref() == Some(&name_leave) {
                    hovered.set(None);
                }
            }
            style="cursor: default;"
        >
            // Drop shadow
            <rect
                x=format!("{:.1}", x + 3.0)
                y=format!("{:.1}", y + 3.0)
                width=format!("{:.1}", w)
                height=format!("{:.1}", h)
                rx=format!("{:.1}", CORNER_RADIUS)
                fill="rgba(0,0,0,0.06)"
            />
            // Box background
            <rect
                x=format!("{:.1}", x)
                y=format!("{:.1}", y)
                width=format!("{:.1}", w)
                height=format!("{:.1}", h)
                rx=format!("{:.1}", CORNER_RADIUS)
                fill="var(--card, #fff)"
                stroke=move || if is_highlighted.get() { "var(--primary, #6366f1)" } else { "var(--border, #e2e8f0)" }
                stroke-width=move || if is_highlighted.get() { "2" } else { "1" }
                opacity=move || if is_dimmed.get() { "0.3" } else { "1" }
            />
            // Header band
            <rect
                x=format!("{:.1}", x)
                y=format!("{:.1}", y)
                width=format!("{:.1}", w)
                height=format!("{:.1}", HEADER_HEIGHT)
                rx=format!("{:.1}", CORNER_RADIUS)
                fill=move || if is_highlighted.get() { "var(--primary, #6366f1)" } else { "var(--muted, #f1f5f9)" }
                opacity=move || if is_dimmed.get() { "0.3" } else { "1" }
            />
            // Header bottom fill (cover rx at bottom of header)
            <rect
                x=format!("{:.1}", x)
                y=format!("{:.1}", y + HEADER_HEIGHT - CORNER_RADIUS)
                width=format!("{:.1}", w)
                height=format!("{:.1}", CORNER_RADIUS)
                fill=move || if is_highlighted.get() { "var(--primary, #6366f1)" } else { "var(--muted, #f1f5f9)" }
                opacity=move || if is_dimmed.get() { "0.3" } else { "1" }
            />
            // Cube name (title or name)
            <text
                x=format!("{:.1}", x + 12.0)
                y=format!("{:.1}", y + 22.0)
                font-size="13"
                font-weight="700"
                fill=move || if is_highlighted.get() { "var(--primary-foreground, #fff)" } else { "var(--foreground, #111)" }
                font-family="monospace,ui-monospace"
                opacity=move || if is_dimmed.get() { "0.3" } else { "1" }
            >
                {display_name.clone()}
            </text>
            // Source label
            <text
                x=format!("{:.1}", x + 12.0)
                y=format!("{:.1}", y + 38.0)
                font-size="10"
                fill=move || if is_highlighted.get() { "rgba(255,255,255,0.75)" } else { "var(--muted-foreground, #888)" }
                font-family="monospace,ui-monospace"
                opacity=move || if is_dimmed.get() { "0.3" } else { "1" }
            >
                {source.clone()}
            </text>
            // Divider line below header
            <line
                x1=format!("{:.1}", x)
                y1=format!("{:.1}", y + HEADER_HEIGHT)
                x2=format!("{:.1}", x + w)
                y2=format!("{:.1}", y + HEADER_HEIGHT)
                stroke="var(--border, #e2e8f0)"
                stroke-width="1"
                opacity=move || if is_dimmed.get() { "0.3" } else { "1" }
            />
            // Field rows
            <g opacity=move || if is_dimmed.get() { "0.3" } else { "1" }>
                {rows_view}
                {more_label}
            </g>
        </g>
    }
}

// ── Relationship line ─────────────────────────────────────────────────────────

#[component]
fn RelationLine(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    label: String,
    is_active: Signal<bool>,
    is_dimmed: Signal<bool>,
) -> impl IntoView {
    let mid_x = (x1 + x2) / 2.0;
    let mid_y = (y1 + y2) / 2.0;

    view! {
        <g>
            <line
                x1=format!("{:.1}", x1)
                y1=format!("{:.1}", y1)
                x2=format!("{:.1}", x2)
                y2=format!("{:.1}", y2)
                stroke=move || if is_active.get() { "var(--primary, #6366f1)" } else { "var(--muted-foreground, #94a3b8)" }
                stroke-width=move || if is_active.get() { "2.5" } else { "1.5" }
                stroke-dasharray=move || if is_active.get() { "none" } else { "none" }
                opacity=move || if is_dimmed.get() { "0.15" } else { "1" }
                marker-end="url(#arrow)"
            />
            // Cardinality label background
            <rect
                x=format!("{:.1}", mid_x - 20.0)
                y=format!("{:.1}", mid_y - 9.0)
                width="40"
                height="14"
                rx="3"
                fill=move || if is_active.get() { "var(--primary, #6366f1)" } else { "var(--card, #fff)" }
                stroke=move || if is_active.get() { "var(--primary, #6366f1)" } else { "var(--border, #e2e8f0)" }
                stroke-width="1"
                opacity=move || if is_dimmed.get() { "0.15" } else { "1" }
            />
            <text
                x=format!("{:.1}", mid_x)
                y=format!("{:.1}", mid_y + 1.5)
                text-anchor="middle"
                dominant-baseline="middle"
                font-size="9"
                font-family="monospace,ui-monospace"
                fill=move || if is_active.get() { "var(--primary-foreground, #fff)" } else { "var(--muted-foreground, #888)" }
                opacity=move || if is_dimmed.get() { "0.15" } else { "1" }
            >
                {label.clone()}
            </text>
        </g>
    }
}

// ── Main diagram component ────────────────────────────────────────────────────

#[component]
pub fn ModelDiagramView(cubes: Vec<FullCube>) -> impl IntoView {
    let hovered: RwSignal<Option<String>> = RwSignal::new(None);

    if cubes.is_empty() {
        return view! {
            <div class="flex flex-col items-center justify-center py-20 text-muted-foreground gap-2">
                <p class="text-sm">"No cubes in the model yet."</p>
                <p class="text-xs">"Apply a model with soma-cli to see the diagram."</p>
            </div>
        }
        .into_any();
    }

    let layouts = compute_layouts(&cubes);
    let (svg_w, svg_h) = total_svg_size(&layouts);

    // Collect all relationships: (src_name, tgt_name, relationship)
    let edges: Vec<(String, String, String)> = cubes
        .iter()
        .flat_map(|c| {
            c.joins.iter().map(move |j| {
                (
                    c.name.clone(),
                    j.target_cube.clone(),
                    j.relationship.clone(),
                )
            })
        })
        .collect();

    let has_joins = !edges.is_empty();

    // Build lookup: cube name → BoxLayout index
    let layouts_clone = layouts.clone();

    let edge_views: Vec<_> = edges
        .iter()
        .map(|(src, tgt, rel)| {
            let src_layout = layouts_clone.iter().find(|b| &b.name == src).cloned();
            let tgt_layout = layouts_clone.iter().find(|b| &b.name == tgt).cloned();
            let label = cardinality_label(rel).to_string();
            let src_name = src.clone();
            let tgt_name = tgt.clone();

            if let (Some(sl), Some(tl)) = (src_layout, tgt_layout) {
                let (x1, y1, x2, y2) = edge_points(&sl, &tl);
                let is_active = {
                    let s = src_name.clone();
                    let t = tgt_name.clone();
                    Signal::derive(move || {
                        hovered
                            .get()
                            .as_ref()
                            .map(|h| h == &s || h == &t)
                            .unwrap_or(false)
                    })
                };
                let is_dimmed = Signal::derive(move || {
                    hovered
                        .get()
                        .as_ref()
                        .map(|h| h != &src_name && h != &tgt_name)
                        .unwrap_or(false)
                });
                Some(view! {
                    <RelationLine x1 y1 x2 y2 label is_active is_dimmed />
                })
            } else {
                None
            }
        })
        .flatten()
        .collect();

    let box_views: Vec<_> = cubes
        .into_iter()
        .zip(layouts.iter())
        .map(|(cube, layout)| {
            let cube_name = cube.name.clone();
            let cube_name2 = cube_name.clone();
            let layout = layout.clone();

            // Check if this cube is connected to hovered
            let connected_names: Vec<String> = edges
                .iter()
                .filter(|(s, t, _)| s == &cube_name || t == &cube_name)
                .flat_map(|(s, t, _)| [s.clone(), t.clone()])
                .collect();

            let is_highlighted = {
                let cn = cube_name.clone();
                let conn = connected_names.clone();
                Signal::derive(move || {
                    hovered
                        .get()
                        .as_ref()
                        .map(|h| h == &cn || conn.contains(h))
                        .unwrap_or(false)
                })
            };
            let is_dimmed = Signal::derive(move || {
                let h = hovered.get();
                match &h {
                    None => false,
                    Some(hov) => {
                        hov != &cube_name2
                            && !connected_names.contains(hov)
                    }
                }
            });

            view! {
                <CubeBox
                    cube=cube
                    layout=layout
                    hovered=hovered
                    is_highlighted=is_highlighted
                    is_dimmed=is_dimmed
                />
            }
        })
        .collect();

    let no_joins_hint = if !has_joins {
        Some(view! {
            <p class="text-xs text-muted-foreground text-center mt-2">
                "No relationships defined yet — add joins to see connector lines."
            </p>
        })
    } else {
        None
    };

    view! {
        <div class="space-y-3">
            {no_joins_hint}
            // Legend
            <div class="flex items-center gap-4 flex-wrap text-xs text-muted-foreground px-1">
                <span class="font-semibold text-foreground">"Legend:"</span>
                <span>"∗──1  many_to_one"</span>
                <span>"1──∗  one_to_many"</span>
                <span>"1──1  one_to_one"</span>
                <span class="ml-2">"Σ = measure · # = number · T = time · A = string · B = boolean"</span>
            </div>
            // SVG canvas (scrollable)
            <div
                class="overflow-auto rounded-lg border border-border bg-background"
                style="max-height: calc(100vh - 220px);"
            >
                <svg
                    width=format!("{:.0}", svg_w)
                    height=format!("{:.0}", svg_h)
                    viewBox=format!("0 0 {:.0} {:.0}", svg_w, svg_h)
                    xmlns="http://www.w3.org/2000/svg"
                    style="display:block;"
                >
                    // Arrowhead marker
                    <defs>
                        <marker
                            id="arrow"
                            markerWidth="8"
                            markerHeight="8"
                            refX="6"
                            refY="3"
                            orient="auto"
                            markerUnits="strokeWidth"
                        >
                            <path
                                d="M0,0 L0,6 L8,3 z"
                                fill="var(--muted-foreground, #94a3b8)"
                            />
                        </marker>
                    </defs>

                    // Relationship lines (drawn below boxes)
                    {edge_views}

                    // Cube boxes (drawn above lines)
                    {box_views}
                </svg>
            </div>
        </div>
    }
    .into_any()
}
