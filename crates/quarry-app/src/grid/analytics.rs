// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use quarry_core::value::DbValue;
use std::collections::HashSet;

/// A 2D selection range in table coordinates (0-indexed).
/// Anchor is where selection started (mouse down or initial cell).
/// Focus is where selection ended (mouse drag or Shift-click/arrow target).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRange {
    pub anchor_row: usize,
    pub anchor_col: usize,
    pub focus_row: usize,
    pub focus_col: usize,
}

impl CellRange {
    #[inline]
    pub fn single(row: usize, col: usize) -> Self {
        Self {
            anchor_row: row,
            anchor_col: col,
            focus_row: row,
            focus_col: col,
        }
    }

    #[inline]
    pub fn new(anchor_row: usize, anchor_col: usize, focus_row: usize, focus_col: usize) -> Self {
        Self {
            anchor_row,
            anchor_col,
            focus_row,
            focus_col,
        }
    }

    #[inline]
    pub fn min_row(&self) -> usize {
        self.anchor_row.min(self.focus_row)
    }

    #[inline]
    pub fn max_row(&self) -> usize {
        self.anchor_row.max(self.focus_row)
    }

    #[inline]
    pub fn min_col(&self) -> usize {
        self.anchor_col.min(self.focus_col)
    }

    #[inline]
    pub fn max_col(&self) -> usize {
        self.anchor_col.max(self.focus_col)
    }

    #[inline]
    pub fn contains(&self, row: usize, col: usize) -> bool {
        row >= self.min_row()
            && row <= self.max_row()
            && col >= self.min_col()
            && col <= self.max_col()
    }

    #[inline]
    pub fn is_single_cell(&self) -> bool {
        self.anchor_row == self.focus_row && self.anchor_col == self.focus_col
    }

    #[inline]
    pub fn row_count(&self) -> usize {
        self.max_row() - self.min_row() + 1
    }

    #[inline]
    pub fn col_count(&self) -> usize {
        self.max_col() - self.min_col() + 1
    }

    #[inline]
    pub fn cell_count(&self) -> usize {
        self.row_count() * self.col_count()
    }
}

/// An individual interactive aggregation chip for the bottom bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatChip {
    pub label: &'static str,
    pub display_value: String,
    pub raw_value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnStats {
    pub column_name: String,
    pub type_name: String,
    pub total_rows: usize,
    pub non_null_count: usize,
    pub null_count: usize,
    pub distinct_count: usize,
    pub is_numeric: bool,
    pub sum: Option<f64>,
    pub avg: Option<f64>,
    pub min_str: Option<String>,
    pub max_str: Option<String>,
}

