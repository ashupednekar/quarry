// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

use crate::value::DbValue;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::ops::Index;

/// Flat column-oriented row buffer.
///
/// Stores query result cells in contiguous, column-oriented vectors rather than
/// an array of individually heap-allocated rows (`Vec<Vec<DbValue>>`).
/// For 100k-row result sets, this replaces ~100k separate heap allocations and
/// pointer indirections with a handful of contiguous column vectors (one per column),
/// dramatically improving CPU cache locality during column iteration, sorting,
/// and grid rendering.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowBuffer {
    row_count: usize,
    columns: Vec<Vec<DbValue>>,
    /// Permutation vector for sorting without reordering or cloning column vectors.
    /// `order[display_row]` maps to `physical_row`.
    order: Option<Vec<usize>>,
}

impl RowBuffer {
    /// Creates an empty row buffer with the given number of columns.
    pub fn new(num_columns: usize) -> Self {
        Self {
            row_count: 0,
            columns: (0..num_columns).map(|_| Vec::new()).collect(),
            order: None,
        }
    }

    /// Creates an empty row buffer with preallocated capacity for rows.
    pub fn with_capacity(num_columns: usize, row_capacity: usize) -> Self {
        Self {
            row_count: 0,
            columns: (0..num_columns)
                .map(|_| Vec::with_capacity(row_capacity))
                .collect(),
            order: None,
        }
    }

    /// Converts a row-major 2D vector into a column-oriented `RowBuffer`.
    pub fn from_rows(rows: Vec<Vec<DbValue>>) -> Self {
        if rows.is_empty() {
            return Self::default();
        }
        let num_cols = rows.first().map_or(0, |r| r.len());
        let mut buffer = Self::with_capacity(num_cols, rows.len());
        for row in rows {
            buffer.push_row(row);
        }
        buffer
    }

    /// Converts the buffer into a row-major vector of rows (respecting sort order).
    pub fn into_rows(self) -> Vec<Vec<DbValue>> {
        let mut out = Vec::with_capacity(self.row_count);
        for r in 0..self.row_count {
            out.push(self.row(r).to_vec());
        }
        out
    }

    /// Returns the number of rows currently in the buffer.
    pub fn len(&self) -> usize {
        self.row_count
    }

    /// Returns true if the buffer has zero rows.
    pub fn is_empty(&self) -> bool {
        self.row_count == 0
    }

    /// Returns the number of columns in the buffer.
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Appends a single row of values to the column vectors.
    pub fn push_row(&mut self, row: impl IntoIterator<Item = DbValue>) {
        let mut row_iter = row.into_iter();
        if self.columns.is_empty() {
            for val in row_iter {
                self.columns.push(vec![val]);
            }
        } else {
            let num_cols = self.columns.len();
            for col_idx in 0..num_cols {
                let val = row_iter.next().unwrap_or(DbValue::Null);
                self.columns[col_idx].push(val);
            }
        }
        self.row_count += 1;
        self.order = None;
    }

    /// Returns a mutable reference to a column's backing vector.
    pub fn column_mut(&mut self, col: usize) -> &mut Vec<DbValue> {
        if col >= self.columns.len() {
            self.columns.resize_with(col + 1, Vec::new);
        }
        &mut self.columns[col]
    }

    /// Increments the registered row count when columns were populated directly via `column_mut`.
    pub fn increment_row_count(&mut self) {
        self.row_count += 1;
        self.order = None;
    }

    /// Sets the row count explicitly.
    pub fn set_row_count(&mut self, count: usize) {
        self.row_count = count;
        self.order = None;
    }

    /// Retrieves a cell value reference at the specified (display_row, col).
    pub fn get_value(&self, row: usize, col: usize) -> Option<&DbValue> {
        if row >= self.row_count {
            return None;
        }
        let phys_row = self
            .order
            .as_ref()
            .map_or(row, |ord| ord.get(row).copied().unwrap_or(row));
        self.columns.get(col)?.get(phys_row)
    }

    /// Retrieves a cell value reference unchecked for maximum performance in tight loops.
    pub fn get_value_unchecked(&self, row: usize, col: usize) -> &DbValue {
        let phys_row = self
            .order
            .as_ref()
            .map_or(row, |ord| ord.get(row).copied().unwrap_or(row));
        &self.columns[col][phys_row]
    }

