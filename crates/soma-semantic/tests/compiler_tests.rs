//! Integration tests for `soma-semantic::compile`.
//!
//! Fixtures use the §5.1 orders/customers model from the spec.
//! All tests are pure (no I/O, no DB).

use soma_semantic::{
    compile,
    compile::{ColumnType, SqlValue},
    error::CompileError,
    model::{AggType, Cube, DataType, Dimension, Join, Measure, Model, Relationship, Segment},
    query::{Filter, FilterOp, Granularity, Order, QueryScope, RowFilter, SemanticQuery, TimeDimension},
    rollup::rollup_match,
    validate::validate_sql_fragment,
};
use uuid::Uuid;

// ─── Fixtures ────────────────────────────────────────────────────────────────

fn orders_cube() -> Cube {
    Cube {
        name: "orders".into(),
        data_source: "warehouse".into(),
        sql_table: Some("public.orders".into()),
        base_sql: None,
        primary_key: "id".into(),
        tenant_column: "tenant_id".into(),
        description: None,
        dimensions: vec![
            Dimension {
                name: "status".into(),
                sql_expr: "status".into(),
                data_type: DataType::String,
                description: None,
            },
            Dimension {
                name: "created_at".into(),
                sql_expr: "created_at".into(),
                data_type: DataType::Time,
                description: None,
            },
        ],
        measures: vec![
            Measure {
                name: "count".into(),
                sql_expr: None,
                agg_type: AggType::Count,
                description: None,
            },
            Measure {
                name: "total_revenue".into(),
                sql_expr: Some("amount_cents".into()),
                agg_type: AggType::Sum,
                description: None,
            },
            Measure {
                name: "avg_revenue".into(),
                sql_expr: Some("amount_cents".into()),
                agg_type: AggType::Avg,
                description: None,
            },
        ],
        segments: vec![Segment {
            name: "high_value".into(),
            sql_expr: "{CUBE}.amount_cents > 100000".into(),
        }],
        joins: vec![Join {
            name: "customers".into(),
            target_cube: "customers".into(),
            relationship: Relationship::ManyToOne,
            sql_on: "{CUBE}.customer_id = {customers}.id".into(),
        }],
    }
}

fn customers_cube() -> Cube {
    Cube {
        name: "customers".into(),
        data_source: "warehouse".into(),
        sql_table: Some("public.customers".into()),
        base_sql: None,
        primary_key: "id".into(),
        tenant_column: "tenant_id".into(),
        description: None,
        dimensions: vec![Dimension {
            name: "region".into(),
            sql_expr: "region".into(),
            data_type: DataType::String,
            description: None,
        }],
        measures: vec![
            Measure {
                name: "customer_count".into(),
                sql_expr: None,
                agg_type: AggType::Count,
                description: None,
            },
            Measure {
                name: "distinct_customers".into(),
                sql_expr: Some("id".into()),
                agg_type: AggType::CountDistinct,
                description: None,
            },
            Measure {
                name: "avg_ltv".into(),
                sql_expr: Some("ltv_cents".into()),
                agg_type: AggType::Avg,
                description: None,
            },
        ],
        segments: vec![],
        joins: vec![],
    }
}

fn third_cube() -> Cube {
    Cube {
        name: "products".into(),
        data_source: "warehouse".into(),
        sql_table: Some("public.products".into()),
        base_sql: None,
        primary_key: "id".into(),
        tenant_column: "tenant_id".into(),
        description: None,
        dimensions: vec![Dimension {
            name: "category".into(),
            sql_expr: "category".into(),
            data_type: DataType::String,
            description: None,
        }],
        measures: vec![],
        segments: vec![],
        joins: vec![],
    }
}

/// Standard two-cube model.
fn model() -> Model {
    Model::from_cubes(vec![orders_cube(), customers_cube()]).unwrap()
}

/// Three-cube model for fan-out test.
fn three_cube_model() -> Model {
    let mut orders = orders_cube();
    orders.joins.push(Join {
        name: "products".into(),
        target_cube: "products".into(),
        relationship: Relationship::ManyToOne,
        sql_on: "{CUBE}.product_id = {products}.id".into(),
    });
    Model::from_cubes(vec![orders, customers_cube(), third_cube()]).unwrap()
}

