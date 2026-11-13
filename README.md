# nmi-notes

Fast note capture for Windows. Lives in the tray; global hotkeys move text
between apps, screens, and virtual desktops without leaving what you're doing.

| Hotkey | Command |
|---|---|
| `Win+Alt+N` | Paste clipboard to a **new** note (becomes Current) — toast only |
| `Win+Alt+A` | Paste clipboard **appended** to the Current note — toast only |
| `Win+Alt+S` | **Show** the Current note in an editor near the pointer |
| `Win+Alt+C` | **Copy** the Current note to the clipboard — toast only |
| `Win+Alt+L` | Note **list**: filter, open, set Current, delete |
| `Win+Alt+H` | **Help**: popup listing the hot keys (click or `Esc` closes it) |

Editor: `Esc`/`Ctrl+W` close (`Esc` returns to the list, filter and
selection intact, when the note was opened from it), `Ctrl+Shift+Enter` make
Current. Edits save automatically (~250 ms after you stop typing, and on focus loss/close).

List: type to filter, `↓`/`Tab` to the list, `Enter` open selected,
`Ctrl+Enter` make Current, `Del` delete, `Ctrl+Z` undo delete, `Esc` close.

**Protect:** tick **Protect** at the bottom of the editor and the note can't be
deleted (🔒 in the list). Untick it to allow deleting again.

**Groups:** named groups (unique, ignoring case) that you create as you go.
- **In the editor:** **Groups ▾** lists every group. Tick or untick one to add
  or remove this note, or choose **New group…**.
- **In the list:** right-click the selected notes (or press `Shift+F10`) for
  **Add to group**, **Remove from group** and **New group…**.
- **Browsing:** the dropdown left of the filter box picks what the list shows:
  **Ungrouped** (notes in no group), one group, or **All notes**. With a group
  chosen, the same menu can rename or delete it. Deleting a group never
  deletes its notes.
- **Where the list opens:** on Ungrouped at first, then on the last view you
  picked, remembered across restarts. After New Note it opens on Ungrouped with
  the new note selected. After Append it opens on the appended note's group
  with that note selected (your last-picked group if the note is in it). Adding
  an ungrouped Current note to a group moves the next list view to that group.
- **Keyboard:** `Tab` cycles filter → list → group dropdown.

## Build & run

```
cargo build --release
target\release\nmi-notes.exe
```

Installer (needs Inno Setup 6: `winget install JRSoftware.InnoSetup`):

```
build-installer.cmd
```

This does a release build, then writes `target\installer\nmi-notes-<version>-setup.exe`.
It's a per-user install to `%LOCALAPPDATA%\Programs\nmi-notes`, so no admin
prompt. It adds a Start menu shortcut and has an optional **Start nmi-notes when I sign in
to Windows** checkbox. The checkbox runs `nmi-notes.exe --autostart on|off`,
which sets `start_with_windows` in `config.toml` and updates the Run key.
Uninstalling closes the app and removes the Run key; your notes and config stay.

Scripting (from another tool, e.g. AutoHotkey or a Stream Deck):
`nmi-notes.exe --cmd new|append|show|copy|list|help`, `nmi-notes.exe --exit`,
`nmi-notes.exe --autostart on|off`.
`nmi-notes.exe --version` prints version and license (tray menu → About shows
the same).

## Files

- Notes: `%LOCALAPPDATA%\nmi-notes\notes.db` (SQLite, WAL)
- Daily backups (7 kept): `%LOCALAPPDATA%\nmi-notes\backups\`
- Log: `%LOCALAPPDATA%\nmi-notes\logs\nmi-notes.log`
- Config: `%APPDATA%\nmi-notes\config.toml` (hotkeys, font, start with Windows).
  The note font defaults to Fira Mono, which is built into the exe.

## Docs

- Design: `docs/superpowers/specs/2026-10-30-nmi-notes-design.md`
- Manual test checklist: `docs/manual-test.md`

## License

Copyright (C) 2026 Tillman O Dickson, Jr. <tillmanjr@gmail.com>

nmi-notes is free software: you can redistribute it and/or modify it under the
terms of the GNU Affero General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option) any
later version. See [LICENSE](LICENSE) for the full text.
(SPDX: `AGPL-3.0-or-later`)

The bundled Fira Mono font (`assets/fonts/FiraMono-Regular.ttf`, embedded in
the exe) is Copyright (c) 2012-2013 The Mozilla Corporation and Telefonica
S.A., licensed under the SIL Open Font License 1.1; see
[assets/fonts/OFL.txt](assets/fonts/OFL.txt).
