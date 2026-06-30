use crate::error::ModelError;

/// Reject SQL fragments that contain injection vectors.
/// Called at model-save time on every sql_expr, sql_on, segment sql_expr, and base_sql.
pub fn validate_sql_fragment(fragment: &str) -> Result<(), ModelError> {
    // Reject comment starters and statement terminators.
    for banned in ["--", "/*", "*/", ";"] {
        if fragment.contains(banned) {
            return Err(ModelError::InvalidSqlFragment(format!(
                "contains forbidden token '{banned}'"
            )));
        }
    }

    // Reject dangerous DDL/DML keywords at word boundaries (case-insensitive).
    const BANNED_KEYWORDS: &[&str] = &[
        "UNION", "DROP", "ALTER", "CREATE", "INSERT", "UPDATE", "DELETE", "TRUNCATE", "GRANT",
        "REVOKE",
    ];
    let upper = fragment.to_ascii_uppercase();
    for kw in BANNED_KEYWORDS {
        if contains_word(&upper, kw) {
            return Err(ModelError::InvalidSqlFragment(format!(
                "contains forbidden keyword '{kw}'"
            )));
        }
    }

    Ok(())
}

/// ASCII word-boundary check: the keyword must not be adjacent to [A-Z0-9_].
fn contains_word(haystack: &str, needle: &str) -> bool {
    let hay = haystack.as_bytes();
    let n = needle.len();
    if n == 0 || n > hay.len() {
        return false;
    }
    for i in 0..=hay.len() - n {
        if &hay[i..i + n] == needle.as_bytes() {
            let before_ok = i == 0 || !is_word_char(hay[i - 1]);
            let after_ok = i + n == hay.len() || !is_word_char(hay[i + n]);
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_semicolon() {
        assert!(validate_sql_fragment("status; DROP TABLE").is_err());
    }

    #[test]
    fn rejects_line_comment() {
        assert!(validate_sql_fragment("status -- comment").is_err());
    }

    #[test]
    fn rejects_block_comment_open() {
        assert!(validate_sql_fragment("status /* oops").is_err());
    }

    #[test]
    fn rejects_block_comment_close() {
        assert!(validate_sql_fragment("*/ status").is_err());
    }

    #[test]
    fn rejects_drop_keyword() {
        assert!(validate_sql_fragment("DROP TABLE orders").is_err());
        assert!(validate_sql_fragment("drop table orders").is_err());
    }

    #[test]
    fn rejects_union_keyword() {
        assert!(validate_sql_fragment("1=1 UNION SELECT").is_err());
    }

    #[test]
    fn rejects_create_keyword() {
        assert!(validate_sql_fragment("CREATE INDEX").is_err());
    }

    #[test]
    fn rejects_truncate_keyword() {
        assert!(validate_sql_fragment("TRUNCATE TABLE foo").is_err());
    }

    #[test]
    fn rejects_grant_keyword() {
        assert!(validate_sql_fragment("GRANT SELECT ON foo TO bar").is_err());
    }

    #[test]
    fn allows_normal_expressions() {
        assert!(validate_sql_fragment("status").is_ok());
        assert!(validate_sql_fragment("{CUBE}.amount_cents > 100000").is_ok());
        assert!(validate_sql_fragment("created_at").is_ok());
        assert!(validate_sql_fragment("public.orders").is_ok());
        // "updates" contains "update" but NOT at a word boundary that isolates it
        // Actually "updates" → the keyword is UPDATE and updates has UPDATE at start but then 'S' follows
        // That is "UPDATES" — word boundary after: 'S' is word char so NOT banned. Correct.
        assert!(validate_sql_fragment("updates_count").is_ok());
        assert!(validate_sql_fragment("user_created_at").is_ok());
    }

    #[test]
    fn allows_select_and_from_in_subquery_base_sql() {
        // base_sql is a SELECT; SELECT and FROM are not banned (only the DDL/DML keywords above).
        assert!(validate_sql_fragment("SELECT id, amount FROM orders WHERE is_active = true").is_ok());
    }
}