fn scope() -> QueryScope {
    QueryScope {
        tenant_id: Uuid::nil(),
        row_filters: vec![],
    }
}

// ─── Single-cube: count + sum + group-by + filter ────────────────────────────

#[test]
fn single_cube_count_sum_groupby_filter() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into(), "orders.total_revenue".into()],
        dimensions: vec!["orders.status".into()],
        filters: vec![Filter {
            member: "orders.status".into(),
            operator: FilterOp::Equals,
            values: vec!["completed".into(), "shipped".into()],
        }],
        segments: vec![],
        time_dimension: None,
        order: vec![("orders.total_revenue".into(), Order::Desc)],
        limit: Some(100),
        offset: None,
    };

    let cq = compile(&m, &q, &scope()).unwrap();

    // SQL shape assertions.
    let sql = &cq.sql;
    assert!(sql.contains("count(*)"), "should have count(*): {sql}");
    assert!(sql.contains("sum(amount_cents)"), "should have sum: {sql}");
    assert!(sql.contains("status"), "should project status: {sql}");
    assert!(sql.contains("FROM public.orders"), "FROM clause: {sql}");
    assert!(sql.contains("GROUP BY"), "should group: {sql}");
    assert!(sql.contains("ORDER BY"), "should order: {sql}");
    assert!(sql.contains("DESC"), "should have DESC: {sql}");
    assert!(sql.contains("LIMIT"), "should have LIMIT: {sql}");

    // Filter values MUST be binds — not inlined.
    assert!(
        !sql.contains("completed"),
        "filter value 'completed' must NOT appear in SQL; found in: {sql}"
    );
    assert!(
        !sql.contains("shipped"),
        "filter value 'shipped' must NOT appear in SQL; found in: {sql}"
    );

    // The filter should use a bind parameter placeholder.
    assert!(sql.contains("$1"), "bind $1 missing: {sql}");

    // Verify binds contain the values.
    assert!(
        cq.binds
            .iter()
            .any(|b| matches!(b, SqlValue::TextArray(v) if v.contains(&"completed".to_string()))),
        "bind should carry filter values: {:?}",
        cq.binds
    );

    // Limit is a bind.
    assert!(
        cq.binds.iter().any(|b| matches!(b, SqlValue::Int(100))),
        "limit should be a bind: {:?}",
        cq.binds
    );

    // Structural tenant predicate always present (A1).
    assert!(
        cq.binds.iter().any(|b| matches!(b, SqlValue::Uuid(_))),
        "tenant Uuid bind must be present: {:?}",
        cq.binds
    );

    // Columns.
    assert_eq!(cq.columns.len(), 3); // status, count, total_revenue
    assert_eq!(cq.columns[0].name, "orders.status");
    assert_eq!(cq.columns[0].data_type, ColumnType::String);
    assert_eq!(cq.columns[1].name, "orders.count");
    assert_eq!(cq.columns[1].data_type, ColumnType::Number);
}

// ─── Time-grain month + dateRange (exclusive upper bound) ────────────────────

#[test]
fn time_grain_month_with_date_range() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: Some(TimeDimension {
            member: "orders.created_at".into(),
            granularity: Granularity::Month,
            date_range: Some(["2026-01-01".into(), "2026-06-30".into()]),
        }),
        order: vec![],
        limit: None,
        offset: None,
    };

    let cq = compile(&m, &q, &scope()).unwrap();
    let sql = &cq.sql;

    // date_trunc in SELECT.
    assert!(
        sql.contains("date_trunc('month'"),
        "should have date_trunc: {sql}"
    );
    // Lower bound inclusive with ::timestamptz cast (A2).
    assert!(sql.contains(">="), "lower bound: {sql}");
    assert!(sql.contains("::timestamptz"), "A2: ::timestamptz cast required: {sql}");
    // Upper bound exclusive.
    assert!(sql.contains("< "), "exclusive upper: {sql}");

    // The exclusive upper must be 2026-07-01 (one month after June 30).
    let hi_bind = cq
        .binds
        .iter()
        .find(|b| matches!(b, SqlValue::Timestamp(s) if s.starts_with("2026-07")));
    assert!(
        hi_bind.is_some(),
        "exclusive upper bound 2026-07-01 should be a bind: {:?}",
        cq.binds
    );

    // Lower bound is a bind.
    let lo_bind = cq
        .binds
        .iter()
        .find(|b| matches!(b, SqlValue::Timestamp(s) if s == "2026-01-01"));
    assert!(lo_bind.is_some(), "lower bound should be a bind: {:?}", cq.binds);

    // time column in output.
    assert!(
        cq.columns
            .iter()
            .any(|c| c.name.contains("month") && c.data_type == ColumnType::Time),
        "time column missing: {:?}",
        cq.columns
    );
}

