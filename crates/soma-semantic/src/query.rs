use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Sort direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    Asc,
    Desc,
}

/// Time-series granularity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Granularity {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Granularity {
    /// The Postgres `date_trunc` unit string.
    pub fn trunc_unit(&self) -> &'static str {
        match self {
            Granularity::Day => "day",
            Granularity::Week => "week",
            Granularity::Month => "month",
            Granularity::Quarter => "quarter",
            Granularity::Year => "year",
        }
    }
}

/// Filter operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// A filter applied to a member value.  Values become bind parameters — never interpolated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    pub member: String,
    pub operator: FilterOp,
    #[serde(default)]
    pub values: Vec<String>,
}

/// A time-dimension selector with optional granularity and date range.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeDimension {
    pub member: String,
    pub granularity: Granularity,
    /// `[inclusive_lower, inclusive_upper]`.  The compiler converts the upper bound to
    /// an exclusive `<` bound internally.
    pub date_range: Option<[String; 2]>,
}

/// A semantic query — the public caller input.
/// All member strings use `"cube.member"` notation.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// A security-scoped row filter, resolved from the embed token by the API layer.
/// `member` must resolve through the cube's dimension whitelist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowFilter {
    pub member: String,
    pub value: String,
}

/// Security context — injected by the API layer from the verified `Principal`.
/// NEVER supplied by the caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryScope {
    pub tenant_id: Uuid,
    #[serde(default)]
    pub row_filters: Vec<RowFilter>,
}