    /// Returns a `RowRef` view of the row at `row`. Panics if `row >= self.len()`.
    pub fn row(&self, row: usize) -> RowRef<'_> {
        assert!(
            row < self.row_count,
            "row index out of bounds: {} >= {}",
            row,
            self.row_count
        );
        RowRef { buffer: self, row }
    }

    /// Returns a `RowRef` view of the row at `row` if within bounds.
    pub fn get(&self, row: usize) -> Option<RowRef<'_>> {
        if row < self.row_count {
            Some(RowRef { buffer: self, row })
        } else {
            None
        }
    }

    /// Returns a `RowRef` view of the first row, or `None` if empty.
    pub fn first(&self) -> Option<RowRef<'_>> {
        self.get(0)
    }

    /// Returns a `RowRef` view of the last row, or `None` if empty.
    pub fn last(&self) -> Option<RowRef<'_>> {
        if self.row_count > 0 {
            self.get(self.row_count - 1)
        } else {
            None
        }
    }

    /// Returns an iterator over rows in display order.
    pub fn iter(&self) -> RowIter<'_> {
        RowIter {
            buffer: self,
            current: 0,
        }
    }

    /// Returns a contiguous slice of column `col` values in physical storage order.
    pub fn column(&self, col: usize) -> Option<&[DbValue]> {
        self.columns.get(col).map(|c| c.as_slice())
    }

    /// Sorts the display order of rows by the specified column using `cmp` without
    /// moving or cloning column data.
    pub fn sort_by<F>(&mut self, col: usize, cmp: F)
    where
        F: Fn(&DbValue, &DbValue) -> Ordering,
    {
        let mut indices: Vec<usize> = (0..self.row_count).collect();
        if let Some(col_vals) = self.columns.get(col) {
            indices.sort_by(|&a, &b| cmp(&col_vals[a], &col_vals[b]));
        }
        self.order = Some(indices);
    }

    /// Resets any active sort order, restoring original query return order in O(1).
    pub fn reset_order(&mut self) {
        self.order = None;
    }

    /// Returns true if a sort order permutation is currently applied.
    pub fn is_sorted(&self) -> bool {
        self.order.is_some()
    }

    /// Truncates the buffer to `len` rows.
    pub fn truncate(&mut self, len: usize) {
        if len < self.row_count {
            self.row_count = len;
            for c in &mut self.columns {
                c.truncate(len);
            }
            self.order = None;
        }
    }

    /// Appends all rows from another `RowBuffer` to this buffer.
    pub fn append(&mut self, other: &RowBuffer) {
        if self.columns.is_empty() {
            *self = other.clone();
            return;
        }
        for (i, col) in self.columns.iter_mut().enumerate() {
            if let Some(other_col) = other.columns.get(i) {
                col.extend(other_col.iter().cloned());
            } else {
                col.resize(col.len() + other.row_count, DbValue::Null);
            }
        }
        self.row_count += other.row_count;
        self.order = None;
    }

    /// Retains only the rows specified by the predicate.
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(RowRef<'_>) -> bool,
    {
        let mut keep = Vec::with_capacity(self.row_count);
        for r in 0..self.row_count {
            if f(self.row(r)) {
                let phys = self
                    .order
                    .as_ref()
                    .map_or(r, |ord| ord.get(r).copied().unwrap_or(r));
                keep.push(phys);
            }
        }
        if keep.len() == self.row_count && self.order.is_none() {
            return;
        }
        for col in &mut self.columns {
            let mut new_col = Vec::with_capacity(keep.len());
            for &k in &keep {
                new_col.push(std::mem::replace(&mut col[k], DbValue::Null));
            }
            *col = new_col;
        }
        self.row_count = keep.len();
        self.order = None;
    }
}

/// A lightweight, borrowed view of a single row inside a `RowBuffer`.
#[derive(Clone, Copy, Debug)]
pub struct RowRef<'a> {
    buffer: &'a RowBuffer,
    row: usize,
}

