//! Rust client for the soma-analytics API.
//!
//! # Example
//!
//! ```rust,no_run
//! use soma_sdk::SomaClient;
//! use soma_semantic::SemanticQuery;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), soma_sdk::SdkError> {
//!     let client = SomaClient::new("http://localhost:8080", "your-api-token");
//!     let cubes = client.list_cubes().await?;
//!     println!("{cubes:?}");
//!
//!     let q = SemanticQuery {
//!         cube: "orders".into(),
//!         measures: vec!["orders.count".into()],
//!         dimensions: vec!["orders.status".into()],
//!         filters: vec![],
//!         segments: vec![],
//!         time_dimension: None,
//!         order: vec![],
//!         limit: Some(100),
//!         offset: None,
//!     };
//!     let rs = client.query(&q).await?;
//!     println!("{} rows", rs.meta.row_count);
//!     Ok(())
//! }
//! ```

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use soma_semantic::{RowFilter, SemanticQuery};
use uuid::Uuid;

// ── Error ─────────────────────────────────────────────────────────────────────

/// All errors soma-sdk can return.
#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    /// The server returned a non-2xx response.
    #[error("api error {status}: {body}")]
    Api { status: u16, body: String },

    /// A network / transport error from reqwest.
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),

    /// JSON deserialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

// ── Response DTOs ─────────────────────────────────────────────────────────────

/// A measure's metadata from `GET /api/v1/meta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasureMeta {
    pub name: String,
    pub agg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A dimension's metadata from `GET /api/v1/meta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionMeta {
    pub name: String,
    /// The data type string: `"string"`, `"number"`, `"time"`, `"boolean"`.
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Per-cube metadata from `GET /api/v1/meta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CubeMeta {
    pub name: String,
    pub measures: Vec<MeasureMeta>,
    pub dimensions: Vec<DimensionMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Response from `GET /api/v1/meta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaResponse {
    pub cubes: Vec<CubeMeta>,
}

/// Metadata about a result column — mirrors `soma_storage::types::ColumnMeta`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: String,
    /// `"string"` | `"number"` | `"time"` | `"boolean"`
    pub data_type: String,
}

/// Per-result-set metadata — mirrors `soma_storage::types::ResultMeta`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMeta {
    /// `"hit"` or `"miss"`.
    pub cache: String,
    /// sha256_hex fingerprint of the canonical cache key.
    pub query_fingerprint: String,
    pub row_count: usize,
}

/// A query result — mirrors `soma_storage::types::ResultSet`.
///
/// Returned by [`SomaClient::query`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub meta: ResultMeta,
}

/// Returned by `create_*` operations: the new entity's id and name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedEntity {
    pub id: Uuid,
    pub name: String,
}

/// Returned by [`SomaClient::create_token`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenResponse {
    pub id: Uuid,
    pub name: String,
    pub role: String,
    /// Plaintext token — shown once; store securely.
    pub token: String,
}

/// Returned by [`SomaClient::mint_embed_token`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedTokenResponse {
    pub token: String,
    pub expires_at: String,
}

// ── Request bodies (pub so the CLI can build them) ────────────────────────────

/// Body for `POST /api/v1/cubes`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCubeBody {
    pub data_source_id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_table: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sql: Option<String>,
    pub primary_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_ttl_secs: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_column: Option<String>,
}

/// Body for `POST /api/v1/cubes/{id}/dimensions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDimensionBody {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub sql_expr: String,
    pub data_type: String,
}

/// Body for `POST /api/v1/cubes/{id}/measures`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMeasureBody {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_expr: Option<String>,
    pub agg_type: String,
}

/// Body for `POST /api/v1/cubes/{id}/joins`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJoinBody {
    pub target_cube_id: Uuid,
    pub name: String,
    pub relationship: String,
    pub sql_on: String,
}

/// Body for `POST /api/v1/cubes/{id}/segments`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSegmentBody {
    pub name: String,
    pub sql_expr: String,
}

// ── Private helper structs ────────────────────────────────────────────────────

/// `GET /api/v1/cubes` returns `{"cubes": ["name1", "name2"]}`.
#[derive(Deserialize)]
struct CubesListResponse {
    cubes: Vec<String>,
}

