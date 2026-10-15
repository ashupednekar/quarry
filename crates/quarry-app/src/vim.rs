// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Vim motions over the editor's text: plain functions of (text, byte offset), so they're testable
//! apart from the editor. The key handling that uses them lives in `QuarryWindow::handle_vim_key`.

use std::ops::Range;

fn line_start(t: &str, p: usize) -> usize {
    t[..p].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(t: &str, p: usize) -> usize {
    t[p..].find('\n').map_or(t.len(), |i| p + i)
}

/// The offset just past the character at `p`
pub fn char_after(t: &str, p: usize) -> usize {
    next_char(t, p)
}

fn next_char(t: &str, p: usize) -> usize {
    t[p..].chars().next().map_or(p, |c| p + c.len_utf8())
}

fn prev_char(t: &str, p: usize) -> usize {
    t[..p].chars().next_back().map_or(p, |c| p - c.len_utf8())
}

/// The byte offset `col` characters into the line starting at `start`, clamped to its end
fn at_col(t: &str, start: usize, col: usize) -> usize {
    let end = line_end(t, start);
    t[start..end]
        .char_indices()
        .nth(col)
        .map_or(end, |(i, _)| start + i)
}

#[derive(PartialEq)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

fn class_at(t: &str, p: usize) -> Option<Class> {
    t[p..].chars().next().map(class)
}

/// `w`: the start of the next word (a run of word chars, or of punctuation)
fn word_next(t: &str, mut p: usize) -> usize {
    if let Some(c) = class_at(t, p).filter(|c| *c != Class::Space) {
        while class_at(t, p).as_ref() == Some(&c) {
            p = next_char(t, p);
        }
    }
    while class_at(t, p) == Some(Class::Space) {
        p = next_char(t, p);
    }
    p
}

/// `b`: the start of this word, or of the previous one when already at a start
fn word_prev(t: &str, mut p: usize) -> usize {
    while p > 0 && class(t[..p].chars().next_back().unwrap()) == Class::Space {
        p = prev_char(t, p);
    }
    let Some(c) = t[..p].chars().next_back().map(class) else {
        return 0;
    };
    while p > 0 && class(t[..p].chars().next_back().unwrap()) == c {
        p = prev_char(t, p);
    }
    p
}

/// `e`: the last character of this word, or of the next one when already at an end
fn word_end(t: &str, p: usize) -> usize {
    let mut p = next_char(t, p);
    while class_at(t, p) == Some(Class::Space) {
        p = next_char(t, p);
    }
    let Some(c) = class_at(t, p) else {
        return prev_char(t, t.len()).min(p);
    };
    while class_at(t, next_char(t, p)).as_ref() == Some(&c) {
        p = next_char(t, p);
    }
    p
}

/// Where a motion key moves the cursor; `g` is `gg` (the second g of a pending pair)
pub fn motion(t: &str, p: usize, key: &str) -> Option<usize> {
    let p = p.min(t.len());
    let start = line_start(t, p);
    let col = t[start..p].chars().count();
    Some(match key {
        "h" | "left" => prev_char(t, p).max(start),
        "l" | "right" => next_char(t, p).min(line_end(t, p)),
        "j" | "down" => {
            let end = line_end(t, p);
            if end == t.len() {
                return Some(p);
            }
            at_col(t, end + 1, col)
        }
        "k" | "up" => {
            if start == 0 {
                return Some(p);
            }
            at_col(t, line_start(t, start - 1), col)
        }
        "w" => word_next(t, p),
        "b" => word_prev(t, p),
        "e" => word_end(t, p),
        "0" => start,
        "^" => {
            let end = line_end(t, p);
            start + t[start..end].len() - t[start..end].trim_start().len()
        }
        "$" => line_end(t, p),
        "G" => t.len(),
        "g" => 0,
        _ => return None,
    })
}

/// The whole line at `p` with its newline, for `dd` / `yy` / `cc`
pub fn line_range(t: &str, p: usize) -> Range<usize> {
    let (start, end) = (line_start(t, p), line_end(t, p));
    if end < t.len() {
        start..end + 1
    } else {
        // The last line takes the newline before it, so no empty line is left behind
        start.saturating_sub(usize::from(start > 0))..end
    }
}

/// The range an operator covers from `p` to `to`: forward motions stop before `to`, as in Vim
pub fn op_range(p: usize, to: usize) -> Range<usize> {
    p.min(to)..p.max(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motions() {
        let t = "select a, b\n  from users;";
        assert_eq!(motion(t, 0, "w"), Some(7)); // select → a
        assert_eq!(motion(t, 7, "w"), Some(8)); // a → ,
        assert_eq!(motion(t, 8, "w"), Some(10)); // , → b
        assert_eq!(motion(t, 10, "b"), Some(8));
        assert_eq!(motion(t, 0, "e"), Some(5)); // end of select
        assert_eq!(motion(t, 3, "$"), Some(11));
        assert_eq!(motion(t, 14, "0"), Some(12));
        assert_eq!(motion(t, 13, "^"), Some(14));
        assert_eq!(motion(t, 3, "j"), Some(15)); // keeps the column
        assert_eq!(motion(t, 15, "k"), Some(3));
        assert_eq!(motion(t, 0, "h"), Some(0)); // never leaves the line
        assert_eq!(motion(t, 11, "l"), Some(11));
        assert_eq!(motion(t, 5, "G"), Some(t.len()));
        assert_eq!(motion(t, 20, "g"), Some(0));
        assert_eq!(motion(t, 0, "q"), None);
    }

    #[test]
    fn lines() {
        let t = "a\nbb\ncc";
        assert_eq!(&t[line_range(t, 3)], "bb\n");
        assert_eq!(&t[line_range(t, 6)], "\ncc"); // last line eats the newline before it
        let one = "one";
        assert_eq!(&one[line_range(one, 1)], "one");
    }
}
