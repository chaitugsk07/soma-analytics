//! Async API helpers for the soma-analytics builder portal.
//! All endpoints optionally use `Authorization: Bearer <token>`.
//! The token is passed as a parameter — callers read it from AppCtx.

use serde::de::DeserializeOwned;

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}

async fn handle_response<T: DeserializeOwned>(
    resp: gloo_net::http::Response,
) -> Result<T, ApiError> {
    let status = resp.status();
    if !resp.ok() {
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or(body);
        return Err(ApiError { status, message: msg });
    }
    resp.json::<T>().await.map_err(|e| ApiError {
        status,
        message: e.to_string(),
    })
}

pub async fn get_json<T: DeserializeOwned>(
    base: &str,
    path: &str,
    token: &str,
) -> Result<T, ApiError> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let mut req = gloo_net::http::Request::get(&url);
    if !token.is_empty() {
        req = req.header("Authorization", &format!("Bearer {}", token));
    }
    let resp = req.send().await.map_err(|e| ApiError {
        status: 0,
        message: e.to_string(),
    })?;
    handle_response(resp).await
}

pub async fn post_json<T: DeserializeOwned>(
    base: &str,
    path: &str,
    token: &str,
    body: &serde_json::Value,
) -> Result<T, ApiError> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let mut builder = gloo_net::http::Request::post(&url)
        .header("Content-Type", "application/json");
    if !token.is_empty() {
        builder = builder.header("Authorization", &format!("Bearer {}", token));
    }
    let resp = builder
        .body(serde_json::to_string(body).unwrap_or_default())
        .map_err(|e| ApiError { status: 0, message: e.to_string() })?
        .send()
        .await
        .map_err(|e| ApiError { status: 0, message: e.to_string() })?;
    handle_response(resp).await
}

/// DELETE with no body — returns () on 204, error otherwise.
pub async fn delete_req(base: &str, path: &str, token: &str) -> Result<(), ApiError> {
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let mut builder = gloo_net::http::Request::delete(&url);
    if !token.is_empty() {
        builder = builder.header("Authorization", &format!("Bearer {}", token));
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| ApiError { status: 0, message: e.to_string() })?;
    let status = resp.status();
    if !resp.ok() {
        let body = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(|s| s.to_string()))
            .unwrap_or(body);
        return Err(ApiError { status, message: msg });
    }
    Ok(())
}

// ── Model DTOs — mirror crates/soma-storage/src/types.rs FullModel ────────────

/// Mirrors `FullModel` from soma-storage/src/types.rs
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FullModel {
    pub cubes: Vec<FullCube>,
}

/// Mirrors `FullCube` from soma-storage/src/types.rs
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FullCube {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub data_source: String,
    #[serde(default)]
    pub sql_table: Option<String>,
    #[serde(default)]
    pub base_sql: Option<String>,
    pub primary_key: String,
    pub tenant_column: String,
    pub dimensions: Vec<FullDimension>,
    pub measures: Vec<FullMeasure>,
    pub joins: Vec<FullJoin>,
    pub segments: Vec<FullSegment>,
}

/// Mirrors `FullDimension` from soma-storage/src/types.rs
/// Note: serde rename — JSON field is `"type"`, Rust field is `data_type`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FullDimension {
    pub id: String,
    pub name: String,
    pub sql: String,
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Mirrors `FullMeasure` from soma-storage/src/types.rs
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FullMeasure {
    pub id: String,
    pub name: String,
    pub agg_type: String,
    #[serde(default)]
    pub sql: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Mirrors `FullJoin` from soma-storage/src/types.rs
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FullJoin {
    pub id: String,
    pub name: String,
    pub target_cube: String,
    pub relationship: String,
    pub sql: String,
}

/// Mirrors `FullSegment` from soma-storage/src/types.rs
#[derive(Debug, Clone, serde::Deserialize)]
pub struct FullSegment {
    pub id: String,
    pub name: String,
    pub sql: String,
}

// ── API functions ─────────────────────────────────────────────────────────────

/// GET /api/v1/model — requires Editor+ role.
pub async fn fetch_model(base: &str, token: &str) -> Result<FullModel, ApiError> {
    get_json::<FullModel>(base, "/api/v1/model", token).await
}

// ── Query result DTOs — mirror crates/soma-storage/src/types.rs ──────────────

/// Metadata about a result column.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,
}

