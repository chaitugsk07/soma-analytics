//! Dashboard and panel CRUD: 08_fct_dashboards and 09_fct_panels.
#![allow(clippy::too_many_arguments)]
//!
//! All writes are audited in-transaction (`record_in_tx`).

use std::sync::Arc;

use soma_audit_pg::LocalSink;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{Error, Result};
use super::audit_in_tx;

// ── Dashboard ─────────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct DashboardRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_dashboard(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    name: &str,
    description: Option<&str>,
) -> Result<DashboardRow> {
    let mut tx = pool.begin().await?;
    let row: DashboardRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."08_fct_dashboards"
               (tenant_id, name, description, created_by)
           VALUES ($1,$2,$3,$4)
           RETURNING id, tenant_id, name, description, is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(name)
    .bind(description)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "dashboard.created",
        "dashboard",
        row.id.to_string(),
        serde_json::json!({ "name": name }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn update_dashboard(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<DashboardRow> {
    let mut tx = pool.begin().await?;
    let row: DashboardRow = sqlx::query_as(
        r#"UPDATE "soma_analytics"."08_fct_dashboards"
           SET name = COALESCE($3, name),
               description = COALESCE($4, description),
               updated_by = $2
           WHERE id = $1 AND tenant_id = $5 AND is_deleted = false
           RETURNING id, tenant_id, name, description, is_deleted, created_at, updated_at"#,
    )
    .bind(id)
    .bind(actor_id)
    .bind(name)
    .bind(description)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| Error::NotFound(format!("dashboard {id}")))?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "dashboard.updated",
        "dashboard",
        id.to_string(),
        serde_json::json!({ "name": row.name }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_dashboard(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;

    // B5: soft-delete child panels first (cascade-soft-delete).
    sqlx::query(
        r#"UPDATE "soma_analytics"."09_fct_panels"
           SET is_deleted = true, deleted_at = now(), deleted_by = $3, updated_by = $3
           WHERE dashboard_id = $1 AND tenant_id = $2 AND is_deleted = false"#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .execute(&mut *tx)
    .await?;

    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."08_fct_dashboards"
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
        return Err(Error::NotFound(format!("dashboard {id}")));
    }

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "dashboard.deleted",
        "dashboard",
        id.to_string(),
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── Panel ─────────────────────────────────────────────────────────────────────

#[derive(Debug, sqlx::FromRow)]
pub struct PanelRow {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub dashboard_id: Uuid,
    pub name: String,
    pub chart_type: String,
    pub query_json: serde_json::Value,
    pub grid_x: i32,
    pub grid_y: i32,
    pub grid_w: i32,
    pub grid_h: i32,
    pub is_deleted: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn create_panel(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    dashboard_id: Uuid,
    name: &str,
    chart_type: &str,
    query_json: serde_json::Value,
    grid_x: i32,
    grid_y: i32,
    grid_w: i32,
    grid_h: i32,
) -> Result<PanelRow> {
    let mut tx = pool.begin().await?;

    // B4: ownership check — dashboard must belong to this tenant.
    let _check: Option<(Uuid,)> = sqlx::query_as(
        r#"SELECT id FROM "soma_analytics"."08_fct_dashboards"
           WHERE id = $1 AND tenant_id = $2 AND is_deleted = false
           FOR UPDATE"#,
    )
    .bind(dashboard_id)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    if _check.is_none() {
        return Err(Error::NotFound(format!("dashboard {dashboard_id}")));
    }

    let row: PanelRow = sqlx::query_as(
        r#"INSERT INTO "soma_analytics"."09_fct_panels"
               (tenant_id, dashboard_id, name, chart_type, query_json,
                grid_x, grid_y, grid_w, grid_h, created_by)
           VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
           RETURNING id, tenant_id, dashboard_id, name, chart_type, query_json,
                     grid_x, grid_y, grid_w, grid_h, is_deleted, created_at, updated_at"#,
    )
    .bind(tenant_id)
    .bind(dashboard_id)
    .bind(name)
    .bind(chart_type)
    .bind(&query_json)
    .bind(grid_x)
    .bind(grid_y)
    .bind(grid_w)
    .bind(grid_h)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "panel.created",
        "panel",
        row.id.to_string(),
        serde_json::json!({ "name": name, "dashboard_id": dashboard_id, "chart_type": chart_type }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn update_panel(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
    name: Option<&str>,
    chart_type: Option<&str>,
    query_json: Option<serde_json::Value>,
    grid_x: Option<i32>,
    grid_y: Option<i32>,
    grid_w: Option<i32>,
    grid_h: Option<i32>,
) -> Result<PanelRow> {
    let mut tx = pool.begin().await?;
    let row: PanelRow = sqlx::query_as(
        r#"UPDATE "soma_analytics"."09_fct_panels"
           SET name        = COALESCE($3,  name),
               chart_type  = COALESCE($4,  chart_type),
               query_json  = COALESCE($5,  query_json),
               grid_x      = COALESCE($6,  grid_x),
               grid_y      = COALESCE($7,  grid_y),
               grid_w      = COALESCE($8,  grid_w),
               grid_h      = COALESCE($9,  grid_h),
               updated_by  = $2
           WHERE id = $1 AND tenant_id = $10 AND is_deleted = false
           RETURNING id, tenant_id, dashboard_id, name, chart_type, query_json,
                     grid_x, grid_y, grid_w, grid_h, is_deleted, created_at, updated_at"#,
    )
    .bind(id)
    .bind(actor_id)
    .bind(name)
    .bind(chart_type)
    .bind(query_json)
    .bind(grid_x)
    .bind(grid_y)
    .bind(grid_w)
    .bind(grid_h)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| Error::NotFound(format!("panel {id}")))?;

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "panel.updated",
        "panel",
        id.to_string(),
        serde_json::json!({ "name": row.name, "dashboard_id": row.dashboard_id }),
    )
    .await?;
    tx.commit().await?;
    Ok(row)
}

pub async fn soft_delete_panel(
    pool: &PgPool,
    sink: &Arc<LocalSink>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    id: Uuid,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let affected = sqlx::query(
        r#"UPDATE "soma_analytics"."09_fct_panels"
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
        return Err(Error::NotFound(format!("panel {id}")));
    }

    audit_in_tx(
        sink,
        &mut tx,
        tenant_id,
        actor_id,
        "panel.deleted",
        "panel",
        id.to_string(),
        serde_json::json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
