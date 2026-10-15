// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

use crate::grid::ResultSet;
use chrono::{DateTime, Utc};
use gpui::component::input::{EditorState, InputState};
use gpui::component::table::TableState;
use gpui::Entity;
use quarry_core::schema::{SchemaCatalog, TableSchema};
use quarry_core::session::DatabaseSession;
use quarry_core::sqlite::SqliteSession;
use quarry_core::{ActivityLog, LoggedSession};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Environment {
    Local,
    Staging,
    Prod,
}

impl Environment {
    pub fn label(&self) -> &'static str {
        match self {
            Environment::Local => "LOCAL",
            Environment::Staging => "STAGING",
            Environment::Prod => "PROD",
        }
    }

    /// For calm UI text: "Local", "Staging", "Production"
    pub fn name(&self) -> &'static str {
        match self {
            Environment::Local => "Local",
            Environment::Staging => "Staging",
            Environment::Prod => "Production",
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    /// Driver id (`quarry_core::drivers()`). Older files stored `"kind": "Postgres"` etc.
    #[serde(alias = "kind", deserialize_with = "driver_id")]
    pub driver: String,
    /// The connection string, without its password (that's in the keychain)
    pub path_or_url: String,
    pub env: Environment,
    pub read_only: bool,
    /// Reach the database through an SSH tunnel (drivers with a host and port)
    #[serde(default)]
    pub ssh: Option<crate::ssh::SshConfig>,
    /// Cancel statements running longer than this (Q10), via the dialect's statement timeout
    #[serde(default)]
    pub statement_timeout_secs: Option<u64>,
}

impl ConnectionProfile {
    pub fn driver(&self) -> Option<&'static dyn quarry_core::Driver> {
        quarry_core::driver(&self.driver)
    }

    /// A new profile for a pasted connection string, if a driver recognizes it
    pub fn from_address(text: &str) -> Option<Self> {
        let text = text.trim();
        let driver = quarry_core::detect(text)?;
        Some(ConnectionProfile {
            id: uuid::Uuid::new_v4().to_string(),
            name: driver.display_address(text),
            driver: driver.id().to_string(),
            path_or_url: text.to_string(),
            env: Environment::Local,
            read_only: false,
            ssh: None,
            statement_timeout_secs: None,
        })
    }
}

/// Reads a driver id, mapping the old `kind` values
fn driver_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let id = <String as serde::Deserialize>::deserialize(d)?;
    Ok(match id.as_str() {
        "Postgres" => "postgres".into(),
        "SqliteFile" | "SqliteMemory" => "sqlite".into(),
        _ => id,
    })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryHistoryItem {
    pub id: String,
    #[serde(default)]
    pub connection: String,
    pub sql: String,
    pub timestamp: DateTime<Utc>,
    pub execution_time: Duration,
    pub rows_affected: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    #[default]
    Normal,
    Insert,
    Visual,
}