/// Per-result metadata from the API.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResultMeta {
    pub cache: String,
    pub query_fingerprint: String,
    pub row_count: usize,
}

/// A query result with typed columns and row data.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ResultSet {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub meta: ResultMeta,
}

/// Returned by POST /api/v1/query/compile.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CompileResult {
    pub sql: String,
    pub columns: Vec<ColumnMeta>,
    pub param_count: usize,
}

// ── SemanticQuery mirror structs ──────────────────────────────────────────────
// soma-semantic pulls in uuid v4 which requires the `js` feature on wasm32.
// Rather than patching the upstream crate, we mirror the wire types here.
// These match the serde serialization of soma_semantic::{SemanticQuery, …} exactly.

/// Sort direction (mirrors soma_semantic::Order).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    Asc,
    Desc,
}

/// Time-series granularity (mirrors soma_semantic::Granularity).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

/// Filter operator (mirrors soma_semantic::FilterOp).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Equals,
    NotEquals,
    Contains,
    Gt,
    Gte,
    Lt,
    Lte,
    Set,
    NotSet,
    InDateRange,
}

/// A filter applied to a member value (mirrors soma_semantic::Filter).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Filter {
    pub member: String,
    pub operator: FilterOp,
    #[serde(default)]
    pub values: Vec<String>,
}

/// A time-dimension selector (mirrors soma_semantic::TimeDimension).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeDimension {
    pub member: String,
    pub granularity: Granularity,
    pub date_range: Option<[String; 2]>,
}

/// Semantic query — mirrors soma_semantic::SemanticQuery wire format exactly.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticQuery {
    pub cube: String,
    #[serde(default)]
    pub measures: Vec<String>,
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub segments: Vec<String>,
    pub time_dimension: Option<TimeDimension>,
    #[serde(default)]
    pub order: Vec<(String, Order)>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// POST /api/v1/query — execute a SemanticQuery and return rows.
pub async fn run_query(
    base: &str,
    token: &str,
    query: &SemanticQuery,
) -> Result<ResultSet, ApiError> {
    let body = serde_json::to_value(query).map_err(|e| ApiError {
        status: 0,
        message: e.to_string(),
    })?;
    post_json::<ResultSet>(base, "/api/v1/query", token, &body).await
}

/// POST /api/v1/query/compile — compile a SemanticQuery and return the SQL.
pub async fn compile_query(
    base: &str,
    token: &str,
    query: &SemanticQuery,
) -> Result<CompileResult, ApiError> {
    let body = serde_json::to_value(query).map_err(|e| ApiError {
        status: 0,
        message: e.to_string(),
    })?;
    post_json::<CompileResult>(base, "/api/v1/query/compile", token, &body).await
}

// ── Editor API helpers ────────────────────────────────────────────────────────

/// Minimal DTO for a data source returned by GET /api/v1/datasources.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DataSourceItem {
    pub id: String,
    pub name: String,
    pub driver: String,
}

/// Minimal DTO returned by create endpoints.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CreatedRow {
    pub id: String,
    pub name: String,
}

/// GET /api/v1/datasources — list data sources (id, name, driver).
pub async fn list_data_sources(base: &str, token: &str) -> Result<Vec<DataSourceItem>, ApiError> {
    get_json(base, "/api/v1/datasources", token).await
}

/// POST /api/v1/datasources — create a new data source.
pub async fn create_data_source(
    base: &str,
    token: &str,
    name: &str,
    driver: &str,
) -> Result<CreatedRow, ApiError> {
    let body = serde_json::json!({ "name": name, "driver": driver });
    post_json(base, "/api/v1/datasources", token, &body).await
}

/// POST /api/v1/cubes — create a new cube.
/// `data_source_id` is the UUID string of the data source.
#[allow(clippy::too_many_arguments)]
pub async fn create_cube(
    base: &str,
    token: &str,
    data_source_id: &str,
    name: &str,
    title: Option<&str>,
    description: Option<&str>,
    sql_table: Option<&str>,
    base_sql: Option<&str>,
    primary_key: &str,
    tenant_column: &str,
) -> Result<CreatedRow, ApiError> {
    let body = serde_json::json!({
        "data_source_id": data_source_id,
        "name": name,
        "title": title,
        "description": description,
        "sql_table": sql_table,
        "base_sql": base_sql,
        "primary_key": primary_key,
        "tenant_column": tenant_column,
    });
    post_json(base, "/api/v1/cubes", token, &body).await
}

