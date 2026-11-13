// SPDX-License-Identifier: AGPL-3.0-or-later

//! The flux store: sole owner of note state. Ops go in, `Change`s come out.
//! No I/O and no Win32 here; the app layer persists and routes changes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::text::{append_separator, utf16_len, utf16_to_byte};

pub type NoteId = i64;
pub type GroupId = i64;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Note {
    pub id: NoteId,
    /// Unix milliseconds UTC.
    pub created: i64,
    pub updated: i64,
    pub revision: u64,
    pub deleted_at: Option<i64>,
    pub text: String,
    /// Protected notes can't be deleted.
    pub protected: bool,
    pub groups: BTreeSet<GroupId>,
}

/// A named set of notes. Names are unique, ignoring case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Create { text: String },
    Append { id: NoteId, text: String },
    /// Replace `old_len` UTF-16 units at `start` with `text`.
    Edit { id: NoteId, base_rev: u64, start: usize, old_len: usize, text: String },
    SetCurrent { id: Option<NoteId> },
    /// Deletes the unprotected notes among `ids`; protected ones are kept.
    Delete { ids: Vec<NoteId> },
    Undelete { ids: Vec<NoteId> },
    SetProtected { id: NoteId, protected: bool },
    CreateGroup { name: String },
    RenameGroup { id: GroupId, name: String },
    /// Removes the group; its notes stay.
    DeleteGroup { id: GroupId },
    AddToGroup { ids: Vec<NoteId>, group: GroupId },
    RemoveFromGroup { ids: Vec<NoteId>, group: GroupId },
}

/// Which notes the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListView {
    /// Notes in no group.
    #[default]
    Ungrouped,
    Group(GroupId),
    All,
}

impl ListView {
    /// The `settings` value: `ungrouped`, `all` or `group:<id>`.
    pub fn to_setting(self) -> String {
        match self {
            ListView::Ungrouped => "ungrouped".into(),
            ListView::All => "all".into(),
            ListView::Group(id) => format!("group:{id}"),
        }
    }

    pub fn from_setting(s: &str) -> Option<ListView> {
        match s {
            "ungrouped" => Some(ListView::Ungrouped),
            "all" => Some(ListView::All),
            _ => s.strip_prefix("group:")?.parse().ok().map(ListView::Group),
        }
    }

    fn shows(self, n: &Note) -> bool {
        match self {
            ListView::Ungrouped => n.groups.is_empty(),
            ListView::Group(g) => n.groups.contains(&g),
            ListView::All => true,
        }
    }
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
    Protected { id: NoteId, protected: bool },
    /// Group memberships of these notes changed.
    Membership { ids: Vec<NoteId> },
    /// Group created or renamed.
    GroupSaved { id: GroupId },
    /// Group removed; `ids` were its members.
    GroupDeleted { id: GroupId, ids: Vec<NoteId> },
}

