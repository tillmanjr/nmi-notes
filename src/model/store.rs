//! The flux store: sole owner of note state. Ops go in, `Change`s come out.
//! No I/O and no Win32 here; the app layer persists and routes changes.

use std::collections::HashMap;

use super::text::{append_separator, utf16_len, utf16_to_byte};

pub type NoteId = i64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub id: NoteId,
    /// Unix milliseconds UTC.
    pub created: i64,
    pub updated: i64,
    pub revision: u64,
    pub deleted_at: Option<i64>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Create { text: String },
    Append { id: NoteId, text: String },
    /// Replace `old_len` UTF-16 units at `start` with `text`.
    Edit { id: NoteId, base_rev: u64, start: usize, old_len: usize, text: String },
    SetCurrent { id: Option<NoteId> },
    Delete { ids: Vec<NoteId> },
    Undelete { ids: Vec<NoteId> },
}

/// A text change in UTF-16 units, suitable for replaying into an editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub old_len: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Created { id: NoteId },
    Text { id: NoteId, rev: u64, splice: Splice },
    Current { old: Option<NoteId>, new: Option<NoteId> },
    Deleted { ids: Vec<NoteId> },
    Undeleted { ids: Vec<NoteId> },
}

impl Change {
    /// Notes whose persisted record changed.
    pub fn touched(&self) -> Vec<NoteId> {
        match self {
            Change::Created { id } | Change::Text { id, .. } => vec![*id],
            Change::Deleted { ids } | Change::Undeleted { ids } => ids.clone(),
            Change::Current { .. } => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NoSuchNote(NoteId),
    StaleRevision { id: NoteId, expected: u64, got: u64 },
    OutOfRange,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NoSuchNote(id) => write!(f, "no such note {id}"),
            StoreError::StaleRevision { id, expected, got } => {
                write!(f, "stale edit on note {id}: at rev {expected}, edit based on {got}")
            }
            StoreError::OutOfRange => write!(f, "edit range out of bounds"),
        }
    }
}

pub struct Store {
    notes: HashMap<NoteId, Note>,
    current: Option<NoteId>,
    next_id: NoteId,
}

impl Store {
    pub fn new(notes: Vec<Note>, current: Option<NoteId>) -> Self {
        let next_id = notes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        let notes: HashMap<_, _> = notes.into_iter().map(|n| (n.id, n)).collect();
        let current = current.filter(|id| notes.get(id).is_some_and(|n| n.deleted_at.is_none()));
        Store { notes, current, next_id }
    }

    pub fn note(&self, id: NoteId) -> Option<&Note> {
        self.notes.get(&id)
    }

    /// The live (non-deleted) note with this id.
    pub fn live(&self, id: NoteId) -> Option<&Note> {
        self.notes.get(&id).filter(|n| n.deleted_at.is_none())
    }

    pub fn current(&self) -> Option<NoteId> {
        self.current
    }

    /// Live notes matching `filter` (case-insensitive substring over the full
    /// text): current first, then by `updated` descending.
    pub fn list(&self, filter: &str) -> Vec<NoteId> {
        let needle = filter.trim().to_lowercase();
        let mut rows: Vec<&Note> = self
            .notes
            .values()
            .filter(|n| n.deleted_at.is_none())
            .filter(|n| needle.is_empty() || n.text.to_lowercase().contains(&needle))
            .collect();
        rows.sort_by(|a, b| {
            let ac = Some(a.id) == self.current;
            let bc = Some(b.id) == self.current;
            bc.cmp(&ac).then(b.updated.cmp(&a.updated)).then(b.id.cmp(&a.id))
        });
        rows.into_iter().map(|n| n.id).collect()
    }

