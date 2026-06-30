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