// ─── Segment inlined ─────────────────────────────────────────────────────────

#[test]
fn segment_is_inlined_not_bound() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec!["orders.high_value".into()],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let cq = compile(&m, &q, &scope()).unwrap();
    let sql = &cq.sql;

    // The segment predicate is expanded from {CUBE}.amount_cents > 100000.
    assert!(
        sql.contains("amount_cents > 100000"),
        "segment should be inlined: {sql}"
    );
    // Only the structural tenant Uuid bind is present (A1); no extra binds from segment.
    assert_eq!(
        cq.binds.len(),
        1,
        "only tenant Uuid bind expected for count-only + segment query: {:?}",
        cq.binds
    );
    assert!(
        matches!(&cq.binds[0], SqlValue::Uuid(_)),
        "the single bind must be the tenant Uuid: {:?}",
        cq.binds
    );
}

// ─── many_to_one join with joined-cube dimension ──────────────────────────────

#[test]
fn many_to_one_join_with_joined_dimension() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec!["customers.region".into()],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let cq = compile(&m, &q, &scope()).unwrap();
    let sql = &cq.sql;

    // LEFT JOIN.
    assert!(
        sql.contains("LEFT JOIN"),
        "should have a LEFT JOIN: {sql}"
    );
    assert!(
        sql.contains("public.customers"),
        "joined table: {sql}"
    );
    // ON clause uses the join's sql_on after token expansion.
    assert!(
        sql.contains("customer_id"),
        "ON clause should have customer_id: {sql}"
    );
    // Projected dimension.
    assert!(sql.contains("region"), "region dimension: {sql}");
    assert!(sql.contains("GROUP BY"), "GROUP BY for joined dim: {sql}");

    // Column meta.
    assert!(
        cq.columns.iter().any(|c| c.name == "customers.region"),
        "column: {:?}",
        cq.columns
    );
}

// ─── Unknown measure → UnknownMember ─────────────────────────────────────────

#[test]
fn unknown_measure_returns_unknown_member() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.nonexistent_measure".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m, &q, &scope()).unwrap_err();
    assert!(
        matches!(err, CompileError::UnknownMember(_)),
        "expected UnknownMember, got: {err}"
    );
}

// ─── Unknown cube → UnknownCube ──────────────────────────────────────────────

#[test]
fn unknown_cube_returns_unknown_cube() {
    let m = model();
    let q = SemanticQuery {
        cube: "nonexistent".into(),
        measures: vec![],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m, &q, &scope()).unwrap_err();
    assert!(
        matches!(err, CompileError::UnknownCube(_)),
        "expected UnknownCube, got: {err}"
    );
}

// ─── 3-cube query → UnsupportedFanout ────────────────────────────────────────

#[test]
fn three_cube_query_returns_unsupported_fanout() {
    let m = three_cube_model();
    // Root = orders, joined dim from customers, another dim from products.
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec!["customers.region".into(), "products.category".into()],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m, &q, &scope()).unwrap_err();
    assert!(
        matches!(err, CompileError::UnsupportedFanout(_)),
        "expected UnsupportedFanout, got: {err}"
    );
}

// ─── one_to_many + avg measure → NonAdditiveMeasureInFanout ──────────────────

