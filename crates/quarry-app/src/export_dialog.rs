// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Export dialog (X1, X2, X7, Flow D): CSV / JSON / SQL from a result set or a whole table.

use crate::state::Tab;
use crate::ui::*;
use crate::QuarryWindow;
use gpui::component::button::ButtonVariants;
use gpui::component::input::InputState;
use gpui::component::Sizable;
use gpui::*;
use quarry_core::export::{ExportFormat, ExportOptions, Exporter};
use quarry_core::{DatabaseSession, DbValue};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const CHUNK: usize = 5_000;

pub struct ExportState {
    pub open: bool,
    format: ExportFormat,
    delimiter: char,
    header: bool,
    null_text: &'static str,
    whole_table: bool,
    /// (column name, included)
    columns: Vec<(String, bool)>,
    limit: Entity<InputState>,
    running: Option<Job>,
}

struct Job {
    progress: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    _ticker: Task<()>,
}

/// Where the rows come from
enum Source {
    Rows(Vec<Vec<DbValue>>),
    Query(Arc<dyn DatabaseSession>, String),
}

impl ExportState {
    pub fn new(window: &mut Window, cx: &mut Context<QuarryWindow>) -> Self {
        Self {
            open: false,
            format: ExportFormat::Csv,
            delimiter: ',',
            header: true,
            null_text: "",
            whole_table: false,
            columns: Vec::new(),
            limit: cx.new(|cx| InputState::new(window, cx).placeholder("No limit")),
            running: None,
        }
    }
}

/// Opens the file manager with the file selected
pub fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(not(target_os = "macos"))]
    let _ = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
}

impl QuarryWindow {
    pub fn open_export(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.state.active_tab() else {
            return;
        };
        let rs = tab.grid().read(cx).delegate();
        let Some(result) = rs.result() else {
            self.toast(
                "Run a query first — there's nothing to export yet",
                true,
                cx,
            );
            return;
        };
        self.export.columns = result.columns.iter().map(|c| (c.clone(), true)).collect();
        // More rows exist than are loaded: a table page, or a result cut at MAX_ROWS
        self.export.whole_table =
            rs.source.as_ref().is_some_and(|s| s.has_more) || result.truncated();
        self.export.open = true;
        cx.notify();
    }

    fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.state.active_tab() else {
            return;
        };
        let rs = tab.grid().read(cx).delegate();
        let Some(result) = rs.result() else { return };
        let e = &self.export;
        let picked: Vec<usize> = e
            .columns
            .iter()
            .enumerate()
            .filter(|(_, (_, on))| *on)
            .map(|(i, _)| i)
            .collect();
        if picked.is_empty() {
            self.toast("Pick at least one column", true, cx);
            return;
        }
        let limit: Option<usize> = e.limit.read(cx).value().trim().parse().ok();
        let (table, base_name) = match (tab, rs.source.as_ref()) {
            (Tab::Table { table_name, .. }, Some(src)) => {
                (src.qualified_name(), table_name.clone())
            }
            _ => ("query_result".to_string(), "query_result".to_string()),
        };
        let source = match (rs.source.as_ref(), self.state.session.clone()) {
            (Some(src), _) if e.whole_table => Source::Query(src.session.clone(), src.select_sql()),
            // A cut-off query result: run the statement again, a page at a time
            (None, Some(session)) if e.whole_table && result.truncated() => Source::Query(
                session,
                format!(
                    "SELECT * FROM (\n{}\n) quarry_all",
                    result.statement.trim().trim_end_matches(';')
                ),
            ),
            _ => Source::Rows(result.rows.clone()),
        };
        let columns = result.columns.clone();
        let opts = ExportOptions {
            format: e.format,
            delimiter: e.delimiter,
            header: e.header,
            null_text: e.null_text.to_string(),
            columns: Some(picked),
            table,
        };

