//! Cache-key construction for the result cache.
//!
//! Key format:  "saq:v1:" + sha256_hex(canonical_json)
//!
//! Canonical JSON guarantees the same query always maps to the same key
//! regardless of member declaration order, so that:
//!   - `measures: ["a","b"]` == `measures: ["b","a"]`  → same key
//!   - `row_filters` sorted by (member, value)          → same key
//!   - different `model_version`                        → different key (stale cache unreachable)
//!   - different `tenant_id`                            → different key (tenant isolation)
//!
//! Tenant isolation note: `tenant_id` and `row_filters` are both injected by
//! the API layer from the verified `Principal`; the storage layer trusts the
//! `QueryScope` it receives as already tenant-scoped (see `Store::run_query`).

use soma_infra::crypto::sha256_hex;
use soma_semantic::{QueryScope, SemanticQuery};
use uuid::Uuid;

/// Construct the canonical cache key for a result-set lookup.
///
/// `model_version` is the `03_fct_cubes.model_version` of the root cube at
/// the time the query is compiled.  Any model mutation bumps this value, making
/// all prior cache entries for that cube unreachable (they expire by TTL).
pub fn build_cache_key(
    tenant_id: Uuid,
    q: &SemanticQuery,
    scope: &QueryScope,
    model_version: i32,
) -> String {
    let canonical = canonical_json(tenant_id, q, scope, model_version);
    let fingerprint = sha256_hex(canonical.as_bytes());
    format!("saq:v1:{fingerprint}")
}

/// Return just the sha256_hex fingerprint (used in `ResultMeta`).
pub fn query_fingerprint(
    tenant_id: Uuid,
    q: &SemanticQuery,
    scope: &QueryScope,
    model_version: i32,
) -> String {
    let canonical = canonical_json(tenant_id, q, scope, model_version);
    format!("sha256:{}", sha256_hex(canonical.as_bytes()))
}

