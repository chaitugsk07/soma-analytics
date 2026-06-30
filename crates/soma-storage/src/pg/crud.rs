//! Model CRUD operations: data sources, cubes, dimensions, measures, joins, segments.
#![allow(clippy::too_many_arguments)]
//!
//! Every write runs in a transaction, bumps the owning cube's `model_version` (for
//! child entities), and writes a soma-audit event via `record_in_tx`. Uses
//! `soma_infra::errors::redact_db_error` when surfacing sqlx errors.

use std::sync::Arc;

use soma_audit_pg::LocalSink;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::{Error, Result};
use super::audit_in_tx;

// ── Bump model_version ────────────────────────────────────────────────────────

/// Increment the `model_version` for a cube inside the given transaction.
/// Called by all child-entity CRUD (dimensions, measures, joins, segments).
async fn bump_model_version(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    cube_id: Uuid,
) -> Result<i32> {
    let row: (i32,) = sqlx::query_as(
        r#"UPDATE "soma_analytics"."03_fct_cubes"
           SET model_version = model_version + 1
           WHERE id = $1 AND tenant_id = $2 AND is_deleted = false
           RETURNING model_version"#,
    )
    .bind(cube_id)
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| {
        tracing::error!(%e, "bump_model_version failed");
        Error::Db(e)
    })?;
    Ok(row.0)
}

// ── Cube ownership check ──────────────────────────────────────────────────────

/// Verify the cube belongs to the tenant before inserting a child entity.
/// Uses SELECT ... FOR UPDATE to prevent concurrent deletes during the transaction.
async fn check_cube_ownership(
    tx: &mut Transaction<'_, Postgres>,
    cube_id: Uuid,
    tenant_id: Uuid,
) -> Result<()> {
    let _check: Option<(Uuid,)> = sqlx::query_as(
        r#"SELECT id FROM "soma_analytics"."03_fct_cubes"
           WHERE id = $1 AND tenant_id = $2 AND is_deleted = false
           FOR UPDATE"#,
    )
    .bind(cube_id)
    .bind(tenant_id)
    .fetch_optional(&mut **tx)
    .await?;
    if _check.is_none() {
        return Err(Error::NotFound(format!("cube {cube_id}")));
    }
    Ok(())
}

// ── Data-source CRUD ──────────────────────────────────────────────────────────

