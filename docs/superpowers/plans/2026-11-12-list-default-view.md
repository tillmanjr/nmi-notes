# List Default View Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The note list opens on a calculated *Ungrouped* view by default, or on the view implied by the last New Note / Append / group assignment, remembered across restarts.

**Architecture:** A `ListView` enum in the pure store model replaces `Option<GroupId>` as the list filter. The app keeps one remembered `list_view` (persisted in the `settings` table via the writer thread) plus a session-only `last_picked` group, and updates them centrally in `App::apply` / `command` / a new `ListGroup` action.

**Tech Stack:** Rust, `windows` crate (Win32), rusqlite.

Spec: `docs/superpowers/specs/2026-11-12-list-default-view-design.md`

## Global Constraints

- Dropdown order: **Ungrouped**, groups A–Z (ignoring case), **All notes**.
- Setting key `list_view`, values `ungrouped` | `all` | `group:<id>`; missing/garbage/deleted group → Ungrouped. No schema migration.
- Titles: `Ungrouped notes (n of N)`, `Notes in “Work” (n of N)`, `All notes (N)` / `All notes (n of N)` when text-filtered; all suffixed ` — nmi-notes`.
- Verify with `cargo test` and `cargo clippy --all-targets -- -D warnings`.

---

### Task 1: `ListView` in the store

**Files:** Modify `src/model/store.rs`

**Produces:**
- `pub enum ListView { #[default] Ungrouped, Group(GroupId), All }` (`Copy, Eq, Debug, Default`)
- `ListView::to_setting(self) -> String`, `ListView::from_setting(&str) -> Option<ListView>`
- `Store::list(&self, filter: &str, view: ListView) -> Vec<NoteId>` (was `Option<GroupId>`)
- `Store::valid_view(&self, v: ListView) -> ListView` — `Group` of a missing group → `Ungrouped`
- `Store::view_for_note(&self, id: NoteId, last_picked: Option<GroupId>) -> ListView` — no groups → `Ungrouped`; in `last_picked` → that; else first of the note's groups A–Z

- [ ] Write tests: `list_ungrouped_and_all`, `view_for_note_rules`, `list_view_setting_roundtrip`, `valid_view_drops_missing_group`; update existing tests' `list(.., None)` → `ListView::All`, `Some(g)` → `ListView::Group(g)`.
- [ ] `cargo test --lib store` → fails to compile / fails.
- [ ] Implement; `cargo test --lib store` → pass.

### Task 2: Persist `list_view`

**Files:** Modify `src/persist/mod.rs`, `src/persist/writer.rs`

**Produces:**
- `Loaded.list_view: Option<ListView>` (parsed; `None` if missing or unparsable)
- `write_batch(conn, groups, notes, current, list_view: Option<ListView>)`
- `Job::SetListView(ListView)`, coalesced in `Pending.list_view`

- [ ] Test `list_view_roundtrips_and_bad_values_load_as_none` (all three forms; garbage value → `None`). Update existing `write_batch` calls with a trailing `None`.
- [ ] Implement; `cargo test --lib persist` → pass.

### Task 3: List window dropdown

**Files:** Modify `src/win/listwin.rs`, `src/app.rs` (Action enum)

**Produces:**
- `listwin::view(h) -> ListView` (replaces `group_filter`), `listwin::select_view(h, ListView)` (replaces `select_group`)
- Dropdown indices: 0 = Ungrouped, 1..=n groups, n+1 = All notes; `set_groups` keeps the selected view, falling back to Ungrouped.
- `Action::ListGroup(isize)` posted on `CBN_SELCHANGE` (user picks only); `EN_CHANGE` keeps posting `ListFilter`.

### Task 4: App wiring

**Files:** Modify `src/app.rs`, `src/main.rs`

- Fields: `list_view: ListView`, `last_picked: Option<GroupId>`, `list_select: Option<NoteId>` (one-shot), and `list_group` renamed `back_view: ListView`.
- `App::new(.., list_view: ListView)`; `main.rs` passes `store.valid_view(loaded.list_view.unwrap_or_default())`. `last_picked` starts as the view's group.
- `fn set_list_view(&mut self, v)` sends `Job::SetListView` when it changes.
- `create_current` (New Note, Append with no Current) → `Ungrouped` + `list_select`.
- Append → `view_for_note(id, last_picked)` + `list_select`.
- `apply`: record whether Current was ungrouped before dispatch; on `Change::Membership` containing Current that is now grouped → `view_for_note(cur, None)`. On `Change::GroupDeleted { id }` → reset `list_view`, `last_picked`, `back_view` that point at it.
- `Action::ListGroup(k)` → refresh, `set_list_view(view(h))`, set `last_picked` for a group.
- `show_list_with(None)`: apply `list_view` to an existing visible list (keep filter) or a new one; take `list_select` and `select_only` it.
- `refresh_list`: new titles; `list_menu` uses `ListView::Group` for rename/delete.

- [ ] `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` clean.
- [ ] Commit.

### Task 5: Docs

**Files:** Modify `README.md`, `docs/manual-test.md`

- README groups section: list opens on Ungrouped / last context; dropdown order.
- manual-test: "Default view" section from the spec's manual checklist.
- [ ] Commit.