#[test]
fn one_to_many_avg_measure_returns_non_additive_error() {
    // Build a model where orders → line_items is one_to_many.
    let line_items = Cube {
        name: "line_items".into(),
        data_source: "warehouse".into(),
        sql_table: Some("public.line_items".into()),
        base_sql: None,
        primary_key: "id".into(),
        tenant_column: "tenant_id".into(),
        description: None,
        dimensions: vec![Dimension {
            name: "product_id".into(),
            sql_expr: "product_id".into(),
            data_type: DataType::String,
            description: None,
        }],
        measures: vec![
            Measure {
                name: "avg_qty".into(),
                sql_expr: Some("qty".into()),
                agg_type: AggType::Avg,
                description: None,
            },
            Measure {
                name: "total_qty".into(),
                sql_expr: Some("qty".into()),
                agg_type: AggType::Sum,
                description: None,
            },
        ],
        segments: vec![],
        joins: vec![],
    };

    let mut orders = orders_cube();
    orders.joins.push(Join {
        name: "line_items".into(),
        target_cube: "line_items".into(),
        relationship: Relationship::OneToMany,
        sql_on: "{CUBE}.id = {line_items}.order_id".into(),
    });

    let m = Model::from_cubes(vec![orders, customers_cube(), line_items]).unwrap();

    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into(), "line_items.avg_qty".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m, &q, &scope()).unwrap_err();
    assert!(
        matches!(
            err,
            CompileError::NonAdditiveMeasureInFanout { ref measure, .. }
            if measure == "avg_qty"
        ),
        "expected NonAdditiveMeasureInFanout(avg_qty), got: {err}"
    );
}

// ─── one_to_many + count_distinct → NonAdditiveMeasureInFanout ───────────────

#[test]
fn one_to_many_count_distinct_returns_non_additive() {
    let line_items = Cube {
        name: "line_items".into(),
        data_source: "warehouse".into(),
        sql_table: Some("public.line_items".into()),
        base_sql: None,
        primary_key: "id".into(),
        tenant_column: "tenant_id".into(),
        description: None,
        dimensions: vec![],
        measures: vec![Measure {
            name: "distinct_skus".into(),
            sql_expr: Some("sku".into()),
            agg_type: AggType::CountDistinct,
            description: None,
        }],
        segments: vec![],
        joins: vec![],
    };

    let mut orders = orders_cube();
    orders.joins.push(Join {
        name: "line_items".into(),
        target_cube: "line_items".into(),
        relationship: Relationship::OneToMany,
        sql_on: "{CUBE}.id = {line_items}.order_id".into(),
    });

    let m = Model::from_cubes(vec![orders, line_items]).unwrap();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["line_items.distinct_skus".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m, &q, &scope()).unwrap_err();
    assert!(
        matches!(err, CompileError::NonAdditiveMeasureInFanout { .. }),
        "expected NonAdditiveMeasureInFanout, got: {err}"
    );
}

// ─── one_to_many + sum (additive) → UnsupportedFanout (Phase-1 increment 2) ──

#[test]
fn one_to_many_sum_measure_returns_unsupported_fanout() {
    let line_items = Cube {
        name: "line_items".into(),
        data_source: "warehouse".into(),
        sql_table: Some("public.line_items".into()),
        base_sql: None,
        primary_key: "id".into(),
        tenant_column: "tenant_id".into(),
        description: None,
        dimensions: vec![],
        measures: vec![Measure {
            name: "total_qty".into(),
            sql_expr: Some("qty".into()),
            agg_type: AggType::Sum,
            description: None,
        }],
        segments: vec![],
        joins: vec![],
    };

    let mut orders = orders_cube();
    orders.joins.push(Join {
        name: "line_items".into(),
        target_cube: "line_items".into(),
        relationship: Relationship::OneToMany,
        sql_on: "{CUBE}.id = {line_items}.order_id".into(),
    });

    let m = Model::from_cubes(vec![orders, line_items]).unwrap();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["line_items.total_qty".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m, &q, &scope()).unwrap_err();
    assert!(
        matches!(err, CompileError::UnsupportedFanout(_)),
        "expected UnsupportedFanout for additive one_to_many, got: {err}"
    );
}

