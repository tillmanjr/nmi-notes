// SPDX-License-Identifier: AGPL-3.0-or-later

//! Schema versioning via `PRAGMA user_version`.

use std::path::Path;

use rusqlite::Connection;

pub const VERSION: i32 = 2;

const MIGRATIONS: &[&str] = &[
    // 0 -> 1
    "CREATE TABLE notes (
        id         INTEGER PRIMARY KEY,
        created    INTEGER NOT NULL,
        updated    INTEGER NOT NULL,
        revision   INTEGER NOT NULL,
        deleted_at INTEGER NULL,
        text       TEXT    NOT NULL
     );
     CREATE INDEX notes_updated ON notes(updated DESC);
     CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
     );",
    // 1 -> 2: protected notes, groups. Group-name uniqueness is enforced by
    // the store, not here, so coalesced renames (a↔b) can't fail a batch.
    "ALTER TABLE notes ADD COLUMN protected INTEGER NOT NULL DEFAULT 0;
     CREATE TABLE groups (
        id   INTEGER PRIMARY KEY,
        name TEXT    NOT NULL
     );
     CREATE TABLE note_groups (
        note_id  INTEGER NOT NULL,
        group_id INTEGER NOT NULL,
        PRIMARY KEY (note_id, group_id)
     ) WITHOUT ROWID;
     CREATE INDEX note_groups_group ON note_groups(group_id);",
];

#[cfg(test)]
pub const MIGRATIONS_FOR_TEST: &[&str] = MIGRATIONS;

/// Brings the database to `VERSION`. Takes a `VACUUM INTO` snapshot next to
/// the database before upgrading an existing (non-empty) schema.
pub fn migrate(conn: &Connection, db_path: &Path) -> rusqlite::Result<()> {
    let current: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current >= VERSION {
        return Ok(());
    }
    if current > 0 {
        let snapshot = db_path.with_extension(format!("pre-v{}.db", VERSION));
        if !snapshot.exists() {
            conn.execute("VACUUM INTO ?1", [snapshot.to_string_lossy()])?;
        }
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i32)?;
        tx.commit()?;
    }
    Ok(())
}
