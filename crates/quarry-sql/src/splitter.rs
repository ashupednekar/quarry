// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

pub use quarry_core::dialect::SplitRules;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementSpan {
    pub start: usize,
    pub end: usize,
    pub sql: String,
}

/// Splits a SQL script into individual statements, correctly ignoring semicolons inside:
/// 1. Single-quoted strings: `'foo;bar'`
/// 2. Double-quoted identifiers: `"col;name"`
/// 3. Single-line comments: `-- comment;`
/// 4. Multi-line comments: `/* comment; */`
/// 5. PostgreSQL dollar-quoted strings: `$$ ...;... $$` or `$tag$ ...;... $tag$`
/// 6. Backtick identifiers: `` `col;name` `` (MySQL)
pub fn split_statements(input: &str) -> Vec<StatementSpan> {
    split_statements_with(input, SplitRules::default())
}

/// `split_statements` with a dialect's extra rules (`#` comments, backslash escapes)
pub fn split_statements_with(input: &str, rules: SplitRules) -> Vec<StatementSpan> {
    let mut statements = Vec::new();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut stmt_start = 0;

    while i < len {
        // SQL Server: a line that's just GO ends the batch. Oracle: a line that's just / ends a block.
        if (rules.go_batches || rules.slash_blocks)
            && input[..i]
                .rsplit('\n')
                .next()
                .is_some_and(|l| l.trim().is_empty())
        {
            let line_end = input[i..].find('\n').map_or(len, |n| i + n);
            let line = input[i..line_end].trim();
            if (rules.go_batches && line.eq_ignore_ascii_case("go"))
                || (rules.slash_blocks && line == "/")
            {
                let sql_part = input[stmt_start..i].trim();
                if !sql_part.is_empty() {
                    statements.push(StatementSpan {
                        start: stmt_start,
                        end: i,
                        sql: sql_part.to_string(),
                    });
                }
                i = line_end;
                stmt_start = i;
                continue;
            }
        }

        // Skip leading whitespace when starting a new statement
        if stmt_start == i && bytes[i].is_ascii_whitespace() {
            stmt_start += 1;
            i += 1;
            continue;
        }

        // Single-line comment: -- ... (or # ... where the dialect says so)
        if i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        if rules.hash_comments && bytes[i] == b'#' {
            i += 1;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        // Multi-line comment: /* ... */
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < len {
                i += 2; // skip */
            }
            continue;
        }

        // Single-quoted string: '...' (with '' escape)
        if bytes[i] == b'\'' {
            i += 1;
            while i < len {
                if rules.backslash_escapes && bytes[i] == b'\\' {
                    i += 1;
                    if i < len {
                        let ch_len = input[i..].chars().next().map_or(1, |c| c.len_utf8());
                        i += ch_len;
                    }
                    continue;
                }
                if bytes[i] == b'\'' {
                    if i + 1 < len && bytes[i + 1] == b'\'' {
                        i += 2; // skip ''
                    } else {
                        i += 1; // closing quote
                        break;
                    }
                } else {
                    let ch_len = input[i..].chars().next().map_or(1, |c| c.len_utf8());
                    i += ch_len;
                }
            }
            continue;
        }

        // Double-quoted identifier: "..." (with "" escape), or a backtick one: `...`
        if bytes[i] == b'"' || bytes[i] == b'`' {
            let q = bytes[i];
            i += 1;
            while i < len {
                if bytes[i] == q {
                    if i + 1 < len && bytes[i + 1] == q {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    let ch_len = input[i..].chars().next().map_or(1, |c| c.len_utf8());
                    i += ch_len;
                }
            }
            continue;
        }

        // PostgreSQL dollar-quoting: $$...$$ or $tag$...$tag$
        if bytes[i] == b'$' {
            if let Some((tag, tag_end)) = parse_dollar_tag(&bytes[i..]) {
                let closing_tag = tag;
                i += tag_end; // move past opening dollar tag

                // Search for matching closing tag
                while i < len {
                    if bytes[i] == b'$' && bytes[i..].starts_with(closing_tag.as_bytes()) {
                        i += closing_tag.len();
                        break;
                    }
                    i += 1;
                }
                continue;
            }
        }

        // Semicolon statement boundary (not in GO-batched scripts: a procedure body is full of them)
        if bytes[i] == b';'
            && !rules.go_batches
            && !(rules.slash_blocks && quarry_core::dialect::is_plsql_block(&input[stmt_start..i]))
        {
            let sql_part = input[stmt_start..i].trim();
            if !sql_part.is_empty() {
                statements.push(StatementSpan {
                    start: stmt_start,
                    end: i + 1,
                    sql: sql_part.to_string(),
                });
            }
            i += 1;
            stmt_start = i;
            continue;
        }

        let ch_len = input[i..].chars().next().map_or(1, |c| c.len_utf8());
        i += ch_len;
    }

    // Trailing statement without trailing semicolon
    if stmt_start < len {
        let sql_part = input[stmt_start..].trim();
        if !sql_part.is_empty() {
            statements.push(StatementSpan {
                start: stmt_start,
                end: len,
                sql: sql_part.to_string(),
            });
        }
    }

    statements
}

fn parse_dollar_tag(slice: &[u8]) -> Option<(String, usize)> {
    if slice.is_empty() || slice[0] != b'$' {
        return None;
    }
    // Simple $$
    if slice.len() >= 2 && slice[1] == b'$' {
        return Some(("$$".to_string(), 2));
    }
    // Tagged $tag$
    let mut i = 1;
    while i < slice.len() {
        let b = slice[i];
        if b == b'$' {
            let tag = String::from_utf8_lossy(&slice[0..=i]).to_string();
            return Some((tag, i + 1));
        }
        if !(b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        i += 1;
    }
    None
}

/// Finds the single statement containing the cursor byte offset, ideal for ⌘↵ (Run Current Statement)
pub fn find_statement_at_offset(input: &str, cursor_offset: usize) -> Option<StatementSpan> {
    find_statement_at_offset_with(input, cursor_offset, SplitRules::default())
}

pub fn find_statement_at_offset_with(
    input: &str,
    cursor_offset: usize,
    rules: SplitRules,
) -> Option<StatementSpan> {
    let statements = split_statements_with(input, rules);
    if statements.is_empty() {
        return None;
    }

    // Find statement where cursor falls in [start, end]
    for stmt in &statements {
        if cursor_offset >= stmt.start && cursor_offset <= stmt.end {
            return Some(stmt.clone());
        }
    }

    // If cursor is past the end, return the last statement
    if let Some(last) = statements.last() {
        if cursor_offset >= last.end {
            return Some(last.clone());
        }
    }

    // Default to the first statement
    statements.first().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_with_string_semicolon() {
        let sql = "SELECT 'hello;world' AS greeting; SELECT 42;";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0].sql, "SELECT 'hello;world' AS greeting");
        assert_eq!(stmts[1].sql, "SELECT 42");
    }

    #[test]
    fn test_split_with_comments() {
        let sql = r#"
            -- This is a comment; with semicolon
            SELECT * FROM users;
            /* Multi-line comment;
               still in comment;
            */
            SELECT * FROM orders;
        "#;
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].sql.contains("SELECT * FROM users"));
        assert!(stmts[1].sql.contains("SELECT * FROM orders"));
    }

    #[test]
    fn test_split_with_dollar_quoting() {
        let sql = r#"
            CREATE OR REPLACE FUNCTION test_func() RETURNS void AS $$
            BEGIN
                INSERT INTO logs VALUES ('done;');
            END;
            $$ LANGUAGE plpgsql;

            SELECT * FROM test_func();
        "#;
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].sql.contains("CREATE OR REPLACE FUNCTION"));
        assert!(stmts[1].sql.contains("SELECT * FROM test_func()"));
    }

    #[test]
    fn test_find_statement_at_cursor() {
        let sql = "SELECT 1;\nSELECT 2;\nSELECT 3;";
        // offset inside "SELECT 2;"
        let span = find_statement_at_offset(sql, 12).unwrap();
        assert_eq!(span.sql, "SELECT 2");
    }
}