// ─── No join path → UnreachableMember ───────────────────────────────────────
// Note: UndeclaredJoinCardinality was removed — join cardinality is a type-level invariant
// (`relationship` is a required field on `Join`) + DB CHECK, not a runtime error.

#[test]
fn no_join_path_returns_unreachable_member() {
    // Build a root cube with no joins, then query a member on a second cube.
    // Must return UnreachableMember (no join path).
    let m_no_joins = Model::from_cubes(vec![
        Cube {
            name: "orders".into(),
            data_source: "warehouse".into(),
            sql_table: Some("public.orders".into()),
            base_sql: None,
            primary_key: "id".into(),
            tenant_column: "tenant_id".into(),
            description: None,
            dimensions: vec![Dimension {
                name: "status".into(),
                sql_expr: "status".into(),
                data_type: DataType::String,
                description: None,
            }],
            measures: vec![Measure {
                name: "count".into(),
                sql_expr: None,
                agg_type: AggType::Count,
                description: None,
            }],
            segments: vec![],
            joins: vec![], // no joins declared
        },
        customers_cube(),
    ])
    .unwrap();

    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec![],
        dimensions: vec!["customers.region".into()],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m_no_joins, &q, &scope()).unwrap_err();
    assert!(
        matches!(err, CompileError::UnreachableMember { .. }),
        "expected UnreachableMember when no join path exists, got: {err}"
    );
}

// ─── SQL injection in filter value stays a bind ───────────────────────────────

#[test]
fn injection_attempt_in_filter_value_stays_bound() {
    let m = model();
    let malicious_value = "'; DROP TABLE orders; --".to_string();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![Filter {
            member: "orders.status".into(),
            operator: FilterOp::Equals,
            values: vec![malicious_value.clone()],
        }],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let cq = compile(&m, &q, &scope()).unwrap();
    let sql = &cq.sql;

    // The malicious string must NOT appear in the SQL.
    assert!(
        !sql.contains(&malicious_value),
        "injection value leaked into SQL: {sql}"
    );
    assert!(
        !sql.contains("DROP"),
        "DROP should not be in SQL: {sql}"
    );

    // The value must be in binds.
    assert!(
        cq.binds
            .iter()
            .any(|b| matches!(b, SqlValue::TextArray(v) if v.iter().any(|s| s == &malicious_value))),
        "injection value should be in binds: {:?}",
        cq.binds
    );
}

// ─── validate_sql_fragment ────────────────────────────────────────────────────

#[test]
fn validate_rejects_semicolon() {
    assert!(validate_sql_fragment(";").is_err());
}

#[test]
fn validate_rejects_line_comment() {
    assert!(validate_sql_fragment("status -- comment").is_err());
}

#[test]
fn validate_rejects_drop() {
    assert!(validate_sql_fragment("DROP TABLE orders").is_err());
    assert!(validate_sql_fragment("drop table orders").is_err());
}

#[test]
fn validate_allows_normal_expressions() {
    assert!(validate_sql_fragment("status").is_ok());
    assert!(validate_sql_fragment("{CUBE}.amount_cents > 100000").is_ok());
    assert!(validate_sql_fragment("public.orders").is_ok());
    assert!(
        validate_sql_fragment("SELECT id, amount FROM orders WHERE is_active = true").is_ok()
    );
}

// ─── rollup_match always returns None ────────────────────────────────────────

#[test]
fn rollup_match_returns_none() {
    assert_eq!(rollup_match(), None);
}

// ─── scope row_filters are bound (not inlined) + ::text cast ─────────────────

#[test]
fn scope_row_filters_are_bound() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let scoped = QueryScope {
        tenant_id: Uuid::nil(),
        row_filters: vec![RowFilter {
            member: "orders.status".into(),
            value: "active".into(),
        }],
    };

    let cq = compile(&m, &q, &scoped).unwrap();
    let sql = &cq.sql;

    // The row filter value must not appear in SQL.
    assert!(
        !sql.contains("active"),
        "row filter value leaked into SQL: {sql}"
    );
    // Must be in binds.
    assert!(
        cq.binds
            .iter()
            .any(|b| matches!(b, SqlValue::Text(v) if v == "active")),
        "row filter must be a bind: {:?}",
        cq.binds
    );
    // WHERE clause present.
    assert!(sql.contains("WHERE"), "WHERE clause: {sql}");
    // A3: row filter uses ::text cast.
    assert!(
        sql.contains("::text"),
        "A3: row filter must use ::text cast: {sql}"
    );
}

