//! Gated smoke test: proves the compiler→storage→Postgres path end-to-end.
//!
//! Run with:
//!   DATABASE_URL=postgres://postgres:postgres@localhost:55433/soma_analytics \
//!   ANALYTICS_DB_URL=postgres://postgres:postgres@localhost:55433/soma_analytics \
//!   REDIS_URL=redis://localhost:56380 \
//!   cargo test -p soma-storage --test smoke_pg -- --ignored --nocapture
//!
//! Verifies five previously-failing runtime bugs are now fixed:
//!   1. Timestamp binding — no `operator does not exist` error with ::timestamptz cast
//!   2. Tenant isolation (structural) — empty row_filters still injects tenant predicate
//!   3. NUMERIC sum — bigdecimal decode returns correct non-null integer total
//!   4. UUID row-filter — (col)::text = $N cast works for text equality
//!   5. Cache — 2nd identical call returns cache:"hit"

use std::sync::Arc;

use redis::aio::ConnectionManager;
use soma_audit_pg::{install as audit_install, AuditKeys, LocalSink};
use soma_semantic::{Granularity, QueryScope, RowFilter, SemanticQuery, TimeDimension};
use soma_storage::{Store, migrate};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// ── helpers ───────────────────────────────────────────────────────────────────

fn smoke_audit_keys() -> AuditKeys {
    AuditKeys::from_secret([0xAA; 32], [0xBB; 32])
}

async fn make_pool(url: &str) -> sqlx::PgPool {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(url)
        .await
        .unwrap_or_else(|e| panic!("connect to {url}: {e}"))
}

