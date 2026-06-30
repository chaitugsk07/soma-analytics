use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompileError {
    #[error("unknown cube: {0}")]
    UnknownCube(String),
    #[error("unknown member: {0}")]
    UnknownMember(String),
    #[error("unknown segment: {0}")]
    UnknownSegment(String),
    // Note: UndeclaredJoinCardinality was removed — join cardinality is a type-level invariant
    // (`relationship` is a required field on `Join`) enforced at model-parse time + DB CHECK, not
    // a runtime compile error.
    #[error("member '{member}' is not reachable from cube '{from_cube}'")]
    UnreachableMember { member: String, from_cube: String },
    #[error("fan-out not supported: {0}")]
    UnsupportedFanout(String),
    #[error(
        "non-additive measure '{measure}' on cube '{cube}' (agg_type={agg_type}) cannot be \
         combined in a fan-out join"
    )]
    NonAdditiveMeasureInFanout {
        measure: String,
        cube: String,
        agg_type: String,
    },
    #[error("invalid filter on '{member}': {reason}")]
    InvalidFilter { member: String, reason: String },
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("invalid sql fragment: {0}")]
    InvalidSqlFragment(String),
    #[error("invalid sql_table '{value}': must be a qualified identifier (schema.table or table)")]
    InvalidSqlTable { value: String },
    #[error("unknown token '{{{token}}}' in sql_on for join '{join_name}': must be {{CUBE}} or a declared cube name")]
    UnknownJoinToken { token: String, join_name: String },
}
