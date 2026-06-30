//! The `Store` — the primary public API of soma-storage.
#![allow(clippy::too_many_arguments)]
//!
//! Holds both pools (metadata + data-source), the Redis cache handle, the
//! audit sink, and the optional DSN KEK. Built in soma-server; consumed by
//! soma-api handlers.
//!
//! # Tenant isolation assumption
//!
//! `Store::run_query` trusts the `QueryScope` it receives as already
//! tenant-scoped. The API layer is responsible for:
//!   1. Verifying the bearer token and extracting a `Principal`.
//!   2. Setting `scope.tenant_id = principal.tenant_id` (never from request body).
//!   3. Injecting embed `row_filters` from the verified embed token payload.
//!
//! The compiler then validates every `row_filter.member` against the model
//! whitelist before using it as a bound parameter — so no user-supplied SQL
//! can reach the data-source pool.

use std::sync::Arc;

use redis::aio::ConnectionManager;
use soma_audit_pg::LocalSink;
use soma_infra::cache;
use soma_infra::crypto::CryptoKey;
use soma_semantic::{compile, QueryScope, SemanticQuery};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::cache_key::{build_cache_key, query_fingerprint};
use crate::error::{Error, Result};
use crate::pg::{crud, dashboard, model};
use crate::types::{ColumnMeta, CompileResult, FullModel, ResultMeta, ResultSet};

// Re-export row types so soma-api doesn't need to reach into pg:: sub-modules.
pub use crate::pg::crud::{
    ApiTokenRow, CubeRow, DataSourceRow, DimensionRow, JoinRow as CubeJoinRow, MeasureRow,
    SegmentRow,
};
pub use crate::pg::dashboard::{DashboardRow, PanelRow};

/// The central data-access handle for soma-analytics.
///
/// Clone is cheap: all fields are either `Arc` or reference-counted pool handles.
#[derive(Clone)]
pub struct Store {
    /// Metadata pool (DATABASE_URL): stores the semantic model + auth tokens.
    pub(crate) meta_pool: PgPool,
    /// Data-source pool (ANALYTICS_DB_URL): the read-only pool over the target service DB.
    pub(crate) ds_pool: PgPool,
    /// Redis `ConnectionManager` for the result cache.
    pub(crate) cache: ConnectionManager,
    /// Audit sink (LocalSink backed by the metadata pool).
    pub(crate) audit: Arc<LocalSink>,
    /// Optional KEK for encrypting per-source DSNs in 02_fct_data_sources.
    /// Phase-1 default: None (all sources use the env ds_pool).
    pub(crate) dsn_kek: Option<Arc<CryptoKey>>,
}

impl Store {
    /// Build a `Store` from its components (typically called from soma-server main).
    pub fn new(
        meta_pool: PgPool,
        ds_pool: PgPool,
        cache: ConnectionManager,
        audit: Arc<LocalSink>,
        dsn_kek: Option<CryptoKey>,
    ) -> Self {
        Self {
            meta_pool,
            ds_pool,
            cache,
            audit,
            dsn_kek: dsn_kek.map(Arc::new),
        }
    }

    // ── Model load ────────────────────────────────────────────────────────────

    /// Load the tenant's full semantic model from the metadata tables.
    pub async fn load_model(&self, tenant_id: Uuid) -> Result<soma_semantic::Model> {
        model::load_model(&self.meta_pool, tenant_id).await
    }

    /// Export the full model for the builder/editor UI. Returns all fields
    /// including `title`, `description`, `sql_table`, `base_sql`, and raw SQL
    /// expressions on dimensions, measures, joins, and segments.
    ///
    /// Uses a direct DB read rather than going through `soma_semantic::Model`
    /// so that fields the semantic model omits (title, description) are preserved.
    pub async fn export_model(&self, tenant_id: Uuid) -> Result<FullModel> {
        model::export_model(&self.meta_pool, tenant_id).await
    }

    /// Compile a semantic query and return the generated SQL with bind-parameter
    /// placeholders (`$N`) — WITHOUT executing it. No cache interaction.
    ///
    /// Returns a `CompileResult` with `sql`, `columns`, and `param_count`.
    /// On a compile error, returns `Err(Error::Compile(_))` so the handler can
    /// surface a 422 with the compiler's message.
    pub async fn compile_query(
        &self,
        tenant_id: Uuid,
        scope: &QueryScope,
        q: &SemanticQuery,
    ) -> Result<CompileResult> {
        let m = model::load_model(&self.meta_pool, tenant_id).await?;
        let compiled = compile(&m, q, scope)?;
        let param_count = compiled.binds.len();
        let columns = compiled
            .columns
            .iter()
            .map(|c| ColumnMeta {
                name: c.name.clone(),
                data_type: match c.data_type {
                    soma_semantic::ColumnType::String => "string".into(),
                    soma_semantic::ColumnType::Number => "number".into(),
                    soma_semantic::ColumnType::Time => "time".into(),
                    soma_semantic::ColumnType::Boolean => "boolean".into(),
                },
            })
            .collect();
        Ok(CompileResult { sql: compiled.sql, columns, param_count })
    }