impl<'a> RowRef<'a> {
    /// Returns the cell value at column `col`.
    pub fn get(&self, col: usize) -> Option<&'a DbValue> {
        self.buffer.get_value(self.row, col)
    }

    /// Returns the number of columns in the row.
    pub fn len(&self) -> usize {
        self.buffer.column_count()
    }

    /// Returns true if the row has no columns.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the first cell value, or `None` if empty.
    pub fn first(&self) -> Option<&'a DbValue> {
        self.get(0)
    }

    /// Returns the last cell value, or `None` if empty.
    pub fn last(&self) -> Option<&'a DbValue> {
        if !self.is_empty() {
            self.get(self.len() - 1)
        } else {
            None
        }
    }

    /// Returns an iterator over cell values in this row.
    pub fn iter(&self) -> impl Iterator<Item = &'a DbValue> + '_ {
        (0..self.len()).filter_map(move |c| self.get(c))
    }

    /// Copies this row into a new heap-allocated `Vec<DbValue>`.
    pub fn to_vec(&self) -> Vec<DbValue> {
        self.iter().cloned().collect()
    }
}

impl<'a> Index<usize> for RowRef<'a> {
    type Output = DbValue;
    fn index(&self, col: usize) -> &Self::Output {
        self.get(col).expect("column index out of bounds")
    }
}

impl<'a> IntoIterator for RowRef<'a> {
    type Item = &'a DbValue;
    type IntoIter = RowRefIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        RowRefIter { row: self, col: 0 }
    }
}

pub struct RowRefIter<'a> {
    row: RowRef<'a>,
    col: usize,
}

