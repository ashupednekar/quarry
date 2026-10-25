// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Visual filter condition builder: create, edit and combine column filters
//! with dropdown operators without manually editing the WHERE clause.

use crate::state::Tab;
use crate::theme::Theme;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::assets::IconName;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::select::{Select, SelectEvent, SelectState};
use gpui::component::{ActiveTheme, IndexPath, Sizable};
use gpui::*;
use quarry_core::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOperator {
    Equals,
    NotEquals,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    Like,
    ILike,
    NotLike,
    Contains,
    StartsWith,
    EndsWith,
    IsNull,
    IsNotNull,
    In,
}

impl FilterOperator {
    pub const ALL: &'static [FilterOperator] = &[
        FilterOperator::Equals,
        FilterOperator::NotEquals,
        FilterOperator::GreaterThan,
        FilterOperator::GreaterThanOrEqual,
        FilterOperator::LessThan,
        FilterOperator::LessThanOrEqual,
        FilterOperator::Like,
        FilterOperator::ILike,
        FilterOperator::NotLike,
        FilterOperator::Contains,
        FilterOperator::StartsWith,
        FilterOperator::EndsWith,
        FilterOperator::IsNull,
        FilterOperator::IsNotNull,
        FilterOperator::In,
    ];

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::Equals => "equals",
            Self::NotEquals => "does not equal",
            Self::GreaterThan => "greater than",
            Self::GreaterThanOrEqual => "at least",
            Self::LessThan => "less than",
            Self::LessThanOrEqual => "at most",
            Self::Like => "matches pattern",
            Self::ILike => "matches, any case",
            Self::NotLike => "does not match",
            Self::Contains => "contains",
            Self::StartsWith => "starts with",
            Self::EndsWith => "ends with",
            Self::IsNull => "is null",
            Self::IsNotNull => "is not null",
            Self::In => "is one of",
        }
    }

    pub fn requires_value(&self) -> bool {
        !matches!(self, Self::IsNull | Self::IsNotNull)
    }

    pub fn from_label(label: &str) -> Self {
        for op in Self::ALL {
            if op.display_label() == label {
                return *op;
            }
        }
        Self::Equals
    }

    /// Formats a single condition into SQL using the given column SQL identifier.
    pub fn to_sql(&self, col_sql: &str, raw_val: &str) -> String {
        let val = raw_val.trim();
        match self {
            Self::IsNull => format!("{col_sql} IS NULL"),
            Self::IsNotNull => format!("{col_sql} IS NOT NULL"),
            Self::Equals => {
                if val.eq_ignore_ascii_case("null") {
                    format!("{col_sql} IS NULL")
                } else {
                    format!("{col_sql} = {}", format_literal(val))
                }
            }
            Self::NotEquals => {
                if val.eq_ignore_ascii_case("null") {
                    format!("{col_sql} IS NOT NULL")
                } else {
                    format!("{col_sql} != {}", format_literal(val))
                }
            }
            Self::GreaterThan => format!("{col_sql} > {}", format_literal(val)),
            Self::GreaterThanOrEqual => format!("{col_sql} >= {}", format_literal(val)),
            Self::LessThan => format!("{col_sql} < {}", format_literal(val)),
            Self::LessThanOrEqual => format!("{col_sql} <= {}", format_literal(val)),
            Self::Like => format!("{col_sql} LIKE '{}'", escape_sql_quotes(val)),
            Self::ILike => format!("{col_sql} ILIKE '{}'", escape_sql_quotes(val)),
            Self::NotLike => format!("{col_sql} NOT LIKE '{}'", escape_sql_quotes(val)),
            Self::Contains => format!("{col_sql} LIKE '%{}%'", escape_sql_quotes(val)),
            Self::StartsWith => format!("{col_sql} LIKE '{}%'", escape_sql_quotes(val)),
            Self::EndsWith => format!("{col_sql} LIKE '%{}'", escape_sql_quotes(val)),
            Self::In => {
                let parts: Vec<String> = val
                    .split(',')
                    .map(|p| format_literal(p.trim()))
                    .filter(|p| !p.is_empty())
                    .collect();
                if parts.is_empty() {
                    format!("{col_sql} IN ()")
                } else {
                    format!("{col_sql} IN ({})", parts.join(", "))
                }
            }
        }
    }
}

