//! Load a tenant's model from the metadata tables and convert to `soma_semantic::Model`.

use sqlx::PgPool;
use uuid::Uuid;

use soma_semantic::{AggType, Cube, DataType, Dimension, Join, Measure, Model, Relationship, Segment};

use crate::error::{Error, Result};
use crate::types::{FullCube, FullDimension, FullJoin, FullMeasure, FullModel, FullSegment};

// ── Raw DB row types ──────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct CubeRow {
    id: Uuid,
    name: String,
    data_source_id: Uuid,
    sql_table: Option<String>,
    base_sql: Option<String>,
    primary_key: String,
    tenant_column: String,
    cache_ttl_secs: i32,
    model_version: i32,
}

/// Extended cube row that includes title and description (for `export_model`).
#[derive(sqlx::FromRow)]
struct FullCubeRow {
    id: Uuid,
    name: String,
    title: Option<String>,
    description: Option<String>,
    data_source_id: Uuid,
    sql_table: Option<String>,
    base_sql: Option<String>,
    primary_key: String,
    tenant_column: String,
}

#[derive(sqlx::FromRow)]
struct DimRow {
    id: Uuid,
    cube_id: Uuid,
    name: String,
    sql_expr: String,
    data_type: String,
    description: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MeasureRow {
    id: Uuid,
    cube_id: Uuid,
    name: String,
    sql_expr: Option<String>,
    agg_type: String,
    description: Option<String>,
}

#[derive(sqlx::FromRow)]
struct JoinRow {
    id: Uuid,
    cube_id: Uuid,
    target_cube_id: Uuid,
    name: String,
    relationship: String,
    sql_on: String,
}

#[derive(sqlx::FromRow)]
struct SegmentRow {
    id: Uuid,
    cube_id: Uuid,
    name: String,
    sql_expr: String,
}

// ── CubeVersionInfo ───────────────────────────────────────────────────────────

/// Minimal cube info needed by `run_query` to build the cache key.
pub struct CubeVersionInfo {
    pub model_version: i32,
    pub cache_ttl_secs: i32,
}

/// Load the `model_version` and `cache_ttl_secs` for a specific cube by name.
/// Kept for backwards compatibility; prefer `load_model_and_cube_version` to avoid
/// the extra round-trip.
#[allow(dead_code)]
pub async fn load_cube_version(
    pool: &PgPool,
    tenant_id: Uuid,
    cube_name: &str,
) -> Result<CubeVersionInfo> {
    let row = sqlx::query_as::<_, (i32, i32)>(
        r#"SELECT model_version, cache_ttl_secs
           FROM "soma_analytics"."03_fct_cubes"
           WHERE tenant_id = $1 AND name = $2 AND is_deleted = false"#,
    )
    .bind(tenant_id)
    .bind(cube_name)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((model_version, cache_ttl_secs)) => {
            Ok(CubeVersionInfo { model_version, cache_ttl_secs })
        }
        None => Err(Error::NotFound(format!("cube '{cube_name}'"))),
    }
}

// ── Full model load ───────────────────────────────────────────────────────────

/// Load the full tenant model from the metadata tables.
///
/// Skips `is_deleted = true` rows at every level.
/// Returns an error if `Model::from_cubes` validation fails (bad sql_expr / sql_on).
pub async fn load_model(pool: &PgPool, tenant_id: Uuid) -> Result<Model> {
    let (model, _) = load_model_internal(pool, tenant_id).await?;
    Ok(model)
}

/// Load the full tenant model AND return the `CubeVersionInfo` for the named cube
/// in one set of queries (avoids a separate round-trip for version info).
pub async fn load_model_and_cube_version(
    pool: &PgPool,
    tenant_id: Uuid,
    cube_name: &str,
) -> Result<(Model, CubeVersionInfo)> {
    let (model, cube_rows) = load_model_internal(pool, tenant_id).await?;
    // Extract version info from the already-fetched cube rows.
    let info = cube_rows
        .iter()
        .find(|cr| cr.name == cube_name)
        .map(|cr| CubeVersionInfo {
            model_version: cr.model_version,
            cache_ttl_secs: cr.cache_ttl_secs,
        })
        .ok_or_else(|| Error::NotFound(format!("cube '{cube_name}'")))?;
    Ok((model, info))
}