/// Build a deterministic JSON string from the query fields.
///
/// Rules:
/// - `measures`, `dimensions`, `segments` are sorted lexicographically.
/// - `filters` are sorted by `(member, operator, values[0])` for stability.
/// - `row_filters` are sorted by `(member, value)`.
/// - `order` preserves caller order (order is semantically significant).
/// - `tenant_id` and `model_version` are included for correctness.
fn canonical_json(
    tenant_id: Uuid,
    q: &SemanticQuery,
    scope: &QueryScope,
    model_version: i32,
) -> String {
    let mut measures = q.measures.clone();
    measures.sort();

    let mut dimensions = q.dimensions.clone();
    dimensions.sort();

    let mut segments = q.segments.clone();
    segments.sort();

    // Filters: sort by (member, operator debug string, first value).
    let mut filters = q.filters.clone();
    filters.sort_by(|a, b| {
        a.member
            .cmp(&b.member)
            .then_with(|| format!("{:?}", a.operator).cmp(&format!("{:?}", b.operator)))
            .then_with(|| {
                a.values
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("")
                    .cmp(b.values.first().map(|s| s.as_str()).unwrap_or(""))
            })
    });

    // Row filters: sorted by (member, value) for determinism.
    let mut row_filters = scope.row_filters.clone();
    row_filters.sort_by(|a, b| a.member.cmp(&b.member).then_with(|| a.value.cmp(&b.value)));

    let doc = serde_json::json!({
        "tenant_id": tenant_id,
        "cube": q.cube,
        "measures": measures,
        "dimensions": dimensions,
        "segments": segments,
        "filters": filters,
        "time_dimension": q.time_dimension,
        "order": q.order,
        "limit": q.limit,
        "offset": q.offset,
        "row_filters": row_filters,
        "model_version": model_version,
    });

    // serde_json serializes map keys in insertion order for Value::Object.
    // We build the entire doc as a single serde_json::json! call so the
    // key order is deterministic at the source level.
    doc.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soma_semantic::{Filter, FilterOp, RowFilter, SemanticQuery};
    use uuid::Uuid;

    fn empty_scope(tenant_id: Uuid) -> QueryScope {
        QueryScope { tenant_id, row_filters: vec![] }
    }

    fn base_query() -> SemanticQuery {
        SemanticQuery {
            cube: "orders".into(),
            measures: vec!["orders.count".into(), "orders.revenue".into()],
            dimensions: vec!["orders.status".into()],
            filters: vec![],
            segments: vec![],
            time_dimension: None,
            order: vec![],
            limit: None,
            offset: None,
        }
    }

    #[test]
    fn same_query_different_measure_order_same_key() {
        let tid = Uuid::new_v4();
        let mut q1 = base_query();
        let mut q2 = base_query();
        q1.measures = vec!["orders.count".into(), "orders.revenue".into()];
        q2.measures = vec!["orders.revenue".into(), "orders.count".into()];

        let scope = empty_scope(tid);
        let k1 = build_cache_key(tid, &q1, &scope, 1);
        let k2 = build_cache_key(tid, &q2, &scope, 1);
        assert_eq!(k1, k2, "different measure order must produce the same cache key");
    }

    #[test]
    fn same_query_different_dimension_order_same_key() {
        let tid = Uuid::new_v4();
        let mut q1 = base_query();
        let mut q2 = base_query();
        q1.dimensions = vec!["orders.status".into(), "orders.region".into()];
        q2.dimensions = vec!["orders.region".into(), "orders.status".into()];

        let scope = empty_scope(tid);
        let k1 = build_cache_key(tid, &q1, &scope, 1);
        let k2 = build_cache_key(tid, &q2, &scope, 1);
        assert_eq!(k1, k2, "different dimension order must produce the same cache key");
    }

    #[test]
    fn different_model_version_different_key() {
        let tid = Uuid::new_v4();
        let q = base_query();
        let scope = empty_scope(tid);
        let k1 = build_cache_key(tid, &q, &scope, 1);
        let k2 = build_cache_key(tid, &q, &scope, 2);
        assert_ne!(k1, k2, "model_version bump must produce a different cache key");
    }

    #[test]
    fn different_tenant_different_key() {
        let t1 = Uuid::new_v4();
        let t2 = Uuid::new_v4();
        let q = base_query();
        let s1 = empty_scope(t1);
        let s2 = empty_scope(t2);
        assert_ne!(
            build_cache_key(t1, &q, &s1, 1),
            build_cache_key(t2, &q, &s2, 1),
            "different tenant must produce a different cache key"
        );
    }

    #[test]
    fn row_filters_different_order_same_key() {
        let tid = Uuid::new_v4();
        let q = base_query();

        let rf_a = RowFilter { member: "orders.region".into(), value: "EU".into() };
        let rf_b = RowFilter { member: "orders.status".into(), value: "active".into() };

        let s1 = QueryScope { tenant_id: tid, row_filters: vec![rf_a.clone(), rf_b.clone()] };
        let s2 = QueryScope { tenant_id: tid, row_filters: vec![rf_b, rf_a] };

        assert_eq!(
            build_cache_key(tid, &q, &s1, 1),
            build_cache_key(tid, &q, &s2, 1),
            "row_filters in different order must produce the same cache key"
        );
    }

    #[test]
    fn different_filter_value_different_key() {
        let tid = Uuid::new_v4();
        let mut q1 = base_query();
        let mut q2 = base_query();
        q1.filters = vec![Filter {
            member: "orders.status".into(),
            operator: FilterOp::Equals,
            values: vec!["active".into()],
        }];
        q2.filters = vec![Filter {
            member: "orders.status".into(),
            operator: FilterOp::Equals,
            values: vec!["inactive".into()],
        }];

        let scope = empty_scope(tid);
        assert_ne!(
            build_cache_key(tid, &q1, &scope, 1),
            build_cache_key(tid, &q2, &scope, 1),
            "different filter values must produce different cache keys"
        );
    }

    #[test]
    fn key_starts_with_saq_v1_prefix() {
        let tid = Uuid::new_v4();
        let q = base_query();
        let scope = empty_scope(tid);
        let key = build_cache_key(tid, &q, &scope, 1);
        assert!(key.starts_with("saq:v1:"), "cache key must start with 'saq:v1:'");
    }
}