// ─── scope row_filter with unknown member → UnknownMember ────────────────────

#[test]
fn scope_row_filter_unknown_member_is_rejected() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let scoped = QueryScope {
        tenant_id: Uuid::nil(),
        row_filters: vec![RowFilter {
            member: "orders.nonexistent".into(),
            value: "x".into(),
        }],
    };

    let err = compile(&m, &q, &scoped).unwrap_err();
    assert!(
        matches!(err, CompileError::UnknownMember(_)),
        "expected UnknownMember for bad row_filter member, got: {err}"
    );
}

// ─── base_sql cube ───────────────────────────────────────────────────────────

#[test]
fn base_sql_cube_compiles_correctly() {
    let cube = Cube {
        name: "active_orders".into(),
        data_source: "warehouse".into(),
        sql_table: None,
        base_sql: Some("SELECT * FROM public.orders WHERE is_active = true".into()),
        primary_key: "id".into(),
        tenant_column: "tenant_id".into(),
        description: None,
        dimensions: vec![Dimension {
            name: "status".into(),
            sql_expr: "status".into(),
            data_type: DataType::String,
            description: None,
        }],
        measures: vec![Measure {
            name: "count".into(),
            sql_expr: None,
            agg_type: AggType::Count,
            description: None,
        }],
        segments: vec![],
        joins: vec![],
    };

    let m = Model::from_cubes(vec![cube]).unwrap();
    let q = SemanticQuery {
        cube: "active_orders".into(),
        measures: vec!["active_orders.count".into()],
        dimensions: vec!["active_orders.status".into()],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let cq = compile(&m, &q, &scope()).unwrap();
    let sql = &cq.sql;

    // Should use (base_sql) AS "active_orders" as the FROM.
    assert!(sql.contains("SELECT * FROM"), "base_sql in FROM: {sql}");
    assert!(sql.contains("is_active"), "base_sql preserved: {sql}");
    assert!(sql.contains("count(*)"), "measure: {sql}");
}

// ─── full worked-example from §5 ─────────────────────────────────────────────

#[test]
fn worked_example_from_spec() {
    // §5.2 query → §5.4 SQL shape.
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into(), "orders.total_revenue".into()],
        dimensions: vec!["orders.status".into()],
        filters: vec![Filter {
            member: "orders.status".into(),
            operator: FilterOp::Equals,
            values: vec!["completed".into(), "shipped".into()],
        }],
        segments: vec!["orders.high_value".into()],
        time_dimension: Some(TimeDimension {
            member: "orders.created_at".into(),
            granularity: Granularity::Month,
            date_range: Some(["2026-01-01".into(), "2026-06-30".into()]),
        }),
        order: vec![("orders.total_revenue".into(), Order::Desc)],
        limit: Some(1000),
        offset: None,
    };

    let cq = compile(&m, &q, &scope()).unwrap();
    let sql = &cq.sql;

    // Validate overall shape per §5.4.
    assert!(sql.contains("count(*)"), "count(*): {sql}");
    assert!(sql.contains("sum(amount_cents)"), "sum: {sql}");
    assert!(sql.contains("date_trunc('month'"), "date_trunc: {sql}");
    assert!(sql.contains("FROM public.orders"), "FROM: {sql}");
    assert!(sql.contains("= ANY("), "equals filter: {sql}");
    assert!(sql.contains("amount_cents > 100000"), "segment: {sql}");
    assert!(sql.contains(">="), "date lower: {sql}");
    assert!(sql.contains("< "), "date upper exclusive: {sql}");
    assert!(sql.contains("::timestamptz"), "A2: timestamptz cast: {sql}");
    assert!(sql.contains("GROUP BY"), "GROUP BY: {sql}");
    assert!(sql.contains("ORDER BY"), "ORDER BY: {sql}");
    assert!(sql.contains("LIMIT"), "LIMIT: {sql}");

    // Filter values are NOT in the SQL.
    assert!(!sql.contains("completed"), "values bound: {sql}");
    assert!(!sql.contains("shipped"), "values bound: {sql}");
    // LIMIT value must be a bind — check via the binds list, not the SQL string.
    assert!(
        cq.binds.iter().any(|b| matches!(b, SqlValue::Int(1000))),
        "limit 1000 should be a bind: {:?}",
        cq.binds
    );

    // Columns = status, created_at.month, count, total_revenue.
    assert_eq!(cq.columns.len(), 4);
    assert_eq!(cq.columns[0].name, "orders.status");
    assert_eq!(cq.columns[1].name, "orders.created_at.month");
    assert_eq!(cq.columns[2].name, "orders.count");
    assert_eq!(cq.columns[3].name, "orders.total_revenue");
}