impl ColumnStats {
    /// Computes summary statistics over an iterator of cell values for a column.
    pub fn compute<'a>(
        column_name: String,
        type_name: String,
        values: impl Iterator<Item = Option<&'a DbValue>>,
    ) -> Self {
        let mut total_rows = 0;
        let mut non_null_count = 0;
        let mut null_count = 0;
        let mut seen_values = HashSet::new();
        let mut is_numeric = true;
        let mut sum_val = 0.0;
        let mut num_numeric = 0;
        let mut min_num = f64::MAX;
        let mut max_num = f64::MIN;
        let mut min_str: Option<String> = None;
        let mut max_str: Option<String> = None;

        for val_opt in values {
            total_rows += 1;
            match val_opt {
                None | Some(DbValue::Null) => {
                    null_count += 1;
                }
                Some(v) => {
                    non_null_count += 1;
                    let disp = v.to_display_string();
                    if seen_values.len() < 2000 {
                        seen_values.insert(disp.clone());
                    }

                    // Check numeric
                    match v {
                        DbValue::Int(i) => {
                            let f = *i as f64;
                            sum_val += f;
                            num_numeric += 1;
                            if f < min_num {
                                min_num = f;
                            }
                            if f > max_num {
                                max_num = f;
                            }
                        }
                        DbValue::Float(f) => {
                            if !f.is_nan() {
                                sum_val += *f;
                                num_numeric += 1;
                                if *f < min_num {
                                    min_num = *f;
                                }
                                if *f > max_num {
                                    max_num = *f;
                                }
                            }
                        }
                        DbValue::Decimal(s) => {
                            if let Ok(f) = s.parse::<f64>() {
                                if !f.is_nan() {
                                    sum_val += f;
                                    num_numeric += 1;
                                    if f < min_num {
                                        min_num = f;
                                    }
                                    if f > max_num {
                                        max_num = f;
                                    }
                                }
                            } else {
                                is_numeric = false;
                            }
                        }
                        _ => {
                            is_numeric = false;
                        }
                    }

                    // String min/max comparison
                    if min_str.as_ref().map_or(true, |m| &disp < m) {
                        min_str = Some(disp.clone());
                    }
                    if max_str.as_ref().map_or(true, |m| &disp > m) {
                        max_str = Some(disp);
                    }
                }
            }
        }

        let is_numeric = is_numeric && num_numeric > 0 && num_numeric == non_null_count;
        let (sum, avg, min_str, max_str) = if is_numeric && non_null_count > 0 {
            let avg = sum_val / (non_null_count as f64);
            let min_s = format_grouped_num(min_num);
            let max_s = format_grouped_num(max_num);
            (Some(sum_val), Some(avg), Some(min_s), Some(max_s))
        } else {
            (None, None, min_str, max_str)
        };

        Self {
            column_name,
            type_name,
            total_rows,
            non_null_count,
            null_count,
            distinct_count: seen_values.len(),
            is_numeric,
            sum,
            avg,
            min_str,
            max_str,
        }
    }

    /// Returns structured stat chips for interactive bottom bar rendering.
    pub fn chips(&self) -> Vec<StatChip> {
        let mut chips = Vec::new();
        chips.push(StatChip {
            label: "Count",
            display_value: group_integer(self.non_null_count as i64),
            raw_value: self.non_null_count.to_string(),
        });
        if self.null_count > 0 {
            chips.push(StatChip {
                label: "Nulls",
                display_value: group_integer(self.null_count as i64),
                raw_value: self.null_count.to_string(),
            });
        }
        if self.is_numeric {
            if let Some(sum) = self.sum {
                chips.push(StatChip {
                    label: "Sum",
                    display_value: format_grouped_num(sum),
                    raw_value: format_raw_num(sum),
                });
            }
            if let Some(avg) = self.avg {
                chips.push(StatChip {
                    label: "Avg",
                    display_value: format_grouped_num(avg),
                    raw_value: format_raw_num(avg),
                });
            }
            if let Some(min) = self.min_str.as_ref() {
                chips.push(StatChip {
                    label: "Min",
                    display_value: min.clone(),
                    raw_value: min.clone(),
                });
            }
            if let Some(max) = self.max_str.as_ref() {
                chips.push(StatChip {
                    label: "Max",
                    display_value: max.clone(),
                    raw_value: max.clone(),
                });
            }
        } else {
            if self.distinct_count > 0 {
                chips.push(StatChip {
                    label: "Distinct",
                    display_value: group_integer(self.distinct_count as i64),
                    raw_value: self.distinct_count.to_string(),
                });
            }
            if let Some(min) = self.min_str.as_ref() {
                chips.push(StatChip {
                    label: "Min",
                    display_value: truncate(min, 16),
                    raw_value: min.clone(),
                });
            }
            if let Some(max) = self.max_str.as_ref() {
                if Some(max) != self.min_str.as_ref() {
                    chips.push(StatChip {
                        label: "Max",
                        display_value: truncate(max, 16),
                        raw_value: max.clone(),
                    });
                }
            }
        }
        chips
    }

    /// Formats analytics summary string for quick readout.
    #[allow(dead_code)]
    pub fn format_summary(&self) -> String {
        let count_str = if self.null_count > 0 {
            format!("{} non-null, {} null", self.non_null_count, self.null_count)
        } else {
            format!("{} rows", self.non_null_count)
        };

        if self.is_numeric {
            if let (Some(sum), Some(avg), Some(min), Some(max)) = (
                self.sum,
                self.avg,
                self.min_str.as_deref(),
                self.max_str.as_deref(),
            ) {
                format!(
                    "{} · Sum: {} · Avg: {} · Min: {} · Max: {}",
                    count_str,
                    format_grouped_num(sum),
                    format_grouped_num(avg),
                    min,
                    max
                )
            } else {
                count_str
            }
        } else {
            let mut parts = vec![count_str];
            if self.distinct_count > 0 {
                parts.push(format!("{} distinct", self.distinct_count));
            }
            if let (Some(min), Some(max)) = (self.min_str.as_deref(), self.max_str.as_deref()) {
                if min != max {
                    parts.push(format!("Min: {} · Max: {}", truncate(min, 24), truncate(max, 24)));
                } else {
                    parts.push(format!("Val: {}", truncate(min, 24)));
                }
            }
            parts.join(" · ")
        }
    }
}

