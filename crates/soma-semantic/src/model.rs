use crate::error::ModelError;
use crate::validate::validate_sql_fragment;
use serde::{Deserialize, Serialize};

/// Validate that `sql_table` is a safe qualified identifier: `[schema.]table`.
/// Only identifier characters (A-Za-z0-9_) and an optional single dot are allowed.
fn validate_sql_table_ident(value: &str) -> Result<(), ModelError> {
    fn is_ident(s: &str) -> bool {
        let mut chars = s.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    let ok = match value.splitn(2, '.').collect::<Vec<_>>().as_slice() {
        [table] => is_ident(table),
        [schema, table] => is_ident(schema) && is_ident(table),
        _ => false,
    };

    if ok {
        Ok(())
    } else {
        Err(ModelError::InvalidSqlTable { value: value.to_string() })
    }
}

/// Validate that every `{token}` in a join's `sql_on` is either `{CUBE}` or a declared cube name.
/// `join_name` is the join alias; `source_cube` is the cube owning this join; `target_cube` is
/// the join's target.  Those two cube names (plus the literal `CUBE`) are the only valid tokens.
fn validate_join_tokens(
    sql_on: &str,
    join_name: &str,
    source_cube: &str,
    target_cube: &str,
) -> Result<(), ModelError> {
    let mut rest = sql_on;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('}') {
            let token = &rest[..close];
            rest = &rest[close + 1..];
            if token == "CUBE" || token == source_cube || token == target_cube {
                continue;
            }
            return Err(ModelError::UnknownJoinToken {
                token: token.to_string(),
                join_name: join_name.to_string(),
            });
        }
        // Unclosed brace — no token, skip.
    }
    Ok(())
}

/// The aggregation type for a measure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggType {
    Count,
    CountDistinct,
    Sum,
    Avg,
    Min,
    Max,
    /// A raw expression — no aggregation wrapper.
    Number,
}

/// The data type of a dimension column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    String,
    Number,
    Time,
    Boolean,
}

/// Join cardinality between two cubes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    ManyToOne,
    OneToMany,
    OneToOne,
}

/// A named dimension on a cube.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    pub name: String,
    /// The SQL expression for this dimension (may use `{CUBE}` token).
    #[serde(alias = "sql")]
    pub sql_expr: String,
    #[serde(rename = "type", alias = "data_type")]
    pub data_type: DataType,
    #[serde(default)]
    pub description: Option<String>,
}

/// A named measure on a cube.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measure {
    pub name: String,
    /// The column / expression to aggregate. Optional for `count`.
    #[serde(alias = "sql", default)]
    pub sql_expr: Option<String>,
    #[serde(rename = "type", alias = "agg_type")]
    pub agg_type: AggType,
    #[serde(default)]
    pub description: Option<String>,
}

/// A single-level join from this cube to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Join {
    /// The join alias / name (used in member references, e.g. `customers`).
    pub name: String,
    /// The name of the target cube.
    #[serde(alias = "target_cube")]
    pub target_cube: String,
    pub relationship: Relationship,
    /// The ON expression (may use `{CUBE}` and `{target}` tokens).
    #[serde(alias = "sql")]
    pub sql_on: String,
}

/// A named, reusable filter predicate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub name: String,
    /// The SQL predicate (may use `{CUBE}` token). Inlined — trusted model content.
    #[serde(alias = "sql")]
    pub sql_expr: String,
}

/// A cube: one logical table / view with typed dimensions, typed measures, joins, and segments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cube {
    pub name: String,
    pub data_source: String,
    /// A fully-qualified table name, e.g. `public.orders`.
    pub sql_table: Option<String>,
    /// A raw SQL SELECT used as a subquery instead of a table reference.
    pub base_sql: Option<String>,
    pub primary_key: String,
    /// The column in the base table used for structural tenant isolation.
    /// The compiler emits `{tenant_column} = $N` as a mandatory WHERE predicate.
    pub tenant_column: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub dimensions: Vec<Dimension>,
    #[serde(default)]
    pub measures: Vec<Measure>,
    #[serde(default)]
    pub joins: Vec<Join>,
    #[serde(default)]
    pub segments: Vec<Segment>,
}

