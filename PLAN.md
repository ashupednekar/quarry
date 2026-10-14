# Plan: movable panes and better inline suggestions

## 1. Movable panes

### Today
`main.rs` hard-codes one tree:

```
h_resizable("main-split")
├── sidebar                      (left, 160–480px, toggled by ⌘B)
└── v_resizable("content-split")
    ├── tab strip + editor/grid
    └── bottom panel             (toggled by ⌘J)
```

Sizes persist per group name in `layout.json` (`layout.rs`). Panel *positions* are not data, so nothing can move.

### Goal
The user can put the sidebar on the left or right, put the bottom panel at the bottom or on the right, and swap which pane sits beside the editor. They do it by dragging a pane header or from a menu, and it survives restarts.

### Model
One small struct, saved next to the sizes in `layout.json`:

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
enum Side { Left, Right, Bottom }

struct Dock {
    sidebar: Side,      // Left | Right
    bottom_panel: Side, // Bottom | Right
}
```

That's two enums, not a general docking tree. The app has 2 movable panes; a tree is only worth it if a third arrives.

Sizes key on the **pane**, not on the group (`"sidebar"`, `"bottom_panel"`), so a pane keeps its width when it moves.

### Rendering
`render` builds the tree from `Dock` instead of the literal above:

- `sidebar: Right`: same `h_resizable`, with the children in reverse order. `size_range` and `.visible()` are unchanged.
- `bottom_panel: Right`: the content area becomes an `h_resizable` instead of a `v_resizable`.
- The sidebar's `border_r_1` becomes a border on whichever edge faces the editor.

### Interaction
1. **Menu first.** Each pane header gets a right-click menu: *Move to Left / Right / Bottom*. Add View menu items plus palette commands *Move Sidebar to Right* and *Reset Layout*. This is keyboard-accessible, and it's the escape hatch if a drag goes wrong.
2. **Drag second.** Pane headers become drag sources (gpui `on_drag` with a `DraggedPane(Pane)` payload). While dragging, a full-window overlay shows the valid drop zones for that pane as translucent accent rectangles on the left, right and bottom edges (~25% of the window). Hovering a zone highlights it and dropping updates `Dock`. Esc or dropping outside a zone cancels.
3. Animate the zone highlight only; panes snap. Respect reduced motion.

### Phases
| Phase | Scope | Status |
|---|---|---|
| 1 | `Dock.sidebar` Left/Right, the menu items, Reset Layout, persistence | Complete |
| 2 | `Dock.bottom_panel` Bottom/Right | Complete |
| 3 | Drag-and-drop with drop-zone overlay & split pane swapping | Complete |
| 4 | Drag a tab into split pane / across split panes | Complete |

### Risks
- **gpui `ResizableState` remembers sizes by index.** Reversing the children swaps which size applies to which pane. Fix: give the group a different id per arrangement (`"main-split:sidebar-right"`) so each layout keeps its own sizes.
- **Maximize (⇧⎋)** saves and restores `sidebar_visible` and `bottom_panel_open` only; it doesn't change `Dock`, so it needs no changes.
- **Window controls.** With the sidebar on the right, the traffic lights still sit over the titlebar, not the sidebar, so the titlebar padding stays as is.

### Done when
- The sidebar moves to the right and back from the View menu and the palette, and stays there after a restart.
- Sizes are kept per arrangement.
- ⌘B and ⌘J still toggle correctly in every arrangement.

---

## 2. Better inline suggestions (ghost text)

### Today
`SchemaCompleter::complete` (`quarry-sql/src/completer.rs`) is already clause-aware: tables after FROM/JOIN, columns of in-scope tables, `alias.` columns, and whole `JOIN … ON …` clauses from foreign keys. Results are ranked by `match_score`: prefix, then substring, then letters in order.

The **inline** provider (`completion.rs::inline_completion`) throws most of that away:
- It takes only the top candidate, and shows it only when the label **starts with** the typed prefix. Fuzzy hits and FK join clauses never become ghost text.
- It triggers only while a word is being typed, never after a space (`JOIN ␣`, `WHERE ␣`, `= ␣`).
- Ties keep catalog order, with no sense of what this user actually uses.
- It fires on keywords, so it's noisy (typing `se` ghosts `lect`, which the user was about to type anyway).

### Changes, in priority order
1. **Ghost after a clause keyword plus space.**
   - After `JOIN ` (with exactly one FK candidate, or one clearly best), ghost the whole `t ON t.a = x.b` clause the dropdown already builds.
   - After `ON ` for a join that already names its table, ghost the condition.
   - This is the most valuable change, and the logic already exists.
2. **Confidence gate.** Ghost only when the best candidate is clearly ahead: it's the only prefix match, or it scores strictly better than the runner-up. If unsure, show nothing. Ghost text that's wrong is worse than none.
3. **Skip bare keywords.** Don't ghost single SQL keywords, which the dropdown and muscle memory already cover. Keep ghosting identifiers and multi-word clauses.
4. **Usage boost.** Count table and column names from `state.history` (this connection's past queries) and from the open buffer, then use the count as a tiebreaker after `match_score`. Keep it as an in-memory `HashMap<String, u32>` built when the catalog loads; no new storage.
5. **Statement endings.**
   - After `SELECT * FROM t `, ghost ` LIMIT 100` when the table is big (we have row estimates) and there's no WHERE yet.
   - After `UPDATE t SET … `, with no WHERE, ghost `WHERE ` as a nudge.
6. **Case from the catalog.** Ghost the identifier as the catalog spells it when it's quoted or mixed-case; Oracle stores names UPPERCASE. Only keywords should follow the user's typing case. Today the suffix always copies the typed case, which is wrong for `"MixedCase"` names.
7. **Accept a word at a time.** Alt-→ accepts the next word of a long ghost (a JOIN clause) instead of all or nothing. Check whether gpui-kit's inline API supports a partial accept; if not, skip this.

### Where the code goes
- Items 1–3 and 6: `completion.rs::inline_completion`. It already calls `SchemaCompleter::complete`; it needs the candidate list, not just `.next()`.
- Item 4: a `usage: &HashMap<…>` argument on `complete()`, used in its `sort_by_key`. This improves the dropdown too.
- Item 5: a new `SchemaCompleter::continuation(stmt, at, catalog) -> Option<String>` in `quarry-sql`.

### Tests
Add cases next to the existing ones in `completer.rs`:
- `JOIN ` with one FK ghosts the clause.
- Two equally good candidates ghost nothing.
- A keyword prefix ghosts nothing.
- A history-heavy column wins a tie.
- An Oracle UPPERCASE name keeps its case.

The criterion benches in `benches/` must stay flat on the 1,000-table catalog.

### Phases
| Phase | Items | Size |
|---|---|---|
| 1 | 1, 2, 3 (FK clause ghost, confidence gate, no keyword ghosts) | ~1 day |
| 2 | 4, 6 (usage ranking, catalog case) | ~½ day |
| 3 | 5, 7 (continuations, word-by-word accept) | ~1 day |

### Later, not now
An optional AI source behind a setting, consulted only when the schema completer has nothing confident. It waits for the MCP and agent work.