#[derive(Serialize)]
struct CreateDataSourceBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    driver: Option<&'a str>,
}

#[derive(Serialize)]
struct CreateTokenBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'a str>,
}

#[derive(Serialize)]
struct MintEmbedTokenBody {
    user_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    row_filters: Vec<RowFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cube: Option<String>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Async client for the soma-analytics REST API.
///
/// Built on `soma_infra::http::client()` — connection pooling is handled inside
/// the underlying `reqwest::Client`. Do not wrap this in an `Arc` pool.
///
/// # Construction
///
/// ```rust,no_run
/// let client = soma_sdk::SomaClient::new("http://localhost:8080", "your-api-token");
/// ```
pub struct SomaClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl SomaClient {
    /// Create a new client.
    ///
    /// `base_url` is the soma-analytics server root (e.g. `"http://localhost:8080"`).
    /// `api_key` is a Bearer token with at least Reader role.
    ///
    /// # Panics
    ///
    /// Panics if `soma_infra::http::client()` fails to build the underlying
    /// `reqwest::Client` (e.g. TLS initialisation failure).
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let http = soma_infra::http::client().expect("reqwest client build failed");
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            api_key: api_key.into(),
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, SdkError> {
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(SdkError::Api { status, body })
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response, SdkError> {
        let url = format!("{}/api/v1{}", self.base_url, path);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        Self::check_response(resp).await
    }

    async fn post<B: Serialize>(&self, path: &str, body: &B) -> Result<reqwest::Response, SdkError> {
        let url = format!("{}/api/v1{}", self.base_url, path);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(body)
            .send()
            .await?;
        Self::check_response(resp).await
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// `GET /api/v1/meta` — governed model (cubes, measures, dimensions with descriptions).
    ///
    /// Requires Reader role or higher.
    pub async fn meta(&self) -> Result<MetaResponse, SdkError> {
        let resp = self.get("/meta").await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/query` — execute a semantic query.
    ///
    /// Requires Reader role or higher. Results may be served from cache.
    pub async fn query(&self, q: &SemanticQuery) -> Result<ResultSet, SdkError> {
        let resp = self.post("/query", q).await?;
        Ok(resp.json().await?)
    }

    /// `GET /api/v1/cubes` — list cube names for the authenticated tenant.
    ///
    /// Requires Reader role or higher.
    pub async fn list_cubes(&self) -> Result<Vec<String>, SdkError> {
        let resp = self.get("/cubes").await?;
        let body: CubesListResponse = resp.json().await?;
        Ok(body.cubes)
    }

    /// `POST /api/v1/datasources` — register a data source.
    ///
    /// Requires Admin role. `driver` defaults to `"postgres"` server-side.
    pub async fn create_data_source(
        &self,
        name: &str,
        driver: Option<&str>,
    ) -> Result<CreatedEntity, SdkError> {
        let body = CreateDataSourceBody { name, driver };
        let resp = self.post("/datasources", &body).await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/cubes` — create a cube.
    ///
    /// Requires Editor role or higher.
    pub async fn create_cube(&self, body: &CreateCubeBody) -> Result<CreatedEntity, SdkError> {
        let resp = self.post("/cubes", body).await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/cubes/{cube_id}/dimensions` — add a dimension to a cube.
    ///
    /// Requires Editor role or higher.
    pub async fn create_dimension(
        &self,
        cube_id: Uuid,
        body: &CreateDimensionBody,
    ) -> Result<CreatedEntity, SdkError> {
        let path = format!("/cubes/{cube_id}/dimensions");
        let resp = self.post(&path, body).await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/cubes/{cube_id}/measures` — add a measure to a cube.
    ///
    /// Requires Editor role or higher.
    pub async fn create_measure(
        &self,
        cube_id: Uuid,
        body: &CreateMeasureBody,
    ) -> Result<CreatedEntity, SdkError> {
        let path = format!("/cubes/{cube_id}/measures");
        let resp = self.post(&path, body).await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/cubes/{cube_id}/joins` — add a join to a cube.
    ///
    /// Requires Editor role or higher.
    pub async fn create_join(
        &self,
        cube_id: Uuid,
        body: &CreateJoinBody,
    ) -> Result<CreatedEntity, SdkError> {
        let path = format!("/cubes/{cube_id}/joins");
        let resp = self.post(&path, body).await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/cubes/{cube_id}/segments` — add a segment to a cube.
    ///
    /// Requires Editor role or higher.
    pub async fn create_segment(
        &self,
        cube_id: Uuid,
        body: &CreateSegmentBody,
    ) -> Result<CreatedEntity, SdkError> {
        let path = format!("/cubes/{cube_id}/segments");
        let resp = self.post(&path, body).await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/tokens` — create an API token.
    ///
    /// Requires Admin role. The plaintext token is returned once — store securely.
    pub async fn create_token(
        &self,
        name: &str,
        role: Option<&str>,
    ) -> Result<CreateTokenResponse, SdkError> {
        let body = CreateTokenBody { name, role };
        let resp = self.post("/tokens", &body).await?;
        Ok(resp.json().await?)
    }

    /// `POST /api/v1/embed/token` — mint a scoped embed token.
    ///
    /// Requires Editor role or higher. `user_id` identifies the end-user in audit
    /// logs. `row_filters` are injected as bound equality conditions on every query
    /// (never raw SQL). `cube` optionally locks the token to a single cube.
    ///
    /// The returned token expires in 10 minutes.
    pub async fn mint_embed_token(
        &self,
        user_id: &str,
        row_filters: Vec<RowFilter>,
        cube: Option<String>,
    ) -> Result<EmbedTokenResponse, SdkError> {
        let body = MintEmbedTokenBody {
            user_id: user_id.to_owned(),
            row_filters,
            cube,
        };
        let resp = self.post("/embed/token", &body).await?;
        Ok(resp.json().await?)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_semantic_query() {
        let q = SemanticQuery {
            cube: "orders".into(),
            measures: vec!["orders.count".into(), "orders.total_revenue".into()],
            dimensions: vec!["orders.status".into()],
            filters: vec![],
            segments: vec![],
            time_dimension: None,
            order: vec![],
            limit: Some(100),
            offset: None,
        };
        let json = serde_json::to_string(&q).unwrap();
        let q2: SemanticQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(q.cube, q2.cube);
        assert_eq!(q.measures, q2.measures);
        assert_eq!(q.dimensions, q2.dimensions);
        assert_eq!(q.limit, q2.limit);
    }

    #[test]
    fn serde_round_trip_result_set() {
        let rs = ResultSet {
            columns: vec![
                ColumnMeta { name: "orders.status".into(), data_type: "string".into() },
                ColumnMeta { name: "orders.count".into(), data_type: "number".into() },
            ],
            rows: vec![vec![serde_json::json!("completed"), serde_json::json!(42)]],
            meta: ResultMeta {
                cache: "miss".into(),
                query_fingerprint: "sha256:abc123".into(),
                row_count: 1,
            },
        };
        let json = serde_json::to_string(&rs).unwrap();
        let rs2: ResultSet = serde_json::from_str(&json).unwrap();
        assert_eq!(rs2.columns.len(), 2);
        assert_eq!(rs2.rows[0][0], serde_json::json!("completed"));
        assert_eq!(rs2.meta.row_count, 1);
        assert_eq!(rs2.meta.cache, "miss");
    }

    #[test]
    fn serde_round_trip_meta_response() {
        let meta = MetaResponse {
            cubes: vec![CubeMeta {
                name: "orders".into(),
                description: Some("Order facts".into()),
                measures: vec![MeasureMeta {
                    name: "count".into(),
                    agg_type: "count".into(),
                    description: None,
                }],
                dimensions: vec![DimensionMeta {
                    name: "status".into(),
                    data_type: "string".into(),
                    description: None,
                }],
            }],
        };
        let json = serde_json::to_string(&meta).unwrap();
        let meta2: MetaResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(meta2.cubes[0].name, "orders");
        assert_eq!(meta2.cubes[0].measures[0].agg_type, "count");
    }
}
