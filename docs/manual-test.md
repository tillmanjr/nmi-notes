# Manual Test Checklist

Run before each release with a release build (`cargo build --release`).
Automated coverage: `cargo test` (model, persistence, config) and
`target\release\nmi-notes.exe --selftest` (real RichEdit ↔ store offsets,
caret/selection preservation; add `--clipboard` to include a clipboard
round-trip, which briefly overwrites the clipboard).

## Capture (no focus change)

- [ ] Copy text in any app → `Win+Alt+N`: toast bottom-center of the
      pointer's monitor, "New note — …". The app you were in keeps focus and
      caret.
- [ ] Copy more text → `Win+Alt+A`: toast "Appended to: …". Note gains a
      line break + the text (no blank line if the note already ended with one).
- [ ] Copy files in Explorer → `Win+Alt+N`: note contains one full path per
      line.
- [ ] Copy an image only → `Win+Alt+N`: "Nothing to paste".
- [ ] `Win+Alt+C`: toast "Copied — N chars"; pasting elsewhere gives CRLF text.
- [ ] Toast never steals focus and is click-through; a second toast replaces
      the first.

## Windows

- [ ] `Win+Alt+S`: editor appears ~200 px left of the pointer (right of it
      near the left edge), vertically centered, inside the monitor.
- [ ] Press again on the same desktop: same window is reused and moved.
- [ ] Switch desktop, press again: a new window opens there; the window on
      the first desktop stays.
- [ ] Type in one window; the other window on another desktop shows the edit
      after ~250 ms.
- [ ] With the editor open and caret mid-text, `Win+Alt+A` from another app:
      text appears at the end, caret/selection/scroll unchanged. With caret at
      the end, the view follows the new text.
- [ ] `Esc` / `Ctrl+W` close; `Ctrl+Shift+Enter` makes the note Current
      (★ in caption and status).
- [ ] Open a note from a filtered list: status shows "Esc: back to list".
      `Esc` closes it and the list returns with the same filter and that note
      selected (focus in the rows). `Ctrl+W` / ✕ just close.
- [ ] `Win+Alt+H`: hot-key popup centered on the pointer's monitor, listing
      the configured bindings. `Esc`, a click on it, or a click elsewhere
      closes it.
- [ ] `Win+Alt+L`: list near pointer, filter focused, Current pinned top in
      bold with ★ and highlight; others newest first.
- [ ] Type in filter: rows filter live (matches anywhere in note text).
      `↓`/`Tab` moves to the list.
- [ ] Multi-select + `Enter`: each opens in its own window (cascaded), list
      closes. `Ctrl+Enter`: focused note becomes Current, list closes.
- [ ] `Del` deletes selected (toast), open editors for them close;
      `Ctrl+Z` in the list restores them.
- [ ] Hover a row: tooltip shows the absolute update time.

## Protect and groups

- [ ] Editor: click **Protect** (box or label): it ticks, 🔒 appears in the
      list, and focus returns to the note text. `Del` in the list on it toasts
      "Protected — not deleted". With a mix selected, only the unprotected
      notes are deleted and the toast says how many were kept.
- [ ] List: select notes, right-click → **Add to group → New group…**, enter a
      name: the group is created, the notes join it, the Groups column shows
      it, and focus is back on the rows.
- [ ] Reusing a name with different case (`work` vs `Work`) keeps the prompt
      open with "A group named “Work” already exists". `Esc` cancels.
- [ ] Group dropdown: choosing a group limits the rows and the title reads
      "Notes in “…” (n of N)". The text filter still works within it.
- [ ] Editor **Groups ▾**: memberships are ticked; ticking or unticking one
      updates the button and the list.
- [ ] With a group chosen in the list, the menu can **Rename** it (name
      updates everywhere) or **Delete** it (its notes stay; dropdown resets to
      All notes).
- [ ] Restart the app: protection, groups and memberships are still there.
- [ ] Open the list from an editor with `Esc` and the group filter is the
      same as when the note was opened.

## Look

- [ ] Editor, list, title bars, scrollbars, tray menu and tooltips are dark.
- [ ] Editor text is Fira Mono (`0O 1lI` clearly distinct) inside a darker
      note area with lighter padding and a thin edge; status strip below it.
- [ ] List header text is readable; Current row is gold and bold.

## Robustness

- [ ] Restart Explorer (`taskkill /f /im explorer.exe && start explorer`):
      tray icon returns.
- [ ] Type in an editor and immediately sign out: the edit is present after
      signing back in.
- [ ] Launch the exe a second time: the Note List opens in the running
      instance; no second tray icon.
- [ ] Move an editor to a monitor with different scaling: fonts rescale.
- [ ] Set a hotkey in `config.toml` to one already in use: startup toast
      names it.
- [ ] Logs show `window visible in N ms` under 50 ms for Show/List.
