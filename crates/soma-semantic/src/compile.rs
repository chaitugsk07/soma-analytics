//! Pure SQL compiler: resolves a `SemanticQuery` + `QueryScope` against a `Model`
//! and emits parameterised SQL (`CompiledQuery`).
//!
//! All user-supplied filter *values* become bind parameters ($N) — never interpolated.
//! Model-authored `sql_expr` / `sql_on` / segment SQL is templated in after `{CUBE}` /
//! `{name}` substitution.

use crate::{
    error::CompileError,
    model::{AggType, Cube, DataType, Dimension, Measure, Model, Relationship},
    query::{FilterOp, Order, QueryScope, SemanticQuery},
};

// ─── Output types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Text(String),
    TextArray(Vec<String>),
    Int(i64),
    Timestamp(String),
    Uuid(uuid::Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnType {
    String,
    Number,
    Time,
    Boolean,
}

#[derive(Debug, Clone)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: ColumnType,
}

#[derive(Debug, Clone)]
pub struct CompiledQuery {
    pub sql: String,
    pub binds: Vec<SqlValue>,
    pub columns: Vec<ColumnMeta>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Quote an identifier with double quotes; escape any embedded `"` as `""`.
fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Expand `{CUBE}` → `"cube_name"` and `{name}` → `"name"` in a model-authored SQL fragment.
fn expand_tokens(sql: &str, cube_name: &str) -> String {
    // Replace {CUBE} first, then any {word} remaining.
    let s = sql.replace("{CUBE}", &quote_ident(cube_name));
    // Replace {other_cube_name} patterns — any remaining {token}.
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('}') {
            let token = &rest[..close];
            out.push_str(&quote_ident(token));
            rest = &rest[close + 1..];
        } else {
            // Unclosed brace — output as-is.
            out.push('{');
        }
    }
    out.push_str(rest);
    out
}

/// Emit a column expression for a dimension, qualifying it with the cube alias.
fn dim_sql(cube_name: &str, dim: &Dimension) -> String {
    expand_tokens(&dim.sql_expr, cube_name)
}

/// The SELECT expression for a measure.
fn measure_select(cube_name: &str, m: &Measure) -> String {
    match m.agg_type {
        AggType::Count => "count(*)".into(),
        AggType::CountDistinct => {
            let expr = expand_tokens(m.sql_expr.as_deref().unwrap_or("*"), cube_name);
            format!("count(distinct {expr})")
        }
        AggType::Sum => {
            let expr = expand_tokens(m.sql_expr.as_deref().unwrap_or("*"), cube_name);
            format!("sum({expr})")
        }
        AggType::Avg => {
            let expr = expand_tokens(m.sql_expr.as_deref().unwrap_or("*"), cube_name);
            format!("avg({expr})")
        }
        AggType::Min => {
            let expr = expand_tokens(m.sql_expr.as_deref().unwrap_or("*"), cube_name);
            format!("min({expr})")
        }
        AggType::Max => {
            let expr = expand_tokens(m.sql_expr.as_deref().unwrap_or("*"), cube_name);
            format!("max({expr})")
        }
        AggType::Number => expand_tokens(m.sql_expr.as_deref().unwrap_or("null"), cube_name),
    }
}

/// Parse `"cube.member"` → `(cube, member)`.
fn split_member(member: &str) -> Option<(&str, &str)> {
    let dot = member.find('.')?;
    Some((&member[..dot], &member[dot + 1..]))
}

// ─── Resolution ──────────────────────────────────────────────────────────────

/// Resolved dimension reference.
struct ResolvedDim<'a> {
    cube: &'a Cube,
    dim: &'a Dimension,
    /// The full `"cube.dimension"` output column label.
    col_label: String,
}

/// Resolved measure reference.
struct ResolvedMeasure<'a> {
    cube: &'a Cube,
    measure: &'a Measure,
    col_label: String,
}

/// Shared join-reachability walk: parse `"cube.member"`, check join reachability, return
/// `(&Cube, member_name)`.  Both `resolve_dimension` and `resolve_measure` use this.
fn resolve_member_cube<'a>(
    model: &'a Model,
    root_cube: &'a Cube,
    member: &'a str,
) -> Result<(&'a Cube, &'a str), CompileError> {
    let (cube_name, member_name) = split_member(member)
        .ok_or_else(|| CompileError::UnknownMember(member.to_string()))?;

    let cube = if cube_name == root_cube.name {
        root_cube
    } else {
        // Must be reachable via a join from root_cube.
        root_cube
            .joins
            .iter()
            .find(|j| j.name == cube_name || j.target_cube == cube_name)
            .ok_or_else(|| CompileError::UnreachableMember {
                member: member.to_string(),
                from_cube: root_cube.name.clone(),
            })?;
        model
            .cube(cube_name)
            .ok_or_else(|| CompileError::UnknownCube(cube_name.to_string()))?
    };

    Ok((cube, member_name))
}

