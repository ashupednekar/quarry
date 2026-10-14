// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/krishna/quarry>

//! Application menu bar and default keyboard shortcut bindings.

use super::*;
use gpui::component::input;
use gpui::{App, KeyBinding, Menu, MenuItem, OsAction};

/// Default keymap. Must run after `gpui::init` so these win over the input's own bindings.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-k", ToggleCommandBar, None),
        KeyBinding::new("secondary-enter", RunQuery, None),
        // Inputs map secondary-enter to "newline"; run the query instead.
        KeyBinding::new("secondary-enter", RunQuery, Some("Input")),
        KeyBinding::new("secondary-shift-enter", RunAll, None),
        KeyBinding::new("secondary-shift-enter", RunAll, Some("Input")),
        KeyBinding::new("secondary-s", ReviewChanges, None),
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        KeyBinding::new("secondary-j", ToggleBottomPanel, None),
        KeyBinding::new("secondary-n", NewQuery, None),
        KeyBinding::new("secondary-shift-n", NewConnection, None),
        KeyBinding::new("secondary-d", SelectNextOccurrence, None),
        KeyBinding::new("secondary-d", SelectNextOccurrence, Some("Input")),
        KeyBinding::new("secondary-w", CloseTab, None),
        KeyBinding::new("escape", Dismiss, None),
        // Standard macOS window/app shortcuts (also shown in the menu bar)
        KeyBinding::new("secondary-q", Quit, None),
        KeyBinding::new("secondary-,", OpenSettings, None),
        KeyBinding::new("secondary-h", Hide, None),
        KeyBinding::new("alt-secondary-h", HideOthers, None),
        KeyBinding::new("secondary-m", Minimize, None),
        KeyBinding::new("shift-secondary-w", CloseWindow, None),
        KeyBinding::new("shift-escape", ToggleMaximize, None),
        KeyBinding::new("secondary-shift-e", ExportResults, None),
        KeyBinding::new("secondary-alt-e", Explain, None),
        KeyBinding::new("shift-alt-f", FormatSql, None),
        KeyBinding::new(
            "alt-enter",
            gpui::base::input::ToggleCodeActions,
            Some("Input"),
        ),
        KeyBinding::new("shift-alt-f", FormatSql, Some("Input")),
        KeyBinding::new("secondary-shift-s", SaveFileAs, None),
        KeyBinding::new("secondary-o", OpenFile, None),
        KeyBinding::new("secondary-shift-t", ReopenClosedTab, None),
        KeyBinding::new("ctrl-tab", TabSwitcherNext, None),
        KeyBinding::new("ctrl-shift-tab", TabSwitcherPrev, None),
        KeyBinding::new("secondary-shift-p", CommandPalette, None),
        // Spaces (U5): one keystroke to switch connection
        KeyBinding::new("ctrl-1", SwitchSpace1, None),
        KeyBinding::new("ctrl-2", SwitchSpace2, None),
        KeyBinding::new("ctrl-3", SwitchSpace3, None),
        KeyBinding::new("ctrl-4", SwitchSpace4, None),
        KeyBinding::new("ctrl-5", SwitchSpace5, None),
        KeyBinding::new("ctrl-6", SwitchSpace6, None),
        KeyBinding::new("ctrl-7", SwitchSpace7, None),
        KeyBinding::new("ctrl-8", SwitchSpace8, None),
        KeyBinding::new("ctrl-9", SwitchSpace9, None),
        KeyBinding::new("secondary-\\", ToggleSplit, None),
        KeyBinding::new("secondary-alt-\\", ToggleSplitDirection, None),
        KeyBinding::new("secondary-alt-right", FocusNextPane, None),
        KeyBinding::new("secondary-alt-left", FocusPrevPane, None),
        KeyBinding::new("secondary-alt-down", FocusNextPane, None),
        KeyBinding::new("secondary-alt-up", FocusPrevPane, None),
        KeyBinding::new("secondary-alt-[", MovePaneLeft, None),
        KeyBinding::new("secondary-alt-]", MovePaneRight, None),
        KeyBinding::new("secondary-alt-shift-[", MovePaneUp, None),
        KeyBinding::new("secondary-alt-shift-]", MovePaneDown, None),
        // Font size / zoom shortcuts (⌘+/⌘- on macOS, Ctrl+/Ctrl- on others)
        KeyBinding::new("secondary-+", IncreaseFontSize, None),
        KeyBinding::new("secondary-=", IncreaseFontSize, None),
        KeyBinding::new("secondary-shift-=", IncreaseFontSize, None),
        KeyBinding::new("secondary-shift-+", IncreaseFontSize, None),
        KeyBinding::new("secondary-+", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-=", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-shift-=", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-shift-+", IncreaseFontSize, Some("Input")),
        KeyBinding::new("secondary--", DecreaseFontSize, None),
        KeyBinding::new("secondary--", DecreaseFontSize, Some("Input")),
        KeyBinding::new("secondary-0", ResetFontSize, None),
        KeyBinding::new("secondary-0", ResetFontSize, Some("Input")),
        // ⌘↵ on a command-bar result opens it in the split (U3); registered after the Input binding so it wins
        KeyBinding::new("secondary-enter", OpenInSplit, Some("CommandBar > Input")),
        KeyBinding::new("secondary-alt-shift-e", ExplainAnalyze, None),
        // In the command bar's input, up/down navigate the candidates rather than the text
        KeyBinding::new("up", MenuUp, Some("CommandBar > Input")),
        KeyBinding::new("down", MenuDown, Some("CommandBar > Input")),
    ]);
}