pub fn escape_sql_quotes(val: &str) -> String {
    val.replace('\'', "''")
}

pub fn format_literal(val: &str) -> String {
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return "''".to_string();
    }
    // Already single-quoted?
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        return trimmed.to_string();
    }
    // Number? (int or float)
    if trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok() {
        return trimmed.to_string();
    }
    // Boolean?
    if trimmed.eq_ignore_ascii_case("true") || trimmed.eq_ignore_ascii_case("false") {
        return trimmed.to_uppercase();
    }
    // String literal
    format!("'{}'", escape_sql_quotes(trimmed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterCombineMode {
    And,
    Or,
}

impl FilterCombineMode {
    pub fn separator(&self) -> &'static str {
        match self {
            Self::And => " AND ",
            Self::Or => " OR ",
        }
    }
}

pub struct FilterRuleState {
    pub column_select: Entity<SelectState<Vec<String>>>,
    pub operator_select: Entity<SelectState<Vec<String>>>,
    pub value_input: Entity<InputState>,
}

pub struct FilterBuilderDialog {
    pub table_name: String,
    pub columns: Vec<String>,
    pub combine_mode: FilterCombineMode,
    pub rules: Vec<FilterRuleState>,
    pub subs: Vec<Subscription>,
}

impl FilterBuilderDialog {
    pub fn compile_to_sql(&self, dialect: &dyn Dialect, cx: &App) -> String {
        let mut clauses = Vec::new();
        for rule in &self.rules {
            let col_name = rule
                .column_select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default();
            let op_label = rule
                .operator_select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_default();
            let val = rule.value_input.read(cx).value();
            let op = FilterOperator::from_label(&op_label);

            if col_name.is_empty() {
                continue;
            }
            if op.requires_value() && val.trim().is_empty() {
                continue;
            }

            let col_sql = dialect.quote_ident(&col_name);
            clauses.push(op.to_sql(&col_sql, &val));
        }

        if clauses.is_empty() {
            String::new()
        } else if clauses.len() == 1 {
            clauses.remove(0)
        } else {
            clauses.join(self.combine_mode.separator())
        }
    }

    pub fn create_rule(
        columns: &[String],
        selected_col: &str,
        operator: FilterOperator,
        value: &str,
        window: &mut Window,
        cx: &mut Context<QuarryWindow>,
    ) -> (FilterRuleState, Vec<Subscription>) {
        let col_idx = columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(selected_col))
            .unwrap_or(0);
        let columns_list = columns.to_vec();
        let col_state =
            cx.new(|cx| SelectState::new(columns_list, Some(IndexPath::new(col_idx)), window, cx));

        let op_options: Vec<String> = FilterOperator::ALL
            .iter()
            .map(|o| o.display_label().to_string())
            .collect();
        let op_idx = FilterOperator::ALL
            .iter()
            .position(|o| *o == operator)
            .unwrap_or(0);
        let op_state =
            cx.new(|cx| SelectState::new(op_options, Some(IndexPath::new(op_idx)), window, cx));

        let val_state = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(value)
                .placeholder("Value…")
        });

        let mut subs = Vec::new();

        // Enter in value input applies the filter
        subs.push(cx.subscribe_in(
            &val_state,
            window,
            |this: &mut QuarryWindow, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.apply_filter_builder(window, cx);
                }
            },
        ));

        // Value change updates live SQL preview
        subs.push(cx.subscribe(&val_state, |_, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        }));

        // Dropdown selection updates re-render
        subs.push(
            cx.subscribe(&col_state, |_, _, _: &SelectEvent<Vec<String>>, cx| {
                cx.notify();
            }),
        );
        subs.push(
            cx.subscribe(&op_state, |_, _, _: &SelectEvent<Vec<String>>, cx| {
                cx.notify();
            }),
        );

        (
            FilterRuleState {
                column_select: col_state,
                operator_select: op_state,
                value_input: val_state,
            },
            subs,
        )
    }
}