/// DELETE /api/v1/cubes/{id}
pub async fn delete_cube(base: &str, token: &str, cube_id: &str) -> Result<(), ApiError> {
    delete_req(base, &format!("/api/v1/cubes/{}", cube_id), token).await
}

/// POST /api/v1/cubes/{cube_id}/dimensions
pub async fn create_dimension(
    base: &str,
    token: &str,
    cube_id: &str,
    name: &str,
    sql_expr: &str,
    data_type: &str,
    description: Option<&str>,
) -> Result<CreatedRow, ApiError> {
    let body = serde_json::json!({
        "name": name,
        "sql_expr": sql_expr,
        "data_type": data_type,
        "description": description,
    });
    post_json(base, &format!("/api/v1/cubes/{}/dimensions", cube_id), token, &body).await
}

/// DELETE /api/v1/cubes/{cube_id}/dimensions/{dim_id}
pub async fn delete_dimension(
    base: &str,
    token: &str,
    cube_id: &str,
    dim_id: &str,
) -> Result<(), ApiError> {
    delete_req(base, &format!("/api/v1/cubes/{}/dimensions/{}", cube_id, dim_id), token).await
}

/// POST /api/v1/cubes/{cube_id}/measures
pub async fn create_measure(
    base: &str,
    token: &str,
    cube_id: &str,
    name: &str,
    agg_type: &str,
    sql_expr: Option<&str>,
    description: Option<&str>,
) -> Result<CreatedRow, ApiError> {
    let body = serde_json::json!({
        "name": name,
        "agg_type": agg_type,
        "sql_expr": sql_expr,
        "description": description,
    });
    post_json(base, &format!("/api/v1/cubes/{}/measures", cube_id), token, &body).await
}

/// DELETE /api/v1/cubes/{cube_id}/measures/{meas_id}
pub async fn delete_measure(
    base: &str,
    token: &str,
    cube_id: &str,
    meas_id: &str,
) -> Result<(), ApiError> {
    delete_req(base, &format!("/api/v1/cubes/{}/measures/{}", cube_id, meas_id), token).await
}

/// POST /api/v1/cubes/{cube_id}/joins
/// `target_cube_id` is the UUID string of the target cube.
pub async fn create_join(
    base: &str,
    token: &str,
    cube_id: &str,
    target_cube_id: &str,
    name: &str,
    relationship: &str,
    sql_on: &str,
) -> Result<CreatedRow, ApiError> {
    let body = serde_json::json!({
        "target_cube_id": target_cube_id,
        "name": name,
        "relationship": relationship,
        "sql_on": sql_on,
    });
    post_json(base, &format!("/api/v1/cubes/{}/joins", cube_id), token, &body).await
}

/// DELETE /api/v1/cubes/{cube_id}/joins/{join_id}
pub async fn delete_join(
    base: &str,
    token: &str,
    cube_id: &str,
    join_id: &str,
) -> Result<(), ApiError> {
    delete_req(base, &format!("/api/v1/cubes/{}/joins/{}", cube_id, join_id), token).await
}

/// POST /api/v1/cubes/{cube_id}/segments
pub async fn create_segment(
    base: &str,
    token: &str,
    cube_id: &str,
    name: &str,
    sql_expr: &str,
) -> Result<CreatedRow, ApiError> {
    let body = serde_json::json!({ "name": name, "sql_expr": sql_expr });
    post_json(base, &format!("/api/v1/cubes/{}/segments", cube_id), token, &body).await
}

/// DELETE /api/v1/cubes/{cube_id}/segments/{seg_id}
pub async fn delete_segment(
    base: &str,
    token: &str,
    cube_id: &str,
    seg_id: &str,
) -> Result<(), ApiError> {
    delete_req(base, &format!("/api/v1/cubes/{}/segments/{}", cube_id, seg_id), token).await
}
