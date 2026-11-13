# List default view — design

Date: 2026-11-12 · Branch: `feature/list-default-view`

## Problem

The note list opens on **All notes**. As groups accumulate, mixing every note
in one list gets unwieldy. The list should open on a focused view: notes with
no group by default, and otherwise the view that fits what the user just did.

## Views

The list's group filter becomes

```rust
enum ListView { Ungrouped, Group(GroupId), All }
```

replacing today's `Option<GroupId>` (where `None` meant All notes).

- **Ungrouped** — calculated: notes with no group memberships.
- **Group(id)** — notes in that group (as today).
- **All** — every note (as today's "All notes").

Dropdown order: **Ungrouped**, the groups A–Z (ignoring case), **All notes**.

The pinned Current row follows the same filter as other rows (unchanged).

Window title: `Ungrouped notes (n of N)`, `Notes in “Work” (n of N)`,
`All notes (N)`.

## The remembered view

The app keeps two values:

- `list_view: ListView` — what the list opens on. Persisted.
- `last_picked: Option<GroupId>` — the group the user last chose by hand in
  the dropdown. Session only; initialised from `list_view` at startup (its
  group, if it is one).

`list_view` is stored in the existing `settings` table under key `list_view`
as `ungrouped`, `all`, or `group:<id>`. No schema migration. A missing or
unparsable value, or a `group:<id>` whose group no longer exists, loads as
**Ungrouped**.

### What updates `list_view`

| Event | New `list_view` |
|---|---|
| User picks a dropdown entry | That entry; if it is a group, also `last_picked = id` |
| New Note creates a note (incl. Append with no Current) | `Ungrouped` |
| Append to the Current note, note has no groups | `Ungrouped` |
| Append, note is in `last_picked` | `Group(last_picked)` |
| Append, note in other groups | `Group(first of the note's groups A–Z)` |
| The Current note goes from no groups to one or more groups (editor **Groups ▾**, list context menu, or **New group…**) | `Group(that group)`; if several were added at once, the first A–Z |
| The group in `list_view` is deleted | `Ungrouped` |
| Show, Copy, Help, opening/editing notes from the list | no change |
| Other membership changes (non-Current notes, Current already grouped, removals) | no change |

New Note / Append that create or change nothing (empty or busy clipboard)
change nothing. Every change to `list_view` is written through the existing
writer thread (same path as `current_note_id`).

Changing `list_view` because of the membership rule does not change the
group shown by an already-open list; it only affects the next open.

## Opening the list

- **Hotkey / tray / `--cmd list`, list not open on this desktop:** open on
  `list_view` with an empty filter. If the most recent update to `list_view`
  came from New Note or Append, select that note (focus stays in the filter
  box, as today). The selection hint is cleared once used.
- **Same, list already open on this desktop:** switch its group to
  `list_view` if different, keep the text filter, bring it forward.
- **Esc back from an editor:** unchanged — restores the group and filter the
  note was opened from, and selects it.

## Code touch points

- `model/store.rs` — `ListView`; listing filter takes `ListView`
  (`Ungrouped` = `groups.is_empty()`); helper choosing the Append view
  (`fn append_view(note, last_picked) -> ListView`).
- `persist/mod.rs` — load `list_view` into `Loaded`; write it in
  `write_batch` (or a sibling setting write).
- `win/listwin.rs` — dropdown entries/order, `group_filter`/`select_group`
  take `ListView`, title text.
- `app.rs` — `list_group` → `list_view` + `last_picked` + selection hint;
  update on the events above; `show_list_with` applies it.

## Testing

Unit:
- Store: Ungrouped filter; `append_view` for no groups, in `last_picked`,
  several groups without `last_picked`.
- Persist: `list_view` round-trips for all three forms; missing, garbage,
  and deleted-group values load as Ungrouped.

Manual (`docs/manual-test.md`, new "Default view" section):
- Fresh install: list opens on Ungrouped; dropdown order is Ungrouped,
  groups, All notes.
- New Note → list opens on Ungrouped with the new note selected.
- Append to a note in Work → list opens on Work with it selected; with the
  note in Work and Ideas and Ideas last picked → Ideas.
- Add the ungrouped Current note to a group → next list open shows that
  group.
- Pick a group, restart the app → list opens on that group.
- Delete the shown group → Ungrouped.
- Esc from an editor still returns to the group it was opened from.