impl Cube {
    pub fn measure(&self, name: &str) -> Option<&Measure> {
        self.measures.iter().find(|m| m.name == name)
    }

    pub fn dimension(&self, name: &str) -> Option<&Dimension> {
        self.dimensions.iter().find(|d| d.name == name)
    }

    pub fn segment(&self, name: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.name == name)
    }
}

/// The compiled, tenant-specific model — a collection of cubes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub cubes: Vec<Cube>,
}

impl Model {
    /// Build a model from a list of cubes, validating all sql_expr / sql_on / base_sql fragments.
    pub fn from_cubes(cubes: Vec<Cube>) -> Result<Self, ModelError> {
        for cube in &cubes {
            // Fix 1: validate sql_table is a safe qualified identifier, not arbitrary SQL.
            if let Some(sql_table) = &cube.sql_table {
                validate_sql_table_ident(sql_table)?;
            }
            if let Some(base) = &cube.base_sql {
                validate_sql_fragment(base)?;
            }
            for dim in &cube.dimensions {
                validate_sql_fragment(&dim.sql_expr)?;
            }
            for m in &cube.measures {
                if let Some(expr) = &m.sql_expr {
                    validate_sql_fragment(expr)?;
                }
            }
            // Fix 10: validate {token} in sql_on — must be {CUBE} or a declared cube name.
            for join in &cube.joins {
                validate_sql_fragment(&join.sql_on)?;
                validate_join_tokens(&join.sql_on, &join.name, &cube.name, &join.target_cube)?;
            }
            for seg in &cube.segments {
                validate_sql_fragment(&seg.sql_expr)?;
            }
        }
        Ok(Model { cubes })
    }

    /// Look up a cube by name.
    pub fn cube(&self, name: &str) -> Option<&Cube> {
        self.cubes.iter().find(|c| c.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare_cube(sql_table: Option<&str>, sql_on: Option<&str>) -> Cube {
        let joins = if let Some(on) = sql_on {
            vec![Join {
                name: "customers".into(),
                target_cube: "customers".into(),
                relationship: crate::model::Relationship::ManyToOne,
                sql_on: on.into(),
            }]
        } else {
            vec![]
        };
        Cube {
            name: "orders".into(),
            data_source: "default".into(),
            sql_table: sql_table.map(|s| s.to_string()),
            base_sql: None,
            primary_key: "id".into(),
            tenant_column: "tenant_id".into(),
            description: None,
            dimensions: vec![],
            measures: vec![],
            joins,
            segments: vec![],
        }
    }

    // Fix 1: sql_table injection
    #[test]
    fn fix1_sql_table_injection_rejected() {
        let cube = bare_cube(Some("public.orders UNION SELECT 1 --"), None);
        assert!(
            Model::from_cubes(vec![cube]).is_err(),
            "expected sql_table with UNION+comment to be rejected"
        );
    }

    #[test]
    fn fix1_valid_sql_table_accepted() {
        let cube = bare_cube(Some("public.orders"), None);
        assert!(Model::from_cubes(vec![cube]).is_ok());
    }

    #[test]
    fn fix1_unqualified_sql_table_accepted() {
        let cube = bare_cube(Some("orders"), None);
        assert!(Model::from_cubes(vec![cube]).is_ok());
    }

    // Fix 10: sql_on token validation
    #[test]
    fn fix10_unknown_token_in_sql_on_rejected() {
        let cube = bare_cube(Some("public.orders"), Some("{CUBE}.x = {bogus}.y"));
        assert!(
            Model::from_cubes(vec![cube]).is_err(),
            "expected unknown {{bogus}} token to be rejected"
        );
    }

    #[test]
    fn fix10_cube_token_in_sql_on_accepted() {
        let cube = bare_cube(Some("public.orders"), Some("{CUBE}.id = {customers}.id"));
        assert!(
            Model::from_cubes(vec![cube]).is_ok(),
            "expected {{CUBE}} and {{customers}} tokens to be accepted"
        );
    }
}
