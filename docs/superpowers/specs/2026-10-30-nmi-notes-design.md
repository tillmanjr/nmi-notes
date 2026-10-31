# nmi-notes — Design (v1)

Date: 2026-10-30
Status: Approved

## Purpose

Capture notes — tiny or large, fleeting or permanent — from anywhere in Windows
with near-zero friction. Primary use: move information from an app on one
screen/virtual desktop to an app on another. The user works on a 49" ultrawide
across ~8 virtual desktops and switches projects constantly.

Speed of invocation is the overriding requirement.

## Scope (v1)

In: tray-resident app, five global hotkey commands, SQLite storage, editor
windows, note list/picker, soft delete, toasts.

Out (later): titles, note lifetimes/expiry, export to files, images/rich
content, cross-machine sync. (Dark mode was added 2026-10-31; see
Implementation Notes.)

## Platform & Toolkit

- Rust, stable MSVC toolchain, Windows 10/11 only.
- Native Win32 via the `windows` crate. RichEdit (`msftedit.dll`, plain-text
  mode) for editing, ListView for the list.
- `rusqlite` (bundled SQLite) for storage.
- Per-monitor DPI v2 aware (manifest).

Rationale: the hard requirements — instant appearance, a toast that never
takes focus, exact placement, virtual-desktop awareness, a robust editor for
large text — are all native OS strengths. Store/ops layer is toolkit-agnostic
so the UI could be replaced later.

## Architecture

### Process

- Single instance via named mutex. A second launch signals the running
  instance to open the Note List and exits.
- Lives in the system tray. Optional start-with-Windows (HKCU Run key) — v1
  provides it as a config flag.

### Threads

- **UI thread**: one Win32 message loop. Owns tray, hotkeys, toast, all
  windows, and the Store.
- **Writer thread**: owns the only SQLite connection. Receives persistence
  jobs via a channel; UI never blocks on disk.

### Store (flux core, no Win32)

- Holds all notes in memory, loaded eagerly at startup (id, created, updated,
  revision, deleted_at, text). Cached first-line preview.
- `dispatch(op)` applies the op in memory, bumps `revision` and `updated`,
  notifies subscribers synchronously, enqueues a persistence job.
- Ops:
  - `CreateNote { text }` → new id
  - `Append { id, text }`
  - `Edit { id, base_rev, range, text }` (range in UTF-16 code units; see
    Text Normalization)
  - `SetCurrent { id }`
  - `Delete { ids }` (soft) / `Undelete { ids }`
- Subscribers: per-note (editor windows) and global (list window).
- Current note id is persisted (settings table).

### Flush-before-dispatch invariant

Because everything runs on one thread, before the app dispatches any op that
targets note X from a non-editor source, it asks every editor showing X to
flush pending local edits first. Consequently, an editor only ever receives
ops based on text it has already sent; no rebasing is needed. `base_rev` is
checked as an assertion: a mismatch is logged and resolved by the editor
reloading full text from the store.

### Persistence

- DB at `%LOCALAPPDATA%\nmi-notes\notes.db`, WAL mode, `synchronous=NORMAL`.
- Schema:
  - `notes(id INTEGER PRIMARY KEY, created INTEGER, updated INTEGER,
    revision INTEGER, deleted_at INTEGER NULL, text TEXT)` — times are Unix
    milliseconds UTC.
  - `settings(key TEXT PRIMARY KEY, value TEXT)` — holds `current_note_id`.
- Writer coalesces bursts: drains all queued jobs, collapses to the latest
  state per note, writes in one transaction. Full note text is written on
  change.
- Versioning via `PRAGMA user_version`; sequential migrations; file copy
  backup before any migration.
