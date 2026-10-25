// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Zero-copy SQL lexer and tokenizer shared across query execution, statement splitting,
//! parameter binding, and dialect analysis.
//!
//! Yields [`Token`] spans borrowing directly from the input string without heap allocations.

use crate::dialect::SplitRules;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind<'a> {
    /// Keyword or identifier (e.g. `SELECT`, `FROM`, `users`, `table_name`)
    Word(&'a str),
    /// Single-quoted string literal (e.g. `'hello'`, `'it\'s'`)
    StringLiteral(&'a str),
    /// Quoted identifier: double quotes `"col"` or MySQL backticks `` `col` ``
    QuotedIdent(&'a str),
    /// PostgreSQL dollar-quoted string: `$$...$$` or `$tag$...$tag$`
    DollarQuoted { tag: &'a str },
    /// Single-line (`--`, `#`) or multi-line (`/* ... */`) comment
    Comment(&'a str),
    /// Parameter placeholder: `:name` or `$1`
    Placeholder(&'a str),
    /// PostgreSQL type cast symbol `::`
    DoubleColon,
    /// Structural punctuation: `;`, `,`, `(`, `)`, `.`, `=`, etc.
    Punctuation(char),
    /// Multi-character operators: `!=`, `<>`, `>=`, `<=`, `->`, `->>`, `#> `, etc.
    Operator(&'a str),
}

/// A parsed lexical token borrowing directly from the underlying SQL text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    /// Grammatical token classification.
    pub kind: TokenKind<'a>,
    /// Byte range of this token within the input text.
    pub span: std::ops::Range<usize>,
    /// Borrowed raw string slice corresponding to `span`.
    pub text: &'a str,
}

impl<'a> Token<'a> {
    #[inline]
    pub fn is_word(&self) -> bool {
        matches!(self.kind, TokenKind::Word(_))
    }

    #[inline]
    pub fn is_keyword(&self, kw: &str) -> bool {
        match self.kind {
            TokenKind::Word(w) => w.eq_ignore_ascii_case(kw),
            _ => false,
        }
    }

    #[inline]
    pub fn is_comment(&self) -> bool {
        matches!(self.kind, TokenKind::Comment(_))
    }

    #[inline]
    pub fn is_punct(&self, ch: char) -> bool {
        matches!(self.kind, TokenKind::Punctuation(c) if c == ch)
    }

    #[inline]
    pub fn is_placeholder(&self) -> bool {
        matches!(self.kind, TokenKind::Placeholder(_))
    }
}

/// Zero-copy SQL lexer yielding borrowed tokens without heap allocation.
pub struct SqlLexer<'a> {
    input: &'a str,
    bytes: &'a [u8],
    pos: usize,
    rules: SplitRules,
}

impl<'a> SqlLexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self::with_rules(input, SplitRules::default())
    }

    pub fn with_rules(input: &'a str, rules: SplitRules) -> Self {
        Self {
            input,
            bytes: input.as_bytes(),
            pos: 0,
            rules,
        }
    }

    /// Remaining input slice from the current lexer position
    #[inline]
    pub fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    /// Current byte position in the input string
    #[inline]
    pub fn position(&self) -> usize {
        self.pos
    }
}

/// Parses a PostgreSQL dollar quote tag from the start of a slice.
/// Returns `Some((tag, total_tag_len))` (e.g. `Some(("$$", 2))` or `Some(("$tag$", 5))`).
pub fn parse_dollar_tag(slice: &[u8]) -> Option<(&str, usize)> {
    if slice.is_empty() || slice[0] != b'$' {
        return None;
    }
    // Simple $$
    if slice.len() >= 2 && slice[1] == b'$' {
        return Some(("$$", 2));
    }
    // Tagged $tag$
    let mut i = 1;
    while i < slice.len() {
        let b = slice[i];
        if b == b'$' {
            let tag = std::str::from_utf8(&slice[0..=i]).ok()?;
            return Some((tag, i + 1));
        }
        if !(b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        i += 1;
    }
    None
}

impl<'a> Iterator for SqlLexer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        // Skip whitespace
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }

        if self.pos >= self.bytes.len() {
            return None;
        }

        let start = self.pos;
        let b = self.bytes[self.pos];

        // 1. Single-line comment: -- ...
        if self.pos + 1 < self.bytes.len() && b == b'-' && self.bytes[self.pos + 1] == b'-' {
            self.pos += 2;
            while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                self.pos += 1;
            }
            let text = &self.input[start..self.pos];
            return Some(Token {
                kind: TokenKind::Comment(text),
                span: start..self.pos,
                text,
            });
        }

        // 2. Single-line comment: # ... (only if rules.hash_comments is active)
        if self.rules.hash_comments && b == b'#' {
            self.pos += 1;
            while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                self.pos += 1;
            }
            let text = &self.input[start..self.pos];
            return Some(Token {
                kind: TokenKind::Comment(text),
                span: start..self.pos,
                text,
            });
        }

        // 3. Multi-line comment: /* ... */ (supports nested block comments)
        if self.pos + 1 < self.bytes.len() && b == b'/' && self.bytes[self.pos + 1] == b'*' {
            self.pos += 2;
            let mut depth = 1;
            while self.pos + 1 < self.bytes.len() && depth > 0 {
                if self.bytes[self.pos] == b'/' && self.bytes[self.pos + 1] == b'*' {
                    depth += 1;
                    self.pos += 2;
                } else if self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/' {
                    depth -= 1;
                    self.pos += 2;
                } else {
                    self.pos += 1;
                }
            }
            if depth > 0 && self.pos < self.bytes.len() {
                self.pos = self.bytes.len();
            }
            let text = &self.input[start..self.pos];
            return Some(Token {
                kind: TokenKind::Comment(text),
                span: start..self.pos,
                text,
            });
        }

        // 4. Single-quoted strings: '...' (with '' and optionally \' escapes)
        if b == b'\'' {
            self.pos += 1;
            while self.pos < self.bytes.len() {
                if self.rules.backslash_escapes && self.bytes[self.pos] == b'\\' {
                    self.pos += 1;
                    if self.pos < self.bytes.len() {
                        let ch_len = self.input[self.pos..]
                            .chars()
                            .next()
                            .map_or(1, |c| c.len_utf8());
                        self.pos += ch_len;
                    }
                    continue;
                }
                if self.bytes[self.pos] == b'\'' {
                    if self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1] == b'\'' {
                        self.pos += 2; // skip ''
                    } else {
                        self.pos += 1; // closing quote
                        break;
                    }
                } else {
                    let ch_len = self.input[self.pos..]
                        .chars()
                        .next()
                        .map_or(1, |c| c.len_utf8());
                    self.pos += ch_len;
                }
            }
            let text = &self.input[start..self.pos];
            return Some(Token {
                kind: TokenKind::StringLiteral(text),
                span: start..self.pos,
                text,
            });
        }

        // 5. Quoted identifiers: "..." or `...`
        if b == b'"' || b == b'`' {
            let q = b;
            self.pos += 1;
            while self.pos < self.bytes.len() {
                if self.bytes[self.pos] == q {
                    if self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1] == q {
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                        break;
                    }
                } else {
                    let ch_len = self.input[self.pos..]
                        .chars()
                        .next()
                        .map_or(1, |c| c.len_utf8());
                    self.pos += ch_len;
                }
            }
            let text = &self.input[start..self.pos];
            return Some(Token {
                kind: TokenKind::QuotedIdent(text),
                span: start..self.pos,
                text,
            });
        }

        // 6. Dollar-quoted string or positional parameter like $1
        if b == b'$' {
            // Positional parameter: $1, $2, etc.
            if self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1].is_ascii_digit() {
                self.pos += 1;
                while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
                let text = &self.input[start..self.pos];
                return Some(Token {
                    kind: TokenKind::Placeholder(text),
                    span: start..self.pos,
                    text,
                });
            }

            // PostgreSQL dollar quoting: $$...$$ or $tag$...$tag$
            if let Some((tag, tag_len)) = parse_dollar_tag(&self.bytes[self.pos..]) {
                self.pos += tag_len;
                while self.pos < self.bytes.len() {
                    if self.bytes[self.pos] == b'$'
                        && self.bytes[self.pos..].starts_with(tag.as_bytes())
                    {
                        self.pos += tag.len();
                        break;
                    }
                    self.pos += 1;
                }
                let text = &self.input[start..self.pos];
                return Some(Token {
                    kind: TokenKind::DollarQuoted { tag },
                    span: start..self.pos,
                    text,
                });
            }
        }

        // 7. Double colon `::` vs named parameter `:param`
        if b == b':' {
            if self.pos + 1 < self.bytes.len() && self.bytes[self.pos + 1] == b':' {
                self.pos += 2;
                return Some(Token {
                    kind: TokenKind::DoubleColon,
                    span: start..self.pos,
                    text: "::",
                });
            }
            if self.pos + 1 < self.bytes.len()
                && (self.bytes[self.pos + 1].is_ascii_alphabetic()
                    || self.bytes[self.pos + 1] == b'_')
            {
                self.pos += 1;
                while self.pos < self.bytes.len()
                    && (self.bytes[self.pos].is_ascii_alphanumeric()
                        || self.bytes[self.pos] == b'_')
                {
                    self.pos += 1;
                }
                let text = &self.input[start..self.pos];
                return Some(Token {
                    kind: TokenKind::Placeholder(text),
                    span: start..self.pos,
                    text,
                });
            }
        }

        // 8. Words (Identifiers / Keywords)
        if b.is_ascii_alphanumeric() || b == b'_' {
            self.pos += 1;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_alphanumeric() || self.bytes[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let text = &self.input[start..self.pos];
            return Some(Token {
                kind: TokenKind::Word(text),
                span: start..self.pos,
                text,
            });
        }

        // 9. Multi-character operators: `!=`, `<>`, `<=`, `>=`, `->`, `->>`, `#>`
        if self.pos + 1 < self.bytes.len() {
            let next = self.bytes[self.pos + 1];
            if (b == b'<' && (next == b'>' || next == b'='))
                || (b == b'>' && next == b'=')
                || (b == b'!' && next == b'=')
                || (b == b'-' && next == b'>')
            {
                if self.pos + 2 < self.bytes.len()
                    && b == b'-'
                    && next == b'>'
                    && self.bytes[self.pos + 2] == b'>'
                {
                    self.pos += 3;
                } else {
                    self.pos += 2;
                }
                let text = &self.input[start..self.pos];
                return Some(Token {
                    kind: TokenKind::Operator(text),
                    span: start..self.pos,
                    text,
                });
            }
            if b == b'#' && next == b'>' {
                if self.pos + 2 < self.bytes.len() && self.bytes[self.pos + 2] == b'>' {
                    self.pos += 3;
                } else {
                    self.pos += 2;
                }
                let text = &self.input[start..self.pos];
                return Some(Token {
                    kind: TokenKind::Operator(text),
                    span: start..self.pos,
                    text,
                });
            }
        }

        // 10. Single character punctuation or character
        let ch_len = self.input[self.pos..]
            .chars()
            .next()
            .map_or(1, |c| c.len_utf8());
        let ch = self.input[self.pos..].chars().next().unwrap_or(b as char);
        self.pos += ch_len;
        let text = &self.input[start..self.pos];
        Some(Token {
            kind: TokenKind::Punctuation(ch),
            span: start..self.pos,
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lexer_basic_tokens() {
        let sql = "SELECT id, name FROM users WHERE id = 123;";
        let tokens: Vec<Token> = SqlLexer::new(sql).collect();

        assert_eq!(tokens[0].kind, TokenKind::Word("SELECT"));
        assert_eq!(tokens[1].kind, TokenKind::Word("id"));
        assert_eq!(tokens[2].kind, TokenKind::Punctuation(','));
        assert_eq!(tokens[3].kind, TokenKind::Word("name"));
        assert_eq!(tokens[4].kind, TokenKind::Word("FROM"));
        assert_eq!(tokens[5].kind, TokenKind::Word("users"));
        assert_eq!(tokens[6].kind, TokenKind::Word("WHERE"));
        assert_eq!(tokens[7].kind, TokenKind::Word("id"));
        assert_eq!(tokens[8].kind, TokenKind::Punctuation('='));
        assert_eq!(tokens[9].kind, TokenKind::Word("123"));
        assert_eq!(tokens[10].kind, TokenKind::Punctuation(';'));
    }

    #[test]
    fn test_lexer_comments_and_strings() {
        let sql = "-- single line\n/* multi\nline */ SELECT 'string '' with quotes' FROM `tbl`;";
        let tokens: Vec<Token> = SqlLexer::new(sql).collect();

        assert_eq!(tokens[0].kind, TokenKind::Comment("-- single line"));
        assert_eq!(tokens[1].kind, TokenKind::Comment("/* multi\nline */"));
        assert_eq!(tokens[2].kind, TokenKind::Word("SELECT"));
        assert_eq!(
            tokens[3].kind,
            TokenKind::StringLiteral("'string '' with quotes'")
        );
        assert_eq!(tokens[4].kind, TokenKind::Word("FROM"));
        assert_eq!(tokens[5].kind, TokenKind::QuotedIdent("`tbl`"));
        assert_eq!(tokens[6].kind, TokenKind::Punctuation(';'));
    }

    #[test]
    fn test_lexer_placeholders_and_dollar_quotes() {
        let sql = "SELECT :user_id, $1, $$ body; with ; inside $$ FROM t WHERE val::int = 1;";
        let tokens: Vec<Token> = SqlLexer::new(sql).collect();

        assert_eq!(tokens[0].kind, TokenKind::Word("SELECT"));
        assert_eq!(tokens[1].kind, TokenKind::Placeholder(":user_id"));
        assert_eq!(tokens[2].kind, TokenKind::Punctuation(','));
        assert_eq!(tokens[3].kind, TokenKind::Placeholder("$1"));
        assert_eq!(tokens[4].kind, TokenKind::Punctuation(','));
        assert_eq!(tokens[5].kind, TokenKind::DollarQuoted { tag: "$$" });
        assert_eq!(tokens[6].kind, TokenKind::Word("FROM"));
        assert_eq!(tokens[7].kind, TokenKind::Word("t"));
        assert_eq!(tokens[8].kind, TokenKind::Word("WHERE"));
        assert_eq!(tokens[9].kind, TokenKind::Word("val"));
        assert_eq!(tokens[10].kind, TokenKind::DoubleColon);
        assert_eq!(tokens[11].kind, TokenKind::Word("int"));
    }
}
