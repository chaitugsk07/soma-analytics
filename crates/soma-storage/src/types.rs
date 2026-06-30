//! Result-set types shared across the storage and API layers.

use serde::{Deserialize, Serialize};

/// Metadata about a result column.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,  // "string" | "number" | "time" | "boolean"
}

/// A query result with typed columns and row data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub meta: ResultMeta,
}

/// Per-result metadata: cache status, fingerprint, row count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultMeta {
    /// "hit" if served from cache, "miss" if the data-source pool was queried.
    pub cache: String,
    /// sha256_hex fingerprint of the canonical cache key (the query identity).
    pub query_fingerprint: String,
    pub row_count: usize,
}

// ── FullModel DTO (for GET /api/v1/model — Editor+ builder export) ────────────

/// A full export of a cube's model, including all SQL expressions and metadata.
/// Returned by GET /api/v1/model (Editor+). Exposes sql_expr fields which /meta hides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullModel {
    pub cubes: Vec<FullCube>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullCube {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub data_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql_table: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sql: Option<String>,
    pub primary_key: String,
    pub tenant_column: String,
    pub dimensions: Vec<FullDimension>,
    pub measures: Vec<FullMeasure>,
    pub joins: Vec<FullJoin>,
    pub segments: Vec<FullSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullDimension {
    pub id: String,
    pub name: String,
    pub sql: String,
    #[serde(rename = "type")]
    pub data_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullMeasure {
    pub id: String,
    pub name: String,
    pub agg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullJoin {
    pub id: String,
    pub name: String,
    pub target_cube: String,
    pub relationship: String,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FullSegment {
    pub id: String,
    pub name: String,
    pub sql: String,
}

// ── CompileResult DTO (for POST /api/v1/query/compile — Reader+) ──────────────

/// Returned by POST /api/v1/query/compile: the generated SQL with $N placeholders
/// (bind values are NOT included), the output column list, and the parameter count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileResult {
    pub sql: String,
    pub columns: Vec<ColumnMeta>,
    pub param_count: usize,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// FullModel serializes all fields including optional sql_table, title, description.
    #[test]
    fn full_model_serializes_all_fields() {
        let model = FullModel {
            cubes: vec![FullCube {
                id: "cube-uuid-here".into(),
                name: "orders".into(),
                title: Some("Orders".into()),
                description: Some("Order records".into()),
                data_source: "ds-uuid-here".into(),
                sql_table: Some("public.orders".into()),
                base_sql: None,
                primary_key: "id".into(),
                tenant_column: "tenant_id".into(),
                dimensions: vec![FullDimension {
                    id: "dim-uuid-here".into(),
                    name: "status".into(),
                    sql: "{CUBE}.status".into(),
                    data_type: "string".into(),
                    description: Some("Order status".into()),
                }],
                measures: vec![FullMeasure {
                    id: "meas-uuid-here".into(),
                    name: "count".into(),
                    agg_type: "count".into(),
                    sql: None,
                    description: None,
                }],
                joins: vec![FullJoin {
                    id: "join-uuid-here".into(),
                    name: "customers".into(),
                    target_cube: "customers".into(),
                    relationship: "many_to_one".into(),
                    sql: "{CUBE}.customer_id = {customers}.id".into(),
                }],
                segments: vec![FullSegment {
                    id: "seg-uuid-here".into(),
                    name: "active".into(),
                    sql: "{CUBE}.is_active = true".into(),
                }],
            }],
        };

        let v = serde_json::to_value(&model).unwrap();
        let cube = &v["cubes"][0];

        assert_eq!(cube["id"], "cube-uuid-here");
        assert_eq!(cube["name"], "orders");
        assert_eq!(cube["title"], "Orders");
        assert_eq!(cube["description"], "Order records");
        assert_eq!(cube["sql_table"], "public.orders");
        assert_eq!(cube["primary_key"], "id");
        assert_eq!(cube["tenant_column"], "tenant_id");

        let dim = &cube["dimensions"][0];
        assert_eq!(dim["id"], "dim-uuid-here");
        assert_eq!(dim["name"], "status");
        assert_eq!(dim["sql"], "{CUBE}.status");
        assert_eq!(dim["type"], "string");
        assert_eq!(dim["description"], "Order status");

        let meas = &cube["measures"][0];
        assert_eq!(meas["id"], "meas-uuid-here");
        assert_eq!(meas["name"], "count");
        assert_eq!(meas["agg_type"], "count");
        // sql is None → should be absent (skip_serializing_if)
        assert!(meas.get("sql").is_none_or(|v| v.is_null()));

        let join = &cube["joins"][0];
        assert_eq!(join["id"], "join-uuid-here");
        assert_eq!(join["name"], "customers");
        assert_eq!(join["target_cube"], "customers");
        assert_eq!(join["relationship"], "many_to_one");
        assert_eq!(join["sql"], "{CUBE}.customer_id = {customers}.id");

        let seg = &cube["segments"][0];
        assert_eq!(seg["id"], "seg-uuid-here");
        assert_eq!(seg["name"], "active");
        assert_eq!(seg["sql"], "{CUBE}.is_active = true");
    }

    /// Optional fields (title, description, sql_table, base_sql) are absent from JSON
    /// when None (skip_serializing_if = "Option::is_none").
    #[test]
    fn full_cube_omits_none_optional_fields() {
        let model = FullModel {
            cubes: vec![FullCube {
                id: "cube-uuid".into(),
                name: "orders".into(),
                title: None,
                description: None,
                data_source: "ds-uuid".into(),
                sql_table: None,
                base_sql: Some("SELECT * FROM orders".into()),
                primary_key: "id".into(),
                tenant_column: "tenant_id".into(),
                dimensions: vec![],
                measures: vec![],
                joins: vec![],
                segments: vec![],
            }],
        };
        let v = serde_json::to_value(&model).unwrap();
        let cube = &v["cubes"][0];
        assert!(cube.get("title").is_none(), "title should be absent when None");
        assert!(cube.get("description").is_none(), "description should be absent when None");
        assert!(cube.get("sql_table").is_none(), "sql_table should be absent when None");
        assert_eq!(cube["base_sql"], "SELECT * FROM orders");
    }

    /// CompileResult serializes sql, columns, and param_count.
    #[test]
    fn compile_result_serializes_correctly() {
        let result = CompileResult {
            sql: "SELECT count(*) AS \"orders.count\" FROM public.orders AS \"orders\" WHERE \"tenant_id\" = $1".into(),
            columns: vec![ColumnMeta { name: "orders.count".into(), data_type: "number".into() }],
            param_count: 1,
        };
        let v = serde_json::to_value(&result).unwrap();
        assert!(v["sql"].as_str().unwrap().contains("SELECT"));
        assert_eq!(v["param_count"], 1);
        assert_eq!(v["columns"][0]["name"], "orders.count");
        assert_eq!(v["columns"][0]["data_type"], "number");
    }

    /// Verify that soma_semantic::compile returns a CompileError for an unknown member,
    /// and that we can construct a CompileResult from a valid compile output.
    /// This mirrors what Store::compile_query does (load_model + compile) without DB I/O.
    #[test]
    fn compile_query_helper_valid_query_produces_sql() {
        use soma_semantic::{AggType, Cube, DataType, Dimension, Measure, Model, QueryScope, SemanticQuery};
        use uuid::Uuid;

        let model = Model::from_cubes(vec![Cube {
            name: "orders".into(),
            data_source: "default".into(),
            sql_table: Some("public.orders".into()),
            base_sql: None,
            primary_key: "id".into(),
            tenant_column: "tenant_id".into(),
            description: None,
            dimensions: vec![Dimension {
                name: "status".into(),
                sql_expr: "{CUBE}.status".into(),
                data_type: DataType::String,
                description: None,
            }],
            measures: vec![Measure {
                name: "count".into(),
                sql_expr: None,
                agg_type: AggType::Count,
                description: None,
            }],
            joins: vec![],
            segments: vec![],
        }])
        .unwrap();

        let q = SemanticQuery {
            cube: "orders".into(),
            measures: vec!["orders.count".into()],
            dimensions: vec![],
            filters: vec![],
            segments: vec![],
            time_dimension: None,
            order: vec![],
            limit: None,
            offset: None,
        };
        let scope = QueryScope { tenant_id: Uuid::nil(), row_filters: vec![] };

        let compiled = soma_semantic::compile(&model, &q, &scope).unwrap();

        // Construct a CompileResult the same way Store::compile_query does.
        let param_count = compiled.binds.len();
        let columns: Vec<ColumnMeta> = compiled
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
        let result = CompileResult { sql: compiled.sql, columns, param_count };

        assert!(result.sql.contains("SELECT"), "sql should be a SELECT statement");
        // param_count = 1: just the tenant predicate ($1)
        assert_eq!(result.param_count, 1);
        assert_eq!(result.columns[0].name, "orders.count");
        assert_eq!(result.columns[0].data_type, "number");
    }

    /// Verify that soma_semantic::compile returns a CompileError for an unknown member,
    /// which Store::compile_query maps to Err(Error::Compile(_)) → 422 in the handler.
    #[test]
    fn compile_query_helper_unknown_member_returns_compile_error() {
        use soma_semantic::{compile, AggType, Cube, Measure, Model, QueryScope, SemanticQuery};
        use uuid::Uuid;

        let model = Model::from_cubes(vec![Cube {
            name: "orders".into(),
            data_source: "default".into(),
            sql_table: Some("public.orders".into()),
            base_sql: None,
            primary_key: "id".into(),
            tenant_column: "tenant_id".into(),
            description: None,
            dimensions: vec![],
            measures: vec![Measure {
                name: "count".into(),
                sql_expr: None,
                agg_type: AggType::Count,
                description: None,
            }],
            joins: vec![],
            segments: vec![],
        }])
        .unwrap();

        // "orders.nonexistent_dimension" is not in the model
        let q = SemanticQuery {
            cube: "orders".into(),
            measures: vec![],
            dimensions: vec!["orders.nonexistent_dimension".into()],
            filters: vec![],
            segments: vec![],
            time_dimension: None,
            order: vec![],
            limit: None,
            offset: None,
        };
        let scope = QueryScope { tenant_id: Uuid::nil(), row_filters: vec![] };

        let err = compile(&model, &q, &scope).unwrap_err();
        assert!(
            matches!(err, soma_semantic::CompileError::UnknownMember(_)),
            "expected UnknownMember compile error, got {err:?}"
        );
    }
}