impl VimMode {
    pub fn label(&self) -> &'static str {
        match self {
            VimMode::Normal => "-- NORMAL --",
            VimMode::Insert => "-- INSERT --",
            VimMode::Visual => "-- VISUAL --",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub enum Tab {
    Query {
        id: String,
        title: String,
        editor: Entity<EditorState>,
        grid: Entity<TableState<ResultSet>>,
        /// The .sql file this tab is saved to, if any
        file: Option<std::path::PathBuf>,
        /// Text as last saved, to show unsaved changes
        saved_text: String,
        /// The connection this tab runs on; switching to the tab switches to it
        connection_id: Option<String>,
        /// Postgres schema runs use (`search_path`); `None` = the connection's default
        schema: Option<String>,
        /// SELECTs without their own LIMIT get this one
        row_limit: Option<u32>,
        is_running: bool,
        error_message: Option<String>,
        /// The last run was an EXPLAIN the dialect could read: shown as a tree instead of the grid
        plan: Option<std::rc::Rc<quarry_core::plan::Plan>>,
        /// Modal editing mode when vim_mode setting is active
        vim_mode: VimMode,
    },
    Table {
        id: String,
        table_name: String,
        schema_name: String,
        filter: Entity<InputState>,
        grid: Entity<TableState<ResultSet>>,
        /// The connection this tab was opened on (tabs keep their own connection)
        connection: Option<ConnectionProfile>,
        view: crate::table_detail::TableView,
        detail: Option<Result<Box<crate::table_detail::TableDetail>, String>>,
    },
}

#[allow(dead_code)]
impl Tab {
    pub fn id(&self) -> &str {
        match self {
            Tab::Query { id, .. } => id,
            Tab::Table { id, .. } => id,
        }
    }

    pub fn grid(&self) -> &Entity<TableState<ResultSet>> {
        match self {
            Tab::Query { grid, .. } | Tab::Table { grid, .. } => grid,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Tab::Query { title, .. } => title,
            Tab::Table { table_name, .. } => table_name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomPanelTab {
    Messages,
    History,
    Logs,
    Server,
}

#[allow(dead_code)]
pub struct AppState {
    pub workspace_name: String,
    pub active_connection: Option<ConnectionProfile>,
    /// A saved query being renamed in the sidebar, and the name box
    pub renaming_query: Option<(std::path::PathBuf, Entity<InputState>)>,
    pub saved_connections: Vec<ConnectionProfile>,
    pub session: Option<Arc<dyn DatabaseSession>>,
    /// Live sessions by connection id, so switching back doesn't reconnect (or lose an in-memory DB)
    pub open_sessions: HashMap<String, (Arc<dyn DatabaseSession>, SchemaCatalog)>,
    /// Every statement the app ran, on every connection (L1)
    pub activity_log: ActivityLog,
    /// Open SSH tunnels by connection id; dropping one stops its ssh process
    pub tunnels: HashMap<String, crate::ssh::Tunnel>,
    pub catalog: SchemaCatalog,
    /// The same catalog, shared with the editors' autocomplete
    pub completion_catalog: crate::completion::SharedCatalog,
    /// Identifier usage frequencies from query history, used as completion tiebreakers
    pub completion_usage: crate::completion::SharedUsage,
    /// Past executed query strings, used for predictive ghost text autosuggestions
    pub completion_history: crate::completion::SharedHistory,
    /// The active session's dialect, read by editor code actions
    pub completion_dialect: crate::completion::SharedDialect,
    /// Bumped on every catalog change; editors are re-linted when it moves past `linted_gen`
    pub catalog_gen: u64,
    pub linted_gen: u64,
    /// Functions, sequences, triggers, types per expanded schema; `None` while loading. Reset with the catalog.
    pub schema_objects: HashMap<String, Option<Vec<quarry_core::schema::SchemaObject>>>,
    /// Open object groups in the sidebar, as "schema/Functions"
    pub expanded_object_groups: HashSet<String>,
    pub expanded_schemas: HashSet<String>,
    /// "connection id/schema" being loaded for a big database's lazy catalog
    pub loading_schemas: HashSet<String>,
    /// Connections folded shut in the sidebar (clicking an open connection folds it)
    pub collapsed_connections: std::collections::HashSet<String>,
    pub sidebar_visible: bool,

    // Tabs
    pub tabs: Vec<Tab>,
    /// Recently opened tables (schema, table), newest first; shown when the command bar is empty
    pub recent_tables: Vec<(String, String)>,
    /// Pinned tables and queries per connection id (U6)
    pub pins: crate::pins::Pins,
    pub active_tab_index: usize,

    // Bottom Panel
    pub bottom_panel_open: bool,
    pub bottom_panel_tab: BottomPanelTab,
    pub history: Vec<QueryHistoryItem>,
    pub messages: Vec<String>,

    // Modals
    pub command_bar_open: bool,
    /// Find Usages results; the palette lists these instead of its usual items while set
    pub usages: Option<crate::usages::Usages>,
    /// Connection picked in "Compare Schema With…"; handled on the next render, which has a window
    pub pending_compare: Option<String>,
    pub command_bar_selected_index: usize,

    pub review_modal_open: bool,
    pub review_modal_sql: Vec<String>,
    pub review_modal_table: String,
    /// What the statements do, shown as a per-row diff; `review_modal_show_sql` flips to the SQL
    pub review_modal_changes: quarry_core::edits::PendingChanges,
    pub review_modal_show_sql: bool,

    pub add_conn_modal_open: bool,
    /// Driver id picked in the add-connection form
    pub add_conn_driver: &'static str,
    pub add_conn_env: Environment,
    pub add_conn_read_only: bool,
    pub add_conn_ssh: bool,
    pub add_conn_feedback: Option<(String, bool)>, // (msg, is_error)
    /// Set while the modal edits a saved connection rather than adding one
    pub editing_conn: Option<String>,
    /// Connection modal tab: 0 General, 1 SSH, 2 Advanced
    pub add_conn_tab: u8,
    /// Sidebar / bottom panel visibility from before ⇧⎋ maximised the editor
    pub maximized: Option<(bool, bool)>,

    pub toast_message: Option<(String, bool)>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let history = crate::history::load();
        let completion_usage: crate::completion::SharedUsage = Default::default();
        {
            let mut map = std::collections::HashMap::new();
            for item in &history {
                for word in item.sql.split(|c: char| !c.is_alphanumeric() && c != '_') {
                    if !word.is_empty() {
                        *map.entry(word.to_lowercase()).or_default() += 1;
                    }
                }
            }
            *completion_usage.borrow_mut() = map;
        }

        let completion_history: crate::completion::SharedHistory = Default::default();
        *completion_history.borrow_mut() =
            history.iter().rev().map(|item| item.sql.clone()).collect();

        let mut state = Self {
            workspace_name: "quarry".to_string(),
            active_connection: None,
            renaming_query: None,
            saved_connections: crate::connections::load(),
            session: None,
            open_sessions: HashMap::new(),
            loading_schemas: HashSet::new(),
            activity_log: ActivityLog::default(),
            tunnels: HashMap::new(),
            catalog: SchemaCatalog::default(),
            completion_catalog: Default::default(),
            completion_usage,
            completion_history,
            completion_dialect: std::rc::Rc::new(std::cell::Cell::new(&quarry_core::dialect::Ansi)),
            catalog_gen: 0,
            linted_gen: 0,
            schema_objects: HashMap::new(),
            expanded_object_groups: HashSet::new(),
            expanded_schemas: HashSet::new(),
            collapsed_connections: Default::default(),
            sidebar_visible: true,

            tabs: Vec::new(),
            recent_tables: Vec::new(),
            pins: crate::pins::load(),
            active_tab_index: 0,

            bottom_panel_open: false,
            bottom_panel_tab: BottomPanelTab::Messages,
            history,
            messages: Vec::new(),

            command_bar_open: false,
            usages: None,
            pending_compare: None,
            command_bar_selected_index: 0,

            review_modal_open: false,
            review_modal_sql: Vec::new(),
            review_modal_table: String::new(),
            review_modal_changes: Default::default(),
            review_modal_show_sql: false,

            add_conn_modal_open: false,
            add_conn_driver: "sqlite",
            add_conn_env: Environment::Local,
            add_conn_read_only: false,
            add_conn_ssh: false,
            maximized: None,
            add_conn_tab: 0,
            editing_conn: None,
            add_conn_feedback: None,

            toast_message: None,
        };

        state.expanded_schemas.insert("main".to_string());
        state.expanded_schemas.insert("public".to_string());
        state
    }

    /// Automatically sets up an in-memory SQLite database on first launch with sample tables
    pub async fn init_demo_playground(&mut self) {
        let profile = ConnectionProfile {
            id: "local-sqlite".to_string(),
            name: "local-sqlite".to_string(),
            driver: "sqlite".to_string(),
            path_or_url: ":memory:".to_string(),
            env: Environment::Local,
            read_only: false,
            ssh: None,
            statement_timeout_secs: None,
        };

        self.saved_connections.insert(0, profile.clone());

        if let Ok(raw) = SqliteSession::open_in_memory() {
            let session: Arc<dyn DatabaseSession> = Arc::new(LoggedSession::new(
                Arc::new(raw),
                profile.name.clone(),
                self.activity_log.clone(),
            ));
            let seed_sql = r#"
                CREATE TABLE users (
                    id INTEGER PRIMARY KEY,
                    email TEXT NOT NULL UNIQUE,
                    name TEXT NOT NULL,
                    role TEXT DEFAULT 'developer',
                    active BOOLEAN DEFAULT 1,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );

                CREATE TABLE products (
                    id INTEGER PRIMARY KEY,
                    sku TEXT NOT NULL,
                    name TEXT NOT NULL,
                    price REAL NOT NULL,
                    stock INTEGER DEFAULT 0
                );

                CREATE TABLE orders (
                    id INTEGER PRIMARY KEY,
                    user_id INTEGER NOT NULL REFERENCES users(id),
                    total REAL NOT NULL,
                    status TEXT DEFAULT 'pending',
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );

                INSERT INTO users (id, email, name, role, active) VALUES
                    (1, 'alice@company.com', 'Alice Smith', 'admin', 1),
                    (2, 'bob@company.com', 'Bob Jones', 'developer', 1),
                    (3, 'carol@staging.io', 'Carol White', 'analyst', 0);

                INSERT INTO products (id, sku, name, price, stock) VALUES
                    (1, 'PRD-001', 'Ergonomic Keyboard', 149.99, 45),
                    (2, 'PRD-002', 'Ultra-wide Monitor', 499.00, 12),
                    (3, 'PRD-003', 'Standing Desk', 620.50, 8);

                INSERT INTO orders (id, user_id, total, status) VALUES
                    (101, 1, 149.99, 'completed'),
                    (102, 2, 1119.50, 'processing'),
                    (103, 1, 499.00, 'pending');
            "#;

            for stmt in quarry_sql::split_statements(seed_sql) {
                if let Err(e) = session.execute(&stmt.sql).await {
                    self.messages.push(format!("Seed failed: {}", e));
                }
            }
            if let Ok(cat) = session.introspect().await {
                self.set_catalog(cat);
            }

            self.open_sessions
                .insert(profile.id.clone(), (session.clone(), self.catalog.clone()));
            self.set_session(Some(session));
            self.active_connection = Some(profile);

            self.messages.push(
                "Connected to local SQLite playground. Seeded users, products, orders.".to_string(),
            );
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab_index)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab_index)
    }

    /// ⇧⎋: hide the sidebar and bottom panel, or bring them back as they were
    pub fn toggle_maximize(&mut self) {
        match self.maximized.take() {
            Some((sidebar, panel)) => {
                (self.sidebar_visible, self.bottom_panel_open) = (sidebar, panel)
            }
            None => {
                self.maximized = Some((self.sidebar_visible, self.bottom_panel_open));
                (self.sidebar_visible, self.bottom_panel_open) = (false, false);
            }
        }
    }

    /// Closes a connection's session (and SSH tunnel); the saved profile stays
    pub fn disconnect(&mut self, id: &str) {
        self.open_sessions.remove(id);
        self.tunnels.remove(id);
        if self.active_connection.as_ref().is_some_and(|c| c.id == id) {
            self.set_session(None);
            self.active_connection = None;
            self.set_catalog(SchemaCatalog::default());
        }
    }

    /// Disconnects and forgets a saved connection, including its keychain password
    pub fn remove_connection(&mut self, id: &str) {
        self.disconnect(id);
        self.saved_connections.retain(|c| c.id != id);
        crate::connections::forget_secret(id);
        if let Err(e) = crate::connections::save(&self.saved_connections) {
            self.messages
                .push(format!("Could not save connections: {}", e));
        }
    }

    /// Makes an already-open connection active again. Returns false if it isn't open.
    pub fn activate_open_session(&mut self, id: &str) -> bool {
        let Some((session, catalog)) = self.open_sessions.get(id).cloned() else {
            return false;
        };
        let Some(profile) = self.saved_connections.iter().find(|c| c.id == id).cloned() else {
            return false;
        };
        self.set_session(Some(session));
        self.set_catalog(catalog);
        self.active_connection = Some(profile);
        true
    }

    /// Records an editor-run statement in history (memory + disk)
    pub fn record_history(
        &mut self,
        sql: String,
        execution_time: Duration,
        rows_affected: u64,
        error: Option<String>,
    ) {
        let item = QueryHistoryItem {
            id: uuid::Uuid::new_v4().to_string(),
            connection: self
                .active_connection
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            sql,
            timestamp: Utc::now(),
            execution_time,
            rows_affected,
            error,
        };
        crate::history::append(&item);
        // Boost usage count for words in this query
        {
            let mut usage = self.completion_usage.borrow_mut();
            for word in item.sql.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if !word.is_empty() {
                    *usage.entry(word.to_lowercase()).or_default() += 1;
                }
            }
        }
        // Add to predictive completion history
        {
            let mut hist = self.completion_history.borrow_mut();
            hist.insert(0, item.sql.clone());
            hist.truncate(500);
        }
        self.history.push(item);
    }

    pub fn set_session(&mut self, session: Option<Arc<dyn DatabaseSession>>) {
        self.completion_dialect.set(
            session
                .as_ref()
                .map_or(&quarry_core::dialect::Ansi, |s| s.dialect()),
        );
        self.session = session;
    }

    /// The SQL dialect of the active connection (ANSI when disconnected)
    pub fn dialect(&self) -> &'static dyn quarry_core::Dialect {
        self.completion_dialect.get()
    }

    /// `SELECT * FROM <table>` limited to `rows`, in the active connection's dialect
    pub fn select_rows_sql(&self, schema: &str, table: &str, rows: u64) -> String {
        let d = self.dialect();
        d.limit(
            &format!("SELECT * FROM {}", d.qualified(schema, table)),
            rows,
            0,
        )
    }

    pub fn set_catalog(&mut self, catalog: SchemaCatalog) {
        *self.completion_catalog.borrow_mut() = catalog.clone();
        self.catalog = catalog;
        self.catalog_gen += 1;
        self.schema_objects.clear();
        self.rebuild_usage();
    }

    pub fn rebuild_usage(&self) {
        let mut map = std::collections::HashMap::new();
        let conn_name = self
            .active_connection
            .as_ref()
            .map(|c| c.name.as_str())
            .unwrap_or("");
        for item in &self.history {
            if conn_name.is_empty() || item.connection == conn_name {
                for word in item.sql.split(|c: char| !c.is_alphanumeric() && c != '_') {
                    if !word.is_empty() {
                        *map.entry(word.to_lowercase()).or_default() += 1;
                    }
                }
            }
        }
        *self.completion_usage.borrow_mut() = map;
    }

    pub fn note_recent_table(&mut self, schema: &str, table: &str) {
        self.recent_tables
            .retain(|(s, t)| !(s == schema && t == table));
        self.recent_tables
            .insert(0, (schema.to_string(), table.to_string()));
        self.recent_tables.truncate(8);
    }

    pub fn find_tab(&self, table_name: &str) -> Option<usize> {
        self.tabs
            .iter()
            .position(|t| matches!(t, Tab::Table { table_name: n, .. } if n == table_name))
    }

    /// Removes a tab (the last one too, which leaves the empty state) and returns it
    pub fn close_tab(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.active_tab_index = after_close(self.active_tab_index, index, self.tabs.len());
        Some(tab)
    }

    pub fn toggle_schema(&mut self, schema_name: &str) {
        if self.expanded_schemas.contains(schema_name) {
            self.expanded_schemas.remove(schema_name);
        } else {
            self.expanded_schemas.insert(schema_name.to_string());
        }
    }

    /// Installs an opened session as the active connection
    pub fn apply_connection(
        &mut self,
        profile: ConnectionProfile,
        session: Arc<dyn DatabaseSession>,
        catalog: SchemaCatalog,
        tunnel: Option<crate::ssh::Tunnel>,
    ) {
        let mut profile = profile;
        let bare = |p: &ConnectionProfile| {
            p.driver().map_or_else(
                || p.path_or_url.clone(),
                |d| d.take_secret(&p.path_or_url).0,
            )
        };
        // Same address as a saved connection: reuse it (name, environment, read-only) instead of duplicating
        if let Some(existing) = self
            .saved_connections
            .iter()
            .find(|c| c.driver == profile.driver && bare(c) == bare(&profile))
        {
            profile = ConnectionProfile {
                path_or_url: profile.path_or_url,
                ..existing.clone()
            };
        }
        self.open_sessions
            .insert(profile.id.clone(), (session.clone(), catalog.clone()));
        if let Some(t) = tunnel {
            self.tunnels.insert(profile.id.clone(), t);
        }
        self.set_session(Some(session));
        self.set_catalog(catalog);
        if !self.saved_connections.iter().any(|c| c.id == profile.id) {
            self.saved_connections.push(profile.clone());
            if let Err(e) = crate::connections::save(&self.saved_connections) {
                self.messages
                    .push(format!("Could not save connection: {}", e));
            }
        }
        self.active_connection = Some(profile);
    }

    #[allow(dead_code)]
    pub fn find_table_schema(&self, table_name: &str) -> Option<&TableSchema> {
        self.catalog
            .tables
            .iter()
            .find(|t| t.table_name == table_name)
    }
}

/// Opens a session for the profile and introspects it. Runs on the DB runtime.
pub async fn open_session(
    profile: &ConnectionProfile,
    log: ActivityLog,
) -> Result<
    (
        Arc<dyn DatabaseSession>,
        SchemaCatalog,
        Option<crate::ssh::Tunnel>,
    ),
    String,
> {
    let driver = profile.driver().ok_or_else(|| {
        format!(
            "This build of Quarry doesn't include the {} driver",
            profile.driver
        )
    })?;
    let addr = driver.display_address(&profile.path_or_url);
    tracing::info!(
        "Connecting to [{}] ({}) via {}...",
        profile.name,
        addr,
        driver.name()
    );
    let mut conn = crate::connections::with_secret(profile);
    let mut tunnel = None;
    if let Some(ssh) = &profile.ssh {
        let (host, port) = driver
            .tunnel_target(&conn)
            .ok_or_else(|| format!("{} connections can't use an SSH tunnel", driver.name()))?;
        let cfg = ssh.clone();
        let t = tokio::task::spawn_blocking(move || crate::ssh::Tunnel::open(&cfg, &host, port))
            .await
            .map_err(|e| e.to_string())??;
        conn = driver.through_tunnel(&conn, t.local_port);
        tunnel = Some(t);
    }
    let session = driver
        .connect(&conn, profile.read_only)
        .await
        .map_err(|e| {
            tracing::error!("Connection to [{}] failed: {e}", profile.name);
            e.to_string()
        })?;
    let session: Arc<dyn DatabaseSession> =
        Arc::new(LoggedSession::new(session, profile.name.clone(), log));
    if let Some(secs) = profile.statement_timeout_secs.filter(|s| *s > 0) {
        if let Some(sql) = session.dialect().statement_timeout(secs) {
            session
                .execute(&sql)
                .await
                .map_err(|e| format!("Couldn't set the statement timeout: {e}"))?;
        }
    }
    let catalog = session
        .load_catalog()
        .await
        .map_err(|e| format!("Introspection error: {}", e))?;
    tracing::info!(
        "Connected to [{}] (loaded {} tables)",
        profile.name,
        catalog.tables.len()
    );
    Ok((session, catalog, tunnel))
}

/// Which tab is active after closing `closed`: closing the active one moves to its right-hand
/// neighbour (or the left one if it was last); closing another keeps the same tab active
pub fn after_close(active: usize, closed: usize, remaining: usize) -> usize {
    if remaining == 0 {
        0
    } else if active > closed {
        active - 1
    } else {
        active.min(remaining - 1)
    }
}

#[cfg(test)]
mod close_tests {
    use super::after_close;

    #[test]
    fn closing_moves_to_the_neighbour_in_view() {
        // tabs A B C D, B active
        assert_eq!(after_close(1, 1, 3), 1); // close B -> C (now at 1)
        assert_eq!(after_close(3, 3, 3), 2); // close the last, D -> C
        assert_eq!(after_close(2, 0, 3), 1); // close A while C active -> still C
        assert_eq!(after_close(0, 3, 3), 0); // close D while A active -> still A
        assert_eq!(after_close(0, 0, 0), 0); // nothing left
    }
}