/// Resolve a `"cube.member"` string to the dimension on `root_cube`, optionally through a join.
fn resolve_dimension<'a>(
    model: &'a Model,
    root_cube: &'a Cube,
    member: &'a str,
) -> Result<ResolvedDim<'a>, CompileError> {
    let (cube, dim_name) = resolve_member_cube(model, root_cube, member)?;
    let dim = cube
        .dimension(dim_name)
        .ok_or_else(|| CompileError::UnknownMember(member.to_string()))?;
    Ok(ResolvedDim { cube, dim, col_label: member.to_string() })
}

/// Resolve a `"cube.member"` string to the measure on `root_cube`, optionally through a join.
fn resolve_measure<'a>(
    model: &'a Model,
    root_cube: &'a Cube,
    member: &'a str,
) -> Result<ResolvedMeasure<'a>, CompileError> {
    let (cube, measure_name) = resolve_member_cube(model, root_cube, member)?;
    let measure = cube
        .measure(measure_name)
        .ok_or_else(|| CompileError::UnknownMember(member.to_string()))?;
    Ok(ResolvedMeasure { cube, measure, col_label: member.to_string() })
}

/// Build the `table_or_subquery AS "cube_name"` FROM fragment for a cube.
/// Used for both the root and any joined cube.
fn cube_from_clause(c: &Cube) -> Result<String, CompileError> {
    if let Some(sql_table) = &c.sql_table {
        Ok(format!("{} AS {}", sql_table, quote_ident(&c.name)))
    } else if let Some(base_sql) = &c.base_sql {
        Ok(format!(
            "({}) AS {}",
            expand_tokens(base_sql, &c.name),
            quote_ident(&c.name)
        ))
    } else {
        Err(CompileError::UnknownCube(format!(
            "cube '{}' has neither sql_table nor base_sql",
            c.name
        )))
    }
}

// ─── Main entry point ────────────────────────────────────────────────────────

