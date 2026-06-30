//! soma-cli — CLI for soma-analytics.
//!
//! # Examples
//!
//! ```sh
//! # Apply a YAML model
//! soma-cli --url http://localhost:8080 --token sk_... apply model.yaml
//!
//! # Run a query from a JSON file
//! soma-cli query --file q.json
//!
//! # Run a query with inline flags
//! soma-cli query --cube orders --measures orders.count --dimensions orders.status --limit 20
//!
//! # Inspect the governed model
//! soma-cli meta
//!
//! # List cubes
//! soma-cli cubes
//!
//! # Create an API token
//! soma-cli token create --name ci-reader --role reader
//! ```

mod model_yaml;

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use soma_sdk::{
    CreateCubeBody, CreateDimensionBody, CreateJoinBody, CreateMeasureBody, CreateSegmentBody,
    SomaClient,
};
use soma_semantic::SemanticQuery;

// ── CLI definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "soma-cli", about = "soma-analytics CLI")]
struct Cli {
    /// soma-analytics server URL.
    #[arg(long, env = "SOMA_ANALYTICS_URL", default_value = "http://localhost:8080")]
    url: String,

    /// Bearer token (API key).
    #[arg(long, env = "SOMA_ANALYTICS_TOKEN")]
    token: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Apply a YAML model file — creates data sources, cubes, dimensions, measures, joins, segments.
    Apply {
        /// Path to the YAML model file.
        model_file: String,
    },
    /// Run a semantic query and print results as a table.
    Query {
        /// Path to a SemanticQuery JSON file.
        #[arg(long)]
        file: Option<String>,
        /// Cube name (alternative to --file).
        #[arg(long)]
        cube: Option<String>,
        /// Comma-separated measures (e.g. `orders.count,orders.total_revenue`).
        #[arg(long)]
        measures: Option<String>,
        /// Comma-separated dimensions (e.g. `orders.status`).
        #[arg(long)]
        dimensions: Option<String>,
        /// Result row limit.
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Print the governed model (cubes → measures/dimensions with descriptions).
    Meta,
    /// List cube names.
    Cubes,
    /// API token management.
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
}

#[derive(Subcommand)]
enum TokenAction {
    /// Create a new API token (plaintext printed once).
    Create {
        /// Human-readable token name.
        #[arg(long)]
        name: String,
        /// Role: reader | editor | admin.
        #[arg(long, default_value = "reader")]
        role: String,
    },
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = SomaClient::new(&cli.url, &cli.token);

