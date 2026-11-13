// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writer thread: coalesces jobs, writes them in one transaction, retries on
//! failure, and dumps unwritten notes to a recovery file if shut down while
//! failing.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::Serialize;

use crate::model::store::{Group, GroupId, ListView, Note, NoteId};

pub enum Job {
    Upsert(Note),
    UpsertGroup(Group),
    DeleteGroup(GroupId),
    SetCurrent(Option<NoteId>),
    SetListView(ListView),
    Backup { dir: PathBuf, date: String },
    /// Acknowledged once everything queued before it is committed.
    Flush(Sender<()>),
    Shutdown(Sender<()>),
}

#[derive(Debug, Clone)]
pub enum Status {
    Healthy,
    Failing(String),
    /// Shut down with unwritten data; it was saved to this file instead.
    Recovered(PathBuf),
    RecoveryFailed(String),
}

pub struct Writer {
    tx: Sender<Job>,
    handle: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn spawn(conn: Connection, db_path: PathBuf, on_status: Box<dyn Fn(Status) + Send>) -> Self {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("nmi-writer".into())
            .spawn(move || run(conn, db_path, rx, on_status))
            .expect("spawn writer thread");
        Writer { tx, handle: Some(handle) }
    }

    /// A sender usable from other threads (e.g. the panic hook).
    pub fn sender(&self) -> Sender<Job> {
        self.tx.clone()
    }

    pub fn send(&self, job: Job) {
        let _ = self.tx.send(job);
    }

    /// Blocks until queued work is committed or `timeout` passes.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (ack, done) = mpsc::channel();
        self.send(Job::Flush(ack));
        done.recv_timeout(timeout).is_ok()
    }

    /// Flushes, stops the thread, and joins it (bounded by `timeout`).
    pub fn shutdown(&mut self, timeout: Duration) -> bool {
        let (ack, done) = mpsc::channel();
        self.send(Job::Shutdown(ack));
        let ok = done.recv_timeout(timeout).is_ok();
        if ok {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
        ok
    }
}

#[derive(Default)]
struct Pending {
    notes: BTreeMap<NoteId, Note>,
    /// `None`: delete.
    groups: BTreeMap<GroupId, Option<Group>>,
    current: Option<Option<NoteId>>,
    list_view: Option<ListView>,
    backups: Vec<(PathBuf, String)>,
    acks: Vec<Sender<()>>,
}

impl Pending {
    fn has_writes(&self) -> bool {
        !self.notes.is_empty() || !self.groups.is_empty() || self.current.is_some() || self.list_view.is_some()
    }

    /// Returns true if this was a shutdown request.
    fn absorb(&mut self, job: Job) -> bool {
        match job {
            Job::Upsert(n) => {
                self.notes.insert(n.id, n);
            }
            Job::UpsertGroup(g) => {
                self.groups.insert(g.id, Some(g));
            }
            Job::DeleteGroup(id) => {
                self.groups.insert(id, None);
            }
            Job::SetCurrent(c) => self.current = Some(c),
            Job::SetListView(v) => self.list_view = Some(v),
            Job::Backup { dir, date } => self.backups.push((dir, date)),
            Job::Flush(ack) => self.acks.push(ack),
            Job::Shutdown(ack) => {
                self.acks.push(ack);
                return true;
            }
        }
        false
    }
}

const MAX_BACKOFF: Duration = Duration::from_secs(5);
const SHUTDOWN_ATTEMPTS: u32 = 3;

fn run(conn: Connection, db_path: PathBuf, rx: Receiver<Job>, on_status: Box<dyn Fn(Status) + Send>) {
    let mut p = Pending::default();
    let mut shutdown = false;
    let mut failures: u32 = 0;
    loop {
        // Wait for work: block when idle, back off when failing.
        if !shutdown {
            let first = if failures == 0 {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                let backoff = (Duration::from_millis(100) * 2u32.pow(failures.min(6))).min(MAX_BACKOFF);
                rx.recv_timeout(backoff)
            };
            match first {
                Ok(job) => shutdown |= p.absorb(job),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => shutdown = true,
            }
            while let Ok(job) = rx.try_recv() {
                shutdown |= p.absorb(job);
            }
        }

        if p.has_writes() {
            match super::write_batch(&conn, &p.groups, p.notes.values(), p.current, p.list_view) {
                Ok(()) => {
                    p.notes.clear();
                    p.groups.clear();
                    p.current = None;
                    p.list_view = None;
                    if failures > 0 {
                        on_status(Status::Healthy);
                    }
                    failures = 0;
                }
                Err(e) => {
                    failures += 1;
                    on_status(Status::Failing(e.to_string()));
                    if shutdown && failures >= SHUTDOWN_ATTEMPTS {
                        on_status(write_recovery(&db_path, &p));
                        p.notes.clear();
                        p.groups.clear();
                        p.current = None;
                        p.list_view = None;
                    } else {
                        if shutdown {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        continue;
                    }
                }
            }
        }

        for (dir, date) in p.backups.drain(..) {
            if let Err(e) = super::backup(&conn, &dir, &date) {
                on_status(Status::Failing(format!("backup failed: {e}")));
            }
        }
        for ack in p.acks.drain(..) {
            let _ = ack.send(());
        }
        if shutdown {
            return;
        }
    }
}

#[derive(Serialize)]
struct RecoveryNote<'a> {
    id: NoteId,
    created: i64,
    updated: i64,
    revision: u64,
    deleted_at: Option<i64>,
    text: &'a str,
    protected: bool,
    groups: Vec<GroupId>,
}