/// Splits SQL on a top-level keyword (AND/OR), respecting quotes and parentheses.
pub fn split_top_level(sql: &str, sep: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut paren_depth: usize = 0;
    let bytes = sql.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\'' {
            if in_quote && i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                current.push('\'');
                current.push('\'');
                i += 2;
                continue;
            }
            in_quote = !in_quote;
            current.push('\'');
            i += 1;
            continue;
        }

        if !in_quote {
            if b == b'(' {
                paren_depth += 1;
            } else if b == b')' {
                paren_depth = paren_depth.saturating_sub(1);
            }

            if paren_depth == 0 {
                let remainder = &sql[i..];
                if remainder.to_uppercase().starts_with(sep) {
                    parts.push(current.trim().to_string());
                    current.clear();
                    i += sep.len();
                    continue;
                }
            }
        }

        current.push(b as char);
        i += 1;
    }

    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    parts
}

/// Parses a WHERE expression into combine mode and list of (column, operator, value).
pub fn parse_filter_expression(
    sql: &str,
    known_columns: &[String],
) -> (FilterCombineMode, Vec<(String, FilterOperator, String)>) {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return (FilterCombineMode::And, Vec::new());
    }

    let has_or =
        !split_top_level(trimmed, " OR ").is_empty() && trimmed.to_uppercase().contains(" OR ");
    let (mode, parts) = if has_or {
        (FilterCombineMode::Or, split_top_level(trimmed, " OR "))
    } else {
        (FilterCombineMode::And, split_top_level(trimmed, " AND "))
    };

    let mut rules = Vec::new();
    for part in parts {
        let mut clean = part.trim();
        while clean.starts_with('(') && clean.ends_with(')') {
            clean = clean[1..clean.len() - 1].trim();
        }
        if clean.is_empty() {
            continue;
        }
        if let Some((col, op, val)) = parse_single_clause(clean, known_columns) {
            rules.push((col, op, val));
        }
    }

    (mode, rules)
}

fn parse_single_clause(
    clause: &str,
    known_columns: &[String],
) -> Option<(String, FilterOperator, String)> {
    let ops = [
        (" IS NOT NULL", FilterOperator::IsNotNull, false),
        (" IS NULL", FilterOperator::IsNull, false),
        (" NOT LIKE ", FilterOperator::NotLike, true),
        (" ILIKE ", FilterOperator::ILike, true),
        (" LIKE ", FilterOperator::Like, true),
        (" >= ", FilterOperator::GreaterThanOrEqual, true),
        (" <= ", FilterOperator::LessThanOrEqual, true),
        (" != ", FilterOperator::NotEquals, true),
        (" <> ", FilterOperator::NotEquals, true),
        (" > ", FilterOperator::GreaterThan, true),
        (" < ", FilterOperator::LessThan, true),
        (" = ", FilterOperator::Equals, true),
        (" IN ", FilterOperator::In, true),
        (">=", FilterOperator::GreaterThanOrEqual, true),
        ("<=", FilterOperator::LessThanOrEqual, true),
        ("!=", FilterOperator::NotEquals, true),
        ("<>", FilterOperator::NotEquals, true),
        (">", FilterOperator::GreaterThan, true),
        ("<", FilterOperator::LessThan, true),
        ("=", FilterOperator::Equals, true),
    ];

    let upper = clause.to_uppercase();
    for (op_str, op, takes_val) in ops {
        if let Some(pos) = upper.find(op_str) {
            let left = clause[..pos].trim();
            let col_name = clean_ident(left);

            // Match known columns case-insensitively
            let matched_col = known_columns
                .iter()
                .find(|c| c.eq_ignore_ascii_case(&col_name))
                .cloned()
                .unwrap_or(col_name);

            let val = if takes_val {
                let right = clause[pos + op_str.len()..].trim();
                clean_literal(right)
            } else {
                String::new()
            };

            return Some((matched_col, op, val));
        }
    }
    None
}

fn clean_ident(ident: &str) -> String {
    let t = ident.trim();
    if (t.starts_with('"') && t.ends_with('"'))
        || (t.starts_with('`') && t.ends_with('`'))
        || (t.starts_with('\'') && t.ends_with('\''))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn clean_literal(lit: &str) -> String {
    let mut t = lit.trim();
    if t.starts_with('(') && t.ends_with(')') {
        t = t[1..t.len() - 1].trim();
    }
    if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
        t[1..t.len() - 1].replace("''", "'")
    } else {
        t.to_string()
    }
}

