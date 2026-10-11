// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Catalog work on the active connection: lazy schema loading, objects, refresh and schema compare

use super::*;

impl QuarryWindow {
    /// Other connected databases to compare the active one with
    pub(crate) fn open_compare_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.state.active_connection.as_ref().map(|c| c.id.clone());
        let choices: Vec<(String, bool, String)> = self
            .state
            .saved_connections
            .iter()
            .filter(|c| {
                Some(&c.id) != active.as_ref() && self.state.open_sessions.contains_key(&c.id)
            })
            .map(|c| (c.name.clone(), false, c.id.clone()))
            .collect();
        if active.is_none() || choices.is_empty() {
            return self.toast(
                "Connect to both databases first: the one to compare and the one to change",
                true,
                cx,
            );
        }
        self.open_choice_menu(
            choices,
            |this, id, cx| {
                this.state.pending_compare = Some(id);
                cx.notify();
            },
            window,
            cx,
        );
    }

    /// Loads schemas a big database's catalog skipped, once each; the sidebar, completion and
    /// squiggles pick them up when they arrive
    pub(crate) fn load_schemas(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        let (Some(session), Some(id)) = (
            self.state.session.clone(),
            self.state.active_connection.as_ref().map(|c| c.id.clone()),
        ) else {
            return;
        };
        for schema in names {
            let key = format!("{id}/{schema}");
            if self.state.catalog.is_loaded(&schema)
                || !self.state.loading_schemas.insert(key.clone())
            {
                continue;
            }
            let (session, id) = (session.clone(), id.clone());
            run_db_bg(
                cx,
                async move {
                    session
                        .introspect_schema(&schema)
                        .await
                        .map(|t| (schema, t))
                },
                move |this, res, cx| {
                    this.state.loading_schemas.remove(&key);
                    match res {
                        Ok((schema, tables)) => {
                            if let Some(open) = this.state.open_sessions.get_mut(&id) {
                                open.1.add_schema(&schema, tables.clone());
                            }
                            // Switched away meanwhile: it's kept for when this connection comes back
                            if this
                                .state
                                .active_connection
                                .as_ref()
                                .is_some_and(|c| c.id == id)
                            {
                                let mut catalog = this.state.catalog.clone();
                                catalog.add_schema(&schema, tables);
                                this.state.set_catalog(catalog);
                            }
                        }
                        Err(e) => this.toast(format!("Couldn't load the schema: {e}"), true, cx),
                    }
                },
            );
        }
    }

    /// The script that makes `target_id` match the active connection's schema, in a tab on the target
    pub(crate) fn compare_schema(
        &mut self,
        target_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((target_session, target_catalog)) =
            self.state.open_sessions.get(target_id).cloned()
        else {
            return;
        };
        // A diff needs every table: big databases' partial catalogs are read in full first
        if !self.state.catalog.unloaded.is_empty() || !target_catalog.unloaded.is_empty() {
            let Some(source_session) = self.state.session.clone() else {
                return;
            };
            let target_id = target_id.to_string();
            self.toast("Reading both schemas in full…".to_string(), false, cx);
            return run_db(
                window,
                cx,
                async move {
                    (
                        source_session.introspect().await,
                        target_session.introspect().await,
                    )
                },
                move |this, res, window, cx| match res {
                    (Ok(source), Ok(target)) => {
                        this.finish_compare(&target_id, &source, &target, window, cx)
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        this.toast(format!("Couldn't read the schemas: {e}"), true, cx)
                    }
                },
            );
        }
        let source_catalog = self.state.catalog.clone();
        self.finish_compare(target_id, &source_catalog, &target_catalog, window, cx);
    }

    pub(crate) fn finish_compare(
        &mut self,
        target_id: &str,
        source_catalog: &quarry_core::SchemaCatalog,
        target_catalog: &quarry_core::SchemaCatalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_session) = self.state.open_sessions.get(target_id).map(|o| o.0.clone())
        else {
            return;
        };
        let source = self
            .state
            .active_connection
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let target = self
            .state
            .saved_connections
            .iter()
            .find(|c| c.id == target_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let Some(script) =
            quarry_core::diff::diff(source_catalog, target_catalog, target_session.dialect())
        else {
            return self.toast(
                format!("{source} and {target} have the same tables and columns"),
                false,
                cx,
            );
        };
        let header = format!("-- Makes {target} match {source} (tables and columns; views, indexes and constraints aren't compared).\n-- Review before running: this tab runs on {target}.\n\n");
        self.open_query_tab(&(header + &script), window, cx);
        // The tab runs on the target; switching to it switches the connection
        if let Some(Tab::Query { connection_id, .. }) = self.state.tabs.last_mut() {
            *connection_id = Some(target_id.to_string());
        }
    }

    /// Fetches the extra objects of every expanded schema that doesn't have them yet
    pub(crate) fn load_schema_objects(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone() else {
            return;
        };
        let missing: Vec<String> = self
            .state
            .catalog
            .schemas
            .iter()
            .filter(|s| {
                self.state.expanded_schemas.contains(*s)
                    && !self.state.schema_objects.contains_key(*s)
            })
            .cloned()
            .collect();
        for schema in missing {
            self.state.schema_objects.insert(schema.clone(), None);
            let (session, gen) = (session.clone(), self.state.catalog_gen);
            let name = schema.clone();
            run_db(
                window,
                cx,
                async move { session.objects(&name).await },
                move |this, res, _, cx| {
                    // A reconnect or refresh since: these belong to the old catalog
                    if this.state.catalog_gen != gen {
                        return;
                    }
                    let objects = res.unwrap_or_else(|e| {
                        this.state
                            .messages
                            .push(format!("Couldn't list objects in {schema}: {e}"));
                        Vec::new()
                    });
                    this.state.schema_objects.insert(schema, Some(objects));
                    cx.notify();
                },
            );
        }
    }

    /// Opens a function, trigger, sequence or type's definition in a new query tab
    pub(crate) fn open_object_definition(
        &mut self,
        schema: String,
        obj: quarry_core::schema::SchemaObject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.state.session.clone() else {
            return;
        };
        run_db(
            window,
            cx,
            async move { session.object_ddl(&schema, &obj).await },
            |this, res, window, cx| match res {
                Ok(ddl) => this.open_query_tab(&ddl, window, cx),
                Err(e) => this.toast(format!("Couldn't load the definition: {e}"), true, cx),
            },
        );
    }

    /// Re-reads the schema tree of the active connection
    pub(crate) fn refresh_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.state.session.clone() else {
            return;
        };
        let Some(id) = self.state.active_connection.as_ref().map(|c| c.id.clone()) else {
            return;
        };
        run_db(
            window,
            cx,
            async move { session.load_catalog().await },
            move |this, res, _, cx| match res {
                Ok(catalog) => {
                    // Kept for when this connection is switched back to
                    if let Some(open) = this.state.open_sessions.get_mut(&id) {
                        open.1 = catalog.clone();
                    }
                    // Switched away mid-refresh: don't put this schema under another connection
                    if this
                        .state
                        .active_connection
                        .as_ref()
                        .is_some_and(|c| c.id == id)
                    {
                        this.state.set_catalog(catalog);
                        // A big database's catalog comes back partial: reload the schemas that were open
                        let open: Vec<String> =
                            this.state.expanded_schemas.iter().cloned().collect();
                        this.load_schemas(open, cx);
                        this.toast("Schema refreshed".to_string(), false, cx);
                    }
                }
                Err(e) => this.toast(format!("Schema refresh failed: {e}"), true, cx),
            },
        );
    }
}
