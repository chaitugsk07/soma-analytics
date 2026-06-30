//! soma-storage — the metadata DB + result-cache layer for soma-analytics.
//!
//! Provides:
//! - `migrate(&pool)` — runs soma-schema migrations at startup.
//! - `Store` — the central data-access handle (model CRUD, query execution,
//!   result cache, dashboard/panel CRUD).
//!
//! All crypto, pooling, caching, and error-redaction primitives are consumed
//! from soma-infra. Nothing is hand-rolled here.

pub mod cache_key;
pub mod error;
mod pg;
pub mod store;
pub mod types;

pub use error::{Error, Result};
pub use store::Store;
pub use types::{
    ColumnMeta, CompileResult, FullCube, FullDimension, FullJoin, FullMeasure, FullModel,
    FullSegment, ResultMeta, ResultSet,
};

use soma_schema::{Migrator, PostgresConfig, PostgresDriver};
use sqlx::PgPool;

/// Run all pending migrations for the `soma_analytics` schema.
///
/// Uses an advisory lock (`0x50A1_A7C5_0001`) unique to this service on the
/// shared Postgres instance — confirmed free across vault (`0x50A1_7A01_7017`),
/// audit (`6020250626000001`), and iam (`7318249506742315`).
///
/// Called at startup before the HTTP server starts accepting traffic.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    let config = PostgresConfig {
        schema: Some("soma_analytics".into()),
        advisory_lock_key: 0x50A1_A7C5_0001_i64,
        ..Default::default()
    };
    let driver = PostgresDriver::new(pool.clone(), config)?;
    let migrator =
        Migrator::from_root(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"));
    migrator.up(&driver).await?;
    Ok(())
}