impl QuarryWindow {
    pub fn open_filter_builder(
        &mut self,
        preselected_col: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Tab::Table {
            table_name,
            filter,
            grid,
            ..
        }) = self.state.active_tab()
        else {
            return;
        };

        let (columns, current_filter) = {
            let rs = grid.read(cx).delegate();
            let cols = if let Some(src) = rs.source.as_ref() {
                src.columns
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
            } else if let Some(r) = rs.result() {
                r.columns.clone()
            } else {
                Vec::new()
            };
            (cols, filter.read(cx).value().to_string())
        };

        if columns.is_empty() {
            return;
        }

        let (combine_mode, parsed_rules) = parse_filter_expression(&current_filter, &columns);

        let mut rules = Vec::new();
        let mut subs = Vec::new();

        if !parsed_rules.is_empty() {
            for (col, op, val) in parsed_rules {
                let (r, s) = FilterBuilderDialog::create_rule(&columns, &col, op, &val, window, cx);
                rules.push(r);
                subs.extend(s);
            }
        } else {
            let initial_col = preselected_col.as_deref().unwrap_or_else(|| &columns[0]);
            let (r, s) = FilterBuilderDialog::create_rule(
                &columns,
                initial_col,
                FilterOperator::Equals,
                "",
                window,
                cx,
            );
            rules.push(r);
            subs.extend(s);
        }

        self.filter_builder = Some(FilterBuilderDialog {
            table_name: table_name.clone(),
            columns,
            combine_mode,
            rules,
            subs,
        });

