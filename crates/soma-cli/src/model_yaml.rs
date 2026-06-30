//! YAML model file deserialization — the §5.1 authoring format.
//!
//! Format:
//! ```yaml
//! data_sources:
//!   - name: warehouse
//!     driver: postgres
//! cubes:
//!   - name: orders
//!     data_source: warehouse
//!     sql_table: public.orders
//!     primary_key: id
//!     dimensions:
//!       - { name: status, sql: status, type: string }
//!     measures:
//!       - { name: count, type: count }
//! ```

use serde::Deserialize;

/// Root of a YAML model file.
#[derive(Debug, Deserialize)]
pub struct ModelFile {
    #[serde(default)]
    pub data_sources: Vec<DataSourceDef>,
    #[serde(default)]
    pub cubes: Vec<CubeDef>,
}

/// A data source entry.
#[derive(Debug, Deserialize)]
pub struct DataSourceDef {
    pub name: String,
    pub driver: Option<String>,
}

/// A cube entry.
#[derive(Debug, Deserialize)]
pub struct CubeDef {
    pub name: String,
    pub data_source: String,
    pub sql_table: Option<String>,
    pub base_sql: Option<String>,
    pub primary_key: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub cache_ttl_secs: Option<i32>,
    /// Column used for structural tenant isolation (default: `"tenant_id"` server-side).
    pub tenant_column: Option<String>,
    #[serde(default)]
    pub dimensions: Vec<DimensionDef>,
    #[serde(default)]
    pub measures: Vec<MeasureDef>,
    #[serde(default)]
    pub joins: Vec<JoinDef>,
    #[serde(default)]
    pub segments: Vec<SegmentDef>,
}

/// A dimension entry.
#[derive(Debug, Deserialize)]
pub struct DimensionDef {
    pub name: String,
    pub sql: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub description: Option<String>,
}

/// A measure entry.
#[derive(Debug, Deserialize)]
pub struct MeasureDef {
    pub name: String,
    #[serde(rename = "type")]
    pub agg_type: String,
    pub sql: Option<String>,
    pub description: Option<String>,
}

/// A join entry.
#[derive(Debug, Deserialize)]
pub struct JoinDef {
    pub name: String,
    pub target_cube: String,
    pub relationship: String,
    pub sql: String,
}

/// A segment entry.
#[derive(Debug, Deserialize)]
pub struct SegmentDef {
    pub name: String,
    pub sql: String,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_YAML: &str = r#"
data_sources:
  - name: warehouse
    driver: postgres

cubes:
  - name: orders
    data_source: warehouse
    sql_table: public.orders
    primary_key: id
    description: "Order facts table"
    dimensions:
      - { name: status, sql: status, type: string }
      - { name: created_at, sql: created_at, type: time }
    measures:
      - { name: count, type: count }
      - { name: total_revenue, sql: amount_cents, type: sum }
    joins:
      - name: customers
        target_cube: customers
        relationship: many_to_one
        sql: "{CUBE}.customer_id = {customers}.id"
    segments:
      - { name: high_value, sql: "{CUBE}.amount_cents > 100000" }

  - name: customers
    data_source: warehouse
    sql_table: public.customers
    primary_key: id
    dimensions:
      - { name: region, sql: region, type: string }
    measures: []
"#;

    #[test]
    fn parse_example_model() {
        let model: ModelFile = serde_yaml::from_str(EXAMPLE_YAML).expect("parse");
        assert_eq!(model.data_sources.len(), 1);
        assert_eq!(model.data_sources[0].name, "warehouse");
        assert_eq!(model.cubes.len(), 2);
        assert_eq!(model.cubes[0].name, "orders");
        assert_eq!(model.cubes[0].dimensions.len(), 2);
        assert_eq!(model.cubes[0].measures.len(), 2);
        assert_eq!(model.cubes[0].joins.len(), 1);
        assert_eq!(model.cubes[0].segments.len(), 1);
        assert_eq!(model.cubes[1].name, "customers");
    }

    #[test]
    fn parse_minimal_cube() {
        let yaml = r#"
data_sources:
  - name: db
cubes:
  - name: events
    data_source: db
    sql_table: public.events
    primary_key: id
"#;
        let model: ModelFile = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(model.cubes[0].dimensions.len(), 0);
        assert_eq!(model.cubes[0].measures.len(), 0);
        assert!(model.cubes[0].tenant_column.is_none());
    }

    #[test]
    fn parse_tenant_column() {
        let yaml = r#"
data_sources:
  - name: db
cubes:
  - name: orders
    data_source: db
    sql_table: public.orders
    primary_key: id
    tenant_column: tenant_id
"#;
        let model: ModelFile = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(model.cubes[0].tenant_column.as_deref(), Some("tenant_id"));
    }

    #[test]
    fn parse_iam_yaml() {
        let yaml = include_str!("../../../models/iam.yaml");
        let model: ModelFile = serde_yaml::from_str(yaml).expect("parse iam.yaml");
        // All cubes in iam.yaml have an explicit tenant_column.
        for cube in &model.cubes {
            assert_eq!(
                cube.tenant_column.as_deref(),
                Some("tenant_id"),
                "cube '{}' should have tenant_column = tenant_id",
                cube.name
            );
        }
    }
}