#[cfg(test)]
mod dialect_tests {
    use super::*;

    const MYSQL: SplitRules = SplitRules {
        hash_comments: true,
        backslash_escapes: true,
        go_batches: false,
        slash_blocks: false,
    };
    const MSSQL: SplitRules = SplitRules {
        hash_comments: false,
        backslash_escapes: false,
        go_batches: true,
        slash_blocks: false,
    };
    const ORACLE: SplitRules = SplitRules {
        hash_comments: false,
        backslash_escapes: false,
        go_batches: false,
        slash_blocks: true,
    };

    #[test]
    fn oracle_blocks_end_at_slash_and_plain_sql_at_semicolon() {
        let sql = "select 1 from dual;\nCREATE OR REPLACE PROCEDURE p AS\nBEGIN\n  NULL;\nEND;\n/\nselect 2 from dual";
        let parts: Vec<String> = split_statements_with(sql, ORACLE)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        assert_eq!(
            parts,
            vec![
                "select 1 from dual",
                "CREATE OR REPLACE PROCEDURE p AS\nBEGIN\n  NULL;\nEND;",
                "select 2 from dual"
            ]
        );
    }

    #[test]
    fn sql_server_batches_end_at_go() {
        let sql = "CREATE PROCEDURE p AS\nBEGIN\n  SELECT 1;\n  SELECT 'GO';\nEND\n  go  \nSELECT 2;\nSELECT 3";
        let parts: Vec<String> = split_statements_with(sql, MSSQL)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        assert_eq!(
            parts,
            vec![
                "CREATE PROCEDURE p AS\nBEGIN\n  SELECT 1;\n  SELECT 'GO';\nEND",
                "SELECT 2;\nSELECT 3"
            ]
        );
    }

    #[test]
    fn mysql_hash_comments_backslashes_and_backticks() {
        let sql = "select 'it\\'s; fine' from `odd;name`; # a comment; not a statement\nselect 2";
        let parts: Vec<String> = split_statements_with(sql, MYSQL)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        assert_eq!(
            parts,
            vec![
                "select 'it\\'s; fine' from `odd;name`",
                "# a comment; not a statement\nselect 2"
            ]
        );
    }

    #[test]
    fn hash_is_an_operator_in_postgres() {
        let sql = "select '{\"a\":1}'::jsonb #> '{a}'; select 2";
        assert_eq!(split_statements(sql).len(), 2);
    }

    #[test]
    fn mysql_hash_comment_on_newline_does_not_swallow_next_statement() {
        let sql = "#\nSELECT 1;\n# comment\nSELECT 2;";
        let parts: Vec<String> = split_statements_with(sql, MYSQL)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        assert_eq!(parts, vec!["#\nSELECT 1", "# comment\nSELECT 2"]);
    }

    #[test]
    fn mysql_multibyte_utf8_backslash_escapes_do_not_panic() {
        let sql = "SELECT 'hello\\🦀'; SELECT 2;";
        let parts: Vec<String> = split_statements_with(sql, MYSQL)
            .into_iter()
            .map(|s| s.sql)
            .collect();
        assert_eq!(parts, vec!["SELECT 'hello\\🦀'", "SELECT 2"]);
    }
}