    // ── Query execution + result cache ────────────────────────────────────────

    /// Compile and execute a semantic query, reading from cache when possible.
    ///
    /// # Tenant isolation
    ///
    /// The API layer injects `scope.tenant_id` from the verified Principal and
    /// sets `scope.row_filters` from the embed token. This method trusts the
    /// scope as already correct — it does NOT re-validate token ownership.
    /// The compiler validates every `row_filter.member` against the model whitelist
    /// before it can become a bound SQL parameter.
    pub async fn run_query(
        &self,
        tenant_id: Uuid,
        scope: &QueryScope,
        q: &SemanticQuery,
    ) -> Result<ResultSet> {
        // 1. Load the model AND the root cube's version info in one query set.
        let (m, cube_info) =
            model::load_model_and_cube_version(&self.meta_pool, tenant_id, &q.cube).await?;
        let model_version = cube_info.model_version;
        let ttl_secs = cube_info.cache_ttl_secs as u64;

        // 2. Compile the query (pure — no I/O, validates all members).
        let compiled = compile(&m, q, scope)?;

        // 3. Build the canonical cache key (includes tenant_id, row_filters, model_version).
        let cache_key = build_cache_key(tenant_id, q, scope, model_version);
        let fingerprint = query_fingerprint(tenant_id, q, scope, model_version);

        // 4. Try the cache first.
        if let Some(bytes) = cache::get(&self.cache, &cache_key).await? {
            let rs: ResultSet = serde_json::from_slice(&bytes)?;
            // Return a new ResultSet with cache:"hit" (the stored value has "miss").
            return Ok(ResultSet {
                meta: ResultMeta {
                    cache: "hit".into(),
                    query_fingerprint: fingerprint,
                    row_count: rs.rows.len(),
                },
                ..rs
            });
        }

        // 5. Cache miss → execute against the data-source pool.
        let rs = self.execute_compiled(&compiled, &fingerprint).await?;

        // 6. Store in cache (best-effort: a Redis failure should not fail the query).
        let cached_bytes = serde_json::to_vec(&rs)?;
        if let Err(e) = cache::set_ex(&self.cache, &cache_key, &cached_bytes, ttl_secs).await {
            tracing::warn!(?e, "cache set_ex failed — result not cached");
        }

        Ok(rs)
    }

    /// Execute a compiled query against the data-source pool and decode rows.
    async fn execute_compiled(
        &self,
        compiled: &soma_semantic::CompiledQuery,
        fingerprint: &str,
    ) -> Result<ResultSet> {
        use soma_semantic::ColumnType;
        use soma_semantic::SqlValue;

        // Build a dynamic query and bind each SqlValue.
        let mut q = sqlx::query(&compiled.sql);
        for bind in &compiled.binds {
            q = match bind {
                SqlValue::Text(s) => q.bind(s.clone()),
                SqlValue::TextArray(v) => q.bind(v.clone()),
                SqlValue::Int(i) => q.bind(*i),
                // Timestamps are bound as text; Postgres casts via ::timestamptz in the SQL.
                SqlValue::Timestamp(s) => q.bind(s.clone()),
                // Uuid: bind natively — sqlx uuid feature handles the encoding.
                SqlValue::Uuid(u) => q.bind(*u),
            };
        }

        let pg_rows = q.fetch_all(&self.ds_pool).await.map_err(|e| {
            tracing::error!(err = %soma_infra::errors::redact_db_error(&e), "data-source query failed");
            Error::Db(e)
        })?;

        // Decode each column by its ColumnType.
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::with_capacity(pg_rows.len());

        for pg_row in &pg_rows {
            let mut row: Vec<serde_json::Value> = Vec::with_capacity(compiled.columns.len());
            for (idx, col_meta) in compiled.columns.iter().enumerate() {
                let v = match col_meta.data_type {
                    ColumnType::Number => {
                        // Try f64 first; fall back to i64, i32, then BigDecimal (NUMERIC columns).
                        if let Ok(f) = pg_row.try_get::<f64, _>(idx) {
                            serde_json::json!(f)
                        } else if let Ok(i) = pg_row.try_get::<i64, _>(idx) {
                            serde_json::json!(i)
                        } else if let Ok(i) = pg_row.try_get::<i32, _>(idx) {
                            serde_json::json!(i)
                        } else if let Ok(bd) = pg_row.try_get::<bigdecimal::BigDecimal, _>(idx) {
                            // Preserve precision — do NOT cast to float (db-standards forbids float in money paths).
                            bd.to_string()
                                .parse::<serde_json::Number>()
                                .map(serde_json::Value::Number)
                                .unwrap_or(serde_json::Value::Null)
                        } else {
                            serde_json::Value::Null
                        }
                    }
                    ColumnType::Time => {
                        // Decode as a UTC datetime string or date string.
                        if let Ok(dt) = pg_row.try_get::<chrono::DateTime<chrono::Utc>, _>(idx) {
                            serde_json::json!(dt.to_rfc3339())
                        } else if let Ok(nd) = pg_row.try_get::<chrono::NaiveDate, _>(idx) {
                            serde_json::json!(nd.to_string())
                        } else {
                            pg_row
                                .try_get::<String, _>(idx)
                                .map(|s| serde_json::json!(s))
                                .unwrap_or(serde_json::Value::Null)
                        }
                    }
                    ColumnType::Boolean => pg_row
                        .try_get::<bool, _>(idx)
                        .map(|b| serde_json::json!(b))
                        .unwrap_or(serde_json::Value::Null),
                    ColumnType::String => pg_row
                        .try_get::<String, _>(idx)
                        .map(|s| serde_json::json!(s))
                        .unwrap_or(serde_json::Value::Null),
                };
                row.push(v);
            }
            rows.push(row);
        }

        let columns: Vec<ColumnMeta> = compiled
            .columns
            .iter()
            .map(|c| ColumnMeta {
                name: c.name.clone(),
                data_type: match c.data_type {
                    ColumnType::String => "string".into(),
                    ColumnType::Number => "number".into(),
                    ColumnType::Time => "time".into(),
                    ColumnType::Boolean => "boolean".into(),
                },
            })
            .collect();

        let row_count = rows.len();
        Ok(ResultSet {
            columns,
            rows,
            meta: ResultMeta {
                cache: "miss".into(),
                query_fingerprint: fingerprint.to_string(),
                row_count,
            },
        })
    }