/// Compile a `SemanticQuery` against a `Model` under a `QueryScope`.
///
/// Pure: no I/O.  Filter values become bind parameters; model SQL is templated in.
pub fn compile(
    model: &Model,
    q: &SemanticQuery,
    scope: &QueryScope,
) -> Result<CompiledQuery, CompileError> {
    // 1. Resolve root cube.
    let root_cube = model
        .cube(&q.cube)
        .ok_or_else(|| CompileError::UnknownCube(q.cube.clone()))?;

    // 2. Resolve every member BEFORE building any SQL.
    let resolved_dims: Vec<ResolvedDim<'_>> = q
        .dimensions
        .iter()
        .map(|m| resolve_dimension(model, root_cube, m))
        .collect::<Result<_, _>>()?;

    let resolved_measures: Vec<ResolvedMeasure<'_>> = q
        .measures
        .iter()
        .map(|m| resolve_measure(model, root_cube, m))
        .collect::<Result<_, _>>()?;

    // Resolve time dimension member (it is also a dimension).
    let resolved_time_dim = q
        .time_dimension
        .as_ref()
        .map(|td| resolve_dimension(model, root_cube, &td.member))
        .transpose()?;

    // Resolve segments.
    // Fix 4a: segments must be reachable from root_cube via a declared join (same rule as
    // dimensions/measures), not just found in the model by cube name.
    for seg_member in &q.segments {
        let (cube_name, seg_name) = split_member(seg_member)
            .ok_or_else(|| CompileError::UnknownSegment(seg_member.clone()))?;
        let cube = if cube_name == root_cube.name {
            root_cube
        } else {
            // Must be reachable via a declared join from root_cube.
            root_cube
                .joins
                .iter()
                .find(|j| j.name == cube_name || j.target_cube == cube_name)
                .ok_or_else(|| CompileError::UnreachableMember {
                    member: seg_member.clone(),
                    from_cube: root_cube.name.clone(),
                })?;
            model
                .cube(cube_name)
                .ok_or_else(|| CompileError::UnknownCube(cube_name.to_string()))?
        };
        cube.segment(seg_name)
            .ok_or_else(|| CompileError::UnknownSegment(seg_member.clone()))?;
    }

    // Resolve scope row-filter members through the same dimension whitelist.
    for rf in &scope.row_filters {
        resolve_dimension(model, root_cube, &rf.member)?;
    }

    // 3. Collect the set of distinct cube names across all members.
    // Fix 4b: also include cubes referenced by segments and order members.
    let mut distinct_cubes: Vec<&str> = vec![root_cube.name.as_str()];
    // push_cube: add a non-root cube name to the set if not already present.
    fn push_cube<'v>(v: &mut Vec<&'v str>, root: &str, name: &'v str) {
        if name != root && !v.contains(&name) {
            v.push(name);
        }
    }
    for rd in &resolved_dims {
        push_cube(&mut distinct_cubes, &root_cube.name, rd.cube.name.as_str());
    }
    for rm in &resolved_measures {
        push_cube(&mut distinct_cubes, &root_cube.name, rm.cube.name.as_str());
    }
    if let Some(td) = &resolved_time_dim {
        push_cube(&mut distinct_cubes, &root_cube.name, td.cube.name.as_str());
    }
    // Segments: cube name is the prefix of each "cube.segment" member.
    for seg_member in &q.segments {
        if let Some((cube_name, _)) = split_member(seg_member) {
            push_cube(&mut distinct_cubes, &root_cube.name, cube_name);
        }
    }
    // Order members: cube name is the prefix of each "cube.member" order key.
    for (order_member, _) in &q.order {
        if let Some((cube_name, _)) = split_member(order_member) {
            push_cube(&mut distinct_cubes, &root_cube.name, cube_name);
        }
    }

    if distinct_cubes.len() > 2 {
        return Err(CompileError::UnsupportedFanout(format!(
            "query spans {} cubes ({}) — Phase 1 supports at most 2 (a root + one join hop)",
            distinct_cubes.len(),
            distinct_cubes.join(", ")
        )));
    }

    // 4. If there is a second cube, find its join and check cardinality.
    let join_info: Option<(&crate::model::Join, &Cube)> = if distinct_cubes.len() == 2 {
        let joined_cube_name = distinct_cubes[1];
        let join = root_cube
            .joins
            .iter()
            .find(|j| j.name == joined_cube_name || j.target_cube == joined_cube_name)
            .ok_or_else(|| CompileError::UnreachableMember {
                member: joined_cube_name.to_string(),
                from_cube: root_cube.name.clone(),
            })?;

        let joined_cube = model
            .cube(joined_cube_name)
            .ok_or_else(|| CompileError::UnknownCube(joined_cube_name.to_string()))?;

        // 5. Fan-out detection: one_to_many is Phase-2+.
        if join.relationship == Relationship::OneToMany {
            // Check for non-additive measures on the joined cube.
            for rm in &resolved_measures {
                if rm.cube.name == joined_cube_name {
                    let non_additive = matches!(
                        rm.measure.agg_type,
                        AggType::Avg | AggType::CountDistinct | AggType::Number
                    );
                    if non_additive {
                        return Err(CompileError::NonAdditiveMeasureInFanout {
                            measure: rm.measure.name.clone(),
                            cube: joined_cube_name.to_string(),
                            agg_type: format!("{:?}", rm.measure.agg_type).to_lowercase(),
                        });
                    }
                }
            }
            // ponytail: two-leg fan-out generation — next increment
            return Err(CompileError::UnsupportedFanout(
                "two-leg fan-out generation is Phase-1 increment 2".into(),
            ));
        }

        Some((join, joined_cube))
    } else {
        None
    };

    // ── From here: single-cube or many_to_one / one_to_one join ──────────────

    // 6. Build binds list + bind counter.
    let mut binds: Vec<SqlValue> = Vec::new();
    let mut bind_n: usize = 0;
    // Returns the next $N placeholder (1-indexed) and advances the counter.
    // Call BEFORE pushing to `binds` so the index matches.
    macro_rules! next_bind {
        () => {{
            bind_n += 1;
            bind_n
        }};
    }

    // 7. SELECT list.
    let mut select_parts: Vec<String> = Vec::new();
    let mut columns: Vec<ColumnMeta> = Vec::new();
    let mut group_by_ordinals: Vec<usize> = Vec::new();

    // Dimensions.
    for rd in &resolved_dims {
        let col_sql = dim_sql(&rd.cube.name, rd.dim);
        let col_label = &rd.col_label;
        select_parts.push(format!("{col_sql} AS {}", quote_ident(col_label)));
        columns.push(ColumnMeta {
            name: col_label.clone(),
            data_type: match rd.dim.data_type {
                DataType::String => ColumnType::String,
                DataType::Number => ColumnType::Number,
                DataType::Time => ColumnType::Time,
                DataType::Boolean => ColumnType::Boolean,
            },
        });
        group_by_ordinals.push(select_parts.len()); // 1-indexed ordinal
    }

    // Time dimension (grouped as date_trunc).
    if let (Some(td_ref), Some(time_dim_info)) = (q.time_dimension.as_ref(), &resolved_time_dim) {
        let col_sql = dim_sql(&time_dim_info.cube.name, time_dim_info.dim);
        let grain = td_ref.granularity.trunc_unit();
        let trunc_expr = format!("date_trunc('{grain}', {col_sql})");
        let col_label = format!("{}.{}", td_ref.member, grain);
        select_parts.push(format!("{trunc_expr} AS {}", quote_ident(&col_label)));
        columns.push(ColumnMeta {
            name: col_label,
            data_type: ColumnType::Time,
        });
        group_by_ordinals.push(select_parts.len());
    }

    // Measures.
    for rm in &resolved_measures {
        let agg_sql = measure_select(&rm.cube.name, rm.measure);
        let col_label = &rm.col_label;
        select_parts.push(format!("{agg_sql} AS {}", quote_ident(col_label)));
        columns.push(ColumnMeta {
            name: col_label.clone(),
            data_type: ColumnType::Number,
        });
    }

    // 8. FROM clause.
    let from_clause = cube_from_clause(root_cube)?;

    // 9. JOIN clause (if two-cube many_to_one or one_to_one).
    let join_clause = if let Some((join, joined_cube)) = join_info {
        let on_sql = expand_tokens(&join.sql_on, &root_cube.name);
        let joined_from = cube_from_clause(joined_cube)?;
        Some(format!("LEFT JOIN {joined_from} ON {on_sql}"))
    } else {
        None
    };

    // 10. WHERE conditions.
    let mut where_parts: Vec<String> = Vec::new();

    // User filters (values → bound).
    for filter in &q.filters {
        let rd = resolve_dimension(model, root_cube, &filter.member)?;
        let col_sql = dim_sql(&rd.cube.name, rd.dim);
        let member = &filter.member;
        let clause = match filter.operator {
            FilterOp::Equals => {
                // Fix 2: require at least 1 value.
                if filter.values.is_empty() {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "Equals requires at least 1 value".into(),
                    });
                }
                // Use `= ANY($N)` for array equality (handles single + multi values uniformly).
                let n = next_bind!();
                binds.push(SqlValue::TextArray(filter.values.clone()));
                format!("{col_sql} = ANY(${n})")
            }
            FilterOp::NotEquals => {
                // Fix 2: require at least 1 value.
                if filter.values.is_empty() {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "NotEquals requires at least 1 value".into(),
                    });
                }
                let n = next_bind!();
                binds.push(SqlValue::TextArray(filter.values.clone()));
                // Fix 8: include NULL rows — col != ALL(...) excludes NULLs via three-valued logic.
                format!("({col_sql} != ALL(${n}) OR {col_sql} IS NULL)")
            }
            FilterOp::Contains => {
                // Fix 2: require at least 1 value.
                if filter.values.is_empty() {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "Contains requires at least 1 value".into(),
                    });
                }
                // Fix 3: escape LIKE wildcards before binding, emit ESCAPE clause.
                let mut clauses: Vec<String> = Vec::new();
                for v in &filter.values {
                    let escaped = v
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_");
                    let n = next_bind!();
                    binds.push(SqlValue::Text(format!("%{escaped}%")));
                    clauses.push(format!("{col_sql} ILIKE ${n} ESCAPE '\\'"));
                }
                format!("({})", clauses.join(" OR "))
            }
            FilterOp::Gt => {
                // Fix 2: require exactly 1 value.
                if filter.values.len() != 1 {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "Gt requires exactly 1 value".into(),
                    });
                }
                let n = next_bind!();
                if rd.dim.data_type == DataType::Time {
                    binds.push(SqlValue::Timestamp(filter.values[0].clone()));
                    format!("{col_sql} > ${n}::timestamptz")
                } else {
                    binds.push(SqlValue::Text(filter.values[0].clone()));
                    format!("{col_sql} > ${n}")
                }
            }
            FilterOp::Gte => {
                // Fix 2: require exactly 1 value.
                if filter.values.len() != 1 {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "Gte requires exactly 1 value".into(),
                    });
                }
                let n = next_bind!();
                if rd.dim.data_type == DataType::Time {
                    binds.push(SqlValue::Timestamp(filter.values[0].clone()));
                    format!("{col_sql} >= ${n}::timestamptz")
                } else {
                    binds.push(SqlValue::Text(filter.values[0].clone()));
                    format!("{col_sql} >= ${n}")
                }
            }
            FilterOp::Lt => {
                // Fix 2: require exactly 1 value.
                if filter.values.len() != 1 {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "Lt requires exactly 1 value".into(),
                    });
                }
                let n = next_bind!();
                if rd.dim.data_type == DataType::Time {
                    binds.push(SqlValue::Timestamp(filter.values[0].clone()));
                    format!("{col_sql} < ${n}::timestamptz")
                } else {
                    binds.push(SqlValue::Text(filter.values[0].clone()));
                    format!("{col_sql} < ${n}")
                }
            }
            FilterOp::Lte => {
                // Fix 2: require exactly 1 value.
                if filter.values.len() != 1 {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "Lte requires exactly 1 value".into(),
                    });
                }
                let n = next_bind!();
                if rd.dim.data_type == DataType::Time {
                    binds.push(SqlValue::Timestamp(filter.values[0].clone()));
                    format!("{col_sql} <= ${n}::timestamptz")
                } else {
                    binds.push(SqlValue::Text(filter.values[0].clone()));
                    format!("{col_sql} <= ${n}")
                }
            }
            FilterOp::Set => {
                // Fix 2: Set/NotSet ignore values — no arity requirement.
                format!("{col_sql} IS NOT NULL")
            }
            FilterOp::NotSet => {
                format!("{col_sql} IS NULL")
            }
            FilterOp::InDateRange => {
                // Fix 2: require exactly 2 values.
                if filter.values.len() != 2 {
                    return Err(CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: "InDateRange requires exactly 2 values [lo, hi]".into(),
                    });
                }
                let lo_n = next_bind!();
                binds.push(SqlValue::Timestamp(filter.values[0].clone()));
                // Fix 7: apply the same +1-day exclusive conversion as date range.
                let hi_exclusive = date_add_one_day(&filter.values[1]).map_err(|_| {
                    CompileError::InvalidFilter {
                        member: member.clone(),
                        reason: format!(
                            "InDateRange upper bound is not a valid YYYY-MM-DD date: '{}'",
                            filter.values[1]
                        ),
                    }
                })?;
                let hi_n = next_bind!();
                binds.push(SqlValue::Timestamp(hi_exclusive));
                format!("{col_sql} >= ${lo_n}::timestamptz AND {col_sql} < ${hi_n}::timestamptz")
            }
        };
        where_parts.push(clause);
    }

    // Segment predicates (model-authored — trusted, inlined after token expansion).
    for seg_member in &q.segments {
        let (cube_name, seg_name) = split_member(seg_member).unwrap();
        let cube = if cube_name == root_cube.name {
            root_cube
        } else {
            model.cube(cube_name).unwrap()
        };
        let seg = cube.segment(seg_name).unwrap();
        where_parts.push(expand_tokens(&seg.sql_expr, cube_name));
    }

    // Time dimension date range.
    if let (Some(td_ref), Some(time_dim_info)) = (q.time_dimension.as_ref(), &resolved_time_dim) {
        if let Some([lo, hi]) = &td_ref.date_range {
            let col_sql = dim_sql(&time_dim_info.cube.name, time_dim_info.dim);
            let lo_n = next_bind!();
            binds.push(SqlValue::Timestamp(lo.clone()));
            where_parts.push(format!("{col_sql} >= ${lo_n}::timestamptz"));
            // Upper bound: spec says inclusive input → exclusive `<` internally.
            let hi_n = next_bind!();
            binds.push(SqlValue::Timestamp(hi_upper_exclusive(hi)));
            where_parts.push(format!("{col_sql} < ${hi_n}::timestamptz"));
        }
    }

    // Scope: row filters (bound — resolved above for whitelist check).
    for rf in &scope.row_filters {
        let rd = resolve_dimension(model, root_cube, &rf.member)?;
        let col_sql = dim_sql(&rd.cube.name, rd.dim);
        let n = next_bind!();
        binds.push(SqlValue::Text(rf.value.clone()));
        // Cast to ::text to handle enum/non-text types (A3).
        where_parts.push(format!("({col_sql})::text = ${n}"));
    }

    // Structural tenant predicate: enforced here, not by the API layer (A1).
    // tenant isolation is structural: enforced here, not by the API layer.
    let tenant_n = next_bind!();
    binds.push(SqlValue::Uuid(scope.tenant_id));
    where_parts.push(format!("{} = ${tenant_n}", quote_ident(&root_cube.tenant_column)));

    // 11. ORDER BY.
    // Fix 5: every order member must be one of the selected measures or dimensions
    // (matched by the "cube.member" key used as the column label / alias).
    let selected_labels: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
    let mut order_parts: Vec<String> = Vec::new();
    for (member, dir) in &q.order {
        if !selected_labels.contains(&member.as_str()) {
            return Err(CompileError::UnknownMember(member.clone()));
        }
        let direction = match dir {
            Order::Asc => "ASC",
            Order::Desc => "DESC",
        };
        order_parts.push(format!("{} {direction}", quote_ident(member)));
    }

    // 12. LIMIT / OFFSET (bound).
    let limit_clause = if let Some(lim) = q.limit {
        let n = next_bind!();
        binds.push(SqlValue::Int(lim as i64));
        Some(format!("LIMIT ${n}"))
    } else {
        None
    };

    let offset_clause = if let Some(off) = q.offset {
        let n = next_bind!();
        binds.push(SqlValue::Int(off as i64));
        Some(format!("OFFSET ${n}"))
    } else {
        None
    };

    // 13. Assemble SQL.
    let mut sql = format!("SELECT\n  {}", select_parts.join(",\n  "));
    sql.push_str(&format!("\nFROM {from_clause}"));

    if let Some(jc) = join_clause {
        sql.push('\n');
        sql.push_str(&jc);
    }

    if !where_parts.is_empty() {
        sql.push_str("\nWHERE ");
        sql.push_str(&where_parts.join("\n  AND "));
    }

    if !group_by_ordinals.is_empty() {
        let gb = group_by_ordinals
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        sql.push_str(&format!("\nGROUP BY {gb}"));
    }

    if !order_parts.is_empty() {
        sql.push_str(&format!("\nORDER BY {}", order_parts.join(", ")));
    }

    if let Some(lc) = limit_clause {
        sql.push('\n');
        sql.push_str(&lc);
    }

    if let Some(oc) = offset_clause {
        sql.push('\n');
        sql.push_str(&oc);
    }

    Ok(CompiledQuery { sql, binds, columns })
}

