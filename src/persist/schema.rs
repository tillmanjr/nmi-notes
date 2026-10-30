//! Schema versioning via `PRAGMA user_version`.

use std::path::Path;

use rusqlite::Connection;

pub const VERSION: i32 = 1;

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
];

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