/// Row shape for a returned data source.
#[derive(Debug, sqlx::FromRow)]
pub struct DataSourceRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub driver: String,
    pub dsn_ciphertext: Option<Vec<u8>>,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_data_source(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    name: &str,
    driver: &str,
    dsn_ciphertext: Option<Vec<u8>>,
) -> Result<DataSourceRow> {
    let mut tx = pool.begin().await?;
    let row: DataSourceRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."02_fct_data_sources"
               (tenant_id, name, driver, dsn_ciphertext, created_by)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING id, tenant_id, name, driver, dsn_ciphertext, is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(name)
    .bind(driver)
    .bind(dsn_ciphertext)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(Error::Db)?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "datasource.connected",
        "data_source",
        row.id.to_string(),
        serde_json::json!({ "name": name, "driver": driver }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_data_source(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;

    // B6: guard — refuse deletion if active cubes reference this data source.
    let count: (i64,) = sqlx::query_as(
        r#"SELECT count(*) FROM "soma_analytics"."03_fct_cubes"
           WHERE data_source_id = $1 AND tenant_id = $2 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .fetch_one(&mut *tx)
    .await?;
    if count.0 > 0 {
        return Err(Error::Conflict(format!(
            "data_source {id} has {} active cube(s); delete them first", count.0
        )));
    }

    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."02_fct_data_sources"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE id = $1 AND tenant_id = $2 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound(format!("data_source {id}")));
    }

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "datasource.deleted",
        "data_source",
        id.to_string(),
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── Cube CRUD ─────────────────────────────────────────────────────────────────

/// Row shape for a returned cube (includes model_version for cache key building).
#[derive(Debug, sqlx::FromRow)]
pub struct CubeRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub data_source_id: Uuid,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub sql_table: Option<String>,
    pub base_sql: Option<String>,
    pub primary_key: String,
    pub tenant_column: String,
    pub cache_ttl_secs: i32,
    pub model_version: i32,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_cube(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    data_source_id: Uuid,
    name: &str,
    title: Option<&str>,
    description: Option<&str>,
    sql_table: Option<&str>,
    base_sql: Option<&str>,
    primary_key: &str,
    cache_ttl_secs: i32,
    tenant_column: &str,
) -> Result<CubeRow> {
    let mut tx = pool.begin().await?;
    let row: CubeRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."03_fct_cubes"
               (tenant_id, data_source_id, name, title, description,
                sql_table, base_sql, primary_key, cache_ttl_secs, tenant_column, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
           RETURNING id, tenant_id, data_source_id, name, title, description,
                     sql_table, base_sql, primary_key, tenant_column, cache_ttl_secs, model_version,
                     is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(data_source_id)
    .bind(name)
    .bind(title)
    .bind(description)
    .bind(sql_table)
    .bind(base_sql)
    .bind(primary_key)
    .bind(cache_ttl_secs)
    .bind(tenant_column)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(Error::Db)?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "cube.created",
        "cube",
        row.id.to_string(),
        serde_json::json!({ "name": name, "model_version": row.model_version }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn update_cube(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
    title: Option<&str>,
    description: Option<&str>,
    cache_ttl_secs: Option<i32>,
) -> Result<CubeRow> {
    let mut tx = pool.begin().await?;
    // model_version bumped on any cube update.
    let row: CubeRow = sqlx::query_as(
        r#"UPDATE "soma_analytics"."03_fct_cubes"
           SET title = COALESCE($3, title),
               description = COALESCE($4, description),
               cache_ttl_secs = COALESCE($5, cache_ttl_secs),
               model_version = model_version + 1,
               updated_by = $2
           WHERE id = $1 AND tenant_id = $6 AND is_deleted = false
           RETURNING id, tenant_id, data_source_id, name, title, description,
                     sql_table, base_sql, primary_key, tenant_column, cache_ttl_secs, model_version,
                     is_deleted, created_at, updated_at"#,
    )
    .bind(id)
    .bind(actor_id)
    .bind(title)
    .bind(description)
    .bind(cache_ttl_secs)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| Error::NotFound(format!("cube {id}")))?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "cube.updated",
        "cube",
        row.id.to_string(),
        serde_json::json!({ "name": row.name, "model_version": row.model_version }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_cube(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."03_fct_cubes"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE id = $1 AND tenant_id = $2 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound(format!("cube {id}")));
    }

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "cube.deleted",
        "cube",
        id.to_string(),
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── Dimension CRUD ────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct DimensionRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub cube_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub sql_expr: String,
    pub data_type: String,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_dimension(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    cube_id: Uuid,
    name: &str,
    description: Option<&str>,
    sql_expr: &str,
    data_type: &str,
) -> Result<DimensionRow> {
    let mut tx = pool.begin().await?;
    // B4: ownership check — cube must belong to this tenant.
    check_cube_ownership(&mut tx, cube_id, tenant_id).await?;

    let row: DimensionRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."04_fct_dimensions"
               (tenant_id, cube_id, name, description, sql_expr, data_type, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7)
           RETURNING id, tenant_id, cube_id, name, description, sql_expr, data_type,
                     is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(cube_id)
    .bind(name)
    .bind(description)
    .bind(sql_expr)
    .bind(data_type)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "dimension.created",
        "dimension",
        row.id.to_string(),
        serde_json::json!({ "name": name, "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_dimension(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
    cube_id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    // B5: also scope by cube_id to prevent cross-cube deletes.
    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."04_fct_dimensions"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE id = $1 AND tenant_id = $2 AND cube_id = $4 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .bind(cube_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound(format!("dimension {id}")));
    }

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "dimension.deleted",
        "dimension",
        id.to_string(),
        serde_json::json!({ "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── Measure CRUD ──────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct MeasureRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub cube_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub sql_expr: Option<String>,
    pub agg_type: String,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_measure(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    cube_id: Uuid,
    name: &str,
    description: Option<&str>,
    sql_expr: Option<&str>,
    agg_type: &str,
) -> Result<MeasureRow> {
    let mut tx = pool.begin().await?;
    // B4: ownership check.
    check_cube_ownership(&mut tx, cube_id, tenant_id).await?;

    let row: MeasureRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."05_fct_measures"
               (tenant_id, cube_id, name, description, sql_expr, agg_type, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7)
           RETURNING id, tenant_id, cube_id, name, description, sql_expr, agg_type,
                     is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(cube_id)
    .bind(name)
    .bind(description)
    .bind(sql_expr)
    .bind(agg_type)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "measure.created",
        "measure",
        row.id.to_string(),
        serde_json::json!({ "name": name, "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_measure(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
    cube_id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    // B5: also scope by cube_id.
    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."05_fct_measures"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE id = $1 AND tenant_id = $2 AND cube_id = $4 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .bind(cube_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound(format!("measure {id}")));
    }

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "measure.deleted",
        "measure",
        id.to_string(),
        serde_json::json!({ "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── Join CRUD ─────────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct JoinRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub cube_id: Uuid,
    pub target_cube_id: Uuid,
    pub name: String,
    pub relationship: String,
    pub sql_on: String,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_join(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    cube_id: Uuid,
    target_cube_id: Uuid,
    name: &str,
    relationship: &str,
    sql_on: &str,
) -> Result<JoinRow> {
    let mut tx = pool.begin().await?;
    // B4: ownership check.
    check_cube_ownership(&mut tx, cube_id, tenant_id).await?;

    let row: JoinRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."06_fct_joins"
               (tenant_id, cube_id, target_cube_id, name, relationship, sql_on, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7)
           RETURNING id, tenant_id, cube_id, target_cube_id, name, relationship, sql_on,
                     is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(cube_id)
    .bind(target_cube_id)
    .bind(name)
    .bind(relationship)
    .bind(sql_on)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "join.created",
        "join",
        row.id.to_string(),
        serde_json::json!({ "name": name, "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_join(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
    cube_id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    // B5: also scope by cube_id.
    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."06_fct_joins"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE id = $1 AND tenant_id = $2 AND cube_id = $4 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .bind(cube_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound(format!("join {id}")));
    }

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "join.deleted",
        "join",
        id.to_string(),
        serde_json::json!({ "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── Segment CRUD ──────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct SegmentRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub cube_id: Uuid,
    pub name: String,
    pub sql_expr: String,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_segment(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    cube_id: Uuid,
    name: &str,
    sql_expr: &str,
) -> Result<SegmentRow> {
    let mut tx = pool.begin().await?;
    // B4: ownership check.
    check_cube_ownership(&mut tx, cube_id, tenant_id).await?;

    let row: SegmentRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."07_fct_segments"
               (tenant_id, cube_id, name, sql_expr, created_by)
           VALUES ($1,$2,$3,$4,$5)
           RETURNING id, tenant_id, cube_id, name, sql_expr, is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(cube_id)
    .bind(name)
    .bind(sql_expr)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "segment.created",
        "segment",
        row.id.to_string(),
        serde_json::json!({ "name": name, "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_segment(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
    cube_id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    // B5: also scope by cube_id.
    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."07_fct_segments"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE id = $1 AND tenant_id = $2 AND cube_id = $4 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .bind(cube_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(Error::NotFound(format!("segment {id}")));
    }

    let new_ver = bump_model_version(&mut tx, tenant_id, cube_id).await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "segment.deleted",
        "segment",
        id.to_string(),
        serde_json::json!({ "cube_id": cube_id, "model_version": new_ver }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── API Token CRUD ────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct ApiTokenRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub role: String,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_api_token(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    name: &str,
    role: &str,
) -> Result<(ApiTokenRow, String)> {
    use soma_infra::crypto::sha256_hex;

    // Generate 32 random bytes of entropy using two UUIDs (each is 16 bytes of v4 random).
    let b1 = Uuid::new_v4().as_bytes().to_vec();
    let b2 = Uuid::new_v4().as_bytes().to_vec();
    let entropy: Vec<u8> = b1.into_iter().chain(b2).collect();
    let token_hex = hex::encode(&entropy);
    let plaintext = format!("sat_{token_hex}");
    let token_sha256 = sha256_hex(plaintext.as_bytes());

    let mut tx = pool.begin().await?;
    let row: ApiTokenRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."01_fct_api_tokens"
               (tenant_id, token_sha256, name, role, created_by)
           VALUES ($1,$2,$3,$4,$5)
           RETURNING id, tenant_id, name, role, expires_at, is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(&token_sha256)
    .bind(name)
    .bind(role)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(Error::Db)?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "api_token.created",
        "api_token",
        row.id.to_string(),
        serde_json::json!({ "name": name, "role": role }),
    )
    .await?;
    tx.commit().await?;
    Ok((row, plaintext))
}

pub async fn list_api_tokens(pool: &PgPool, tenant_id: Uuid) -> Result<Vec<ApiTokenRow>> {
    let rows = sqlx::query_as(
        r#"SELECT id, tenant_id, name, role, expires_at, is_deleted, created_at, updated_at
           FROM "soma_analytics"."01_fct_api_tokens"
           WHERE tenant_id = $1 AND is_deleted = false
           ORDER BY created_at DESC"#,
    )
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn soft_delete_api_token(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."01_fct_api_tokens"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE id = $1 AND tenant_id = $2 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if affected == 0 {
        return Err(Error::NotFound(format!("api_token {id}")));
    }
    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "api_token.deleted",
        "api_token",
        id.to_string(),
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn count_active_tokens(pool: &PgPool, tenant_id: Uuid) -> Result<i64> {
    let (count,): (i64,) = sqlx::query_as(
        r#"SELECT count(*) FROM "soma_analytics"."01_fct_api_tokens"
           WHERE tenant_id = $1 AND is_deleted = false
             AND (expires_at IS NULL OR expires_at > now())"#,
    )
    .bind(tenant_id)
    .fetch_one(pool)
    .await?;
    Ok(count)
}