// ─── Date helpers ─────────────────────────────────────────────────────────────

/// Convert an inclusive upper date bound to exclusive (+1 calendar day).
/// Fix 6: the exclusive bound is ALWAYS +1 calendar day regardless of granularity.
/// The granularity only affects date_trunc bucketing in SELECT, not the range bound.
/// Fix 9: use char-safe extraction (no byte-slicing) to avoid panics on multi-byte input.
fn hi_upper_exclusive(hi: &str) -> String {
    date_add_one_day(hi).unwrap_or_else(|_| hi.to_string())
}

/// Parse the `YYYY-MM-DD` prefix of `s` (char-safe), add one calendar day, return
/// `YYYY-MM-DD`.  Returns `Err(())` if the prefix is not a valid date.
fn date_add_one_day(s: &str) -> Result<String, ()> {
    // Take the first 10 chars (YYYY-MM-DD) in a Unicode-safe way.
    let date_str: String = s.chars().take(10).collect();
    if date_str.len() != 10 {
        return Err(());
    }
    let parts: Vec<&str> = date_str.splitn(3, '-').collect();
    if parts.len() != 3 {
        return Err(());
    }
    let year: i32 = parts[0].parse().map_err(|_| ())?;
    let month: u32 = parts[1].parse().map_err(|_| ())?;
    let day: u32 = parts[2].parse().map_err(|_| ())?;

    // Basic sanity: month 1-12, day 1-31.
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(());
    }

    Ok(add_days(year, month, day, 1))
}

