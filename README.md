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

Editor: `Esc`/`Ctrl+W` close (`Esc` returns to the list, filter and
selection intact, when the note was opened from it), `Ctrl+Shift+Enter` make
Current. Edits save automatically (~250 ms after you stop typing, and on focus loss/close).

List: type to filter, `↓`/`Tab` to the list, `Enter` open selected,
`Ctrl+Enter` make Current, `Del` delete, `Ctrl+Z` undo delete, `Esc` close.

## Build & run

```
cargo build --release
target\release\nmi-notes.exe
```

Scripting (from another tool, e.g. AutoHotkey or a Stream Deck):
`nmi-notes.exe --cmd new|append|show|copy|list`, `nmi-notes.exe --exit`.
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
