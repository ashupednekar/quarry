// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

use chrono::{DateTime, Utc};
use gpui::component::input::{EditorState, InputState};
use crate::grid::ResultSet;
use gpui::component::table::TableState;
use gpui::Entity;
use quarry_core::postgres::PostgresSession;
use quarry_core::schema::{SchemaCatalog, TableSchema};
use quarry_core::session::DatabaseSession;
use quarry_core::{ActivityLog, LoggedSession};
use quarry_core::sqlite::SqliteSession;
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
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConnectionTargetKind {
    SqliteMemory,
    SqliteFile,
    Postgres,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub kind: ConnectionTargetKind,
    pub path_or_url: String,
    pub env: Environment,
    pub read_only: bool,
    /// Reach the database through an SSH tunnel (Postgres)
    #[serde(default)]
    pub ssh: Option<crate::ssh::SshConfig>,
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
        is_running: bool,
        error_message: Option<String>,
    },
    Table {
        id: String,
        table_name: String,
        schema_name: String,
        filter: Entity<InputState>,
        grid: Entity<TableState<ResultSet>>,
        /// The connection this tab was opened on (tabs keep their own connection)
        connection: Option<ConnectionProfile>,
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
    pub expanded_schemas: HashSet<String>,
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
    pub command_bar_selected_index: usize,

    pub review_modal_open: bool,
    pub review_modal_sql: Vec<String>,
    pub review_modal_table: String,

    pub add_conn_modal_open: bool,
    pub add_conn_kind: ConnectionTargetKind,
    pub add_conn_env: Environment,
    pub add_conn_read_only: bool,
    pub add_conn_ssh: bool,
    pub add_conn_feedback: Option<(String, bool)>, // (msg, is_error)



    pub toast_message: Option<(String, bool)>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let mut state = Self {
            workspace_name: "quarry".to_string(),
            active_connection: None,
            saved_connections: crate::connections::load(),
            session: None,
            open_sessions: HashMap::new(),
            activity_log: ActivityLog::default(),
            tunnels: HashMap::new(),
            catalog: SchemaCatalog::default(),
            completion_catalog: Default::default(),
            expanded_schemas: HashSet::new(),
            sidebar_visible: true,

            tabs: Vec::new(),
            recent_tables: Vec::new(),
            pins: crate::pins::load(),
            active_tab_index: 0,

            bottom_panel_open: false,
            bottom_panel_tab: BottomPanelTab::Messages,
            history: crate::history::load(),
            messages: Vec::new(),

            command_bar_open: false,
            command_bar_selected_index: 0,

            review_modal_open: false,
            review_modal_sql: Vec::new(),
            review_modal_table: String::new(),

            add_conn_modal_open: false,
            add_conn_kind: ConnectionTargetKind::SqliteMemory,
            add_conn_env: Environment::Local,
            add_conn_read_only: false,
            add_conn_ssh: false,
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
            kind: ConnectionTargetKind::SqliteMemory,
            path_or_url: ":memory:".to_string(),
            env: Environment::Local,
            read_only: false,
            ssh: None,
        };

        self.saved_connections.insert(0, profile.clone());

        if let Ok(raw) = SqliteSession::open_in_memory() {
            let session: Arc<dyn DatabaseSession> =
                Arc::new(LoggedSession::new(Arc::new(raw), profile.name.clone(), self.activity_log.clone()));
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

            self.open_sessions.insert(profile.id.clone(), (session.clone(), self.catalog.clone()));
            self.session = Some(session);
            self.active_connection = Some(profile);

            self.messages.push("Connected to local SQLite playground. Seeded users, products, orders.".to_string());
        }
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab_index)
    }

    /// Makes an already-open connection active again. Returns false if it isn't open.
    pub fn activate_open_session(&mut self, id: &str) -> bool {
        let Some((session, catalog)) = self.open_sessions.get(id).cloned() else { return false };
        let Some(profile) = self.saved_connections.iter().find(|c| c.id == id).cloned() else { return false };
        self.session = Some(session);
        self.set_catalog(catalog);
        self.active_connection = Some(profile);
        true
    }

    /// Records an editor-run statement in history (memory + disk)
    pub fn record_history(&mut self, sql: String, execution_time: Duration, rows_affected: u64, error: Option<String>) {
        let item = QueryHistoryItem {
            id: uuid::Uuid::new_v4().to_string(),
            connection: self.active_connection.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
            sql,
            timestamp: Utc::now(),
            execution_time,
            rows_affected,
            error,
        };
        crate::history::append(&item);
        self.history.push(item);
    }

    pub fn set_catalog(&mut self, catalog: SchemaCatalog) {
        *self.completion_catalog.borrow_mut() = catalog.clone();
        self.catalog = catalog;
    }

    pub fn note_recent_table(&mut self, schema: &str, table: &str) {
        self.recent_tables.retain(|(s, t)| !(s == schema && t == table));
        self.recent_tables.insert(0, (schema.to_string(), table.to_string()));
        self.recent_tables.truncate(8);
    }

    pub fn find_tab(&self, table_name: &str) -> Option<usize> {
        self.tabs.iter().position(|t| matches!(t, Tab::Table { table_name: n, .. } if n == table_name))
    }

    /// Removes a tab (the last one too, which leaves the empty state) and returns it
    pub fn close_tab(&mut self, index: usize) -> Option<Tab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if self.active_tab_index >= self.tabs.len() {
            self.active_tab_index = self.tabs.len().saturating_sub(1);
        } else if self.active_tab_index > index {
            self.active_tab_index -= 1;
        }
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
        let bare = |p: &ConnectionProfile| crate::connections::split_password(&p.path_or_url).0;
        // Same address as a saved connection: reuse it (name, environment, read-only) instead of duplicating
        if let Some(existing) = self.saved_connections.iter().find(|c| c.kind == profile.kind && bare(c) == bare(&profile)) {
            profile = ConnectionProfile { path_or_url: profile.path_or_url, ..existing.clone() };
        }
        self.open_sessions.insert(profile.id.clone(), (session.clone(), catalog.clone()));
        if let Some(t) = tunnel {
            self.tunnels.insert(profile.id.clone(), t);
        }
        self.session = Some(session);
        self.set_catalog(catalog);
        if !self.saved_connections.iter().any(|c| c.id == profile.id) {
            self.saved_connections.push(profile.clone());
            if let Err(e) = crate::connections::save(&self.saved_connections) {
                self.messages.push(format!("Could not save connection: {}", e));
            }
        }
        self.active_connection = Some(profile);
    }