        let dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Downloads"))
            .unwrap_or_default();
        let path_rx = cx.prompt_for_new_path(
            &dir,
            Some(&format!("{}.{}", base_name, e.format.extension())),
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path_rx.await else {
                return;
            };
            let progress = Arc::new(AtomicUsize::new(0));
            let cancel = Arc::new(AtomicBool::new(false));
            let (p, c, out) = (progress.clone(), cancel.clone(), path.clone());
            let job = crate::runtime::DB_RUNTIME.spawn(async move {
                write_export(&out, &columns, opts, source, limit, &p, &c).await
            });

            this.update(cx, |this, cx| {
                // Repaint the live row count while the export runs
                let ticker = cx.spawn(async move |this, cx| loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(150))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                });
                this.export.running = Some(Job {
                    progress,
                    cancel,
                    _ticker: ticker,
                });
                cx.notify();
            })
            .ok();

            let outcome = job.await.unwrap_or_else(|e| Err(e.to_string()));
            this.update(cx, |this, cx| {
                this.export.running = None;
                this.export.open = false;
                match outcome {
                    Ok(Some(n)) => {
                        let name = path
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default();
                        this.toast(format!("Exported {} rows to {}", n, name), false, cx);
                        this.toast_reveal = Some(path);
                    }
                    Ok(None) => {
                        let _ = std::fs::remove_file(&path);
                        this.toast("Export cancelled", false, cx);
                    }
                    Err(e) => this.toast(format!("Export failed: {}", e), true, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    /// Whole database as a SQL script (X3), by the driver's dumper
    pub fn dump_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(profile)) = self.current_connection(cx) else {
            self.toast("Connect to a database first", true, cx);
            return;
        };
        if session.dumper().is_none() {
            return self.toast("This database can't be dumped yet", true, cx);
        }
        let dir = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Downloads"))
            .unwrap_or_default();
        let stem = profile.name.replace(['/', ':', ' '], "_");
        let path_rx = cx.prompt_for_new_path(&dir, Some(&format!("{stem}.sql")));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path_rx.await else {
                return;
            };
            let out = path.clone();
            let job = crate::runtime::DB_RUNTIME.spawn(async move {
                let dumper = session.dumper().ok_or("This database can't be dumped")?;
                dumper.dump(&out).await
            });
            let outcome = job.await.unwrap_or_else(|e| Err(e.to_string()));
            this.update(cx, |this, cx| match outcome {
                Ok(what) => {
                    let name = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    this.toast(format!("Dumped {what} to {name}"), false, cx);
                    this.toast_reveal = Some(path);
                }
                Err(e) => this.toast(format!("Dump failed: {e}"), true, cx),
            })
            .ok();
        })
        .detach();
    }

    pub fn render_export_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.export.open {
            return None;
        }
        let t = self.theme.clone();
        let e = &self.export;
        let is_table = matches!(self.state.active_tab(), Some(Tab::Table { .. }));
        let truncated = self
            .state
            .active_tab()
            .and_then(|tab| {
                tab.grid()
                    .read(cx)
                    .delegate()
                    .result()
                    .map(|r| r.truncated())
            })
            .unwrap_or(false);
        let loaded = self
            .state
            .active_tab()
            .and_then(|tab| {
                tab.grid()
                    .read(cx)
                    .delegate()
                    .result()
                    .map(|r| r.rows.len())
            })
            .unwrap_or(0);
        let running = e
            .running
            .as_ref()
            .map(|j| j.progress.load(Ordering::Relaxed));

        // Options are chips; while an export runs they stop responding
        let busy = running.is_some();
        let choice = |id: &'static str, label: &'static str, on: bool| {
            chip(&t, id, label, on).when(busy, |d| d.opacity(0.5))
        };
        let row = |label: &'static str| {
            h_flex().gap_4().items_center().child(
                div()
                    .w(px(104.0))
                    .flex_shrink_0()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_secondary)
                    .child(label),
            )
        };

        let body = v_flex()
            .gap_4()
            .child(
                row("Format").children(
                    [
                        (ExportFormat::Csv, "CSV"),
                        (ExportFormat::Json, "JSON"),
                        (ExportFormat::Sql, "SQL INSERT"),
                    ]
                    .map(|(f, label)| {
                        choice(label, label, e.format == f).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.export.format = f;
                                cx.notify();
                            },
                        ))
                    }),
                ),
            )
            .when(e.format == ExportFormat::Csv, |this| {
                this.child(row("Delimiter").children(
                    [(',', "Comma"), (';', "Semicolon"), ('\t', "Tab")].map(|(d, label)| {
                        choice(label, label, e.delimiter == d).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.export.delimiter = d;
                                cx.notify();
                            },
                        ))
                    }),
                ))
                .child(
                    row("Header row").child(choice("header-yes", "Include", e.header).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.export.header = !this.export.header;
                            cx.notify();
                        }),
                    )),
                )
                .child(row("NULL as").children(
                    [("", "Empty"), ("NULL", "NULL"), ("\\N", "\\N")].map(|(n, label)| {
                        choice(label, label, e.null_text == n).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.export.null_text = n;
                                cx.notify();
                            },
                        ))
                    }),
                ))
            })
            .child(
                row("Rows")
                    .child(
                        choice("rows-loaded", "Loaded rows", !e.whole_table).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.export.whole_table = false;
                                cx.notify();
                            },
                        )),
                    )
                    .when(is_table || truncated, |this| {
                        this.child(
                            choice(
                                "rows-all",
                                if is_table { "Entire table" } else { "All rows" },
                                e.whole_table,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.export.whole_table = true;
                                cx.notify();
                            })),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child(if e.whole_table {
                                if is_table {
                                    "re-runs the table query".to_string()
                                } else {
                                    "re-runs the query".to_string()
                                }
                            } else {
                                format!("{loaded} rows")
                            }),
                    ),
            )
            .child(row("Limit").child(div().w(px(140.0)).child(form_input(&e.limit))))
            .child(
                row("Columns")
                    .items_start()
                    .child(
                        chip_group()
                            .gap_1()
                            .children(e.columns.iter().enumerate().map(|(i, (name, on))| {
                                chip(&t, ("export-col", i), name.clone(), *on)
                                    .gap_1()
                                    .when(*on, |d| {
                                        d.child(
                                            gpui::component::Icon::new(
                                                gpui::assets::IconName::Check,
                                            )
                                            .xsmall()
                                            .text_color(t.accent),
                                        )
                                    })
                                    .flex_row_reverse()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(c) = this.export.columns.get_mut(i) {
                                            c.1 = !c.1;
                                        }
                                        cx.notify();
                                    }))
                            })),
                    ),
            );

        let footer = modal_footer(&t)
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(match running {
                        Some(n) => format!("{n} rows written…"),
                        None => String::new(),
                    }),
            )
            .when(running.is_some(), |this| {
                this.child(
                    zbutton("export-stop")
                        .pad_sm()
                        .label("Cancel export")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, _| {
                            if let Some(job) = &this.export.running {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        })),
                )
            })
            .when(running.is_none(), |this| {
                this.child(
                    zbutton("export-close")
                        .pad_sm()
                        .label("Cancel")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.export.open = false;
                            cx.notify();
                        })),
                )
                .child(
                    zbutton("export-go")
                        .pad_sm()
                        .label("Export…")
                        .primary()
                        .small()
                        .on_click(cx.listener(|this, _, window, cx| this.start_export(window, cx))),
                )
            });

        Some(
            modal_backdrop("export-card-backdrop", dismiss_on_click, cx)
                .child(
                    modal_card("export-card", 560.0, &t)
                        // Enter in any field exports, like the primary button
                        .on_action(cx.listener(
                            |this, _: &gpui::component::input::Enter, window, cx| {
                                if this.export.running.is_none() {
                                    this.start_export(window, cx)
                                }
                            },
                        ))
                        .overflow_hidden()
                        .child(modal_header(
                            &t,
                            "Export",
                            Some(
                                format!(
                                    "{} columns · CSV, JSON or SQL",
                                    e.columns.iter().filter(|c| c.1).count()
                                )
                                .into(),
                            ),
                            dismiss_on_click,
                        ))
                        .child(modal_body("export-body").max_h(px(460.0)).child(body))
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}

/// Runs on the DB runtime. `Ok(None)` means cancelled.
async fn write_export(
    path: &std::path::Path,
    columns: &[String],
    opts: ExportOptions,
    source: Source,
    limit: Option<usize>,
    progress: &AtomicUsize,
    cancel: &AtomicBool,
) -> Result<Option<usize>, String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut ex =
        Exporter::begin(std::io::BufWriter::new(file), columns, opts).map_err(|e| e.to_string())?;
    let cap = limit.unwrap_or(usize::MAX);
    match source {
        Source::Rows(rows) => {
            for chunk in rows[..rows.len().min(cap)].chunks(CHUNK) {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                ex.rows(chunk).map_err(|e| e.to_string())?;
                progress.store(ex.written(), Ordering::Relaxed);
            }
        }
        // Whole table: page through it so memory stays flat (X7)
        Source::Query(session, sql) => loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let want = CHUNK.min(cap - ex.written());
            let page = session
                .execute(
                    &session
                        .dialect()
                        .limit(&sql, want as u64, ex.written() as u64),
                )
                .await
                .map_err(|e| e.to_string())?;
            ex.rows(&page.rows).map_err(|e| e.to_string())?;
            progress.store(ex.written(), Ordering::Relaxed);
            if page.rows.len() < want || ex.written() >= cap {
                break;
            }
        },
    }
    ex.finish().map(Some).map_err(|e| e.to_string())
}