/// Aggregated statistics over a highlighted selection of cells.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionStats {
    pub total_cells: usize,
    pub non_null_count: usize,
    pub null_count: usize,
    pub distinct_count: usize,
    pub is_numeric: bool,
    pub sum: Option<f64>,
    pub avg: Option<f64>,
    pub min_str: Option<String>,
    pub max_str: Option<String>,
}

impl SelectionStats {
    /// Computes summary statistics over an iterator of cell values in a selection range.
    pub fn compute<'a>(values: impl Iterator<Item = Option<&'a DbValue>>) -> Self {
        let mut total_cells = 0;
        let mut non_null_count = 0;
        let mut null_count = 0;
        let mut seen_values = HashSet::new();
        let mut is_numeric = true;
        let mut sum_val = 0.0;
        let mut num_numeric = 0;
        let mut min_num = f64::MAX;
        let mut max_num = f64::MIN;
        let mut min_str: Option<String> = None;
        let mut max_str: Option<String> = None;

        for val_opt in values {
            total_cells += 1;
            match val_opt {
                None | Some(DbValue::Null) => {
                    null_count += 1;
                }
                Some(v) => {
                    non_null_count += 1;
                    let disp = v.to_display_string();
                    if seen_values.len() < 2000 {
                        seen_values.insert(disp.clone());
                    }

                    match v {
                        DbValue::Int(i) => {
                            let f = *i as f64;
                            sum_val += f;
                            num_numeric += 1;
                            if f < min_num {
                                min_num = f;
                            }
                            if f > max_num {
                                max_num = f;
                            }
                        }
                        DbValue::Float(f) => {
                            if !f.is_nan() {
                                sum_val += *f;
                                num_numeric += 1;
                                if *f < min_num {
                                    min_num = *f;
                                }
                                if *f > max_num {
                                    max_num = *f;
                                }
                            }
                        }
                        DbValue::Decimal(s) => {
                            if let Ok(f) = s.parse::<f64>() {
                                if !f.is_nan() {
                                    sum_val += f;
                                    num_numeric += 1;
                                    if f < min_num {
                                        min_num = f;
                                    }
                                    if f > max_num {
                                        max_num = f;
                                    }
                                }
                            } else {
                                is_numeric = false;
                            }
                        }
                        _ => {
                            is_numeric = false;
                        }
                    }

                    if min_str.as_ref().map_or(true, |m| &disp < m) {
                        min_str = Some(disp.clone());
                    }
                    if max_str.as_ref().map_or(true, |m| &disp > m) {
                        max_str = Some(disp);
                    }
                }
            }
        }

        let is_numeric = is_numeric && num_numeric > 0 && num_numeric == non_null_count;
        let (sum, avg, min_str, max_str) = if is_numeric && non_null_count > 0 {
            let avg = sum_val / (non_null_count as f64);
            let min_s = format_grouped_num(min_num);
            let max_s = format_grouped_num(max_num);
            (Some(sum_val), Some(avg), Some(min_s), Some(max_s))
        } else {
            (None, None, min_str, max_str)
        };

        Self {
            total_cells,
            non_null_count,
            null_count,
            distinct_count: seen_values.len(),
            is_numeric,
            sum,
            avg,
            min_str,
            max_str,
        }
    }

    /// Returns structured stat chips for interactive bottom bar rendering.
    pub fn chips(&self) -> Vec<StatChip> {
        let mut chips = Vec::new();
        chips.push(StatChip {
            label: "Count",
            display_value: group_integer(self.non_null_count as i64),
            raw_value: self.non_null_count.to_string(),
        });
        if self.null_count > 0 {
            chips.push(StatChip {
                label: "Nulls",
                display_value: group_integer(self.null_count as i64),
                raw_value: self.null_count.to_string(),
            });
        }
        if self.is_numeric {
            if let Some(sum) = self.sum {
                chips.push(StatChip {
                    label: "Sum",
                    display_value: format_grouped_num(sum),
                    raw_value: format_raw_num(sum),
                });
            }
            if let Some(avg) = self.avg {
                chips.push(StatChip {
                    label: "Avg",
                    display_value: format_grouped_num(avg),
                    raw_value: format_raw_num(avg),
                });
            }
            if let Some(min) = self.min_str.as_ref() {
                chips.push(StatChip {
                    label: "Min",
                    display_value: min.clone(),
                    raw_value: min.clone(),
                });
            }
            if let Some(max) = self.max_str.as_ref() {
                chips.push(StatChip {
                    label: "Max",
                    display_value: max.clone(),
                    raw_value: max.clone(),
                });
            }
        } else {
            if self.distinct_count > 0 {
                chips.push(StatChip {
                    label: "Distinct",
                    display_value: group_integer(self.distinct_count as i64),
                    raw_value: self.distinct_count.to_string(),
                });
            }
            if let Some(min) = self.min_str.as_ref() {
                chips.push(StatChip {
                    label: "Min",
                    display_value: truncate(min, 16),
                    raw_value: min.clone(),
                });
            }
            if let Some(max) = self.max_str.as_ref() {
                if Some(max) != self.min_str.as_ref() {
                    chips.push(StatChip {
                        label: "Max",
                        display_value: truncate(max, 16),
                        raw_value: max.clone(),
                    });
                }
            }
        }
        chips
    }
}