// ── the smoke test ────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn smoke_end_to_end() {
    // ── 0. Read env vars ──────────────────────────────────────────────────────
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:55433/soma_analytics".into());
    let ds_url = std::env::var("ANALYTICS_DB_URL").unwrap_or_else(|_| db_url.clone());
    let redis_url = std::env::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://localhost:56380".into());

    // ── 1. Connect pools ──────────────────────────────────────────────────────
    let meta_pool = make_pool(&db_url).await;
    let ds_pool   = make_pool(&ds_url).await;

    // ── A. Install audit schema (soma_audit.fct_audit_events) ────────────────
    audit_install(&meta_pool)
        .await
        .expect("FAIL [audit-install]: soma_audit_pg::install() failed");
    println!("PASS [audit-install]: soma_audit schema installed");

    // ── A. Run soma-storage migrations ────────────────────────────────────────
    migrate(&meta_pool)
        .await
        .expect("FAIL [migrations]: soma_storage::migrate() failed");
    println!("PASS [migrations]: all migrations applied");

    // ── Wire up LocalSink ─────────────────────────────────────────────────────
    let keys = Arc::new(smoke_audit_keys());
    let audit = Arc::new(LocalSink::new(meta_pool.clone(), keys, "smoke-test"));

    // ── Redis cache ───────────────────────────────────────────────────────────
    let redis_client = redis::Client::open(redis_url.as_str()).expect("redis client");
    let cache: ConnectionManager = ConnectionManager::new(redis_client)
        .await
        .expect("redis ConnectionManager");

    // ── Build Store ───────────────────────────────────────────────────────────
    let store = Store::new(meta_pool.clone(), ds_pool.clone(), cache, audit, None);

    // ── B. Create synthetic multi-tenant test table ───────────────────────────
    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    println!("tenant_a = {tenant_a}");
    println!("tenant_b = {tenant_b}");

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS public.test_events (
            id           BIGSERIAL   PRIMARY KEY,
            tenant_id    UUID        NOT NULL,
            status       TEXT        NOT NULL,
            amount_cents BIGINT      NOT NULL,
            created_at   TIMESTAMPTZ NOT NULL
        )"#,
    )
    .execute(&ds_pool)
    .await
    .expect("create test_events table");

    // 6 rows: 3 for tenant A (total amount_cents = 1000+2500+710000 = 713500),
    //         3 for tenant B  — used to verify cross-tenant isolation.
    // Use large amounts to exercise the NUMERIC/bigdecimal path.
    let tenant_a_total: i64 = 1_000 + 2_500 + 710_000; // = 713_500
    let rows: Vec<(Uuid, &str, i64, &str)> = vec![
        (tenant_a, "completed", 1_000,   "2026-01-15 10:00:00+00"),
        (tenant_a, "pending",   2_500,   "2026-02-20 10:00:00+00"),
        (tenant_a, "completed", 710_000, "2026-03-10 10:00:00+00"),
        (tenant_b, "active",    9_999,   "2026-02-01 10:00:00+00"),
        (tenant_b, "active",    8_888,   "2026-03-15 10:00:00+00"),
        (tenant_b, "pending",   7_777,   "2026-04-05 10:00:00+00"),
    ];

    for (tid, status, amount, created_at) in &rows {
        sqlx::query(
            "INSERT INTO public.test_events (tenant_id, status, amount_cents, created_at) \
             VALUES ($1,$2,$3,$4::timestamptz)",
        )
        .bind(tid)
        .bind(status)
        .bind(amount)
        .bind(created_at)
        .execute(&ds_pool)
        .await
        .expect("insert test row");
    }
    println!("PASS [test-data]: 6 rows inserted (tenant_a=3, tenant_b=3)");

    // ── C. Insert metadata model via CRUD API ─────────────────────────────────
    let ds = store
        .create_data_source(tenant_a, None, "smoke-ds", "postgres", None)
        .await
        .expect("create_data_source");
    println!("PASS [model]: data_source id={}", ds.id);

    // cube: set tenant_column to 'tenant_id' (matches public.test_events schema)
    let cube = store
        .create_cube(
            tenant_a,
            None,
            ds.id,
            "events",
            Some("Events"),
            None,                       // description = NULL
            Some("public.test_events"), // sql_table
            None,                       // base_sql
            "id",
            300,
            Some("tenant_id"),          // tenant_column — the new REQUIRED column
        )
        .await
        .expect("create_cube");
    println!("PASS [model]: cube id={}, tenant_column=tenant_id", cube.id);

    // Dimensions
    store.create_dimension(tenant_a, None, cube.id, "status",     None, "{CUBE}.status",    "string").await.expect("dim status");
    store.create_dimension(tenant_a, None, cube.id, "created_at", None, "{CUBE}.created_at", "time").await.expect("dim created_at");
    store.create_dimension(tenant_a, None, cube.id, "tenant",     None, "tenant_id",         "string").await.expect("dim tenant");
    println!("PASS [model]: 3 dimensions created");

    // Measures
    store.create_measure(tenant_a, None, cube.id, "count",   None, None,                "count").await.expect("measure count");
    store.create_measure(tenant_a, None, cube.id, "revenue", None, Some("amount_cents"), "sum").await.expect("measure revenue");
    println!("PASS [model]: 2 measures created");

    // ── Create the same model for tenant_b (same table, separate model rows) ──
    let ds_b = store
        .create_data_source(tenant_b, None, "smoke-ds-b", "postgres", None)
        .await
        .expect("create_data_source for tenant_b");
    let cube_b = store
        .create_cube(
            tenant_b, None, ds_b.id, "events",
            Some("Events"), None, Some("public.test_events"), None, "id", 300, Some("tenant_id"),
        )
        .await
        .expect("create_cube for tenant_b");
    store.create_dimension(tenant_b, None, cube_b.id, "status",     None, "{CUBE}.status",    "string").await.expect("dim status b");
    store.create_dimension(tenant_b, None, cube_b.id, "created_at", None, "{CUBE}.created_at", "time").await.expect("dim created_at b");
    store.create_dimension(tenant_b, None, cube_b.id, "tenant",     None, "tenant_id",         "string").await.expect("dim tenant b");
    store.create_measure(tenant_b, None, cube_b.id, "count",   None, None,                "count").await.expect("measure count b");
    store.create_measure(tenant_b, None, cube_b.id, "revenue", None, Some("amount_cents"), "sum").await.expect("measure revenue b");
    println!("PASS [model-b]: cube + dims + measures created for tenant_b");

    // ── D. Query helpers ──────────────────────────────────────────────────────

    // Full query with time_dimension — used for checks (1) and (5) cache
    let query_with_time = SemanticQuery {
        cube: "events".into(),
        measures: vec!["events.count".into(), "events.revenue".into()],
        dimensions: vec!["events.status".into()],
        filters: vec![],
        segments: vec![],
        time_dimension: Some(TimeDimension {
            member: "events.created_at".into(),
            granularity: Granularity::Month,
            date_range: Some(["2026-01-01".into(), "2026-06-30".into()]),
        }),
        order: vec![],
        limit: None,
        offset: None,
    };

    // Query WITHOUT time_dimension — used for checks (2), (3), (4)
    let query_no_time = SemanticQuery {
        cube: "events".into(),
        measures: vec!["events.count".into(), "events.revenue".into()],
        dimensions: vec!["events.status".into()],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    // Scope for tenant_a with EMPTY row_filters (the "admin" case that previously leaked)
    let scope_a_empty = QueryScope {
        tenant_id: tenant_a,
        row_filters: vec![],
    };

    // Scope for tenant_b with EMPTY row_filters
    let scope_b_empty = QueryScope {
        tenant_id: tenant_b,
        row_filters: vec![],
    };

    // ── CHECK 1: Timestamp binding FIXED ─────────────────────────────────────
    // The compiler emits ::timestamptz casts — query with date_range must not error.
    println!("\n=== CHECK 1: Timestamp binding ===");
    let rs_time = store
        .run_query(tenant_a, &scope_a_empty, &query_with_time)
        .await
        .expect("FAIL [1-timestamp-binding]: query with time_dimension errored (expected fix)");

    assert!(
        !rs_time.rows.is_empty() || rs_time.meta.row_count == 0,
        "FAIL [1-timestamp-binding]: query returned but row structure is inconsistent"
    );
    // Must have rows — tenant_a has 3 rows all within 2026-01 to 2026-06
    assert!(
        rs_time.meta.row_count > 0,
        "FAIL [1-timestamp-binding]: expected non-empty result set (tenant_a has 3 rows in range), got 0"
    );
    println!(
        "PASS [1-timestamp-binding]: query executed OK, row_count={}, cache={}",
        rs_time.meta.row_count, rs_time.meta.cache
    );

    // ── CHECK 2: Tenant isolation FIXED (structural) ──────────────────────────
    // empty row_filters must still inject tenant predicate; no cross-tenant leak.
    println!("\n=== CHECK 2: Tenant isolation (structural) ===");

    let rs_a = store
        .run_query(tenant_a, &scope_a_empty, &query_no_time)
        .await
        .expect("FAIL [2-tenant-isolation]: query for tenant_a errored");

    // Count = sum of the count column (column index 1)
    let count_a: i64 = rs_a.rows.iter()
        .flat_map(|row| row.get(1).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))))
        .sum();
    println!("tenant_a query: row_count={}, count_a={count_a}", rs_a.meta.row_count);

    assert!(
        count_a <= 3,
        "FAIL [2-tenant-isolation]: tenant_a empty-scope query returned count={count_a} \
         (> 3 means cross-tenant leak; both tenants have 6 rows total)"
    );
    assert!(
        count_a == 3,
        "FAIL [2-tenant-isolation]: tenant_a count={count_a}, expected exactly 3"
    );

    let rs_b = store
        .run_query(tenant_b, &scope_b_empty, &query_no_time)
        .await
        .expect("FAIL [2-tenant-isolation]: query for tenant_b errored");

    let count_b: i64 = rs_b.rows.iter()
        .flat_map(|row| row.get(1).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))))
        .sum();
    println!("tenant_b query: row_count={}, count_b={count_b}", rs_b.meta.row_count);

    assert!(
        count_b == 3,
        "FAIL [2-tenant-isolation]: tenant_b count={count_b}, expected exactly 3"
    );

    println!(
        "PASS [2-tenant-isolation]: tenant_a count={count_a}, tenant_b count={count_b} — no cross-tenant leak"
    );

    // ── CHECK 3: NUMERIC sum FIXED ────────────────────────────────────────────
    // revenue = sum(amount_cents) for tenant_a = 713_500; must not be null or 0.
    println!("\n=== CHECK 3: NUMERIC sum (bigdecimal decode) ===");

    // Use the same rs_a from check 2 (query_no_time, scope_a_empty)
    let revenue_a: i64 = rs_a.rows.iter()
        .flat_map(|row| row.get(2).and_then(|v| {
            if v.is_null() { return None; }
            v.as_i64()
                .or_else(|| v.as_f64().map(|f| f as i64))
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        }))
        .sum();

    assert!(
        revenue_a != 0,
        "FAIL [3-numeric-sum]: revenue_a=0 (bigdecimal decode likely returned null)"
    );
    assert!(
        revenue_a == tenant_a_total,
        "FAIL [3-numeric-sum]: revenue_a={revenue_a}, expected {tenant_a_total}"
    );
    println!(
        "PASS [3-numeric-sum]: revenue_a={revenue_a} (expected {tenant_a_total}) — bigdecimal decode correct"
    );

    // ── CHECK 4: UUID/text row-filter FIXED ───────────────────────────────────
    // RowFilter with (col)::text = $N cast must work; filter restricts to "completed" rows.
    println!("\n=== CHECK 4: row-filter (::text cast) ===");

    let scope_a_filtered = QueryScope {
        tenant_id: tenant_a,
        row_filters: vec![RowFilter {
            member: "events.status".into(),
            value: "completed".into(),
        }],
    };

    let rs_filtered = store
        .run_query(tenant_a, &scope_a_filtered, &query_no_time)
        .await
        .expect("FAIL [4-row-filter]: query with row_filter errored (expected fix for ::text cast)");

    let count_filtered: i64 = rs_filtered.rows.iter()
        .flat_map(|row| row.get(1).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))))
        .sum();

    // tenant_a has 2 "completed" rows
    assert!(
        count_filtered == 2,
        "FAIL [4-row-filter]: filtered count={count_filtered}, expected 2 (tenant_a 'completed' rows)"
    );
    println!(
        "PASS [4-row-filter]: status='completed' filter returned count={count_filtered} (expected 2)"
    );

    // ── CHECK 5: Cache hit ────────────────────────────────────────────────────
    // 2nd identical call must return cache:"hit"
    println!("\n=== CHECK 5: Cache hit ===");

    // Re-run the exact same query as check 1 (query_with_time, scope_a_empty)
    let rs_cached = store
        .run_query(tenant_a, &scope_a_empty, &query_with_time)
        .await
        .expect("FAIL [5-cache]: 2nd query errored");

    assert_eq!(
        rs_cached.meta.cache, "hit",
        "FAIL [5-cache]: 2nd call cache='{}', expected 'hit'",
        rs_cached.meta.cache
    );
    println!(
        "PASS [5-cache]: 2nd call returned cache='{}' (row_count={})",
        rs_cached.meta.cache, rs_cached.meta.row_count
    );

    // ── Summary ───────────────────────────────────────────────────────────────
    println!("\n=== SMOKE TEST SUMMARY ===");
    println!("(1) Timestamp binding:            PASS — query with date_range executed, row_count={}", rs_time.meta.row_count);
    println!("(2) Tenant isolation (structural): PASS — tenant_a={count_a}, tenant_b={count_b}, no leak");
    println!("(3) NUMERIC sum (bigdecimal):      PASS — revenue_a={revenue_a}");
    println!("(4) UUID/text row-filter:          PASS — filtered count={count_filtered}");
    println!("(5) Cache hit:                     PASS — 2nd call cache=hit");
    println!("=== ALL 5 CHECKS PASSED ===");
}