/// Internal: load cube rows and build a Model. Returns raw cube rows alongside for
/// version-info extraction (avoids re-querying when both model and version are needed).
async fn load_model_internal(pool: &PgPool, tenant_id: Uuid) -> Result<(Model, Vec<CubeRow>)> {
    // Fetch all non-deleted cubes for the tenant.
    let cube_rows: Vec<CubeRow> = sqlx::query_as(
        r#"SELECT id, name, data_source_id, sql_table, base_sql, primary_key,
                  cache_ttl_secs, model_version, tenant_column
           FROM "soma_analytics"."03_fct_cubes"
           WHERE tenant_id = $1 AND is_deleted = false
           ORDER BY name"#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    if cube_rows.is_empty() {
        let model = Model::from_cubes(vec![]).map_err(|e| {
            Error::Compile(soma_semantic::CompileError::UnknownCube(format!("model validation: {e:?}")))
        })?;
        return Ok((model, cube_rows));
    }

    let cube_ids: Vec<Uuid> = cube_rows.iter().map(|r| r.id).collect();

    // Batch-fetch all child entities in one round-trip each.
    let dim_rows: Vec<DimRow> = sqlx::query_as(
        r#"SELECT id, cube_id, name, sql_expr, data_type, description
           FROM "soma_analytics"."04_fct_dimensions"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    let measure_rows: Vec<MeasureRow> = sqlx::query_as(
        r#"SELECT id, cube_id, name, sql_expr, agg_type, description
           FROM "soma_analytics"."05_fct_measures"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    let join_rows: Vec<JoinRow> = sqlx::query_as(
        r#"SELECT id, cube_id, target_cube_id, name, relationship, sql_on
           FROM "soma_analytics"."06_fct_joins"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    let segment_rows: Vec<SegmentRow> = sqlx::query_as(
        r#"SELECT id, cube_id, name, sql_expr
           FROM "soma_analytics"."07_fct_segments"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    // Build a cube_name lookup so join target_cube_id can be resolved to a name.
    let id_to_name: std::collections::HashMap<Uuid, &str> =
        cube_rows.iter().map(|r| (r.id, r.name.as_str())).collect();

    // Assemble cubes.
    let mut cubes: Vec<Cube> = Vec::with_capacity(cube_rows.len());

    for cr in &cube_rows {
        let data_source = cr.data_source_id.to_string(); // used as a handle; the pool is fixed

        let dimensions: Vec<Dimension> = dim_rows
            .iter()
            .filter(|d| d.cube_id == cr.id)
            .map(|d| Dimension {
                name: d.name.clone(),
                sql_expr: d.sql_expr.clone(),
                data_type: parse_data_type(&d.data_type),
                description: d.description.clone(),
            })
            .collect();

        let measures: Vec<Measure> = measure_rows
            .iter()
            .filter(|m| m.cube_id == cr.id)
            .map(|m| Measure {
                name: m.name.clone(),
                sql_expr: m.sql_expr.clone(),
                agg_type: parse_agg_type(&m.agg_type),
                description: m.description.clone(),
            })
            .collect();

        let joins: Vec<Join> = join_rows
            .iter()
            .filter(|j| j.cube_id == cr.id)
            .map(|j| Join {
                name: j.name.clone(),
                target_cube: id_to_name
                    .get(&j.target_cube_id)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| j.target_cube_id.to_string()),
                relationship: parse_relationship(&j.relationship),
                sql_on: j.sql_on.clone(),
            })
            .collect();

        let segments: Vec<Segment> = segment_rows
            .iter()
            .filter(|s| s.cube_id == cr.id)
            .map(|s| Segment { name: s.name.clone(), sql_expr: s.sql_expr.clone() })
            .collect();

        cubes.push(Cube {
            name: cr.name.clone(),
            data_source,
            sql_table: cr.sql_table.clone(),
            base_sql: cr.base_sql.clone(),
            primary_key: cr.primary_key.clone(),
            tenant_column: cr.tenant_column.clone(),
            description: None,
            dimensions,
            measures,
            joins,
            segments,
        });
    }

    let model = Model::from_cubes(cubes).map_err(|e| {
        Error::Compile(soma_semantic::CompileError::UnknownCube(format!("model validation: {e:?}")))
    })?;
    Ok((model, cube_rows))
}

// ── Full model export (for GET /api/v1/model) ─────────────────────────────────

/// Export the full tenant model as a `FullModel` DTO, including title, description,
/// sql_table, base_sql, and all SQL expressions on child entities.
///
/// Reads directly from the metadata tables (does NOT go through `soma_semantic::Model`)
/// so that fields the semantic model omits (title, description) are preserved.
pub async fn export_model(pool: &PgPool, tenant_id: Uuid) -> Result<FullModel> {
    let cube_rows: Vec<FullCubeRow> = sqlx::query_as(
        r#"SELECT id, name, title, description, data_source_id,
                  sql_table, base_sql, primary_key, tenant_column
           FROM "soma_analytics"."03_fct_cubes"
           WHERE tenant_id = $1 AND is_deleted = false
           ORDER BY name"#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;

    if cube_rows.is_empty() {
        return Ok(FullModel { cubes: vec![] });
    }

    let cube_ids: Vec<Uuid> = cube_rows.iter().map(|r| r.id).collect();

    // Batch-fetch all child entities.
    let dim_rows: Vec<DimRow> = sqlx::query_as(
        r#"SELECT id, cube_id, name, sql_expr, data_type, description
           FROM "soma_analytics"."04_fct_dimensions"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    let measure_rows: Vec<MeasureRow> = sqlx::query_as(
        r#"SELECT id, cube_id, name, sql_expr, agg_type, description
           FROM "soma_analytics"."05_fct_measures"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    // Join rows need the target cube name — resolve via a name lookup.
    let join_rows: Vec<JoinRow> = sqlx::query_as(
        r#"SELECT id, cube_id, target_cube_id, name, relationship, sql_on
           FROM "soma_analytics"."06_fct_joins"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    let segment_rows: Vec<SegmentRow> = sqlx::query_as(
        r#"SELECT id, cube_id, name, sql_expr
           FROM "soma_analytics"."07_fct_segments"
           WHERE tenant_id = $1 AND cube_id = ANY($2) AND is_deleted = false
           ORDER BY cube_id, name"#,
    )
    .bind(tenant_id)
    .bind(&cube_ids)
    .fetch_all(pool)
    .await?;

    // Build a cube_id → name map for resolving join target names.
    let id_to_name: std::collections::HashMap<Uuid, &str> =
        cube_rows.iter().map(|r| (r.id, r.name.as_str())).collect();

    let cubes = cube_rows
        .iter()
        .map(|cr| {
            let dimensions = dim_rows
                .iter()
                .filter(|d| d.cube_id == cr.id)
                .map(|d| FullDimension {
                    id: d.id.to_string(),
                    name: d.name.clone(),
                    sql: d.sql_expr.clone(),
                    data_type: d.data_type.clone(),
                    description: d.description.clone(),
                })
                .collect();

            let measures = measure_rows
                .iter()
                .filter(|m| m.cube_id == cr.id)
                .map(|m| FullMeasure {
                    id: m.id.to_string(),
                    name: m.name.clone(),
                    agg_type: m.agg_type.clone(),
                    sql: m.sql_expr.clone(),
                    description: m.description.clone(),
                })
                .collect();

            let joins = join_rows
                .iter()
                .filter(|j| j.cube_id == cr.id)
                .map(|j| FullJoin {
                    id: j.id.to_string(),
                    name: j.name.clone(),
                    target_cube: id_to_name
                        .get(&j.target_cube_id)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| j.target_cube_id.to_string()),
                    relationship: j.relationship.clone(),
                    sql: j.sql_on.clone(),
                })
                .collect();

            let segments = segment_rows
                .iter()
                .filter(|s| s.cube_id == cr.id)
                .map(|s| FullSegment {
                    id: s.id.to_string(),
                    name: s.name.clone(),
                    sql: s.sql_expr.clone(),
                })
                .collect();

            FullCube {
                id: cr.id.to_string(),
                name: cr.name.clone(),
                title: cr.title.clone(),
                description: cr.description.clone(),
                data_source: cr.data_source_id.to_string(),
                sql_table: cr.sql_table.clone(),
                base_sql: cr.base_sql.clone(),
                primary_key: cr.primary_key.clone(),
                tenant_column: cr.tenant_column.clone(),
                dimensions,
                measures,
                joins,
                segments,
            }
        })
        .collect();

    Ok(FullModel { cubes })
}

// ── Converters ────────────────────────────────────────────────────────────────

fn parse_data_type(s: &str) -> DataType {
    match s {
        "number" => DataType::Number,
        "time" => DataType::Time,
        "boolean" => DataType::Boolean,
        _ => DataType::String,
    }
}

fn parse_agg_type(s: &str) -> AggType {
    match s {
        "count_distinct" => AggType::CountDistinct,
        "sum" => AggType::Sum,
        "avg" => AggType::Avg,
        "min" => AggType::Min,
        "max" => AggType::Max,
        "number" => AggType::Number,
        _ => AggType::Count,
    }
}

fn parse_relationship(s: &str) -> Relationship {
    match s {
        "one_to_many" => Relationship::OneToMany,
        "one_to_one" => Relationship::OneToOne,
        _ => Relationship::ManyToOne,
    }
}