    match cli.cmd {
        Cmd::Apply { model_file } => run_apply(&client, &model_file).await,
        Cmd::Query { file, cube, measures, dimensions, limit } => {
            run_query(&client, file, cube, measures, dimensions, limit).await
        }
        Cmd::Meta => run_meta(&client).await,
        Cmd::Cubes => run_cubes(&client).await,
        Cmd::Token { action: TokenAction::Create { name, role } } => {
            run_token_create(&client, &name, &role).await
        }
    }
}

// ── apply ─────────────────────────────────────────────────────────────────────

async fn run_apply(client: &SomaClient, path: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading model file: {path}"))?;
    let model: model_yaml::ModelFile =
        serde_yaml::from_str(&text).context("parsing YAML model")?;

    let mut ds_ids: HashMap<String, uuid::Uuid> = HashMap::new();
    let mut cube_ids: HashMap<String, uuid::Uuid> = HashMap::new();

    // 1. Data sources
    for ds in &model.data_sources {
        let created = client
            .create_data_source(&ds.name, ds.driver.as_deref())
            .await
            .with_context(|| format!("create_data_source '{}'", ds.name))?;
        println!("created data_source '{}' (id={})", created.name, created.id);
        ds_ids.insert(ds.name.clone(), created.id);
    }

    // 2. Cubes
    for cube in &model.cubes {
        let ds_id = ds_ids.get(&cube.data_source).copied().with_context(|| {
            format!(
                "cube '{}' references unknown data_source '{}'",
                cube.name, cube.data_source
            )
        })?;

        let body = CreateCubeBody {
            data_source_id: ds_id,
            name: cube.name.clone(),
            title: cube.title.clone(),
            description: cube.description.clone(),
            sql_table: cube.sql_table.clone(),
            base_sql: cube.base_sql.clone(),
            primary_key: cube.primary_key.clone(),
            cache_ttl_secs: cube.cache_ttl_secs,
            tenant_column: cube.tenant_column.clone(),
        };
        let created = client
            .create_cube(&body)
            .await
            .with_context(|| format!("create_cube '{}'", cube.name))?;
        println!("created cube '{}' (id={})", created.name, created.id);
        cube_ids.insert(cube.name.clone(), created.id);
    }

    // 3. Dimensions, measures, joins, segments (per cube)
    for cube in &model.cubes {
        let cube_id = *cube_ids
            .get(&cube.name)
            .expect("cube_id present — just inserted");

        for dim in &cube.dimensions {
            let body = CreateDimensionBody {
                name: dim.name.clone(),
                description: dim.description.clone(),
                sql_expr: dim.sql.clone(),
                data_type: dim.data_type.clone(),
            };
            let created = client
                .create_dimension(cube_id, &body)
                .await
                .with_context(|| format!("create_dimension '{}.{}'", cube.name, dim.name))?;
            println!("  + dimension '{}'", created.name);
        }

        for meas in &cube.measures {
            let body = CreateMeasureBody {
                name: meas.name.clone(),
                description: meas.description.clone(),
                sql_expr: meas.sql.clone(),
                agg_type: meas.agg_type.clone(),
            };
            let created = client
                .create_measure(cube_id, &body)
                .await
                .with_context(|| format!("create_measure '{}.{}'", cube.name, meas.name))?;
            println!("  + measure '{}'", created.name);
        }

        for join in &cube.joins {
            let target_cube_id = *cube_ids.get(&join.target_cube).with_context(|| {
                format!(
                    "join '{}' on cube '{}' references unknown cube '{}'",
                    join.name, cube.name, join.target_cube
                )
            })?;
            let body = CreateJoinBody {
                target_cube_id,
                name: join.name.clone(),
                relationship: join.relationship.clone(),
                sql_on: join.sql.clone(),
            };
            let created = client
                .create_join(cube_id, &body)
                .await
                .with_context(|| format!("create_join '{}.{}'", cube.name, join.name))?;
            println!("  + join '{}'", created.name);
        }

        for seg in &cube.segments {
            let body = CreateSegmentBody {
                name: seg.name.clone(),
                sql_expr: seg.sql.clone(),
            };
            let created = client
                .create_segment(cube_id, &body)
                .await
                .with_context(|| format!("create_segment '{}.{}'", cube.name, seg.name))?;
            println!("  + segment '{}'", created.name);
        }
    }

    println!(
        "\napply complete: {} data source(s), {} cube(s)",
        ds_ids.len(),
        cube_ids.len()
    );
    Ok(())
}

// ── query ─────────────────────────────────────────────────────────────────────

async fn run_query(
    client: &SomaClient,
    file: Option<String>,
    cube: Option<String>,
    measures: Option<String>,
    dimensions: Option<String>,
    limit: Option<u32>,
) -> Result<()> {
    let q: SemanticQuery = if let Some(path) = file {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading query file: {path}"))?;
        serde_json::from_str(&text).context("parsing SemanticQuery JSON")?
    } else {
        let cube = cube.context("--cube is required when --file is not provided")?;
        SemanticQuery {
            cube,
            measures: measures
                .map(|s| s.split(',').map(str::trim).map(str::to_owned).collect())
                .unwrap_or_default(),
            dimensions: dimensions
                .map(|s| s.split(',').map(str::trim).map(str::to_owned).collect())
                .unwrap_or_default(),
            filters: vec![],
            segments: vec![],
            time_dimension: None,
            order: vec![],
            limit,
            offset: None,
        }
    };

    let rs = client.query(&q).await.context("query failed")?;

    print_result_set(&rs);
    eprintln!(
        "\n{} row(s) — cache: {} | fingerprint: {}",
        rs.meta.row_count, rs.meta.cache, rs.meta.query_fingerprint
    );
    Ok(())
}

fn print_result_set(rs: &soma_sdk::ResultSet) {
    if rs.columns.is_empty() {
        println!("(no columns)");
        return;
    }

    // Collect column headers and per-cell strings.
    let headers: Vec<&str> = rs.columns.iter().map(|c| c.name.as_str()).collect();
    let cell_strings: Vec<Vec<String>> = rs
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => "NULL".to_owned(),
                    other => other.to_string(),
                })
                .collect()
        })
        .collect();

    // Compute column widths.
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in &cell_strings {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Print header row.
    let header_line: Vec<String> = headers
        .iter()
        .zip(&widths)
        .map(|(h, &w)| format!("{:width$}", h, width = w))
        .collect();
    println!("{}", header_line.join(" | "));

    // Separator.
    let sep: Vec<String> = widths.iter().map(|&w| "-".repeat(w)).collect();
    println!("{}", sep.join("-+-"));

    // Data rows.
    for row in &cell_strings {
        let cols: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let w = widths.get(i).copied().unwrap_or(cell.len());
                format!("{:width$}", cell, width = w)
            })
            .collect();
        println!("{}", cols.join(" | "));
    }
}

// ── meta ──────────────────────────────────────────────────────────────────────

async fn run_meta(client: &SomaClient) -> Result<()> {
    let meta = client.meta().await.context("meta failed")?;
    for cube in &meta.cubes {
        let desc = cube
            .description
            .as_deref()
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        println!("cube: {}{}", cube.name, desc);

        for m in &cube.measures {
            let desc = m
                .description
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default();
            println!("  measure  {}.{}  [{}]{}", cube.name, m.name, m.agg_type, desc);
        }
        for d in &cube.dimensions {
            let desc = d
                .description
                .as_deref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default();
            println!("  dimension {}.{}  [{}]{}", cube.name, d.name, d.data_type, desc);
        }
    }
    Ok(())
}

// ── cubes ─────────────────────────────────────────────────────────────────────

async fn run_cubes(client: &SomaClient) -> Result<()> {
    let cubes = client.list_cubes().await.context("list_cubes failed")?;
    if cubes.is_empty() {
        println!("(no cubes)");
    } else {
        for name in &cubes {
            println!("{name}");
        }
    }
    Ok(())
}

// ── token create ──────────────────────────────────────────────────────────────

async fn run_token_create(client: &SomaClient, name: &str, role: &str) -> Result<()> {
    match role {
        "reader" | "editor" | "admin" => {}
        other => bail!("invalid role '{other}': must be reader | editor | admin"),
    }
    let resp = client
        .create_token(name, Some(role))
        .await
        .context("create_token failed")?;
    println!("created token '{}' (id={}, role={})", resp.name, resp.id, resp.role);
    println!("token: {}", resp.token);
    println!("Store this value securely — it will not be shown again.");
    Ok(())
}