- Daily `VACUUM INTO` backup to `%LOCALAPPDATA%\nmi-notes\backups\`, keep 7.
- Clean exit flushes the writer queue before returning.

## Text Normalization

- Store holds `\n` line endings only.
- Clipboard in: CRLF/CR → `\n`. Clipboard out: `\n` → CRLF.
- RichEdit uses `\r` paragraph markers; the editor converts at its boundary.
- Offsets in `Edit` ops are counted in UTF-16 code units of the
  `\n`-normalized text (RichEdit's native unit; with `\r`↔`\n` being 1:1,
  offsets line up directly).

## Hotkeys & Commands

Bindings in `%APPDATA%\nmi-notes\config.toml`, registered via
`RegisterHotKey`. Registration failures are reported (toast + log), never
silent.

| # | Command | Default | Takes focus |
|---|---|---|---|
| 1 | Paste → New Note | `Win+Alt+N` | no (toast) |
| 2 | Paste → Append to Current | `Win+Alt+A` | no (toast) |
| 3 | Show Current Note | `Win+Alt+S` | yes |
| 4 | Copy Current Note | `Win+Alt+C` | no (toast) |
| 5 | Note List | `Win+Alt+L` | yes |

Behavior:

1. **New from clipboard.** Read clipboard (retry ~100 ms if locked). Accept
   `CF_UNICODETEXT` as-is; else `CF_HDROP` → one full path per line. Text
   wins if both present. Empty/unsupported → toast "Nothing to paste".
   Otherwise `CreateNote` + `SetCurrent`; toast "New note — ‹first line›
   (N lines, M chars)".
2. **Append.** Separator: insert `\n` before the pasted text only if the note
   is non-empty and does not already end with `\n`. No Current → creates a
   new note and makes it Current; toast "New note (no current) — …".
   Otherwise toast "Appended to: ‹first line›".
3. **Show Current.** If a window for the Current note exists on the current
   virtual desktop, focus and reposition it; else open a new one there. No
   Current → toast "No current note".
4. **Copy Current.** Place CRLF text on clipboard; toast "Copied — N chars".
   Does not modify `updated`. No Current → toast "No current note".
5. **Note List.** Reuse list window on current desktop, else create one there.

Clipboard busy after retries → toast "Clipboard busy".

Tray: right-click menu with the five commands, "Open config", "Open data
folder", "Exit". Double-click opens Note List. Re-add icon on
`TaskbarCreated`.

## Placement

- **Toast**: bottom-center of the work area of the monitor under the mouse
  pointer, topmost, `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`,
  shown with `SW_SHOWNOACTIVATE`, auto-hides after ~1.5 s with fade.
  A new toast replaces the current one.
- **Windows (editor, list)**: on the monitor under the pointer. Window's right
  edge placed 200 px (DPI-scaled) left of the pointer, vertically centered
  on the pointer. If that doesn't fit, left edge 200 px right of the pointer.
  Always clamped to the monitor work area. Placement is a pure function:
  `place(pointer, work_area, size, offset) -> Rect`.
- Editor size defaults ~700×500 (DPI-scaled); last-used size remembered.
  Multiple windows opened at once from the list cascade (+24 px each).

## Editor Window

- Plain-text RichEdit, raised text limit, font from config (default Fira Mono,
  embedded in the exe; fallback Consolas).
- Caption: first line of note (+ " ★" if Current). Status strip: `★ Current`
  when applicable, updated time, sync state.
- Keys: `Esc` / `Ctrl+W` close (flush first); `Ctrl+Shift+Enter` make
  Current. Select/copy/cut/paste/undo native.
- Local edits: `EN_CHANGE` restarts a 250 ms timer. On timer / focus loss /
  close: read text, diff against last-synced text (common prefix/suffix) →
  one `Edit` op.
- Incoming ops: applied as range replacement; caret/selection offsets after
  the change point are shifted; scroll position preserved, except if the
  caret was at the very end, in which case the view follows (tail).
- Known v1 limitation: an incoming remote edit may clear that window's undo
  history.
- One hidden editor window is pre-created and reused for instant Show.
- Multiple windows for the same note are allowed (e.g., on different
  desktops); all stay in sync via the Store.

## Note List Window

- Filter edit box (focused on open) above a ListView (report mode, multi
  select). Columns: ★, First line, Updated (relative; absolute on hover via
  tooltip), Size.
- Current note pinned in the top row, bold, followed by all others sorted by
  `updated` descending. Deleted notes hidden.
- Filter: live, case-insensitive substring over full note text.
- `Enter` / double-click: open selected notes in their own windows, then
  close list. `Ctrl+Enter`: make focused note Current, close list. `Del`:
  soft-delete selected, toast "Deleted N — Ctrl+Z in list to undo".
  `Ctrl+Z` in the list undeletes the most recent delete batch. `Esc`: close.
- Live updates via global subscription; selection tracked by note id.

## Error Handling

- Never silently lose a note.
- Writer failure → retry with backoff; in-memory store remains authoritative;
  tray icon tooltip shows warning. On exit with unflushed jobs → write
  `recovery-<timestamp>.json` beside the DB and notify.
- DB open failure → error dialog at startup; app does not run with an empty
  store.
- `WM_QUERYENDSESSION` / `WM_ENDSESSION` → flush editors and writer
  synchronously.
- Panic hook → log + bounded flush attempt via writer thread.
- Logs: `%LOCALAPPDATA%\nmi-notes\logs\`.
- Virtual desktop API failure → fall back to reusing any existing window.

## Testing

- `model`: unit tests for every op, append-separator rule, revisions/updated,
  notification order, soft delete/undelete, Copy not touching `updated`,
  flush-before-dispatch; `proptest` for diff round-trip and caret/selection
  adjustment.
- Pure helpers: clipboard conversion (HDROP → lines, line-ending
  normalization, text-over-files precedence); placement (left preferred,
  right fallback, vertical clamp, narrow monitors).
- `persist`: real temp SQLite — round-trip, coalescing, migrations, crash
  mid-queue consistency.
- Win32 layer: manual checklist in `docs/manual-test.md`.
- Latency instrumentation: log hotkey→toast-visible and hotkey→window-shown.
  Budgets: toast < 30 ms, Show < 50 ms.

## Code Layout

Single crate `nmi-notes`:

- `src/model/` — model, ops, store, diff, text normalization, placement math.
- `src/persist/` — schema, migrations, writer thread, backups.
- `src/win/` — tray, hotkeys, clipboard, toast, vdesktop, editor, list,
  window placement glue.
- `src/app.rs` — wires commands to store and UI.
- `src/config.rs` — TOML config load/defaults.
- `src/main.rs` — startup, single instance, message loop.

## Implementation Notes (v1 build, 2026-10-30)

Decisions made during the build that refine the design above:

- **Current note in the list** is bold, starred, and has a pale-yellow row
  highlight instead of a separator line.
- **Editor size** is remembered for the session only (not persisted).
- **Pre-warmed spares**: one hidden editor *and* one hidden list window are
  created at startup and after each use, shown once while DWM-cloaked to pay
  first-show costs up front.
- **Show then activate**: windows are shown with `SWP_NOACTIVATE` first, then
  activated. Measured (release, this machine): on screen in ~9–19 ms from the
  hotkey; activation (`SetForegroundWindow`) adds ~30–50 ms waiting on the
  previously active app, which is outside our process. The 50 ms budget is
  applied to "on screen".
- **Remote control**: `--cmd new|append|show|copy|list` and `--exit` forward
  to the running instance (scripting, and used for smoke tests).
- **`--selftest`** exercises the real RichEdit against the store (offsets,
  `\r`↔`\n`, caret/selection/tail behavior).
- **Font fallback**: the configured face is checked with `GetTextFaceW`;
  Consolas is used if it isn't installed.

### Dark mode (2026-10-31)

- Always dark (no light theme). Palette and helpers live in `src/win/theme.rs`.
- Title bars: `DWMWA_USE_IMMERSIVE_DARK_MODE`. Scrollbars, tray menu, list
  header and tooltips: uxtheme `SetPreferredAppMode(ForceDark)` (ordinal 135)
  plus `DarkMode_Explorer` / `DarkMode_CFD` / `DarkMode_ItemsView` visual
  styles. The list header text is recolored via a ListView subclass (the dark
  header theme leaves it dark).
- Editor: note area (`#1B1C1F`) sits inside 12 px of lighter padding
  (`#2C2E33`) with a 1 px edge (`#454951`); text inset 10 px horizontally and
  8 px from the top; the status strip lives in the bottom padding.
- Note font: Fira Mono Regular (SIL OFL 1.1, `assets/fonts/`), registered
  process-private with `AddFontMemResourceEx`; `font` in config.toml still
  overrides it.