impl Change {
    /// Notes whose persisted record changed.
    pub fn touched(&self) -> Vec<NoteId> {
        match self {
            Change::Created { id } | Change::Text { id, .. } | Change::Protected { id, .. } => vec![*id],
            Change::Deleted { ids }
            | Change::Undeleted { ids }
            | Change::Membership { ids }
            | Change::GroupDeleted { ids, .. } => ids.clone(),
            Change::Current { .. } | Change::GroupSaved { .. } => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NoSuchNote(NoteId),
    StaleRevision { id: NoteId, expected: u64, got: u64 },
    OutOfRange,
    NoSuchGroup(GroupId),
    EmptyGroupName,
    /// Another group already has this name (ignoring case).
    GroupNameTaken(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NoSuchNote(id) => write!(f, "no such note {id}"),
            StoreError::StaleRevision { id, expected, got } => {
                write!(f, "stale edit on note {id}: at rev {expected}, edit based on {got}")
            }
            StoreError::OutOfRange => write!(f, "edit range out of bounds"),
            StoreError::NoSuchGroup(id) => write!(f, "no such group {id}"),
            StoreError::EmptyGroupName => write!(f, "group name is empty"),
            StoreError::GroupNameTaken(name) => write!(f, "a group named \u{201c}{name}\u{201d} already exists"),
        }
    }
}

pub struct Store {
    notes: HashMap<NoteId, Note>,
    groups: BTreeMap<GroupId, Group>,
    current: Option<NoteId>,
    next_id: NoteId,
    next_group_id: GroupId,
}

impl Store {
    pub fn new(notes: Vec<Note>, groups: Vec<Group>, current: Option<NoteId>) -> Self {
        let next_id = notes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
        let next_group_id = groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
        let groups: BTreeMap<_, _> = groups.into_iter().map(|g| (g.id, g)).collect();
        let notes: HashMap<_, _> = notes
            .into_iter()
            .map(|mut n| {
                n.groups.retain(|g| groups.contains_key(g));
                (n.id, n)
            })
            .collect();
        let current = current.filter(|id| notes.get(id).is_some_and(|n| n.deleted_at.is_none()));
        Store { notes, groups, current, next_id, next_group_id }
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.get(&id)
    }

    /// All groups, sorted by name (ignoring case).
    pub fn groups(&self) -> Vec<&Group> {
        let mut v: Vec<&Group> = self.groups.values().collect();
        v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        v
    }

    /// Names of a note's groups, sorted, comma-separated.
    pub fn group_names(&self, id: NoteId) -> String {
        let Some(n) = self.notes.get(&id) else { return String::new() };
        self.groups().iter().filter(|g| n.groups.contains(&g.id)).map(|g| g.name.as_str()).collect::<Vec<_>>().join(", ")
    }

    /// Validates a group name for `id` (None: a new group); returns it trimmed.
    pub fn check_group_name(&self, name: &str, id: Option<GroupId>) -> Result<String, StoreError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(StoreError::EmptyGroupName);
        }
        let lower = name.to_lowercase();
        if let Some(g) = self.groups.values().find(|g| Some(g.id) != id && g.name.to_lowercase() == lower) {
            return Err(StoreError::GroupNameTaken(g.name.clone()));
        }
        Ok(name.to_owned())
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

    /// `v`, or Ungrouped if it names a group that doesn't exist.
    pub fn valid_view(&self, v: ListView) -> ListView {
        match v {
            ListView::Group(g) if !self.groups.contains_key(&g) => ListView::Ungrouped,
            v => v,
        }
    }

    /// The view that shows note `id`: Ungrouped if it has no groups, else
    /// `last_picked` if it's a member, else its first group A–Z.
    pub fn view_for_note(&self, id: NoteId, last_picked: Option<GroupId>) -> ListView {
        let Some(n) = self.notes.get(&id).filter(|n| !n.groups.is_empty()) else { return ListView::Ungrouped };
        if let Some(g) = last_picked.filter(|g| n.groups.contains(g)) {
            return ListView::Group(g);
        }
        self.groups().iter().find(|g| n.groups.contains(&g.id)).map_or(ListView::Ungrouped, |g| ListView::Group(g.id))
    }