        cx.notify();
    }

    pub fn add_filter_builder_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.filter_builder.as_mut() else {
            return;
        };
        let first_col = dialog.columns.first().cloned().unwrap_or_default();
        let (rule, subs) = FilterBuilderDialog::create_rule(
            &dialog.columns,
            &first_col,
            FilterOperator::Equals,
            "",
            window,
            cx,
        );
        dialog.rules.push(rule);
        dialog.subs.extend(subs);
        cx.notify();
    }

    pub fn apply_filter_builder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.filter_builder.take() else {
            return;
        };
        let dialect = self.state.dialect();
        let compiled = dialog.compile_to_sql(dialect, cx);
        if let Some(Tab::Table { filter, .. }) = self.state.active_tab() {
            filter
                .clone()
                .update(cx, |f, cx| f.set_value(&compiled, window, cx));
        }
        self.refresh_active_tab(window, cx);
        self.refocus_root(window, cx);
        let msg = if compiled.is_empty() {
            "Filter cleared".to_string()
        } else {
            format!("Filter applied: {compiled}")
        };
        self.toast(msg, false, cx);
    }

    pub fn apply_filter_clause(
        &mut self,
        clause: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Tab::Table { filter, .. }) = self.state.active_tab() else {
            return;
        };
        let current = filter.read(cx).value().trim().to_string();
        let new_filter = if current.is_empty() {
            clause.to_string()
        } else {
            format!("{current} AND ({clause})")
        };
        filter
            .clone()
            .update(cx, |f, cx| f.set_value(&new_filter, window, cx));
        self.refresh_active_tab(window, cx);
        self.toast(format!("Filter added: {clause}"), false, cx);
    }

    fn render_filter_builder_combine_bar(
        &self,
        dialog: &FilterBuilderDialog,
        t: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        let mode_chip = |id: &'static str, label: &'static str, mode: FilterCombineMode| {
            chip(t, id, label, dialog.combine_mode == mode).on_click(cx.listener(
                move |this, _, _, cx| {
                    if let Some(d) = this.filter_builder.as_mut() {
                        d.combine_mode = mode;
                        cx.notify();
                    }
                },
            ))
        };
        h_flex()
            .gap_2()
            .items_center()
            .justify_between()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().text_color(t.text_muted).child("Match"))
                    .child(
                        segmented(t)
                            .child(mode_chip("combine-and", "All", FilterCombineMode::And))
                            .child(mode_chip("combine-or", "Any", FilterCombineMode::Or)),
                    ),
            )
            .child(
                zbutton("add-rule-btn")
                    .icon(IconName::Plus)
                    .label("Add condition")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_filter_builder_rule(window, cx);
                    })),
            )
    }

    fn render_filter_builder_rule_row(
        &self,
        ix: usize,
        rule: &FilterRuleState,
        cx: &Context<Self>,
    ) -> Div {
        let op_label = rule
            .operator_select
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        let op = FilterOperator::from_label(&op_label);
        let requires_val = op.requires_value();

        h_flex()
            .gap_2()
            .items_center()
            .child(div().w(px(160.0)).child(Select::new(&rule.column_select)))
            .child(div().w(px(170.0)).child(Select::new(&rule.operator_select)))
            .child(
                div()
                    .flex_1()
                    .when(requires_val, |d| d.child(Input::new(&rule.value_input))),
            )
            .child(
                zbutton(("delete-rule-btn", ix))
                    .icon(IconName::X)
                    .ghost()
                    .small()
                    .tooltip("Remove condition")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(d) = this.filter_builder.as_mut() {
                            if d.rules.len() > ix {
                                d.rules.remove(ix);
                                cx.notify();
                            }
                        }
                    })),
            )
    }

    fn render_filter_builder_sql_preview(
        &self,
        preview_sql: &str,
        has_rules: bool,
        t: &Theme,
        cx: &Context<Self>,
    ) -> Div {
        div()
            .w_full()
            .p_3()
            .rounded(px(crate::ui::RADIUS_MD))
            .bg(t.bg_panel)
            .border_1()
            .border_color(t.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_muted)
                            .child("WHERE"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(if preview_sql.is_empty() {
                                t.text_muted
                            } else {
                                t.text_primary
                            })
                            .child(if preview_sql.is_empty() {
                                if has_rules {
                                    "Fill in a value to see the filter"
                                } else {
                                    "No conditions, so every row matches"
                                }
                                .to_string()
                            } else {
                                preview_sql.to_string()
                            }),
                    ),
            )
    }

    pub fn render_filter_builder_modal(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.filter_builder.as_ref()?;
        let t = self.theme.clone();
        let dialect = self.state.dialect();
        let preview_sql = dialog.compile_to_sql(dialect, cx);
        let subtitle = format!("Table: {}", dialog.table_name);

        let combine_bar = self.render_filter_builder_combine_bar(dialog, &t, cx);
        let rows = dialog
            .rules
            .iter()
            .enumerate()
            .map(|(ix, rule)| self.render_filter_builder_rule_row(ix, rule, cx));
        let preview =
            self.render_filter_builder_sql_preview(&preview_sql, !dialog.rules.is_empty(), &t, cx);

        Some(
            crate::ui::fade_in(
                "filter-builder-backdrop-fade",
                modal_backdrop("filter-builder-backdrop", dismiss_on_click, cx).child(
                    modal_card("filter-builder-card", 640.0, &t)
                        .overflow_hidden()
                        .child(modal_header(
                            &t,
                            "Filter Builder",
                            Some(subtitle.into()),
                            dismiss_on_click,
                        ))
                        .child(
                            modal_body("filter-builder-body")
                                .max_h(px(450.0))
                                .child(combine_bar)
                                .child(
                                    v_flex()
                                        .gap_2()
                                        .when(dialog.rules.is_empty(), |d| {
                                            d.child(
                                                div()
                                                    .py_4()
                                                    .flex()
                                                    .justify_center()
                                                    .text_sm()
                                                    .text_color(t.text_muted)
                                                    .child(
                                                        "No conditions yet. Add one to filter rows.",
                                                    ),
                                            )
                                        })
                                        .children(rows),
                                )
                                .child(preview),
                        )
                        .child(
                            modal_footer(&t)
                                .child(
                                    zbutton("filter-builder-reset")
                                        .label("Reset filter")
                                        .ghost()
                                        .small()
                                        .tooltip("Clear the current filter on this table")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.filter_builder = None;
                                            this.clear_filter(window, cx);
                                        })),
                                )
                                .child(
                                    zbutton("filter-builder-cancel")
                                        .label("Cancel")
                                        .ghost()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.filter_builder = None;
                                            this.refocus_root(window, cx);
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    zbutton("filter-builder-apply")
                                        .icon(IconName::Check)
                                        .label("Apply")
                                        .primary()
                                        .small()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.apply_filter_builder(window, cx);
                                        })),
                                ),
                        ),
                ),
            )
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Named imports, not `super::*`: that would bring in `gpui::*`, whose `test` attribute macro
    // shadows the built-in `#[test]` and recurses until rustc overflows its stack (SIGBUS)
    use super::{parse_filter_expression, split_top_level, FilterCombineMode, FilterOperator};

    #[test]
    fn test_filter_operator_to_sql() {
        assert_eq!(FilterOperator::Equals.to_sql("age", "30"), "age = 30");
        assert_eq!(
            FilterOperator::Equals.to_sql("name", "Alice"),
            "name = 'Alice'"
        );
        assert_eq!(
            FilterOperator::Equals.to_sql("name", "O'Connor"),
            "name = 'O''Connor'"
        );
        assert_eq!(
            FilterOperator::Equals.to_sql("deleted", "null"),
            "deleted IS NULL"
        );
        assert_eq!(
            FilterOperator::NotEquals.to_sql("status", "draft"),
            "status != 'draft'"
        );
        assert_eq!(
            FilterOperator::GreaterThan.to_sql("price", "99.95"),
            "price > 99.95"
        );
        assert_eq!(
            FilterOperator::IsNull.to_sql("archived_at", ""),
            "archived_at IS NULL"
        );
        assert_eq!(
            FilterOperator::IsNotNull.to_sql("created_at", ""),
            "created_at IS NOT NULL"
        );
        assert_eq!(
            FilterOperator::Contains.to_sql("bio", "engineer"),
            "bio LIKE '%engineer%'"
        );
        assert_eq!(
            FilterOperator::StartsWith.to_sql("code", "US"),
            "code LIKE 'US%'"
        );
        assert_eq!(
            FilterOperator::EndsWith.to_sql("email", "@gmail.com"),
            "email LIKE '%@gmail.com'"
        );
        assert_eq!(
            FilterOperator::In.to_sql("category_id", "1, 2, 3"),
            "category_id IN (1, 2, 3)"
        );
        assert_eq!(
            FilterOperator::In.to_sql("tag", "alpha, beta"),
            "tag IN ('alpha', 'beta')"
        );
    }

    #[test]
    fn test_split_top_level() {
        let sql = "status = 'active' AND title = 'Cat AND Dog' AND age > 20";
        let parts = split_top_level(sql, " AND ");
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "status = 'active'");
        assert_eq!(parts[1], "title = 'Cat AND Dog'");
        assert_eq!(parts[2], "age > 20");
    }

    #[test]
    fn test_parse_filter_expression() {
        let cols = vec![
            "id".to_string(),
            "name".to_string(),
            "age".to_string(),
            "deleted_at".to_string(),
        ];
        let sql = "name = 'Alice' AND age >= 21 AND deleted_at IS NULL";
        let (mode, rules) = parse_filter_expression(sql, &cols);
        assert_eq!(mode, FilterCombineMode::And);
        assert_eq!(rules.len(), 3);
        assert_eq!(
            rules[0],
            (
                "name".to_string(),
                FilterOperator::Equals,
                "Alice".to_string()
            )
        );
        assert_eq!(
            rules[1],
            (
                "age".to_string(),
                FilterOperator::GreaterThanOrEqual,
                "21".to_string()
            )
        );
        assert_eq!(
            rules[2],
            (
                "deleted_at".to_string(),
                FilterOperator::IsNull,
                "".to_string()
            )
        );
    }

    #[test]
    fn test_parse_filter_expression_or() {
        let cols = vec!["status".to_string()];
        let sql = "status = 'open' OR status = 'pending'";
        let (mode, rules) = parse_filter_expression(sql, &cols);
        assert_eq!(mode, FilterCombineMode::Or);
        assert_eq!(rules.len(), 2);
    }
}