// ─── unknown segment → UnknownSegment ────────────────────────────────────────

#[test]
fn unknown_segment_returns_error() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec!["orders.nonexistent_segment".into()],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };

    let err = compile(&m, &q, &scope()).unwrap_err();
    assert!(
        matches!(err, CompileError::UnknownSegment(_)),
        "expected UnknownSegment, got: {err}"
    );
}

// ─── A1: structural tenant predicate always present ──────────────────────────

#[test]
fn structural_tenant_predicate_always_present() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };
    let tenant_id = Uuid::new_v4();
    let s = QueryScope { tenant_id, row_filters: vec![] };
    let result = compile(&m, &q, &s).unwrap();
    // Must always have a Uuid bind for the tenant predicate.
    assert!(
        result.binds.iter().any(|b| matches!(b, SqlValue::Uuid(u) if *u == tenant_id)),
        "expected Uuid bind for tenant predicate: {:?}",
        result.binds
    );
    // Tenant column must appear in SQL.
    assert!(
        result.sql.contains("\"tenant_id\""),
        "expected tenant_column in SQL: {}",
        result.sql
    );
}

// ─── A2: timestamp ::timestamptz cast on dateRange ───────────────────────────

#[test]
fn date_range_emits_timestamptz_cast() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec![],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: Some(TimeDimension {
            member: "orders.created_at".into(),
            granularity: Granularity::Day,
            date_range: Some(["2026-01-01".into(), "2026-06-30".into()]),
        }),
        order: vec![],
        limit: None,
        offset: None,
    };
    let result = compile(&m, &q, &scope()).unwrap();
    assert!(
        result.sql.contains("::timestamptz"),
        "expected ::timestamptz cast in date range SQL: {}",
        result.sql
    );
}

// ─── A2: Gt filter on Time dimension emits ::timestamptz + Timestamp bind ─────

#[test]
fn gt_filter_on_time_dim_emits_timestamptz_cast() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![Filter {
            member: "orders.created_at".into(),
            operator: FilterOp::Gt,
            values: vec!["2026-01-01".into()],
        }],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };
    let result = compile(&m, &q, &scope()).unwrap();
    assert!(
        result.sql.contains("::timestamptz"),
        "expected ::timestamptz cast for Gt on Time dim: {}",
        result.sql
    );
    assert!(
        result.binds.iter().any(|b| matches!(b, SqlValue::Timestamp(_))),
        "expected Timestamp bind for Gt on Time dim: {:?}",
        result.binds
    );
}

// ─── A3: row filter uses ::text cast ─────────────────────────────────────────

#[test]
fn row_filter_emits_text_cast() {
    let m = model();
    let q = SemanticQuery {
        cube: "orders".into(),
        measures: vec!["orders.count".into()],
        dimensions: vec![],
        filters: vec![],
        segments: vec![],
        time_dimension: None,
        order: vec![],
        limit: None,
        offset: None,
    };
    let s = QueryScope {
        tenant_id: Uuid::nil(),
        row_filters: vec![RowFilter {
            member: "orders.status".into(),
            value: "active".into(),
        }],
    };
    let result = compile(&m, &q, &s).unwrap();
    assert!(
        result.sql.contains("::text"),
        "expected ::text cast for row_filter: {}",
        result.sql
    );
}