    // ── Data-source CRUD ──────────────────────────────────────────────────────

    pub async fn list_data_sources(&self, tenant_id: Uuid) -> Result<Vec<DataSourceRow>> {
        crud::list_data_sources(&self.meta_pool, tenant_id).await
    }

    pub async fn create_data_source(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        name: &str,
        driver: &str,
        dsn_ciphertext: Option<Vec<u8>>,
    ) -> Result<DataSourceRow> {
        crud::create_data_source(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            name,
            driver,
            dsn_ciphertext,
        )
        .await
    }

    pub async fn delete_data_source(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<()> {
        crud::soft_delete_data_source(&self.meta_pool, &self.audit, tenant_id, actor_id, id).await
    }

    // ── DSN crypto ────────────────────────────────────────────────────────────

    /// Encrypt a DSN string for a tenant. Returns `None` if no KEK is configured.
    pub fn encrypt_dsn(
        &self,
        tenant_id: Uuid,
        dsn: &str,
    ) -> Result<Option<Vec<u8>>> {
        match &self.dsn_kek {
            None => Ok(None),
            Some(kek) => {
                let ct = crate::pg::dsn::encrypt_dsn(kek, tenant_id, dsn)?;
                Ok(Some(ct))
            }
        }
    }

    /// Decrypt a DSN ciphertext for a tenant. Returns `None` if no KEK is configured.
    pub fn decrypt_dsn(
        &self,
        tenant_id: Uuid,
        ciphertext: &[u8],
    ) -> Result<Option<String>> {
        match &self.dsn_kek {
            None => Ok(None),
            Some(kek) => {
                let s = crate::pg::dsn::decrypt_dsn(kek, tenant_id, ciphertext)?;
                Ok(Some(s))
            }
        }
    }

    // ── Cube CRUD ─────────────────────────────────────────────────────────────

    pub async fn create_cube(
        &self,
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
        tenant_column: Option<&str>,
    ) -> Result<CubeRow> {
        crud::create_cube(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            data_source_id,
            name,
            title,
            description,
            sql_table,
            base_sql,
            primary_key,
            cache_ttl_secs,
            tenant_column.unwrap_or("tenant_id"),
        )
        .await
    }

    pub async fn update_cube(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
        title: Option<&str>,
        description: Option<&str>,
        cache_ttl_secs: Option<i32>,
    ) -> Result<CubeRow> {
        crud::update_cube(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            id,
            title,
            description,
            cache_ttl_secs,
        )
        .await
    }

    pub async fn delete_cube(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<()> {
        crud::soft_delete_cube(&self.meta_pool, &self.audit, tenant_id, actor_id, id).await
    }

    // ── Dimension CRUD ────────────────────────────────────────────────────────

    pub async fn create_dimension(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        cube_id: Uuid,
        name: &str,
        description: Option<&str>,
        sql_expr: &str,
        data_type: &str,
    ) -> Result<DimensionRow> {
        crud::create_dimension(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            cube_id,
            name,
            description,
            sql_expr,
            data_type,
        )
        .await
    }

    pub async fn delete_dimension(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
        cube_id: Uuid,
    ) -> Result<()> {
        crud::soft_delete_dimension(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            id,
            cube_id,
        )
        .await
    }

    // ── Measure CRUD ──────────────────────────────────────────────────────────

    pub async fn create_measure(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        cube_id: Uuid,
        name: &str,
        description: Option<&str>,
        sql_expr: Option<&str>,
        agg_type: &str,
    ) -> Result<MeasureRow> {
        crud::create_measure(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            cube_id,
            name,
            description,
            sql_expr,
            agg_type,
        )
        .await
    }

    pub async fn delete_measure(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
        cube_id: Uuid,
    ) -> Result<()> {
        crud::soft_delete_measure(&self.meta_pool, &self.audit, tenant_id, actor_id, id, cube_id)
            .await
    }

    // ── Join CRUD ─────────────────────────────────────────────────────────────

    pub async fn create_join(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        cube_id: Uuid,
        target_cube_id: Uuid,
        name: &str,
        relationship: &str,
        sql_on: &str,
    ) -> Result<CubeJoinRow> {
        crud::create_join(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            cube_id,
            target_cube_id,
            name,
            relationship,
            sql_on,
        )
        .await
    }

    pub async fn delete_join(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
        cube_id: Uuid,
    ) -> Result<()> {
        crud::soft_delete_join(&self.meta_pool, &self.audit, tenant_id, actor_id, id, cube_id)
            .await
    }

    // ── Segment CRUD ──────────────────────────────────────────────────────────

    pub async fn create_segment(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        cube_id: Uuid,
        name: &str,
        sql_expr: &str,
    ) -> Result<SegmentRow> {
        crud::create_segment(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            cube_id,
            name,
            sql_expr,
        )
        .await
    }

    pub async fn delete_segment(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
        cube_id: Uuid,
    ) -> Result<()> {
        crud::soft_delete_segment(&self.meta_pool, &self.audit, tenant_id, actor_id, id, cube_id)
            .await
    }

    // ── Dashboard CRUD ────────────────────────────────────────────────────────

    pub async fn create_dashboard(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        name: &str,
        description: Option<&str>,
    ) -> Result<DashboardRow> {
        dashboard::create_dashboard(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            name,
            description,
        )
        .await
    }

    pub async fn update_dashboard(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<DashboardRow> {
        dashboard::update_dashboard(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            id,
            name,
            description,
        )
        .await
    }

    pub async fn delete_dashboard(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<()> {
        dashboard::soft_delete_dashboard(&self.meta_pool, &self.audit, tenant_id, actor_id, id)
            .await
    }

    // ── Panel CRUD ────────────────────────────────────────────────────────────

    pub async fn create_panel(
        &self,
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
        dashboard::create_panel(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            dashboard_id,
            name,
            chart_type,
            query_json,
            grid_x,
            grid_y,
            grid_w,
            grid_h,
        )
        .await
    }

    pub async fn update_panel(
        &self,
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
        dashboard::update_panel(
            &self.meta_pool,
            &self.audit,
            tenant_id,
            actor_id,
            id,
            name,
            chart_type,
            query_json,
            grid_x,
            grid_y,
            grid_w,
            grid_h,
        )
        .await
    }

    pub async fn delete_panel(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<()> {
        dashboard::soft_delete_panel(&self.meta_pool, &self.audit, tenant_id, actor_id, id).await
    }

    // ── API Token CRUD ────────────────────────────────────────────────────────

    pub async fn create_api_token(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        name: &str,
        role: &str,
    ) -> Result<(ApiTokenRow, String)> {
        crud::create_api_token(&self.meta_pool, &self.audit, tenant_id, actor_id, name, role).await
    }

    pub async fn list_api_tokens(&self, tenant_id: Uuid) -> Result<Vec<ApiTokenRow>> {
        crud::list_api_tokens(&self.meta_pool, tenant_id).await
    }

    pub async fn delete_api_token(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<()> {
        crud::soft_delete_api_token(&self.meta_pool, &self.audit, tenant_id, actor_id, id).await
    }

    /// Bootstrap: if no active tokens exist for the tenant, create a root admin token
    /// and return the plaintext (shown once). The value is never logged.
    pub async fn bootstrap_root_token(&self, tenant_id: Uuid) -> Result<Option<String>> {
        let count = crud::count_active_tokens(&self.meta_pool, tenant_id).await?;
        if count == 0 {
            let (_row, plaintext) = crud::create_api_token(
                &self.meta_pool,
                &self.audit,
                tenant_id,
                None,
                "root",
                "admin",
            )
            .await?;
            Ok(Some(plaintext))
        } else {
            Ok(None)
        }
    }
}
