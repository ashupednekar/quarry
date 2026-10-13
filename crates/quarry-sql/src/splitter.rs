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

///// `split_statements` with a dialect's extra rules (`#` comments, backslash escapes)
pub fn split_statements_with(input: &str, rules: SplitRules) -> Vec<StatementSpan> {
    let mut statements = Vec::new();
    let mut stmt_start: Option<usize> = None;

    for tok in quarry_core::lexer::SqlLexer::with_rules(input, rules) {
        if stmt_start.is_none() {
            stmt_start = Some(tok.span.start);
        }

        // SQL Server GO batch or Oracle / block
        if (rules.go_batches && tok.is_keyword("go")) || (rules.slash_blocks && tok.is_punct('/')) {
            let line_start = input[..tok.span.start].rfind('\n').map_or(0, |p| p + 1);
            let line_end = input[tok.span.end..]
                .find('\n')
                .map_or(input.len(), |p| tok.span.end + p);
            let before = &input[line_start..tok.span.start];
            let after = &input[tok.span.end..line_end];

            if before.trim().is_empty() && after.trim().is_empty() {
                let start = stmt_start.unwrap_or(line_start);
                let sql_part = input[start..tok.span.start].trim();
                if !sql_part.is_empty() {
                    statements.push(StatementSpan {
                        start,
                        end: tok.span.start,
                        sql: sql_part.to_string(),
                    });
                }
                stmt_start = None;
                continue;
            }
        }

        // Semicolon statement boundary
        if tok.is_punct(';') {
            if rules.go_batches {
                // Semicolons do not split batches in SQL Server scripts
                continue;
            }

            let start = stmt_start.unwrap_or(tok.span.start);
            if rules.slash_blocks
                && quarry_core::dialect::is_plsql_block(&input[start..tok.span.start])
            {
                // In Oracle PL/SQL blocks, semicolons inside the block do not split
                continue;
            }

            let sql_part = input[start..tok.span.start].trim();
            if !sql_part.is_empty() {
                statements.push(StatementSpan {
                    start,
                    end: tok.span.end,
                    sql: sql_part.to_string(),
                });
            }
            stmt_start = None;
        }
    }

    // Trailing statement without semicolon
    if let Some(start) = stmt_start {
        let sql_part = input[start..].trim();
        if !sql_part.is_empty() {
            statements.push(StatementSpan {
                start,
                end: input.len(),
                sql: sql_part.to_string(),
            });
        }
    }

    statements
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

/// AST-based statement splitting using Tree-sitter SQL grammar
pub fn tree_sitter_split_statements(input: &str) -> Vec<StatementSpan> {
    let mut parser = tree_sitter::Parser::new();
    let language: tree_sitter::Language = tree_sitter_sequel::LANGUAGE.into();
    if parser.set_language(&language).is_err() {
        return split_statements(input);
    }
    let Some(tree) = parser.parse(input, None) else {
        return split_statements(input);
    };

    let root = tree.root_node();
    let mut statements = Vec::new();
    let mut cursor = root.walk();

    for child in root.children(&mut cursor) {
        let kind = child.kind();
        if kind == ";" || kind == "comment" || kind == "marginalia" {
            continue;
        }

        let start = child.start_byte();
        let mut end = child.end_byte();

        if end < input.len() && input.as_bytes()[end] == b';' {
            end += 1;
        }

        let sql_part = input[start..end].trim();
        if !sql_part.is_empty() {
            statements.push(StatementSpan {
                start,
                end,
                sql: sql_part.to_string(),
            });
        }
    }

    if statements.is_empty() && !input.trim().is_empty() {
        return split_statements(input);
    }

    statements
}

/// Finds the statement at the given cursor byte offset using Tree-sitter AST boundaries
pub fn tree_sitter_statement_at_offset(input: &str, cursor_offset: usize) -> Option<StatementSpan> {
    let statements = tree_sitter_split_statements(input);
    if statements.is_empty() {
        return None;
    }

    for stmt in &statements {
        if cursor_offset >= stmt.start && cursor_offset <= stmt.end {
            return Some(stmt.clone());
        }
    }

    if let Some(last) = statements.last() {
        if cursor_offset >= last.end {
            return Some(last.clone());
        }
    }

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

    #[test]
    fn test_tree_sitter_split_statements() {
        let sql =
            "SELECT * FROM users WHERE active = true;\n\nINSERT INTO logs (msg) VALUES ('test');";
        let spans = tree_sitter_split_statements(sql);
        assert_eq!(spans.len(), 2);
        assert!(spans[0].sql.starts_with("SELECT"));
        assert!(spans[1].sql.starts_with("INSERT"));

        let at_first = tree_sitter_statement_at_offset(sql, 10);
        assert!(at_first.is_some());
        assert!(at_first.unwrap().sql.starts_with("SELECT"));

        let at_second = tree_sitter_statement_at_offset(sql, 55);
        assert!(at_second.is_some());
        assert!(at_second.unwrap().sql.starts_with("INSERT"));
    }
}
