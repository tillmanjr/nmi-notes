//! SQLite persistence. The writer thread owns the only connection after
//! startup; `open` loads everything once on the UI thread.

mod schema;
mod writer;

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::store::{Note, NoteId};

pub use writer::{Job, Status, Writer};

const CURRENT_KEY: &str = "current_note_id";
const BACKUPS_KEPT: usize = 7;

pub struct Loaded {
    pub notes: Vec<Note>,
    pub current: Option<NoteId>,
}

/// Opens (creating if needed) the database, migrates it, and loads all notes.
pub fn open(path: &Path) -> rusqlite::Result<(Connection, Loaded)> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let conn = Connection::open(path)?;
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    schema::migrate(&conn, path)?;
    let loaded = load(&conn)?;
    Ok((conn, loaded))
}

fn load(conn: &Connection) -> rusqlite::Result<Loaded> {
    let mut stmt = conn.prepare("SELECT id, created, updated, revision, deleted_at, text FROM notes")?;
    let notes = stmt
        .query_map([], |r| {
            Ok(Note {
                id: r.get(0)?,
                created: r.get(1)?,
                updated: r.get(2)?,
                revision: r.get::<_, i64>(3)? as u64,
                deleted_at: r.get(4)?,
                text: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let current = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [CURRENT_KEY], |r| r.get::<_, String>(0))
        .optional()?
        .and_then(|v| v.parse().ok());
    Ok(Loaded { notes, current })
}

/// Writes a batch of note upserts and an optional current-note change in one
/// transaction.
pub fn write_batch<'a>(
    conn: &Connection,
    notes: impl IntoIterator<Item = &'a Note>,
    current: Option<Option<NoteId>>,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut up = tx.prepare_cached(
            "INSERT INTO notes (id, created, updated, revision, deleted_at, text)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               updated = excluded.updated, revision = excluded.revision,
               deleted_at = excluded.deleted_at, text = excluded.text",
        )?;
        for n in notes {
            up.execute(params![n.id, n.created, n.updated, n.revision as i64, n.deleted_at, n.text])?;
        }
    }
    match current {
        Some(Some(id)) => {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![CURRENT_KEY, id.to_string()],
            )?;
        }
        Some(None) => {
            tx.execute("DELETE FROM settings WHERE key = ?1", [CURRENT_KEY])?;
        }
        None => {}
    }
    tx.commit()
}

/// Writes `notes-<date>.db` into `dir` unless it exists, then prunes to the
/// newest `BACKUPS_KEPT`.
pub fn backup(conn: &Connection, dir: &Path, date: &str) -> std::io::Result<Option<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join(format!("notes-{date}.db"));
    let made = if target.exists() {
        None
    } else {
        conn.execute("VACUUM INTO ?1", [target.to_string_lossy()])
            .map_err(std::io::Error::other)?;
        Some(target)
    };
    let mut existing: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("notes-") && n.ends_with(".db"))
        })
        .collect();
    existing.sort();
    let excess = existing.len().saturating_sub(BACKUPS_KEPT);
    for old in &existing[..excess] {
        let _ = std::fs::remove_file(old);
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: NoteId, text: &str, rev: u64) -> Note {
        Note { id, created: 1, updated: 2, revision: rev, deleted_at: None, text: text.into() }
    }

    #[test]
    fn open_creates_schema_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("notes.db");
        {
            let (conn, loaded) = open(&path).unwrap();
            assert!(loaded.notes.is_empty());
            assert_eq!(loaded.current, None);
            let v: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, schema::VERSION);
            write_batch(&conn, &[note(1, "a\nb", 1), note(2, "😀", 4)], Some(Some(2))).unwrap();
            write_batch(&conn, &[note(1, "changed", 2)], None).unwrap();
        }
        let (_c, loaded) = open(&path).unwrap();
        let mut notes = loaded.notes;
        notes.sort_by_key(|n| n.id);
        assert_eq!(notes, vec![note(1, "changed", 2), note(2, "😀", 4)]);
        assert_eq!(loaded.current, Some(2));
    }

    #[test]
    fn clearing_current_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        {
            let (conn, _) = open(&path).unwrap();
            write_batch(&conn, &[note(1, "a", 1)], Some(Some(1))).unwrap();
            write_batch(&conn, std::iter::empty(), Some(None)).unwrap();
        }
        assert_eq!(open(&path).unwrap().1.current, None);
    }

    #[test]
    fn backup_creates_once_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let (conn, _) = open(&dir.path().join("notes.db")).unwrap();
        write_batch(&conn, &[note(1, "a", 1)], None).unwrap();
        let bdir = dir.path().join("backups");
        for day in 1..=9 {
            let date = format!("2026-10-{day:02}");
            assert!(backup(&conn, &bdir, &date).unwrap().is_some());
        }
        assert!(backup(&conn, &bdir, "2026-10-09").unwrap().is_none());
        let mut names: Vec<String> = std::fs::read_dir(&bdir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), 7);
        assert_eq!(names[0], "notes-2026-10-03.db");
        let (_c, restored) = open(&bdir.join("notes-2026-10-09.db")).unwrap();
        assert_eq!(restored.notes.len(), 1);
    }
}
