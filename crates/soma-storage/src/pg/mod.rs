//! Postgres-backed implementation modules.

pub mod crud;
pub mod dashboard;
pub mod dsn;
pub mod model;

use soma_audit_core::{AuditEvent, Outcome};
use soma_audit_pg::LocalSink;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::error::Result;

/// Shared audit helper: writes an audit event inside an existing transaction.
/// Accessible to child modules as `super::audit_in_tx`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn audit_in_tx(
    sink: &LocalSink,
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    actor_id: Option<Uuid>,
    action: &'static str,
    resource_type: &'static str,
    resource_id: String,
    metadata: serde_json::Value,
) -> Result<()> {
    let mut ev = AuditEvent::builder(tenant_id, action, Outcome::Success)
        .source_service("soma-analytics")
        .resource(resource_type, resource_id)
        .metadata(metadata);
    if let Some(id) = actor_id {
        ev = ev.actor_id(id);
    }
    sink.record_in_tx(&ev.build(), tx).await?;
    Ok(())
}
