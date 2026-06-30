//! Error type for soma-storage.

use soma_semantic::CompileError;

/// All errors that can be produced by the soma-storage layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation failed.
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    /// A Redis cache operation failed.
    #[error("cache error: {0}")]
    Cache(#[from] soma_infra::cache::CacheError),

    /// A crypto operation failed (DSN encrypt/decrypt).
    #[error("crypto error: {0}")]
    Crypto(#[from] soma_infra::crypto::CryptoError),

    /// The semantic compiler rejected the query.
    #[error("compile error: {0}")]
    Compile(#[from] CompileError),

    /// The requested resource was not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// The operation would violate a business rule (uniqueness, soft-delete, etc.).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Migration failed.
    #[error("migration error: {0}")]
    Migration(#[from] soma_schema::Error),

    /// Audit sink error.
    #[error("audit error: {0}")]
    Audit(#[from] soma_audit_pg::AuditPgError),

    /// Serialization / deserialization error.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
