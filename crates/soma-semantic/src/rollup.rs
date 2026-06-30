/// Phase-2 seam: checks whether a pre-aggregation rollup matches the requested members.
/// Always returns `None` in Phase 1 — rollup generation is deferred.
/// ponytail: rollup matching — Phase-2 pre-aggregations (Postgres MATERIALIZED VIEWs)
pub fn rollup_match() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollup_match_always_none() {
        assert_eq!(rollup_match(), None);
    }
}