    /// Live notes matching `filter` (case-insensitive substring over the full
    /// text) and shown by `view`: current first, then by `updated` descending.
    pub fn list(&self, filter: &str, view: ListView) -> Vec<NoteId> {
        let needle = filter.trim().to_lowercase();
        let mut rows: Vec<&Note> = self
            .notes
            .values()
            .filter(|n| n.deleted_at.is_none())
            .filter(|n| view.shows(n))
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
                self.notes.insert(id, Note { id, created: now_ms, updated: now_ms, revision: 1, text, ..Default::default() });
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
                    if let Some(n) = self.notes.get_mut(&id).filter(|n| n.deleted_at.is_none() && !n.protected) {
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
            Op::SetProtected { id, protected } => {
                let note = self.live_mut(id)?;
                if note.protected == protected {
                    return Ok(vec![]);
                }
                note.protected = protected;
                Ok(vec![Change::Protected { id, protected }])
            }
            Op::CreateGroup { name } => {
                let name = self.check_group_name(&name, None)?;
                let id = self.next_group_id;
                self.next_group_id += 1;
                self.groups.insert(id, Group { id, name });
                Ok(vec![Change::GroupSaved { id }])
            }
            Op::RenameGroup { id, name } => {
                let name = self.check_group_name(&name, Some(id))?;
                let g = self.groups.get_mut(&id).ok_or(StoreError::NoSuchGroup(id))?;
                if g.name == name {
                    return Ok(vec![]);
                }
                g.name = name;
                Ok(vec![Change::GroupSaved { id }])
            }
            Op::DeleteGroup { id } => {
                self.groups.remove(&id).ok_or(StoreError::NoSuchGroup(id))?;
                let mut ids: Vec<NoteId> =
                    self.notes.values_mut().filter_map(|n| n.groups.remove(&id).then_some(n.id)).collect();
                ids.sort_unstable();
                Ok(vec![Change::GroupDeleted { id, ids }])
            }
            Op::AddToGroup { ids, group } => self.set_membership(ids, group, true),
            Op::RemoveFromGroup { ids, group } => self.set_membership(ids, group, false),
        }
    }

    fn set_membership(&mut self, ids: Vec<NoteId>, group: GroupId, member: bool) -> Result<Vec<Change>, StoreError> {
        if !self.groups.contains_key(&group) {
            return Err(StoreError::NoSuchGroup(group));
        }
        let mut done = Vec::new();
        for id in ids {
            if let Some(n) = self.notes.get_mut(&id).filter(|n| n.deleted_at.is_none()) {
                let changed = if member { n.groups.insert(group) } else { n.groups.remove(&group) };
                if changed {
                    done.push(id);
                }
            }
        }
        Ok(if done.is_empty() { vec![] } else { vec![Change::Membership { ids: done }] })
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
        let loaded = Note { id: 7, created: 0, updated: 0, revision: 3, deleted_at: None, text: "x".into(), ..Default::default() };
        let mut s = Store::new(vec![loaded], vec![], None);
        assert_eq!(create(&mut s, "a", 1), 8);
        assert_eq!(create(&mut s, "b", 2), 9);
        assert_eq!(s.note(8).unwrap().revision, 1);
    }

    #[test]
    fn append_applies_separator_and_bumps() {
        let mut s = Store::new(vec![], vec![], None);
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
        let mut s = Store::new(vec![], vec![], None);
        let id = create(&mut s, "a😀b", 1);
        s.dispatch(Op::Edit { id, base_rev: 1, start: 1, old_len: 2, text: "Z".into() }, 2).unwrap();
        assert_eq!(s.note(id).unwrap().text, "aZb");
        assert_eq!(s.note(id).unwrap().revision, 2);
    }

    #[test]
    fn edit_rejects_stale_and_out_of_range() {
        let mut s = Store::new(vec![], vec![], None);
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
        let mut s = Store::new(vec![], vec![], None);
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
        let mut s = Store::new(vec![], vec![], None);
        let a = create(&mut s, "a", 1);
        let b = create(&mut s, "b", 2);
        s.dispatch(Op::SetCurrent { id: Some(a) }, 3).unwrap();
        let ch = s.dispatch(Op::Delete { ids: vec![a] }, 4).unwrap();
        assert_eq!(
            ch,
            vec![Change::Current { old: Some(a), new: None }, Change::Deleted { ids: vec![a] }]
        );
        assert_eq!(s.current(), None);
        assert_eq!(s.list("", ListView::All), vec![b]);
        assert_eq!(s.dispatch(Op::Append { id: a, text: "x".into() }, 5), Err(StoreError::NoSuchNote(a)));
        s.dispatch(Op::Undelete { ids: vec![a] }, 6).unwrap();
        assert_eq!(s.list("", ListView::All), vec![b, a]);
    }

