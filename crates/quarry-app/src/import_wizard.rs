// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Import wizard (X4-X8, Flow E): file → preview → map columns → behavior → run.

use crate::grid::{DataGrid, ResultSet};
use crate::state::Tab;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::button::ButtonVariants;
use gpui::component::input::{Input, InputState};
use gpui::component::select::{Select, SelectState};
use gpui::component::table::TableState;
use gpui::component::{Disableable, IndexPath, Sizable};
use gpui::*;
use quarry_core::import::{self, Encoding, ImportData, ImportPlan, ImportReport, OnConflict};
use quarry_core::{DbValue, LogEntry, QueryResult};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const SKIP: &str = "(skip)";
const PREVIEW_ROWS: usize = 20;

pub struct ImportState {
    pub open: bool,
    path: Option<PathBuf>,
    bytes: Vec<u8>,
    is_json: bool,
    encoding: Encoding,
    delimiter: char,
    header: bool,
    empty_is_null: bool,
    data: ImportData,
    parse_error: Option<String>,
    preview: Entity<TableState<ResultSet>>,
    /// Existing target (schema, table, columns), when opened from a table tab
    existing: Option<(String, String, Vec<String>)>,
    new_table: bool,
    new_name: Entity<InputState>,
    /// Existing-table mode: per source column, which target column (or skip)
    mapping: Vec<Entity<SelectState<Vec<String>>>>,
    /// New-table mode: per source column, its SQL type (editable)
    types: Vec<Entity<InputState>>,
    on_conflict: OnConflict,
    all_or_nothing: bool,
    running: Option<Job>,
    report: Option<ImportReport>,
}

struct Job {
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    total: usize,
    _ticker: Task<()>,
}

impl ImportState {
    pub fn new(
        theme: &crate::theme::Theme,
        window: &mut Window,
        cx: &mut Context<QuarryWindow>,
    ) -> Self {
        Self {
            open: false,
            path: None,
            bytes: Vec::new(),
            is_json: false,
            encoding: Encoding::Utf8,
            delimiter: ',',
            header: true,
            empty_is_null: true,
            data: ImportData::default(),
            parse_error: None,
            preview: cx.new(|cx| {
                TableState::new(ResultSet::new(theme, None), window, cx).col_selectable(false)
            }),
            existing: None,
            new_table: true,
            new_name: cx.new(|cx| InputState::new(window, cx).placeholder("new_table")),
            mapping: Vec::new(),
            types: Vec::new(),
            on_conflict: OnConflict::Error,
            all_or_nothing: true,
            running: None,
            report: None,
        }
    }
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "imported".into()
    } else {
        s
    }
}