fn add_days(year: i32, month: u32, day: u32, delta: u32) -> String {
    let mut d = day + delta;
    let mut m = month;
    let mut y = year;
    loop {
        let days_in_m = days_in_month(y, m);
        if d <= days_in_m {
            break;
        }
        d -= days_in_m;
        m += 1;
        if m > 12 {
            m = 1;
            y += 1;
        }
    }
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::error::CompileError;
    use crate::model::{AggType, Cube, DataType, Dimension, Measure, Model, Segment};
    use crate::query::{Filter, FilterOp, Order, QueryScope, SemanticQuery};
    use uuid::Uuid;

    // ── existing tests ────────────────────────────────────────────────────────

    #[test]
    fn hi_upper_month() {
        assert_eq!(hi_upper_exclusive("2026-06-30"), "2026-07-01");
    }

    #[test]
    fn hi_upper_year() {
        assert_eq!(hi_upper_exclusive("2026-12-31"), "2027-01-01");
    }

    #[test]
    fn hi_upper_day() {
        assert_eq!(hi_upper_exclusive("2026-01-31"), "2026-02-01");
    }

    #[test]
    fn quote_ident_escapes_double_quote() {
        assert_eq!(quote_ident("orders"), "\"orders\"");
        assert_eq!(quote_ident("or\"ders"), "\"or\"\"ders\"");
    }

    #[test]
    fn expand_tokens_replaces_cube_and_named() {
        let out = expand_tokens("{CUBE}.col = {customers}.id", "orders");
        assert_eq!(out, "\"orders\".col = \"customers\".id");
    }

    // ── helpers ───────────────────────────────────────────────────────────────

    fn minimal_model() -> Model {
        Model::from_cubes(vec![Cube {
            name: "orders".into(),
            data_source: "default".into(),
            sql_table: Some("public.orders".into()),
            base_sql: None,
            primary_key: "id".into(),
            tenant_column: "tenant_id".into(),
            description: None,
            dimensions: vec![
                Dimension {
                    name: "status".into(),
                    sql_expr: "{CUBE}.status".into(),
                    data_type: DataType::String,
                    description: None,
                },
                Dimension {
                    name: "amount".into(),
                    sql_expr: "{CUBE}.amount".into(),
                    data_type: DataType::Number,
                    description: None,
                },
                Dimension {
                    name: "created_at".into(),
                    sql_expr: "{CUBE}.created_at".into(),
                    data_type: DataType::Time,
                    description: None,
                },
            ],
            measures: vec![Measure {
                name: "count".into(),
                sql_expr: None,
                agg_type: AggType::Count,
                description: None,
            }],
            joins: vec![],
            segments: vec![Segment {
                name: "active".into(),
                sql_expr: "{CUBE}.is_active = true".into(),
            }],
        }])
        .unwrap()
    }

    fn scope() -> QueryScope {
        QueryScope { tenant_id: Uuid::nil(), row_filters: vec![] }
    }

    fn base_query() -> SemanticQuery {
        SemanticQuery {
            cube: "orders".into(),
            measures: vec!["orders.count".into()],
            dimensions: vec!["orders.status".into()],
            filters: vec![],
            segments: vec![],
            time_dimension: None,
            order: vec![],
            limit: None,
            offset: None,
        }
    }

    // ── Fix 2: filter arity ───────────────────────────────────────────────────

    #[test]
    fn fix2_gt_empty_values_returns_err() {
        let model = minimal_model();
        let mut q = base_query();
        q.filters = vec![Filter {
            member: "orders.amount".into(),
            operator: FilterOp::Gt,
            values: vec![],
        }];
        let err = compile(&model, &q, &scope()).unwrap_err();
        assert!(
            matches!(err, CompileError::InvalidFilter { .. }),
            "expected InvalidFilter, got {err:?}"
        );
    }

    #[test]
    fn fix2_in_date_range_one_value_returns_err() {
        let model = minimal_model();
        let mut q = base_query();
        q.filters = vec![Filter {
            member: "orders.created_at".into(),
            operator: FilterOp::InDateRange,
            values: vec!["2026-01-01".into()],
        }];
        let err = compile(&model, &q, &scope()).unwrap_err();
        assert!(
            matches!(err, CompileError::InvalidFilter { .. }),
            "expected InvalidFilter, got {err:?}"
        );
    }

    // ── Fix 3: LIKE wildcard escaping ─────────────────────────────────────────

    #[test]
    fn fix3_contains_percent_is_escaped_and_escape_clause_emitted() {
        let model = minimal_model();
        let mut q = base_query();
        q.filters = vec![Filter {
            member: "orders.status".into(),
            operator: FilterOp::Contains,
            values: vec!["%".into()],
        }];
        let result = compile(&model, &q, &scope()).unwrap();
        // The bound value must be the escaped pattern, not a bare '%'.
        assert!(
            result.binds.iter().any(|b| matches!(b, SqlValue::Text(v) if v == "%\\%%")),
            "expected bind \\% but got: {:?}",
            result.binds
        );
        // SQL must contain ESCAPE clause.
        assert!(
            result.sql.contains("ESCAPE"),
            "expected ESCAPE in SQL but got: {}",
            result.sql
        );
    }

    // ── Fix 4a: segment reachability ─────────────────────────────────────────

    #[test]
    fn fix4a_segment_unreachable_cube_returns_err() {
        // "products" cube exists nowhere in the model.
        let model = minimal_model();
        let mut q = base_query();
        q.segments = vec!["products.expensive".into()];
        let err = compile(&model, &q, &scope()).unwrap_err();
        assert!(
            matches!(err, CompileError::UnreachableMember { .. }),
            "expected UnreachableMember, got {err:?}"
        );
    }

    // ── Fix 5: ORDER BY validation ────────────────────────────────────────────

    #[test]
    fn fix5_order_on_nonexistent_member_returns_err() {
        let model = minimal_model();
        let mut q = base_query();
        q.order = vec![("orders.nonexistent".into(), Order::Asc)];
        let err = compile(&model, &q, &scope()).unwrap_err();
        assert!(
            matches!(err, CompileError::UnknownMember(_)),
            "expected UnknownMember, got {err:?}"
        );
    }

    #[test]
    fn fix5_order_on_selected_measure_is_ok() {
        let model = minimal_model();
        let mut q = base_query();
        q.order = vec![("orders.count".into(), Order::Desc)];
        assert!(compile(&model, &q, &scope()).is_ok());
    }

    // ── Fix 6: date upper bound always +1 day ─────────────────────────────────

    #[test]
    fn fix6_quarter_upper_bound_is_one_day_not_three_months() {
        // dateRange ["2026-01-01","2026-03-31"] grain=quarter → exclusive upper "2026-04-01"
        assert_eq!(hi_upper_exclusive("2026-03-31"), "2026-04-01");
    }

    #[test]
    fn fix6_feb28_non_leap_year() {
        // 2026 is not a leap year — Feb has 28 days.
        assert_eq!(date_add_one_day("2026-02-28").unwrap(), "2026-03-01");
    }

    #[test]
    fn fix6_feb28_leap_year() {
        // 2024 is a leap year — Feb has 29 days.
        assert_eq!(date_add_one_day("2024-02-28").unwrap(), "2024-02-29");
    }

    #[test]
    fn fix6_dec31_rolls_year() {
        assert_eq!(date_add_one_day("2026-12-31").unwrap(), "2027-01-01");
    }

    // ── Fix 7: InDateRange exclusive upper bound ───────────────────────────────

    #[test]
    fn fix7_in_date_range_exclusive_upper_bind() {
        let model = minimal_model();
        let mut q = base_query();
        q.filters = vec![Filter {
            member: "orders.created_at".into(),
            operator: FilterOp::InDateRange,
            values: vec!["2026-01-01".into(), "2026-06-30".into()],
        }];
        let result = compile(&model, &q, &scope()).unwrap();
        // The second bind (hi) must be converted to the day after 2026-06-30.
        assert!(
            result
                .binds
                .iter()
                .any(|b| matches!(b, SqlValue::Timestamp(v) if v == "2026-07-01")),
            "expected Timestamp(2026-07-01) bind but got: {:?}",
            result.binds
        );
    }

    // ── Fix 8: NotEquals includes NULL rows ───────────────────────────────────

    #[test]
    fn fix8_not_equals_includes_null_check() {
        let model = minimal_model();
        let mut q = base_query();
        q.filters = vec![Filter {
            member: "orders.status".into(),
            operator: FilterOp::NotEquals,
            values: vec!["active".into()],
        }];
        let result = compile(&model, &q, &scope()).unwrap();
        assert!(
            result.sql.contains("IS NULL"),
            "expected 'IS NULL' in NotEquals SQL but got: {}",
            result.sql
        );
    }

    // ── Fix 9: UTF-8 / malformed date does not panic ──────────────────────────

    #[test]
    fn fix9_unicode_date_string_returns_err_not_panic() {
        // A string with a multi-byte char near byte 10 must not panic.
        let result = date_add_one_day("2026-01-\u{1F600}1");
        assert!(result.is_err(), "expected Err for invalid date, got {result:?}");
    }

    #[test]
    fn fix9_malformed_date_string_returns_err() {
        let result = date_add_one_day("not-a-date");
        assert!(result.is_err());
    }

    // ── A1: structural tenant predicate ──────────────────────────────────────

    #[test]
    fn structural_tenant_predicate_always_present() {
        let model = minimal_model();
        let q = base_query();
        let s = QueryScope { tenant_id: Uuid::new_v4(), row_filters: vec![] };
        let result = compile(&model, &q, &s).unwrap();
        assert!(result.binds.iter().any(|b| matches!(b, SqlValue::Uuid(_))),
            "expected Uuid bind for tenant predicate");
        assert!(result.sql.contains("\"tenant_id\""),
            "expected tenant_column in SQL: {}", result.sql);
    }

    // ── A2: ::timestamptz cast ────────────────────────────────────────────────

    #[test]
    fn date_range_emits_timestamptz_cast() {
        use crate::query::{TimeDimension, Granularity};
        let model = minimal_model();
        let mut q = base_query();
        q.measures = vec![];
        q.dimensions = vec![];
        q.time_dimension = Some(TimeDimension {
            member: "orders.created_at".into(),
            granularity: Granularity::Day,
            date_range: Some(["2026-01-01".into(), "2026-06-30".into()]),
        });
        let result = compile(&model, &q, &scope()).unwrap();
        assert!(result.sql.contains("::timestamptz"),
            "expected ::timestamptz cast in SQL: {}", result.sql);
    }

    #[test]
    fn gt_filter_on_time_dim_emits_timestamptz_cast() {
        let model = minimal_model();
        let mut q = base_query();
        q.filters = vec![Filter {
            member: "orders.created_at".into(),
            operator: FilterOp::Gt,
            values: vec!["2026-01-01".into()],
        }];
        let result = compile(&model, &q, &scope()).unwrap();
        assert!(result.sql.contains("::timestamptz"),
            "expected ::timestamptz cast for Gt on Time dim: {}", result.sql);
        assert!(result.binds.iter().any(|b| matches!(b, SqlValue::Timestamp(_))),
            "expected Timestamp bind for Gt on Time dim");
    }

    // ── A3: row filter ::text cast ────────────────────────────────────────────

    #[test]
    fn row_filter_emits_text_cast() {
        use crate::query::RowFilter;
        let model = minimal_model();
        let q = base_query();
        let s = QueryScope {
            tenant_id: Uuid::nil(),
            row_filters: vec![RowFilter { member: "orders.status".into(), value: "active".into() }],
        };
        let result = compile(&model, &q, &s).unwrap();
        assert!(result.sql.contains("::text"),
            "expected ::text cast for row_filter: {}", result.sql);
    }
}