    #[test]
    fn list_pins_current_then_updated_desc() {
        let mut s = Store::new(vec![], vec![], None);
        let a = create(&mut s, "alpha", 1);
        let b = create(&mut s, "Beta", 2);
        let c = create(&mut s, "gamma beta", 3);
        assert_eq!(s.list("", ListView::All), vec![c, b, a]);
        s.dispatch(Op::SetCurrent { id: Some(a) }, 4).unwrap();
        assert_eq!(s.list("", ListView::All), vec![a, c, b]);
        s.dispatch(Op::Append { id: b, text: "more".into() }, 10).unwrap();
        assert_eq!(s.list("", ListView::All), vec![a, b, c]);
        assert_eq!(s.list("BETA", ListView::All), vec![b, c]);
    }

    #[test]
    fn load_drops_current_pointing_at_deleted() {
        let n = Note { id: 1, created: 0, updated: 0, revision: 1, deleted_at: Some(5), text: "".into(), ..Default::default() };
        assert_eq!(Store::new(vec![n], vec![], Some(1)).current(), None);
    }

    fn group(s: &mut Store, name: &str) -> GroupId {
        match s.dispatch(Op::CreateGroup { name: name.into() }, 0).unwrap()[..] {
            [Change::GroupSaved { id }] => id,
            ref other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn protected_notes_survive_delete() {
        let mut s = Store::new(vec![], vec![], None);
        let a = create(&mut s, "a", 1);
        let b = create(&mut s, "b", 2);
        assert_eq!(
            s.dispatch(Op::SetProtected { id: a, protected: true }, 3).unwrap(),
            vec![Change::Protected { id: a, protected: true }]
        );
        assert!(s.dispatch(Op::SetProtected { id: a, protected: true }, 3).unwrap().is_empty());
        let ch = s.dispatch(Op::Delete { ids: vec![a, b] }, 4).unwrap();
        assert_eq!(ch, vec![Change::Deleted { ids: vec![b] }]);
        assert!(s.dispatch(Op::Delete { ids: vec![a] }, 5).unwrap().is_empty());
        assert_eq!(s.list("", ListView::All), vec![a]);
        s.dispatch(Op::SetProtected { id: a, protected: false }, 6).unwrap();
        assert_eq!(s.dispatch(Op::Delete { ids: vec![a] }, 7).unwrap(), vec![Change::Deleted { ids: vec![a] }]);
    }

    #[test]
    fn group_names_are_unique_ignoring_case() {
        let mut s = Store::new(vec![], vec![], None);
        let work = group(&mut s, "  Work ");
        assert_eq!(s.group(work).unwrap().name, "Work");
        assert_eq!(s.dispatch(Op::CreateGroup { name: "work".into() }, 0), Err(StoreError::GroupNameTaken("Work".into())));
        assert_eq!(s.dispatch(Op::CreateGroup { name: "   ".into() }, 0), Err(StoreError::EmptyGroupName));
        let ideas = group(&mut s, "ideas");
        assert_eq!(
            s.dispatch(Op::RenameGroup { id: ideas, name: "WORK".into() }, 0),
            Err(StoreError::GroupNameTaken("Work".into()))
        );
        // Changing only the case of its own name is fine.
        s.dispatch(Op::RenameGroup { id: work, name: "work".into() }, 0).unwrap();
        assert_eq!(s.groups().iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), vec!["ideas", "work"]);
    }

    #[test]
    fn membership_filters_list_and_survives_group_delete_as_notes() {
        let mut s = Store::new(vec![], vec![], None);
        let a = create(&mut s, "alpha", 1);
        let b = create(&mut s, "beta", 2);
        let work = group(&mut s, "work");
        let ideas = group(&mut s, "ideas");
        assert_eq!(
            s.dispatch(Op::AddToGroup { ids: vec![a, b], group: work }, 3).unwrap(),
            vec![Change::Membership { ids: vec![a, b] }]
        );
        assert!(s.dispatch(Op::AddToGroup { ids: vec![a], group: work }, 3).unwrap().is_empty());
        s.dispatch(Op::AddToGroup { ids: vec![a], group: ideas }, 3).unwrap();
        assert_eq!(s.group_names(a), "ideas, work");
        assert_eq!(s.list("", ListView::Group(ideas)), vec![a]);
        assert_eq!(s.list("bet", ListView::Group(work)), vec![b]);
        assert_eq!(s.note(a).unwrap().updated, 1, "membership doesn't bump updated");

        s.dispatch(Op::RemoveFromGroup { ids: vec![a, b], group: ideas }, 4).unwrap();
        assert!(s.list("", ListView::Group(ideas)).is_empty());
        assert_eq!(s.dispatch(Op::DeleteGroup { id: work }, 5).unwrap(), vec![Change::GroupDeleted { id: work, ids: vec![a, b] }]);
        assert_eq!(s.list("", ListView::All).len(), 2);
        assert_eq!(s.group_names(a), "");
        assert_eq!(s.dispatch(Op::AddToGroup { ids: vec![a], group: work }, 6), Err(StoreError::NoSuchGroup(work)));
    }

    #[test]
    fn load_drops_memberships_of_unknown_groups() {
        let n = Note { id: 1, groups: [1, 9].into(), ..Default::default() };
        let s = Store::new(vec![n], vec![Group { id: 1, name: "g".into() }], None);
        assert_eq!(s.note(1).unwrap().groups, [1].into());
        let mut s = s;
        assert_eq!(group(&mut s, "h"), 2);
    }

    #[test]
    fn list_ungrouped_and_all() {
        let mut s = Store::new(vec![], vec![], None);
        let a = create(&mut s, "a", 1);
        let b = create(&mut s, "b", 2);
        let work = group(&mut s, "work");
        s.dispatch(Op::AddToGroup { ids: vec![a], group: work }, 3).unwrap();
        assert_eq!(s.list("", ListView::Ungrouped), vec![b]);
        assert_eq!(s.list("", ListView::Group(work)), vec![a]);
        assert_eq!(s.list("", ListView::All), vec![b, a]);
    }

    #[test]
    fn view_for_note_rules() {
        let mut s = Store::new(vec![], vec![], None);
        let a = create(&mut s, "a", 1);
        let work = group(&mut s, "work");
        let ideas = group(&mut s, "Ideas");
        assert_eq!(s.view_for_note(a, Some(work)), ListView::Ungrouped);
        assert_eq!(s.view_for_note(99, None), ListView::Ungrouped);
        s.dispatch(Op::AddToGroup { ids: vec![a], group: work }, 2).unwrap();
        s.dispatch(Op::AddToGroup { ids: vec![a], group: ideas }, 2).unwrap();
        assert_eq!(s.view_for_note(a, Some(work)), ListView::Group(work));
        assert_eq!(s.view_for_note(a, None), ListView::Group(ideas), "first A–Z, ignoring case");
        let other = group(&mut s, "other");
        assert_eq!(s.view_for_note(a, Some(other)), ListView::Group(ideas));
    }

    #[test]
    fn list_view_setting_roundtrip() {
        for v in [ListView::Ungrouped, ListView::All, ListView::Group(7)] {
            assert_eq!(ListView::from_setting(&v.to_setting()), Some(v));
        }
        for bad in ["", "group:", "group:x", "Group:1", "none"] {
            assert_eq!(ListView::from_setting(bad), None, "{bad}");
        }
    }

    #[test]
    fn valid_view_drops_missing_group() {
        let mut s = Store::new(vec![], vec![], None);
        let work = group(&mut s, "work");
        assert_eq!(s.valid_view(ListView::Group(work)), ListView::Group(work));
        assert_eq!(s.valid_view(ListView::Group(work + 1)), ListView::Ungrouped);
        assert_eq!(s.valid_view(ListView::All), ListView::All);
    }
}