    pub fn dispatch(&mut self, op: Op, now_ms: i64) -> Result<Vec<Change>, StoreError> {
        match op {
            Op::Create { text } => {
                let id = self.next_id;
                self.next_id += 1;
                self.notes.insert(
                    id,
                    Note { id, created: now_ms, updated: now_ms, revision: 1, deleted_at: None, text },
                );
                Ok(vec![Change::Created { id }])
            }
            Op::Append { id, text } => {
                let note = self.live_mut(id)?;
                let start = utf16_len(&note.text);
                let mut inserted = append_separator(&note.text).to_owned();
                inserted.push_str(&text);
                note.text.push_str(&inserted);
                note.revision += 1;
                note.updated = now_ms;
                let splice = Splice { start, old_len: 0, text: inserted };
                Ok(vec![Change::Text { id, rev: note.revision, splice }])
            }
            Op::Edit { id, base_rev, start, old_len, text } => {
                let note = self.live_mut(id)?;
                if note.revision != base_rev {
                    return Err(StoreError::StaleRevision { id, expected: note.revision, got: base_rev });
                }
                let b0 = utf16_to_byte(&note.text, start).ok_or(StoreError::OutOfRange)?;
                let b1 = utf16_to_byte(&note.text, start + old_len).ok_or(StoreError::OutOfRange)?;
                note.text.replace_range(b0..b1, &text);
                note.revision += 1;
                note.updated = now_ms;
                let splice = Splice { start, old_len, text };
                Ok(vec![Change::Text { id, rev: note.revision, splice }])
            }
            Op::SetCurrent { id } => {
                if let Some(id) = id {
                    self.live_mut(id)?;
                }
                let old = self.current;
                if old == id {
                    return Ok(vec![]);
                }
                self.current = id;
                Ok(vec![Change::Current { old, new: id }])
            }
            Op::Delete { ids } => {
                let mut done = Vec::new();
                for id in ids {
                    if let Some(n) = self.notes.get_mut(&id).filter(|n| n.deleted_at.is_none()) {
                        n.deleted_at = Some(now_ms);
                        done.push(id);
                    }
                }
                let mut changes = Vec::new();
                if let Some(cur) = self.current.filter(|c| done.contains(c)) {
                    self.current = None;
                    changes.push(Change::Current { old: Some(cur), new: None });
                }
                if !done.is_empty() {
                    changes.push(Change::Deleted { ids: done });
                }
                Ok(changes)
            }
            Op::Undelete { ids } => {
                let mut done = Vec::new();
                for id in ids {
                    if let Some(n) = self.notes.get_mut(&id).filter(|n| n.deleted_at.is_some()) {
                        n.deleted_at = None;
                        done.push(id);
                    }
                }
                Ok(if done.is_empty() { vec![] } else { vec![Change::Undeleted { ids: done }] })
            }
        }
    }