/// Formats an integer with standard thousands separators (e.g. 1,234,567).
pub fn group_integer(n: i64) -> String {
    let sign = if n < 0 { "-" } else { "" };
    let s = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{}{}", sign, out)
}

/// Formats a floating-point number with thousands separators and 2 decimal places if fractional.
pub fn format_grouped_num(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "∞".into() } else { "-∞".into() };
    }
    if n.fract() == 0.0 && n.abs() <= 1e14 {
        group_integer(n as i64)
    } else {
        let abs_n = n.abs();
        let sign = if n < 0.0 { "-" } else { "" };
        let int_part = abs_n.trunc() as i64;
        let fract_part = (abs_n.fract() * 100.0).round() as u64;
        if fract_part == 0 {
            format!("{}{}", sign, group_integer(int_part))
        } else if fract_part >= 100 {
            format!("{}{}", sign, group_integer(int_part + 1))
        } else {
            format!("{}{}.{:02}", sign, group_integer(int_part), fract_part)
        }
    }
}

/// Formats a raw unformatted number suitable for copying into calculators or SQL queries.
pub fn format_raw_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() <= 1e15 {
        format!("{:.0}", n)
    } else {
        let s = format!("{:.4}", n);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() > max_len {
        format!("{}…", s.chars().take(max_len).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_range() {
        let r = CellRange::new(2, 5, 8, 3);
        assert_eq!(r.min_row(), 2);
        assert_eq!(r.max_row(), 8);
        assert_eq!(r.min_col(), 3);
        assert_eq!(r.max_col(), 5);
        assert_eq!(r.row_count(), 7);
        assert_eq!(r.col_count(), 3);
        assert_eq!(r.cell_count(), 21);
        assert!(!r.is_single_cell());
        assert!(r.contains(2, 3));
        assert!(r.contains(8, 5));
        assert!(!r.contains(1, 3));
        assert!(!r.contains(2, 6));

        let single = CellRange::single(4, 4);
        assert!(single.is_single_cell());
        assert_eq!(single.cell_count(), 1);
        assert!(single.contains(4, 4));
        assert!(!single.contains(4, 5));
    }

    #[test]
    fn test_format_numbers() {
        assert_eq!(group_integer(0), "0");
        assert_eq!(group_integer(1234), "1,234");
        assert_eq!(group_integer(1234567), "1,234,567");
        assert_eq!(group_integer(-987654), "-987,654");

        assert_eq!(format_grouped_num(1000.0), "1,000");
        assert_eq!(format_grouped_num(1234.5), "1,234.50");
        assert_eq!(format_grouped_num(-4567.89), "-4,567.89");

        assert_eq!(format_raw_num(1000.0), "1000");
        assert_eq!(format_raw_num(1234.5), "1234.5");
    }

    #[test]
    fn test_numeric_column_stats() {
        let vals = vec![
            Some(DbValue::Int(10)),
            Some(DbValue::Int(20)),
            Some(DbValue::Int(30)),
            Some(DbValue::Null),
        ];
        let stats = ColumnStats::compute("amount".into(), "INTEGER".into(), vals.iter().map(|v| v.as_ref()));
        assert_eq!(stats.total_rows, 4);
        assert_eq!(stats.non_null_count, 3);
        assert_eq!(stats.null_count, 1);
        assert!(stats.is_numeric);
        assert_eq!(stats.sum, Some(60.0));
        assert_eq!(stats.avg, Some(20.0));
        assert_eq!(stats.min_str.as_deref(), Some("10"));
        assert_eq!(stats.max_str.as_deref(), Some("30"));

        let chips = stats.chips();
        assert_eq!(chips.len(), 6);
        assert_eq!(chips[0].label, "Count");
        assert_eq!(chips[0].display_value, "3");
        assert_eq!(chips[1].label, "Nulls");
        assert_eq!(chips[1].display_value, "1");
        assert_eq!(chips[2].label, "Sum");
        assert_eq!(chips[2].display_value, "60");
        assert_eq!(chips[3].label, "Avg");
        assert_eq!(chips[3].display_value, "20");
    }

    #[test]
    fn test_selection_stats() {
        let vals = vec![
            Some(DbValue::Int(100)),
            Some(DbValue::Int(200)),
            Some(DbValue::Int(300)),
        ];
        let stats = SelectionStats::compute(vals.iter().map(|v| v.as_ref()));
        assert_eq!(stats.total_cells, 3);
        assert_eq!(stats.non_null_count, 3);
        assert_eq!(stats.sum, Some(600.0));
        assert_eq!(stats.avg, Some(200.0));
        let chips = stats.chips();
        assert_eq!(chips.len(), 5); // COUNT, SUM, AVG, MIN, MAX
    }

    #[test]
    fn test_text_column_stats() {
        let vals = vec![
            Some(DbValue::Text("apple".into())),
            Some(DbValue::Text("banana".into())),
            Some(DbValue::Text("apple".into())),
            Some(DbValue::Null),
        ];
        let stats = ColumnStats::compute("fruit".into(), "VARCHAR".into(), vals.iter().map(|v| v.as_ref()));
        assert_eq!(stats.total_rows, 4);
        assert_eq!(stats.non_null_count, 3);
        assert_eq!(stats.distinct_count, 2);
        assert!(!stats.is_numeric);
        assert_eq!(stats.min_str.as_deref(), Some("apple"));
        assert_eq!(stats.max_str.as_deref(), Some("banana"));

        let chips = stats.chips();
        assert_eq!(chips[0].label, "Count");
        assert_eq!(chips[1].label, "Nulls");
        assert_eq!(chips[2].label, "Distinct");
        assert_eq!(chips[2].display_value, "2");
    }
}
