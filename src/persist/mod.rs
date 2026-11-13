// SPDX-License-Identifier: AGPL-3.0-or-later

//! SQLite persistence. The writer thread owns the only connection after
//! startup; `open` loads everything once on the UI thread.

mod schema;
mod writer;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::store::{Group, GroupId, ListView, Note, NoteId};

pub use writer::{Job, Status, Writer};

const CURRENT_KEY: &str = "current_note_id";
const LIST_VIEW_KEY: &str = "list_view";
const BACKUPS_KEPT: usize = 7;

pub struct Loaded {
    pub notes: Vec<Note>,
    pub groups: Vec<Group>,
    pub current: Option<NoteId>,
    /// The list's remembered view (None: missing or unreadable).
    pub list_view: Option<ListView>,
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
    let mut members: BTreeMap<NoteId, Vec<GroupId>> = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT note_id, group_id FROM note_groups")?;
    for row in stmt.query_map([], |r| Ok((r.get::<_, NoteId>(0)?, r.get::<_, GroupId>(1)?)))? {
        let (n, g) = row?;
        members.entry(n).or_default().push(g);
    }
    let mut stmt = conn.prepare("SELECT id, created, updated, revision, deleted_at, text, protected FROM notes")?;
    let notes = stmt
        .query_map([], |r| {
            let id: NoteId = r.get(0)?;
            Ok(Note {
                id,
                created: r.get(1)?,
                updated: r.get(2)?,
                revision: r.get::<_, i64>(3)? as u64,
                deleted_at: r.get(4)?,
                text: r.get(5)?,
                protected: r.get(6)?,
                groups: members.get(&id).into_iter().flatten().copied().collect(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare("SELECT id, name FROM groups")?;
    let groups = stmt
        .query_map([], |r| Ok(Group { id: r.get(0)?, name: r.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let current = setting(conn, CURRENT_KEY)?
        .and_then(|v| v.parse().ok());
    let list_view = setting(conn, LIST_VIEW_KEY)?.and_then(|v| ListView::from_setting(&v));
    Ok(Loaded { notes, groups, current, list_view })
}

fn setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
}

/// Writes a batch of group upserts (`Some`) and deletes (`None`), note
/// upserts (with their group memberships), and optional current-note and
/// list-view changes in one transaction.
pub fn write_batch<'a>(
    conn: &Connection,
    groups: &BTreeMap<GroupId, Option<Group>>,
    notes: impl IntoIterator<Item = &'a Note>,
    current: Option<Option<NoteId>>,
    list_view: Option<ListView>,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut up_group = tx.prepare_cached(
            "INSERT INTO groups (id, name) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
        )?;
        let mut del_group = tx.prepare_cached("DELETE FROM groups WHERE id = ?1")?;
        let mut del_group_members = tx.prepare_cached("DELETE FROM note_groups WHERE group_id = ?1")?;
        for (id, g) in groups {
            match g {
                Some(g) => {
                    up_group.execute(params![g.id, g.name])?;
                }
                None => {
                    del_group.execute([id])?;
                    del_group_members.execute([id])?;
                }
            }
        }
        let mut up = tx.prepare_cached(
            "INSERT INTO notes (id, created, updated, revision, deleted_at, text, protected)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               updated = excluded.updated, revision = excluded.revision,
               deleted_at = excluded.deleted_at, text = excluded.text,
               protected = excluded.protected",
        )?;
        let mut clear_members = tx.prepare_cached("DELETE FROM note_groups WHERE note_id = ?1")?;
        let mut add_member = tx.prepare_cached("INSERT INTO note_groups (note_id, group_id) VALUES (?1, ?2)")?;
        for n in notes {
            up.execute(params![n.id, n.created, n.updated, n.revision as i64, n.deleted_at, n.text, n.protected])?;
            clear_members.execute([n.id])?;
            for g in &n.groups {
                add_member.execute([n.id, *g])?;
            }
        }
    }
    let set = |key: &str, value: String| {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
    };
    match current {
        Some(Some(id)) => {
            set(CURRENT_KEY, id.to_string())?;
        }
        Some(None) => {
            tx.execute("DELETE FROM settings WHERE key = ?1", [CURRENT_KEY])?;
        }
        None => {}
    }
    if let Some(v) = list_view {
        set(LIST_VIEW_KEY, v.to_setting())?;
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
        Note { id, created: 1, updated: 2, revision: rev, deleted_at: None, text: text.into(), ..Default::default() }
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
            write_batch(&conn, &BTreeMap::new(), &[note(1, "a\nb", 1), note(2, "😀", 4)], Some(Some(2)), None).unwrap();
            write_batch(&conn, &BTreeMap::new(), &[note(1, "changed", 2)], None, None).unwrap();
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
            write_batch(&conn, &BTreeMap::new(), &[note(1, "a", 1)], Some(Some(1)), None).unwrap();
            write_batch(&conn, &BTreeMap::new(), std::iter::empty(), Some(None), None).unwrap();
        }
        assert_eq!(open(&path).unwrap().1.current, None);
    }

    #[test]
    fn groups_and_protected_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        let g = |id: GroupId, name: &str| (id, Some(Group { id, name: name.into() }));
        {
            let (conn, _) = open(&path).unwrap();
            let mut a = note(1, "a", 1);
            a.protected = true;
            a.groups = [1, 2].into();
            let mut b = note(2, "b", 1);
            b.groups = [2].into();
            write_batch(&conn, &[g(1, "work"), g(2, "ideas"), g(3, "gone")].into(), &[a, b], None, None).unwrap();
            // Rename 1, delete 2 (its memberships go too), drop note 1 from nothing else.
            write_batch(&conn, &[g(1, "Work"), (2, None), (3, None)].into(), std::iter::empty(), None, None).unwrap();
        }
        let (_c, loaded) = open(&path).unwrap();
        assert_eq!(loaded.groups, vec![Group { id: 1, name: "Work".into() }]);
        let mut notes = loaded.notes;
        notes.sort_by_key(|n| n.id);
        assert!(notes[0].protected);
        assert_eq!(notes[0].groups, [1].into());
        assert!(!notes[1].protected);
        assert!(notes[1].groups.is_empty());
    }

    #[test]
    fn migrates_v1_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(schema::MIGRATIONS_FOR_TEST[0]).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute("INSERT INTO notes VALUES (5, 1, 2, 3, NULL, 'old')", []).unwrap();
        }
        let (conn, loaded) = open(&path).unwrap();
        let v: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, schema::VERSION);
        assert_eq!(loaded.notes, vec![Note { id: 5, created: 1, updated: 2, revision: 3, text: "old".into(), ..Default::default() }]);
        assert!(path.with_extension("pre-v2.db").exists(), "snapshot taken before upgrading");
    }

    #[test]
    fn backup_creates_once_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let (conn, _) = open(&dir.path().join("notes.db")).unwrap();
        write_batch(&conn, &BTreeMap::new(), &[note(1, "a", 1)], None, None).unwrap();
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

    #[test]
    fn list_view_roundtrips_and_bad_values_load_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        assert_eq!(open(&path).unwrap().1.list_view, None);
        for v in [ListView::All, ListView::Group(3), ListView::Ungrouped] {
            {
                let (conn, _) = open(&path).unwrap();
                write_batch(&conn, &BTreeMap::new(), std::iter::empty(), None, Some(v)).unwrap();
            }
            assert_eq!(open(&path).unwrap().1.list_view, Some(v));
        }
        {
            let (conn, _) = open(&path).unwrap();
            conn.execute("UPDATE settings SET value = 'group:x' WHERE key = ?1", [LIST_VIEW_KEY]).unwrap();
        }
        assert_eq!(open(&path).unwrap().1.list_view, None);
    }
}