/// The macOS menu bar; its items also make the standard shortcuts (⌘Q, ⌘H, ⌘M…) work
pub fn set_app_menus(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    // Closing the last window quits, like other single-window macOS apps
    cx.on_window_closed(|cx, _| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    })
    .detach();
    cx.set_menus([
        Menu {
            name: "Quarry".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Hide Quarry", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Quarry", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Query", NewQuery),
                MenuItem::action("New Connection…", NewConnection),
                MenuItem::separator(),
                MenuItem::action("Open File…", OpenFile),
                MenuItem::action("Save", SaveFile),
                MenuItem::action("Save As…", SaveFileAs),
                MenuItem::separator(),
                MenuItem::action("Export Results…", ExportResults),
                MenuItem::separator(),
                MenuItem::action("Reopen Closed Tab", ReopenClosedTab),
                MenuItem::action("Close Tab", CloseTab),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
                MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
                MenuItem::separator(),
                MenuItem::action("Select Next Occurrence", SelectNextOccurrence),
                MenuItem::action("Format SQL", FormatSql),
            ],
            disabled: false,
        },
        Menu {
            name: "Query".into(),
            items: vec![
                MenuItem::action("Run Statement", RunQuery),
                MenuItem::action("Run All", RunAll),
                MenuItem::separator(),
                MenuItem::action("Explain Plan", Explain),
                MenuItem::action("Explain Analyze", ExplainAnalyze),
                MenuItem::separator(),
                MenuItem::action("Review Pending Changes", ReviewChanges),
                MenuItem::action("Manual Commit Mode", ToggleTxMode),
                MenuItem::action("Commit Transaction", CommitTx),
                MenuItem::action("Roll Back Transaction", RollbackTx),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette", ToggleCommandBar),
                MenuItem::separator(),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Move Sidebar to Left", MoveSidebarLeft),
                MenuItem::action("Move Sidebar to Right", MoveSidebarRight),
                MenuItem::separator(),
                MenuItem::action("Toggle Bottom Panel", ToggleBottomPanel),
                MenuItem::action("Move Bottom Panel to Bottom", MoveBottomPanelBottom),
                MenuItem::action("Move Bottom Panel to Right", MoveBottomPanelRight),
                MenuItem::separator(),
                MenuItem::action("Reset Layout", ResetLayout),
                MenuItem::separator(),
                MenuItem::action("Toggle Split View", ToggleSplit),
                MenuItem::action("Toggle Split Orientation", ToggleSplitDirection),
                MenuItem::action("Focus Other Split Pane", FocusNextPane),
                MenuItem::action("Swap Split Panes", SwapSplitPanes),
                MenuItem::action("Move Active Pane to Left", MovePaneLeft),
                MenuItem::action("Move Active Pane to Right", MovePaneRight),
                MenuItem::action("Move Active Pane to Top", MovePaneUp),
                MenuItem::action("Move Active Pane to Bottom", MovePaneDown),
                MenuItem::action("Maximize Editor", ToggleMaximize),
                MenuItem::separator(),
                MenuItem::action("Toggle Light/Dark Theme", ToggleTheme),
                MenuItem::separator(),
                MenuItem::action("Zoom In", IncreaseFontSize),
                MenuItem::action("Zoom Out", DecreaseFontSize),
                MenuItem::separator(),
                MenuItem::action("Actual Size", ResetFontSize),
            ],
            disabled: false,
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::action("Close Window", CloseWindow),
            ],
            disabled: false,
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("Report Issue on GitHub…", ReportIssue),
                MenuItem::action("View Last Crash Report…", ViewLastCrash),
            ],
            disabled: false,
        },
    ]);
}