#[allow(dead_code)]
    pub fn find_table_schema(&self, table_name: &str) -> Option<&TableSchema> {
        self.catalog.tables.iter().find(|t| t.table_name == table_name)
    }
}

/// Opens a session for the profile and introspects it. Runs on the DB runtime.
pub async fn open_session(
    profile: &ConnectionProfile,
    log: ActivityLog,
) -> Result<(Arc<dyn DatabaseSession>, SchemaCatalog, Option<crate::ssh::Tunnel>), String> {
    let mut tunnel = None;
    let session: Arc<dyn DatabaseSession> = match profile.kind {
        ConnectionTargetKind::SqliteMemory => Arc::new(SqliteSession::open_in_memory().map_err(|e| e.to_string())?),
        ConnectionTargetKind::SqliteFile => {
            Arc::new(SqliteSession::open_file(&profile.path_or_url, profile.read_only).map_err(|e| e.to_string())?)
        }
        ConnectionTargetKind::Postgres => {
            let mut url = crate::connections::with_secret(profile);
            if let Some(ssh) = &profile.ssh {
                let (host, port) = crate::ssh::target(&url);
                let cfg = ssh.clone();
                let t = tokio::task::spawn_blocking(move || crate::ssh::Tunnel::open(&cfg, &host, port))
                    .await
                    .map_err(|e| e.to_string())??;
                url = crate::ssh::retarget(&url, t.local_port);
                tunnel = Some(t);
            }
            Arc::new(PostgresSession::connect(&url, profile.read_only).await.map_err(|e| e.to_string())?)
        }
    };
    let session: Arc<dyn DatabaseSession> = Arc::new(LoggedSession::new(session, profile.name.clone(), log));
    let catalog = session.introspect().await.map_err(|e| format!("Introspection error: {}", e))?;
    Ok((session, catalog, tunnel))
}