    fn live_mut(&mut self, id: NoteId) -> Result<&mut Note, StoreError> {
        self.notes
            .get_mut(&id)
            .filter(|n| n.deleted_at.is_none())
            .ok_or(StoreError::NoSuchNote(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(s: &mut Store, text: &str, now: i64) -> NoteId {
        match s.dispatch(Op::Create { text: text.into() }, now).unwrap()[..] {
            [Change::Created { id }] => id,
            ref other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn create_assigns_increasing_ids_after_loaded() {
        let loaded = Note { id: 7, created: 0, updated: 0, revision: 3, deleted_at: None, text: "x".into() };
        let mut s = Store::new(vec![loaded], None);
        assert_eq!(create(&mut s, "a", 1), 8);
        assert_eq!(create(&mut s, "b", 2), 9);
        assert_eq!(s.note(8).unwrap().revision, 1);
    }

    #[test]
    fn append_applies_separator_and_bumps() {
        let mut s = Store::new(vec![], None);
        let id = create(&mut s, "first", 1);
        let ch = s.dispatch(Op::Append { id, text: "second".into() }, 5).unwrap();
        assert_eq!(
            ch,
            vec![Change::Text { id, rev: 2, splice: Splice { start: 5, old_len: 0, text: "\nsecond".into() } }]
        );
        let n = s.note(id).unwrap();
        assert_eq!(n.text, "first\nsecond");
        assert_eq!(n.updated, 5);

        let id2 = create(&mut s, "", 6);
        s.dispatch(Op::Append { id: id2, text: "x".into() }, 7).unwrap();
        assert_eq!(s.note(id2).unwrap().text, "x");
    }

    #[test]
    fn edit_applies_utf16_range() {
        let mut s = Store::new(vec![], None);
        let id = create(&mut s, "a😀b", 1);
        s.dispatch(Op::Edit { id, base_rev: 1, start: 1, old_len: 2, text: "Z".into() }, 2).unwrap();
        assert_eq!(s.note(id).unwrap().text, "aZb");
        assert_eq!(s.note(id).unwrap().revision, 2);
    }

    #[test]
    fn edit_rejects_stale_and_out_of_range() {
        let mut s = Store::new(vec![], None);
        let id = create(&mut s, "abc", 1);
        assert_eq!(
            s.dispatch(Op::Edit { id, base_rev: 0, start: 0, old_len: 0, text: "x".into() }, 2),
            Err(StoreError::StaleRevision { id, expected: 1, got: 0 })
        );
        assert_eq!(
            s.dispatch(Op::Edit { id, base_rev: 1, start: 2, old_len: 5, text: "x".into() }, 2),
            Err(StoreError::OutOfRange)
        );
        assert_eq!(s.note(id).unwrap().text, "abc");
    }

    #[test]
    fn set_current_and_noop() {
        let mut s = Store::new(vec![], None);
        let id = create(&mut s, "a", 1);
        assert_eq!(
            s.dispatch(Op::SetCurrent { id: Some(id) }, 2).unwrap(),
            vec![Change::Current { old: None, new: Some(id) }]
        );
        assert!(s.dispatch(Op::SetCurrent { id: Some(id) }, 3).unwrap().is_empty());
        assert_eq!(s.dispatch(Op::SetCurrent { id: Some(99) }, 3), Err(StoreError::NoSuchNote(99)));
    }

    #[test]
    fn delete_hides_and_clears_current_undelete_restores() {
        let mut s = Store::new(vec![], None);
        let a = create(&mut s, "a", 1);
        let b = create(&mut s, "b", 2);
        s.dispatch(Op::SetCurrent { id: Some(a) }, 3).unwrap();
        let ch = s.dispatch(Op::Delete { ids: vec![a] }, 4).unwrap();
        assert_eq!(
            ch,
            vec![Change::Current { old: Some(a), new: None }, Change::Deleted { ids: vec![a] }]
        );
        assert_eq!(s.current(), None);
        assert_eq!(s.list(""), vec![b]);
        assert_eq!(s.dispatch(Op::Append { id: a, text: "x".into() }, 5), Err(StoreError::NoSuchNote(a)));
        s.dispatch(Op::Undelete { ids: vec![a] }, 6).unwrap();
        assert_eq!(s.list(""), vec![b, a]);
    }

    #[test]
    fn list_pins_current_then_updated_desc() {
        let mut s = Store::new(vec![], None);
        let a = create(&mut s, "alpha", 1);
        let b = create(&mut s, "Beta", 2);
        let c = create(&mut s, "gamma beta", 3);
        assert_eq!(s.list(""), vec![c, b, a]);
        s.dispatch(Op::SetCurrent { id: Some(a) }, 4).unwrap();
        assert_eq!(s.list(""), vec![a, c, b]);
        s.dispatch(Op::Append { id: b, text: "more".into() }, 10).unwrap();
        assert_eq!(s.list(""), vec![a, b, c]);
        assert_eq!(s.list("BETA"), vec![b, c]);
    }

    #[test]
    fn load_drops_current_pointing_at_deleted() {
        let n = Note { id: 1, created: 0, updated: 0, revision: 1, deleted_at: Some(5), text: "".into() };
        assert_eq!(Store::new(vec![n], Some(1)).current(), None);
    }
}