impl<'a> Iterator for RowRefIter<'a> {
    type Item = &'a DbValue;
    fn next(&mut self) -> Option<Self::Item> {
        if self.col < self.row.len() {
            let val = self.row.get(self.col);
            self.col += 1;
            val
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.row.len().saturating_sub(self.col);
        (rem, Some(rem))
    }
}

impl<'a> ExactSizeIterator for RowRefIter<'a> {}

impl<'a> PartialEq<Vec<DbValue>> for RowRef<'a> {
    fn eq(&self, other: &Vec<DbValue>) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<'a> PartialEq<[DbValue]> for RowRef<'a> {
    fn eq(&self, other: &[DbValue]) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<'a, 'b> PartialEq<RowRef<'b>> for RowRef<'a> {
    fn eq(&self, other: &RowRef<'b>) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<'a> PartialEq<RowRef<'a>> for Vec<DbValue> {
    fn eq(&self, other: &RowRef<'a>) -> bool {
        other == self
    }
}

impl<'a> PartialEq<RowRef<'a>> for [DbValue] {
    fn eq(&self, other: &RowRef<'a>) -> bool {
        other == self
    }
}

impl PartialEq<Vec<Vec<DbValue>>> for RowBuffer {
    fn eq(&self, other: &Vec<Vec<DbValue>>) -> bool {
        if self.len() != other.len() {
            return false;
        }
        for (i, other_row) in other.iter().enumerate() {
            if self.row(i) != *other_row {
                return false;
            }
        }
        true
    }
}

impl PartialEq<RowBuffer> for Vec<Vec<DbValue>> {
    fn eq(&self, other: &RowBuffer) -> bool {
        other == self
    }
}

/// Iterator over rows in a `RowBuffer`.
pub struct RowIter<'a> {
    buffer: &'a RowBuffer,
    current: usize,
}

impl<'a> Iterator for RowIter<'a> {
    type Item = RowRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current < self.buffer.len() {
            let row = self.buffer.row(self.current);
            self.current += 1;
            Some(row)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.buffer.len().saturating_sub(self.current);
        (rem, Some(rem))
    }
}

impl<'a> ExactSizeIterator for RowIter<'a> {}

impl<'a> IntoIterator for &'a RowBuffer {
    type Item = RowRef<'a>;
    type IntoIter = RowIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl FromIterator<Vec<DbValue>> for RowBuffer {
    fn from_iter<T: IntoIterator<Item = Vec<DbValue>>>(iter: T) -> Self {
        let rows: Vec<Vec<DbValue>> = iter.into_iter().collect();
        RowBuffer::from_rows(rows)
    }
}

impl From<Vec<Vec<DbValue>>> for RowBuffer {
    fn from(rows: Vec<Vec<DbValue>>) -> Self {
        Self::from_rows(rows)
    }
}

impl Index<(usize, usize)> for RowBuffer {
    type Output = DbValue;
    fn index(&self, (row, col): (usize, usize)) -> &Self::Output {
        self.get_value(row, col).expect("cell index out of bounds")
    }
}

#[derive(Serialize, Deserialize)]
struct ColumnarWire {
    row_count: usize,
    columns: Vec<Vec<DbValue>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WireFormat {
    Columnar(ColumnarWire),
    Rows(Vec<Vec<DbValue>>),
}

impl Serialize for RowBuffer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = ColumnarWire {
            row_count: self.row_count,
            columns: self.columns.clone(),
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RowBuffer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match WireFormat::deserialize(deserializer)? {
            WireFormat::Columnar(c) => Ok(Self {
                row_count: c.row_count,
                columns: c.columns,
                order: None,
            }),
            WireFormat::Rows(rows) => Ok(Self::from_rows(rows)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_row_buffer_basic() {
        let mut buf = RowBuffer::new(3);
        buf.push_row(vec![
            DbValue::Int(1),
            DbValue::Text("alice".into()),
            DbValue::Boolean(true),
        ]);
        buf.push_row(vec![
            DbValue::Int(2),
            DbValue::Text("bob".into()),
            DbValue::Boolean(false),
        ]);

        assert_eq!(buf.len(), 2);
        assert_eq!(buf.column_count(), 3);
        assert_eq!(buf.get_value(0, 0), Some(&DbValue::Int(1)));
        assert_eq!(buf.get_value(0, 1), Some(&DbValue::Text("alice".into())));
        assert_eq!(buf.get_value(1, 0), Some(&DbValue::Int(2)));
        assert_eq!(buf.get_value(1, 1), Some(&DbValue::Text("bob".into())));

        let r0 = buf.row(0);
        assert_eq!(r0[0], DbValue::Int(1));
        assert_eq!(r0[1], DbValue::Text("alice".into()));
        assert_eq!(r0.len(), 3);

        let r1 = buf.row(1);
        assert_eq!(r1[0], DbValue::Int(2));
        assert_eq!(r1[1], DbValue::Text("bob".into()));
    }

    #[test]
    fn test_row_buffer_sorting() {
        let mut buf = RowBuffer::new(2);
        buf.push_row(vec![DbValue::Int(30), DbValue::Text("c".into())]);
        buf.push_row(vec![DbValue::Int(10), DbValue::Text("a".into())]);
        buf.push_row(vec![DbValue::Int(20), DbValue::Text("b".into())]);

        buf.sort_by(0, |a, b| a.cmp_val(b));
        assert_eq!(buf.get_value(0, 0), Some(&DbValue::Int(10)));
        assert_eq!(buf.get_value(1, 0), Some(&DbValue::Int(20)));
        assert_eq!(buf.get_value(2, 0), Some(&DbValue::Int(30)));

        buf.reset_order();
        assert_eq!(buf.get_value(0, 0), Some(&DbValue::Int(30)));
        assert_eq!(buf.get_value(1, 0), Some(&DbValue::Int(10)));
        assert_eq!(buf.get_value(2, 0), Some(&DbValue::Int(20)));
    }

    #[test]
    fn test_row_buffer_serde_roundtrip() {
        let mut buf = RowBuffer::new(2);
        buf.push_row(vec![DbValue::Int(42), DbValue::Text("hello".into())]);

        let json = serde_json::to_string(&buf).unwrap();
        let decoded: RowBuffer = serde_json::from_str(&json).unwrap();
        assert_eq!(buf, decoded);

        // Test backwards compatibility with raw rows JSON
        let raw_json = r#"[[{"Int": 42}, {"Text": "hello"}]]"#;
        let from_raw: RowBuffer = serde_json::from_str(raw_json).unwrap();
        assert_eq!(from_raw.len(), 1);
        assert_eq!(from_raw.get_value(0, 0), Some(&DbValue::Int(42)));
    }
}