#[derive(Serialize)]
struct RecoveryGroup<'a> {
    id: GroupId,
    /// `None`: the group was deleted.
    name: Option<&'a str>,
}

#[derive(Serialize)]
struct Recovery<'a> {
    current: Option<Option<NoteId>>,
    groups: Vec<RecoveryGroup<'a>>,
    notes: Vec<RecoveryNote<'a>>,
}

fn write_recovery(db_path: &std::path::Path, p: &Pending) -> Status {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let path = db_path.with_file_name(format!("recovery-{ms}.json"));
    let doc = Recovery {
        current: p.current,
        groups: p.groups.iter().map(|(id, g)| RecoveryGroup { id: *id, name: g.as_ref().map(|g| g.name.as_str()) }).collect(),
        notes: p
            .notes
            .values()
            .map(|n| RecoveryNote {
                id: n.id,
                created: n.created,
                updated: n.updated,
                revision: n.revision,
                deleted_at: n.deleted_at,
                text: &n.text,
                protected: n.protected,
                groups: n.groups.iter().copied().collect(),
            })
            .collect(),
    };
    match serde_json::to_vec_pretty(&doc).map_err(std::io::Error::other).and_then(|b| std::fs::write(&path, b)) {
        Ok(()) => Status::Recovered(path),
        Err(e) => Status::RecoveryFailed(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn note(id: NoteId, text: &str, rev: u64) -> Note {
        Note { id, created: 1, updated: rev as i64, revision: rev, deleted_at: None, text: text.into(), ..Default::default() }
    }

    #[test]
    fn coalesces_and_persists_latest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        let (conn, _) = super::super::open(&path).unwrap();
        let mut w = Writer::spawn(conn, path.clone(), Box::new(|_| {}));
        for rev in 1..=50 {
            w.send(Job::Upsert(note(1, &format!("text {rev}"), rev)));
        }
        w.send(Job::SetCurrent(Some(1)));
        assert!(w.flush(Duration::from_secs(5)));
        w.send(Job::Upsert(note(2, "second", 1)));
        assert!(w.shutdown(Duration::from_secs(5)));

        let (_c, loaded) = super::super::open(&path).unwrap();
        let mut notes = loaded.notes;
        notes.sort_by_key(|n| n.id);
        assert_eq!(notes, vec![note(1, "text 50", 50), note(2, "second", 1)]);
        assert_eq!(loaded.current, Some(1));
    }

    #[test]
    fn dropped_writer_leaves_consistent_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        let (conn, _) = super::super::open(&path).unwrap();
        let w = Writer::spawn(conn, path.clone(), Box::new(|_| {}));
        for i in 1..=200 {
            w.send(Job::Upsert(note(i, "x", 1)));
        }
        drop(w); // sender dropped mid-queue: thread drains what it has and exits
        std::thread::sleep(Duration::from_millis(500));
        let (_c, loaded) = super::super::open(&path).unwrap();
        assert!(loaded.notes.iter().all(|n| n.text == "x"));
    }

    #[test]
    fn failing_writes_are_recovered_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.db");
        let (conn, _) = super::super::open(&path).unwrap();
        conn.execute_batch("DROP TABLE notes;").unwrap(); // force write failures
        let statuses = Arc::new(Mutex::new(Vec::new()));
        let s2 = statuses.clone();
        let mut w = Writer::spawn(conn, path.clone(), Box::new(move |s| s2.lock().unwrap().push(s)));
        w.send(Job::Upsert(note(1, "precious", 1)));
        assert!(!w.flush(Duration::from_millis(300)));
        assert!(w.shutdown(Duration::from_secs(10)));
        let statuses = statuses.lock().unwrap();
        let recovered = statuses
            .iter()
            .find_map(|s| match s {
                Status::Recovered(p) => Some(p.clone()),
                _ => None,
            })
            .expect("recovery file written");
        let body = std::fs::read_to_string(recovered).unwrap();
        assert!(body.contains("precious"));
    }
}