impl QuarryWindow {
    /// Flow E: pick a file, then the wizard opens with a preview
    pub fn open_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.current_connection(cx).0.is_none() {
            self.toast("Connect to a database first", true, cx);
            return;
        }
        // Importing into the table tab you're looking at, if any
        self.import.existing = match self.state.active_tab() {
            Some(Tab::Table {
                schema_name,
                table_name,
                grid,
                ..
            }) => grid
                .read(cx)
                .delegate()
                .source
                .as_ref()
                // Tables without a primary key are fine to import into; views and read-only connections aren't
                .filter(|s| {
                    !matches!(
                        s.read_only_reason,
                        Some("Read-only connection" | "Views are read-only")
                    )
                })
                .map(|s| {
                    (
                        schema_name.clone(),
                        table_name.clone(),
                        s.columns.iter().map(|c| c.name.clone()).collect(),
                    )
                }),
            _ => None,
        };
        self.import.new_table = self.import.existing.is_none();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let bytes = std::fs::read(&path);
            this.update_in(cx, |this, window, cx| match bytes {
                Ok(bytes) => this.load_import_file(path, bytes, window, cx),
                Err(e) => this.toast(format!("Couldn't read {}: {}", path.display(), e), true, cx),
            })
            .ok();
        })
        .detach();
    }

    fn load_import_file(
        &mut self,
        path: PathBuf,
        bytes: Vec<u8>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let st = &mut self.import;
        st.is_json = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        let (text, enc) = import::decode(&bytes, None);
        st.encoding = enc;
        st.delimiter = import::guess_delimiter(&text);
        let stem = path
            .file_stem()
            .map(|s| sanitize(&s.to_string_lossy()))
            .unwrap_or_default();
        st.new_name
            .update(cx, |i, cx| i.set_value(stem, window, cx));
        st.path = Some(path);
        st.bytes = bytes;
        st.report = None;
        st.open = true;
        self.reparse_import(window, cx);
    }

    /// Re-reads the file with the current source options and rebuilds preview, mapping and types
    fn reparse_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialect = self.current_connection(cx).0.map_or(
            &quarry_core::dialect::Ansi as &dyn quarry_core::Dialect,
            |s| s.dialect(),
        );
        let st = &mut self.import;
        let (text, _) = import::decode(&st.bytes, Some(st.encoding));
        let parsed = if st.is_json {
            import::parse_json(&text)
        } else {
            Ok(import::parse_csv(
                &text,
                st.delimiter,
                st.header,
                st.empty_is_null,
            ))
        };
        match parsed {
            Ok(d) => {
                st.data = d;
                st.parse_error = None;
            }
            Err(e) => {
                st.data = ImportData::default();
                st.parse_error = Some(e);
            }
        }

        let preview = QueryResult {
            columns: st.data.headers.clone(),
            rows: st
                .data
                .rows
                .iter()
                .take(PREVIEW_ROWS)
                .map(|r| {
                    r.iter()
                        .map(|v| v.clone().map(DbValue::Text).unwrap_or(DbValue::Null))
                        .collect()
                })
                .collect(),
            rows_affected: 0,
            execution_time: Default::default(),
            statement: String::new(),
        };
        st.preview.update(cx, |t, cx| {
            t.delegate_mut().set_results(vec![preview]);
            t.refresh(cx);
        });

        // Existing table: match columns by name (case-insensitive), otherwise skip
        let targets: Vec<String> = st
            .existing
            .as_ref()
            .map(|e| e.2.clone())
            .unwrap_or_default();
        let options: Vec<String> = std::iter::once(SKIP.to_string())
            .chain(targets.iter().cloned())
            .collect();
        st.mapping = st
            .data
            .headers
            .iter()
            .map(|h| {
                let pick = options
                    .iter()
                    .position(|o| o.eq_ignore_ascii_case(h))
                    .unwrap_or(0);
                let options = options.clone();
                cx.new(|cx| SelectState::new(options, Some(IndexPath::new(pick)), window, cx))
            })
            .collect();
        st.types = import::infer_types(&st.data, dialect)
            .into_iter()
            .map(|ty| cx.new(|cx| InputState::new(window, cx).default_value(ty)))
            .collect();
        cx.notify();
    }

    fn run_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(profile)) = self.current_connection(cx) else {
            return;
        };
        let st = &self.import;
        let catalog_default = self.state.catalog.default_schema.clone();
        let default_schema = catalog_default
            .as_deref()
            .or(self.state.catalog.schemas.first().map(String::as_str))
            .unwrap_or("main");
        let (table, create, mapping) = if st.new_table {
            let name = sanitize(&st.new_name.read(cx).value());
            let cols: Vec<(String, String)> = st
                .data
                .headers
                .iter()
                .zip(&st.types)
                .map(|(h, ty)| (sanitize(h), ty.read(cx).value().trim().to_string()))
                .collect();
            let mapping = cols
                .iter()
                .enumerate()
                .map(|(i, (c, _))| (i, c.clone()))
                .collect();
            (
                session.dialect().qualified(default_schema, &name),
                Some(cols),
                mapping,
            )
        } else {
            let Some((schema, name, _)) = &st.existing else {
                return;
            };
            let mapping: Vec<(usize, String)> = st
                .mapping
                .iter()
                .enumerate()
                .filter_map(|(i, sel)| {
                    sel.read(cx)
                        .selected_value()
                        .filter(|v| *v != SKIP)
                        .map(|v| (i, v.clone()))
                })
                .collect();
            (session.dialect().qualified(schema, name), None, mapping)
        };
        if mapping.is_empty() {
            self.toast("Map at least one column", true, cx);
            return;
        }
        let plan = ImportPlan {
            table: table.clone(),
            create,
            mapping,
            on_conflict: st.on_conflict,
            all_or_nothing: st.all_or_nothing,
        };
        let data = st.data.clone();
        let total = data.rows.len();
        let progress = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c) = (progress.clone(), cancel.clone());
        let started = std::time::Instant::now();
        let job = crate::runtime::DB_RUNTIME
            .spawn(async move { import::run(session.as_ref(), &data, &plan, &p, &c).await });

        let ticker = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        });
        let file = st
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        let created = st.new_table;
        self.import.running = Some(Job {
            progress,
            cancel,
            total,
            _ticker: ticker,
        });
        self.import.report = None;

        cx.spawn_in(window, async move |this, cx| {
            let outcome = job.await.unwrap_or_else(|e| Err(e.to_string()));
            this.update_in(cx, |this, window, cx| {
                this.import.running = None;
                // One activity-log entry for the whole import (L1)
                this.state.activity_log.push(LogEntry {
                    at: chrono::Utc::now(),
                    connection: profile.name.clone(),
                    sql: format!("-- import {file} into {table}"),
                    duration: started.elapsed(),
                    rows: outcome.as_ref().map(|r| r.inserted as u64).unwrap_or(0),
                    error: outcome.as_ref().err().cloned(),
                });
                match outcome {
                    Ok(report) => {
                        let ok = !report.rolled_back && !report.cancelled;
                        if ok && created {
                            this.refresh_catalog(window, cx);
                        } else if ok {
                            this.refresh_active_tab(window, cx);
                        }
                        let msg = if report.cancelled {
                            "Import cancelled. Nothing was written.".to_string()
                        } else if report.rolled_back {
                            format!("Nothing was imported: row {} failed", report.failures[0].0)
                        } else {
                            format!(
                                "{} rows imported, {} skipped, {} failed",
                                report.inserted,
                                report.skipped,
                                report.failures.len()
                            )
                        };
                        this.toast(msg, report.rolled_back, cx);
                        this.import.report = Some(report);
                    }
                    Err(e) => this.toast(format!("Import failed: {e}"), true, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// X8: row number + reason for every failed row
    fn save_error_report(&mut self, cx: &mut Context<Self>) {
        let Some(report) = &self.import.report else {
            return;
        };
        let mut csv = String::from("row,error\n");
        for (row, err) in &report.failures {
            csv += &format!("{},\"{}\"\n", row, err.replace('"', "\"\""));
        }
        let dir = std::env::home_dir().map(|h| h.join("Downloads"))
            .unwrap_or_default();
        let rx = cx.prompt_for_new_path(&dir, Some("import-errors.csv"));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let res = std::fs::write(&path, csv);
            this.update(cx, |this, cx| match res {
                Ok(()) => {
                    this.toast("Saved the error report", false, cx);
                    this.toast_reveal = Some(path);
                }
                Err(e) => this.toast(e.to_string(), true, cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn import_busy(&self) -> bool {
        self.import.running.is_some()
    }

    pub fn render_import_wizard(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.import.open {
            return None;
        }
        let t = self.theme.clone();
        let st = &self.import;
        let busy = st.running.is_some();
        let file = st
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();

        // Options are chips; while an import runs they dim
        let choice = |id: SharedString, label: &'static str, on: bool| {
            chip(&t, id, label, on).when(busy, |d| d.opacity(0.5))
        };
        let row = |label: &'static str| {
            h_flex().gap_4().items_center().child(
                div()
                    .w(px(110.0))
                    .flex_shrink_0()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_secondary)
                    .child(label),
            )
        };
        let reparse = |f: fn(&mut crate::import_wizard::ImportState)| {
            move |this: &mut QuarryWindow,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<QuarryWindow>| {
                f(&mut this.import);
                this.reparse_import(window, cx);
            }
        };

        let source = v_flex()
            .gap_2()
            .child(
                row("Encoding")
                    .child(
                        choice("enc-utf8".into(), "UTF-8", st.encoding == Encoding::Utf8)
                            .on_click(cx.listener(reparse(|s| s.encoding = Encoding::Utf8))),
                    )
                    .child(
                        choice(
                            "enc-latin1".into(),
                            "Latin-1",
                            st.encoding == Encoding::Latin1,
                        )
                        .on_click(cx.listener(reparse(|s| s.encoding = Encoding::Latin1))),
                    )
                    .when(st.encoding == Encoding::Latin1, |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(t.warning)
                                .child("Not valid UTF-8, read as Latin-1"),
                        )
                    }),
            )
            .when(!st.is_json, |this| {
                this.child(
                    row("Delimiter")
                        .child(
                            choice("delim-comma".into(), "Comma", st.delimiter == ',')
                                .on_click(cx.listener(reparse(|s| s.delimiter = ','))),
                        )
                        .child(
                            choice("delim-semi".into(), "Semicolon", st.delimiter == ';')
                                .on_click(cx.listener(reparse(|s| s.delimiter = ';'))),
                        )
                        .child(
                            choice("delim-tab".into(), "Tab", st.delimiter == '\t')
                                .on_click(cx.listener(reparse(|s| s.delimiter = '\t'))),
                        )
                        .child(div().w(px(12.0)))
                        .child(
                            choice("header".into(), "First row is header", st.header)
                                .on_click(cx.listener(reparse(|s| s.header = !s.header))),
                        )
                        .child(
                            choice("emptynull".into(), "Empty is NULL", st.empty_is_null).on_click(
                                cx.listener(reparse(|s| s.empty_is_null = !s.empty_is_null)),
                            ),
                        ),
                )
            })
            .when_some(st.parse_error.clone(), |this, e| {
                this.child(div().text_sm().text_color(t.danger).child(e))
            });

        let target_name = st.existing.as_ref().map(|e| format!("Into {}", e.1));
        let target = row("Target")
            .when_some(target_name, |this, name| {
                this.child(
                    zbutton("target-existing")
                        .pad_xs()
                        .label(name)
                        .small()
                        .disabled(busy)
                        .map(|b| {
                            if !st.new_table {
                                b.primary()
                            } else {
                                b.ghost()
                            }
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.import.new_table = false;
                            cx.notify();
                        })),
                )
            })
            .child(
                choice("target-new".into(), "New table", st.new_table).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.import.new_table = true;
                        cx.notify();
                    },
                )),
            )
            .when(st.new_table, |this| {
                this.child(div().w(px(220.0)).child(Input::new(&st.new_name).xsmall()))
            });

        // One line per source column: → target column, or its type for a new table
        let columns = v_flex()
            .gap_1()
            .children(st.data.headers.iter().enumerate().map(|(i, h)| {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .w(px(180.0))
                            .text_sm()
                            .text_color(t.text_primary)
                            .truncate()
                            .child(h.clone()),
                    )
                    .child(
                        gpui::component::Icon::new(gpui::assets::IconName::ArrowRight)
                            .xsmall()
                            .text_color(t.text_muted),
                    )
                    .child(div().w(px(240.0)).child(if st.new_table {
                        Input::new(&st.types[i]).xsmall().into_any_element()
                    } else {
                        Select::new(&st.mapping[i]).xsmall().into_any_element()
                    }))
            }));

        let behavior =
            v_flex()
                .gap_2()
                .child(
                    row("Duplicates")
                        .child(
                            choice(
                                "dup-error".into(),
                                "Stop on conflict",
                                st.on_conflict == OnConflict::Error,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.import.on_conflict = OnConflict::Error;
                                cx.notify();
                            })),
                        )
                        .child(
                            choice(
                                "dup-skip".into(),
                                "Skip duplicates",
                                st.on_conflict == OnConflict::SkipDuplicates,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.import.on_conflict = OnConflict::SkipDuplicates;
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    row("If a row fails")
                        .child(
                            choice("fail-all".into(), "Import nothing", st.all_or_nothing)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.import.all_or_nothing = true;
                                    cx.notify();
                                })),
                        )
                        .child(
                            choice("fail-skip".into(), "Skip that row", !st.all_or_nothing)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.import.all_or_nothing = false;
                                    cx.notify();
                                })),
                        ),
                );

        let status: AnyElement = match (&st.running, &st.report) {
            (Some(job), _) => div()
                .text_sm()
                .text_color(t.text_secondary)
                .child(format!(
                    "{} / {} rows…",
                    job.progress.load(Ordering::Relaxed),
                    job.total
                ))
                .into_any_element(),
            (None, Some(r)) if r.rolled_back => div()
                .text_sm()
                .text_color(t.danger)
                .child(format!(
                    "Nothing was imported. Row {}: {}",
                    r.failures[0].0, r.failures[0].1
                ))
                .into_any_element(),
            (None, Some(r)) => div()
                .text_sm()
                .text_color(if r.failures.is_empty() {
                    t.success
                } else {
                    t.warning
                })
                .child(format!(
                    "{} imported · {} skipped · {} failed",
                    r.inserted,
                    r.skipped,
                    r.failures.len()
                ))
                .into_any_element(),
            _ => div()
                .text_sm()
                .text_color(t.text_muted)
                .child(format!(
                    "{} rows · {} columns",
                    st.data.rows.len(),
                    st.data.headers.len()
                ))
                .into_any_element(),
        };
        let has_failures = st.report.as_ref().is_some_and(|r| !r.failures.is_empty());
        let done = st
            .report
            .as_ref()
            .is_some_and(|r| !r.rolled_back && !r.cancelled);

        let footer = modal_footer(&t)
            .child(div().flex_1().min_w_0().child(status))
            .when(has_failures && !busy, |this| {
                this.child(
                    zbutton("import-errors")
                        .pad_sm()
                        .label("Save error report")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| this.save_error_report(cx))),
                )
            })
            .when(busy, |this| {
                this.child(
                    zbutton("import-stop")
                        .pad_sm()
                        .label("Cancel import")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, _| {
                            if let Some(job) = &this.import.running {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        })),
                )
            })
            .when(!busy, |this| {
                this.child(
                    zbutton("import-close")
                        .pad_sm()
                        .label(if done { "Done" } else { "Cancel" })
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.import.open = false;
                            cx.notify();
                        })),
                )
                .when(!done, |this| {
                    this.child(
                        zbutton("import-run")
                            .pad_sm()
                            .label("Import")
                            .primary()
                            .small()
                            .disabled(st.data.rows.is_empty())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.run_import(window, cx)),
                            ),
                    )
                })
            });

        let section = |title: &'static str| section_title(&t, title);
        Some(
            crate::ui::fade_in(
                "import-card-backdrop-fade",
                modal_backdrop("import-card-backdrop", dismiss_on_click, cx).child(
                    modal_card("import-card", 760.0, &t)
                        // Enter in any field imports, like the primary button
                        .on_action(cx.listener(
                            |this, _: &gpui::component::input::Enter, window, cx| {
                                let st = &this.import;
                                let done = st
                                    .report
                                    .as_ref()
                                    .is_some_and(|r| !r.rolled_back && !r.cancelled);
                                if !this.import_busy() && !done && !st.data.rows.is_empty() {
                                    this.run_import(window, cx)
                                }
                            },
                        ))
                        .max_h(px(760.0))
                        .overflow_hidden()
                        .child(modal_header(
                            &t,
                            "Import",
                            Some(file.clone().into()),
                            dismiss_on_click,
                        ))
                        .child(
                            modal_body("import-body")
                                .flex_1()
                                .gap_4()
                                .child(source)
                                .child(section("Preview"))
                                .child(
                                    div()
                                        .h(px(170.0))
                                        .border_1()
                                        .border_color(t.border)
                                        .rounded(px(crate::ui::RADIUS_SM))
                                        .overflow_hidden()
                                        .child(DataGrid::render(
                                            &st.preview,
                                            &t,
                                            cx,
                                            |_, _, _| {},
                                            |_, _, _| {},
                                            |_, _, _| {},
                                            |_, _, _| {},
                                            |_, _, _, _| {},
                                            |_, _, _, _| {},
                                            |_, _, _, _| {},
                                        )),
                                )
                                .child(target)
                                .child(section(if st.new_table {
                                    "Columns and types"
                                } else {
                                    "Map columns"
                                }))
                                .child(columns)
                                .child(behavior),
                        )
                        .child(footer),
                ),
            )
            .into_any_element(),
        )
    }
}
