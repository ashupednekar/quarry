// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

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
pub fn split_statements(input: &str) -> Vec<StatementSpan> {
    let mut statements = Vec::new();
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut stmt_start = 0;

    while i < len {
        // Skip leading whitespace when starting a new statement
        if stmt_start == i && bytes[i].is_ascii_whitespace() {
            stmt_start += 1;
            i += 1;
            continue;
        }

        // Single-line comment: -- ...
        if i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'-' {
            i += 2;
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
                if bytes[i] == b'\'' {
                    if i + 1 < len && bytes[i + 1] == b'\'' {
                        i += 2; // skip ''
                    } else {
                        i += 1; // closing quote
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            continue;
        }

        // Double-quoted identifier: "..." (with "" escape)
        if bytes[i] == b'"' {
            i += 1;
            while i < len {
                if bytes[i] == b'"' {
                    if i + 1 < len && bytes[i + 1] == b'"' {
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    i += 1;
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

        // Semicolon statement boundary
        if bytes[i] == b';' {
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

        i += 1;
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
    let statements = split_statements(input);
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
