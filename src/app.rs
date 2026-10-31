//! App: owns the Store, writer, and window bookkeeping. Window procedures
//! `post` actions; `pump` runs them whenever the app isn't already borrowed,
//! so Win32 re-entrancy (messages sent while we're mid-action) is safe.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nmi_notes::config::{parse_hotkey, Config};
use nmi_notes::model::diff::{apply_utf16, diff_utf16};
use nmi_notes::model::placement::{place_window, Point};
use nmi_notes::model::store::{Change, NoteId, Op, Store, StoreError};
use nmi_notes::model::text::{compact_count, ellipsize, first_line, line_count, relative_time, utf16_len};
use nmi_notes::persist::{Job, Status, Writer};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetTextFaceW, ReleaseDC, SelectObject, FW_BOLD, FW_NORMAL, HFONT, HGDIOBJ,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_TAB};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetAncestor, IsWindow, IsWindowVisible, MessageBoxW, PostMessageW, PostQuitMessage, SetTimer,
    GA_ROOT, MB_ICONERROR, MB_ICONWARNING, MB_OK, MSG, SW_SHOWNORMAL, WM_APP, WM_KEYDOWN,
};

use crate::log;
use crate::win::{self, autostart, clipboard, editor, hotkeys, listwin, monitor, timefmt, toast, tray::Tray, vdesktop::VDesk};

pub const WM_APP_SHOW_LIST: u32 = WM_APP + 2;
pub const WM_APP_SPARE: u32 = WM_APP + 3;
pub const WM_APP_WRITER: u32 = WM_APP + 4;
/// From a second instance: wParam = command id (`--cmd`), or 0 to exit.
pub const WM_APP_REMOTE: u32 = WM_APP + 5;
pub const TIMER_BACKUP: usize = 1;
const BACKUP_CHECK_MS: u32 = 60 * 60 * 1000;
const WINDOW_OFFSET: i32 = 200;
const CASCADE: i32 = 24;
const EDITOR_SIZE: (i32, i32) = (700, 500);
const LIST_SIZE: (i32, i32) = (640, 520);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewNote,
    Append,
    ShowCurrent,
    CopyCurrent,
    NoteList,
}

impl Command {
    pub const ALL: [Command; 5] =
        [Command::NewNote, Command::Append, Command::ShowCurrent, Command::CopyCurrent, Command::NoteList];

    pub fn id(self) -> i32 {
        self as i32 + 1
    }

    pub fn from_name(name: &str) -> Option<Command> {
        Some(match name {
            "new" => Command::NewNote,
            "append" => Command::Append,
            "show" => Command::ShowCurrent,
            "copy" => Command::CopyCurrent,
            "list" => Command::NoteList,
            _ => return None,
        })
    }

    pub fn from_id(id: i32) -> Option<Command> {
        Command::ALL.get((id - 1) as usize).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Command::NewNote => "Paste to new note",
            Command::Append => "Paste append to current",
            Command::ShowCurrent => "Show current note",
            Command::CopyCurrent => "Copy current note",
            Command::NoteList => "Note list",
        }
    }

    fn binding(self, c: &Config) -> &str {
        match self {
            Command::NewNote => &c.hotkeys.new_note,
            Command::Append => &c.hotkeys.append,
            Command::ShowCurrent => &c.hotkeys.show_current,
            Command::CopyCurrent => &c.hotkeys.copy_current,
            Command::NoteList => &c.hotkeys.note_list,
        }
    }
}

pub enum Action {
    Startup,
    Command(Command, Instant),
    TrayMenu,
    TrayRecreate,
    EditorDirty(isize),
    EditorFlush(isize),
    EditorClose(isize),
    EditorMakeCurrent(isize),
    ListFilter(isize),
    ListOpen(isize),
    ListMakeCurrent(isize),
    ListDelete(isize),
    ListUndo,
    ListClose(isize),
    WindowDpi(isize),
    WindowDestroyed(isize),
    CreateSpare,
    WriterStatus,
    BackupTick,
    FlushAll,
    EndSession,
    Exit,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static QUEUE: RefCell<VecDeque<Action>> = const { RefCell::new(VecDeque::new()) };
}

pub fn install(app: App) {
    APP.with(|a| *a.borrow_mut() = Some(app));
}

/// Queues an action and runs the queue if the app is free.
pub fn post(a: Action) {
    QUEUE.with(|q| q.borrow_mut().push_back(a));
    pump();
}

fn pump() {
    APP.with(|cell| loop {
        let Ok(mut guard) = cell.try_borrow_mut() else { return };
        let Some(app) = guard.as_mut() else { return };
        let Some(action) = QUEUE.with(|q| q.borrow_mut().pop_front()) else { return };
        app.handle(action);
    })
}

/// Keyboard shortcuts for our windows, handled before child controls see
/// them. Returns true if consumed.
pub fn pre_translate(msg: &MSG) -> bool {
    if msg.message != WM_KEYDOWN {
        return false;
    }
    let root = unsafe { GetAncestor(msg.hwnd, GA_ROOT) };
    if root.is_invalid() {
        return false;
    }
    let vk = msg.wParam.0 as u16;
    let down = |k: u16| unsafe { GetKeyState(k as i32) } < 0;
    let ctrl = down(VK_CONTROL.0);
    let shift = down(VK_SHIFT.0);
    let k = win::key(root);
    match win::class_name(root).as_str() {
        "NmiEditor" => {
            if vk == VK_ESCAPE.0 || (ctrl && !shift && vk == b'W' as u16) {
                post(Action::EditorClose(k));
                return true;
            }
            if ctrl && shift && vk == VK_RETURN.0 {
                post(Action::EditorMakeCurrent(k));
                return true;
            }
        }
        "NmiList" => {
            let in_list = listwin::list_has_focus(root);
            match vk {
                v if v == VK_ESCAPE.0 => post(Action::ListClose(k)),
                v if v == VK_RETURN.0 && ctrl => post(Action::ListMakeCurrent(k)),
                v if v == VK_RETURN.0 => post(Action::ListOpen(k)),
                v if v == VK_DOWN.0 && !in_list => listwin::focus_list(root),
                v if v == VK_TAB.0 => {
                    if in_list {
                        listwin::focus_filter(root)
                    } else {
                        listwin::focus_list(root)
                    }
                }
                v if v == VK_DELETE.0 && in_list => post(Action::ListDelete(k)),
                v if v == b'Z' as u16 && ctrl && in_list => post(Action::ListUndo),
                _ => return false,
            }
            return true;
        }
        _ => {}
    }
    false
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub struct Paths {
    pub data_dir: PathBuf,
    pub backups: PathBuf,
    pub config: PathBuf,
}

struct EditorState {
    note: NoteId,
    rev: u64,
    synced: Vec<u16>,
}

#[derive(Default)]
struct Fonts {
    by_key: HashMap<(u8, u32), HFONT>,
    text_face: Option<String>,
}

impl Fonts {
    fn get(&mut self, kind: u8, dpi: u32, cfg: &Config) -> HFONT {
        if let Some(f) = self.by_key.get(&(kind, dpi)) {
            return *f;
        }
        let f = match kind {
            0 => {
                let face = self.text_face.get_or_insert_with(|| resolve_face(&cfg.font)).clone();
                win::create_font(&face, cfg.font_size, dpi, FW_NORMAL)
            }
            1 => win::create_font("Segoe UI", 9, dpi, FW_NORMAL),
            _ => win::create_font("Segoe UI", 9, dpi, FW_BOLD),
        };
        self.by_key.insert((kind, dpi), f);
        f
    }
}

/// Uses the configured face if installed, else Consolas.
fn resolve_face(face: &str) -> String {
    let f = win::create_font(face, 10, 96, FW_NORMAL);
    let mut buf = [0u16; 64];
    let got = unsafe {
        let dc = GetDC(None);
        let old = SelectObject(dc, HGDIOBJ(f.0));
        let n = GetTextFaceW(dc, Some(&mut buf));
        SelectObject(dc, old);
        ReleaseDC(None, dc);
        let _ = DeleteObject(HGDIOBJ(f.0));
        String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize])
    };
    if got.eq_ignore_ascii_case(face) {
        face.to_owned()
    } else {
        log!("font '{face}' not available (got '{got}'), using Consolas");
        "Consolas".to_owned()
    }
}

pub struct App {
    main: HWND,
    store: Store,
    writer: Writer,
    config: Config,
    config_error: Option<String>,
    paths: Paths,
    vdesk: VDesk,
    tray: Option<Tray>,
    editors: HashMap<isize, EditorState>,
    spare: Option<HWND>,
    spare_list: Option<HWND>,
    lists: Vec<isize>,
    last_deleted: Vec<NoteId>,
    fonts: Fonts,
    editor_size_96: (i32, i32),
    writer_events: Arc<Mutex<Vec<Status>>>,
    save_failing: bool,
    shut_down: bool,
    /// When the current command's hotkey arrived (for latency logging).
    cmd_started: Option<Instant>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        main: HWND,
        store: Store,
        writer: Writer,
        config: Config,
        config_error: Option<String>,
        paths: Paths,
        writer_events: Arc<Mutex<Vec<Status>>>,
    ) -> App {
        App {
            main,
            store,
            writer,
            config,
            config_error,
            paths,
            vdesk: VDesk::new(),
            tray: None,
            editors: HashMap::new(),
            spare: None,
            spare_list: None,
            lists: Vec::new(),
            last_deleted: Vec::new(),
            fonts: Fonts::default(),
            editor_size_96: EDITOR_SIZE,
            writer_events,
            save_failing: false,
            shut_down: false,
            cmd_started: None,
        }
    }

    fn handle(&mut self, action: Action) {
        match action {
            Action::Startup => self.startup(),
            Action::Command(c, t0) => {
                self.cmd_started = Some(t0);
                self.command(c);
                self.cmd_started = None;
                log!("{:?} done in {:.1} ms", c, ms(t0));
            }
            Action::TrayMenu => self.tray_menu(),
            Action::TrayRecreate => {
                let tip = self.tip();
                if let Some(t) = &self.tray {
                    t.readd(&tip);
                }
            }
            Action::EditorDirty(k) => {
                if self.editors.contains_key(&k) {
                    self.refresh_editor_chrome_for(k);
                }
            }
            Action::EditorFlush(k) => self.flush_editor(k),
            Action::EditorClose(k) => self.close_editor(k),
            Action::EditorMakeCurrent(k) => {
                if let Some(id) = self.editors.get(&k).map(|s| s.note) {
                    self.flush_editor(k);
                    self.apply(Op::SetCurrent { id: Some(id) }, None);
                    toast::show(&format!("Current: {}", self.preview(id, 60)));
                }
            }
            Action::ListFilter(k) => self.refresh_list(win::hwnd(k)),
            Action::ListOpen(k) => {
                let h = win::hwnd(k);
                let mut ids = listwin::selected_ids(h);
                if ids.is_empty() {
                    ids.extend(listwin::first_id(h));
                }
                if ids.is_empty() {
                    return;
                }
                unsafe {
                    let _ = DestroyWindow(h);
                }
                for (i, id) in ids.into_iter().enumerate() {
                    self.show_note(id, i as i32);
                }
            }
            Action::ListMakeCurrent(k) => {
                let h = win::hwnd(k);
                let id = listwin::focused_id(h)
                    .or_else(|| listwin::selected_ids(h).first().copied())
                    .or_else(|| listwin::first_id(h));
                if let Some(id) = id {
                    self.apply(Op::SetCurrent { id: Some(id) }, None);
                    toast::show(&format!("Current: {}", self.preview(id, 60)));
                    unsafe {
                        let _ = DestroyWindow(h);
                    }
                }
            }
            Action::ListDelete(k) => {
                let ids = listwin::selected_ids(win::hwnd(k));
                if ids.is_empty() {
                    return;
                }
                for id in &ids {
                    self.flush_note_editors(*id);
                }
                if self.apply(Op::Delete { ids: ids.clone() }, None).is_some() {
                    let n = ids.len();
                    self.last_deleted = ids;
                    toast::show(&format!("Deleted {n} note{} — Ctrl+Z in list to undo", plural(n)));
                }
            }
            Action::ListUndo => {
                if self.last_deleted.is_empty() {
                    return;
                }
                let ids = std::mem::take(&mut self.last_deleted);
                let n = ids.len();
                self.apply(Op::Undelete { ids }, None);
                toast::show(&format!("Restored {n} note{}", plural(n)));
            }
            Action::ListClose(k) => unsafe {
                let _ = DestroyWindow(win::hwnd(k));
            },
            Action::WindowDpi(k) => self.apply_fonts(win::hwnd(k)),
            Action::WindowDestroyed(k) => {
                self.editors.remove(&k);
                self.lists.retain(|l| *l != k);
                if self.spare.map(win::key) == Some(k) {
                    self.spare = None;
                }
                if self.spare_list.map(win::key) == Some(k) {
                    self.spare_list = None;
                }
            }
            Action::CreateSpare => self.create_spare(),
            Action::WriterStatus => self.writer_status(),
            Action::BackupTick => {
                self.writer.send(Job::Backup { dir: self.paths.backups.clone(), date: timefmt::today() })
            }
            Action::FlushAll => {
                self.flush_all_editors();
                if !self.writer.flush(Duration::from_secs(3)) {
                    log!("flush on query-end-session timed out");
                }
            }
            Action::EndSession => self.shutdown(),
            Action::Exit => {
                self.shutdown();
                unsafe { PostQuitMessage(0) };
            }
        }
    }

    // ---- startup / shutdown -------------------------------------------------

    fn startup(&mut self) {
        let mut failed = Vec::new();
        for c in Command::ALL {
            let binding = c.binding(&self.config).to_owned();
            match parse_hotkey(&binding) {
                Ok((mods, vk)) => {
                    if !hotkeys::register(self.main, c.id(), mods, vk) {
                        failed.push(format!("{binding} ({})", c.label()));
                    }
                }
                Err(e) => failed.push(e),
            }
        }
        self.tray = Some(Tray::add(self.main, &self.tip()));
        unsafe {
            SetTimer(Some(self.main), TIMER_BACKUP, BACKUP_CHECK_MS, None);
        }
        self.writer.send(Job::Backup { dir: self.paths.backups.clone(), date: timefmt::today() });
        autostart::apply(self.config.start_with_windows);
        self.create_spare();

        if let Some(e) = self.config_error.take() {
            log!("{e}");
            toast::show("config.toml is invalid — using defaults (see log)");
        } else if !failed.is_empty() {
            log!("hotkeys unavailable: {}", failed.join(", "));
            toast::show(&format!("Hotkey unavailable: {}", failed.join(", ")));
        } else {
            toast::show(&format!("nmi-notes ready — {} notes", self.store.list("").len()));
        }
        log!("started with {} notes", self.store.list("").len());
    }

    fn shutdown(&mut self) {
        if self.shut_down {
            return;
        }
        self.shut_down = true;
        self.flush_all_editors();
        if !self.writer.shutdown(Duration::from_secs(8)) {
            log!("writer shutdown timed out");
        }
        for c in Command::ALL {
            hotkeys::unregister(self.main, c.id());
        }
        if let Some(t) = &self.tray {
            t.remove();
        }
        let events: Vec<Status> = std::mem::take(&mut *self.writer_events.lock().unwrap());
        for e in events {
            match e {
                Status::Recovered(p) => message_box(
                    &format!("Some notes could not be saved to the database.\nThey were written to:\n{}", p.display()),
                    MB_ICONWARNING,
                ),
                Status::RecoveryFailed(err) => {
                    message_box(&format!("Some notes could not be saved: {err}"), MB_ICONERROR)
                }
                _ => {}
            }
        }
        log!("shut down");
    }

    fn writer_status(&mut self) {
        let events: Vec<Status> = std::mem::take(&mut *self.writer_events.lock().unwrap());
        for e in events {
            match e {
                Status::Failing(err) => {
                    log!("writer failing: {err}");
                    if !self.save_failing {
                        self.save_failing = true;
                        toast::show("⚠ Saving notes failed — retrying (see log)");
                    }
                }
                Status::Healthy => {
                    if self.save_failing {
                        self.save_failing = false;
                        toast::show("Saving recovered");
                    }
                    log!("writer healthy");
                }
                Status::Recovered(p) => log!("recovery file written: {}", p.display()),
                Status::RecoveryFailed(err) => log!("recovery failed: {err}"),
            }
        }
        let tip = self.tip();
        if let Some(t) = &self.tray {
            t.set_tip(&tip);
        }
    }

    fn tip(&self) -> String {
        if self.save_failing {
            "nmi-notes — ⚠ saving failed".into()
        } else {
            "nmi-notes".into()
        }
    }

    fn tray_menu(&mut self) {
        const OPEN_CONFIG: u32 = 100;
        const OPEN_DATA: u32 = 101;
        const EXIT: u32 = 199;
        let labels: Vec<(u32, String)> = Command::ALL
            .iter()
            .map(|c| (c.id() as u32, format!("{}\t{}", c.label(), c.binding(&self.config))))
            .collect();
        let mut items: Vec<Option<(u32, &str)>> = labels.iter().map(|(id, l)| Some((*id, l.as_str()))).collect();
        items.push(None);
        items.push(Some((OPEN_CONFIG, "Open config")));
        items.push(Some((OPEN_DATA, "Open data folder")));
        items.push(None);
        items.push(Some((EXIT, "Exit")));
        let Some(choice) = self.tray.as_ref().and_then(|t| t.show_menu(&items)) else { return };
        match choice {
            OPEN_CONFIG => shell_open("notepad.exe", Some(&self.paths.config.display().to_string())),
            OPEN_DATA => shell_open(&self.paths.data_dir.display().to_string(), None),
            EXIT => post(Action::Exit),
            id => {
                if let Some(c) = Command::from_id(id as i32) {
                    self.command(c);
                }
            }
        }
    }

    // ---- commands -------------------------------------------------------------

    fn command(&mut self, c: Command) {
        match c {
            Command::NewNote => match clipboard::read(self.main) {
                clipboard::ClipRead::Text(t) => {
                    if let Some(id) = self.create_current(t) {
                        toast::show(&format!("New note — {}", self.summary(id)));
                    }
                }
                clipboard::ClipRead::Empty => toast::show("Nothing to paste"),
                clipboard::ClipRead::Busy => toast::show("Clipboard busy"),
            },
            Command::Append => match clipboard::read(self.main) {
                clipboard::ClipRead::Text(t) => match self.store.current() {
                    Some(id) => {
                        self.flush_note_editors(id);
                        if self.apply(Op::Append { id, text: t }, None).is_some() {
                            toast::show(&format!("Appended to: {}", self.preview(id, 60)));
                        }
                    }
                    None => {
                        if let Some(id) = self.create_current(t) {
                            toast::show(&format!("New note (no current) — {}", self.summary(id)));
                        }
                    }
                },
                clipboard::ClipRead::Empty => toast::show("Nothing to paste"),
                clipboard::ClipRead::Busy => toast::show("Clipboard busy"),
            },
            Command::ShowCurrent => match self.store.current() {
                Some(id) => self.show_note(id, 0),
                None => toast::show("No current note"),
            },
            Command::CopyCurrent => match self.store.current() {
                Some(id) => {
                    self.flush_note_editors(id);
                    let text = self.store.note(id).map(|n| n.text.clone()).unwrap_or_default();
                    if clipboard::write_text(self.main, &text) {
                        toast::show(&format!("Copied — {} chars", text.chars().count()));
                    } else {
                        toast::show("Clipboard busy");
                    }
                }
                None => toast::show("No current note"),
            },
            Command::NoteList => self.show_list(),
        }
    }

    fn create_current(&mut self, text: String) -> Option<NoteId> {
        let changes = self.apply(Op::Create { text }, None)?;
        let id = changes.iter().find_map(|c| match c {
            Change::Created { id } => Some(*id),
            _ => None,
        })?;
        self.apply(Op::SetCurrent { id: Some(id) }, None);
        Some(id)
    }

    fn preview(&self, id: NoteId, max: usize) -> String {
        let line = self.store.note(id).map(|n| first_line(&n.text)).unwrap_or("");
        if line.is_empty() {
            "(empty)".into()
        } else {
            ellipsize(line, max)
        }
    }

    fn summary(&self, id: NoteId) -> String {
        let n = self.store.note(id).map(|n| n.text.as_str()).unwrap_or("");
        let lines = line_count(n);
        format!("{}  ({} line{}, {} chars)", self.preview(id, 60), lines, plural(lines), n.chars().count())
    }

    // ---- store plumbing -------------------------------------------------------

    /// Dispatches, persists, and routes. `origin` is the editor that produced
    /// the op (it already shows the result).
    fn apply(&mut self, op: Op, origin: Option<isize>) -> Option<Vec<Change>> {
        match self.store.dispatch(op, now_ms()) {
            Ok(changes) => {
                self.persist(&changes);
                self.route(&changes, origin);
                Some(changes)
            }
            Err(e) => {
                log!("dispatch failed: {e}");
                None
            }
        }
    }

    fn persist(&self, changes: &[Change]) {
        for ch in changes {
            for id in ch.touched() {
                if let Some(n) = self.store.note(id) {
                    self.writer.send(Job::Upsert(n.clone()));
                }
            }
            if let Change::Current { new, .. } = ch {
                self.writer.send(Job::SetCurrent(*new));
            }
        }
    }

    fn editors_of(&self, id: NoteId) -> Vec<isize> {
        self.editors.iter().filter(|(_, s)| s.note == id).map(|(k, _)| *k).collect()
    }

    fn route(&mut self, changes: &[Change], origin: Option<isize>) {
        for ch in changes {
            match ch {
                Change::Text { id, rev, splice } => {
                    for k in self.editors_of(*id) {
                        if Some(k) == origin {
                            continue;
                        }
                        let h = win::hwnd(k);
                        if editor::is_dirty(h) {
                            log!("remote change routed to editor with unflushed edits (note {id})");
                        }
                        let Some(st) = self.editors.get_mut(&k) else { continue };
                        if st.rev + 1 == *rev {
                            editor::apply_splice(h, splice.start, splice.old_len, &splice.text);
                            let ins: Vec<u16> = splice.text.encode_utf16().collect();
                            st.synced = apply_utf16(&st.synced, splice.start, splice.old_len, &ins);
                            st.rev = *rev;
                        } else if let Some(n) = self.store.note(*id) {
                            editor::set_text(h, &n.text);
                            st.synced = n.text.encode_utf16().collect();
                            st.rev = n.revision;
                        }
                    }
                    self.refresh_editor_chrome(*id);
                }
                Change::Current { old, new } => {
                    for id in [old, new].into_iter().flatten() {
                        self.refresh_editor_chrome(*id);
                    }
                }
                Change::Deleted { ids } => {
                    for id in ids {
                        for k in self.editors_of(*id) {
                            self.editors.remove(&k);
                            unsafe {
                                let _ = DestroyWindow(win::hwnd(k));
                            }
                        }
                    }
                }
                Change::Created { .. } | Change::Undeleted { .. } => {}
            }
        }
        if !changes.is_empty() {
            self.refresh_lists();
        }
    }

    // ---- editors --------------------------------------------------------------

    fn flush_note_editors(&mut self, id: NoteId) {
        for k in self.editors_of(id) {
            self.flush_editor(k);
        }
    }

    fn flush_all_editors(&mut self) {
        let keys: Vec<isize> = self.editors.keys().copied().collect();
        for k in keys {
            self.flush_editor(k);
        }
    }

    /// Sends an editor's pending local edits to the store as one range edit.
    fn flush_editor(&mut self, k: isize) {
        let h = win::hwnd(k);
        let Some(st) = self.editors.get(&k) else { return };
        if unsafe { !IsWindow(Some(h)).as_bool() } {
            return;
        }
        let was_dirty = editor::is_dirty(h);
        editor::mark_clean(h);
        let current = editor::get_text16(h);
        let (note, base) = (st.note, st.rev);
        let Some(sp) = diff_utf16(&st.synced, &current) else {
            if was_dirty {
                self.refresh_editor_chrome(note);
            }
            return;
        };
        let text = String::from_utf16_lossy(&current[sp.start..sp.start + sp.new_len]);
        let op = Op::Edit { id: note, base_rev: base, start: sp.start, old_len: sp.old_len, text };
        match self.store.dispatch(op, now_ms()) {
            Ok(changes) => self.after_editor_dispatch(k, current, changes),
            Err(StoreError::StaleRevision { .. }) | Err(StoreError::OutOfRange) => {
                // Should not happen (flush-before-dispatch). Keep what the
                // user sees: replace the whole note with the editor's text.
                log!("editor out of sync on note {note}; saving editor text as-is");
                let Some(n) = self.store.note(note) else { return };
                let op = Op::Edit {
                    id: note,
                    base_rev: n.revision,
                    start: 0,
                    old_len: utf16_len(&n.text),
                    text: String::from_utf16_lossy(&current),
                };
                if let Ok(changes) = self.store.dispatch(op, now_ms()) {
                    self.after_editor_dispatch(k, current, changes);
                }
            }
            Err(StoreError::NoSuchNote(_)) => {
                // Note was deleted under us: never drop the text.
                log!("note {note} vanished while editing; saving as a new note");
                let text = String::from_utf16_lossy(&current);
                if let Some(changes) = self.apply(Op::Create { text }, None) {
                    if let Some(Change::Created { id }) = changes.first() {
                        if let Some(n) = self.store.note(*id) {
                            self.editors.insert(k, EditorState { note: *id, rev: n.revision, synced: current });
                            self.refresh_editor_chrome(*id);
                        }
                    }
                }
            }
        }
    }

    fn after_editor_dispatch(&mut self, k: isize, current: Vec<u16>, changes: Vec<Change>) {
        if let Some(st) = self.editors.get_mut(&k) {
            if let Some(Change::Text { rev, .. }) = changes.first() {
                st.rev = *rev;
            }
            st.synced = current;
        }
        self.persist(&changes);
        self.route(&changes, Some(k));
    }

    fn close_editor(&mut self, k: isize) {
        self.flush_editor(k);
        let h = win::hwnd(k);
        let (w, hgt) = editor::size(h);
        let dpi = unsafe { GetDpiForWindow(h) }.max(96) as i32;
        self.editor_size_96 = (w * 96 / dpi, hgt * 96 / dpi);
        self.editors.remove(&k);
        unsafe {
            let _ = DestroyWindow(h);
        }
    }

    /// Logs hotkey -> window-on-screen latency (budget: 50 ms). Activation
    /// that follows waits on the previously active app and is logged as
    /// part of the command total.
    fn log_visible(&self) {
        if let Some(t0) = self.cmd_started {
            log!("window visible in {:.1} ms", ms(t0));
        }
    }

    /// Keeps one hidden, pre-warmed editor and list window ready.
    fn create_spare(&mut self) {
        if self.spare_list.is_none() {
            if let Some(h) = listwin::create() {
                let ctx = monitor::pointer_context();
                let ui = self.fonts.get(1, ctx.dpi, &self.config);
                let bold = self.fonts.get(2, ctx.dpi, &self.config);
                listwin::set_fonts(h, ui, bold);
                let (w, hgt) = (win::scale(LIST_SIZE.0, ctx.dpi), win::scale(LIST_SIZE.1, ctx.dpi));
                editor::prewarm_window(h, place_window(ctx.pointer, ctx.work, w, hgt, 0));
                self.spare_list = Some(h);
            }
        }
        if self.spare.is_some() {
            return;
        }
        let Some(h) = editor::create() else { return };
        let ctx = monitor::pointer_context();
        let dpi = ctx.dpi;
        let text_font = self.fonts.get(0, dpi, &self.config);
        let ui_font = self.fonts.get(1, dpi, &self.config);
        editor::set_fonts(h, text_font, ui_font, dpi);
        let (w, hgt) = (win::scale(self.editor_size_96.0, dpi), win::scale(self.editor_size_96.1, dpi));
        let r = place_window(ctx.pointer, ctx.work, w, hgt, win::scale(WINDOW_OFFSET, dpi));
        editor::prewarm(h, r);
        self.spare = Some(h);
    }

    fn refresh_editor_chrome(&mut self, id: NoteId) {
        for k in self.editors_of(id) {
            self.refresh_editor_chrome_for(k);
        }
    }

    fn refresh_editor_chrome_for(&mut self, k: isize) {
        let Some(st) = self.editors.get(&k) else { return };
        let h = win::hwnd(k);
        let Some(n) = self.store.note(st.note) else { return };
        let is_current = self.store.current() == Some(n.id);
        let star = if is_current { " ★" } else { "" };
        editor::set_caption(h, &format!("{}{} — nmi-notes", self.preview(n.id, 80), star));
        let mut status = String::new();
        if is_current {
            status.push_str("★ Current   ·   ");
        }
        if editor::is_dirty(h) {
            status.push_str("editing…");
        } else {
            status.push_str(&format!("saved   ·   updated {}", timefmt::local_time(n.updated)));
        }
        editor::set_status(h, &status);
    }

    fn apply_fonts(&mut self, h: HWND) {
        let dpi = unsafe { GetDpiForWindow(h) }.max(96);
        match win::class_name(h).as_str() {
            "NmiEditor" => {
                let text = self.fonts.get(0, dpi, &self.config);
                let ui = self.fonts.get(1, dpi, &self.config);
                editor::set_fonts(h, text, ui, dpi);
            }
            "NmiList" => {
                let ui = self.fonts.get(1, dpi, &self.config);
                let bold = self.fonts.get(2, dpi, &self.config);
                listwin::set_fonts(h, ui, bold);
            }
            _ => {}
        }
    }

    /// Shows `id` near the pointer: reuses a window on this desktop, else
    /// opens one here. `cascade` offsets multiple windows opened together.
    fn show_note(&mut self, id: NoteId, cascade: i32) {
        let Some(note) = self.store.live(id) else { return };
        let (text, rev) = (note.text.clone(), note.revision);
        let ctx = monitor::pointer_context();
        let dpi = ctx.dpi;
        let off = win::scale(CASCADE, dpi) * cascade;
        let pointer = Point { x: ctx.pointer.x + off, y: ctx.pointer.y + off };
        let (w, hgt) = (win::scale(self.editor_size_96.0, dpi), win::scale(self.editor_size_96.1, dpi));
        let r = place_window(pointer, ctx.work, w, hgt, win::scale(WINDOW_OFFSET, dpi));

        let existing = self.editors_of(id).into_iter().find(|k| {
            let h = win::hwnd(*k);
            (unsafe { IsWindowVisible(h).as_bool() }) && self.vdesk.is_on_current(h)
        });
        if let Some(k) = existing {
            editor::show_at(win::hwnd(k), r);
            self.log_visible();
            editor::focus(win::hwnd(k));
            return;
        }
        let Some(h) = self.spare.take().or_else(editor::create) else {
            toast::show("Could not open a window");
            return;
        };
        // Fonts for the target monitor before showing, so first paint is right.
        let text_font = self.fonts.get(0, dpi, &self.config);
        let ui_font = self.fonts.get(1, dpi, &self.config);
        editor::set_fonts(h, text_font, ui_font, dpi);
        editor::set_text(h, &text);
        self.editors.insert(win::key(h), EditorState { note: id, rev, synced: text.encode_utf16().collect() });
        self.refresh_editor_chrome_for(win::key(h));
        editor::show_at(h, r);
        self.log_visible();
        editor::focus(h);
        if unsafe { GetDpiForWindow(h) } != dpi {
            self.apply_fonts(h);
        }
        unsafe {
            let _ = PostMessageW(Some(self.main), WM_APP_SPARE, WPARAM(0), LPARAM(0));
        }
    }

    // ---- list -----------------------------------------------------------------

    fn show_list(&mut self) {
        let ctx = monitor::pointer_context();
        let dpi = ctx.dpi;
        let r = place_window(
            ctx.pointer,
            ctx.work,
            win::scale(LIST_SIZE.0, dpi),
            win::scale(LIST_SIZE.1, dpi),
            win::scale(WINDOW_OFFSET, dpi),
        );
        let existing = self.lists.iter().copied().find(|k| {
            let h = win::hwnd(*k);
            (unsafe { IsWindowVisible(h).as_bool() }) && self.vdesk.is_on_current(h)
        });
        if let Some(k) = existing {
            listwin::show_at(win::hwnd(k), r);
            self.log_visible();
            listwin::focus(win::hwnd(k));
            return;
        }
        let Some(h) = self.spare_list.take().or_else(listwin::create) else {
            toast::show("Could not open the note list");
            return;
        };
        self.lists.push(win::key(h));
        let ui = self.fonts.get(1, dpi, &self.config);
        let bold = self.fonts.get(2, dpi, &self.config);
        listwin::set_fonts(h, ui, bold);
        self.refresh_list(h);
        listwin::show_at(h, r);
        self.log_visible();
        listwin::focus(h);
        if unsafe { GetDpiForWindow(h) } != dpi {
            self.apply_fonts(h);
        }
        unsafe {
            let _ = PostMessageW(Some(self.main), WM_APP_SPARE, WPARAM(0), LPARAM(0));
        }
    }

    fn refresh_lists(&mut self) {
        for k in self.lists.clone() {
            self.refresh_list(win::hwnd(k));
        }
    }

    fn refresh_list(&mut self, h: HWND) {
        let filter = listwin::filter_text(h);
        let now = now_ms();
        let current = self.store.current();
        let rows = self
            .store
            .list(&filter)
            .into_iter()
            .filter_map(|id| self.store.note(id))
            .map(|n| listwin::Row {
                id: n.id,
                current: Some(n.id) == current,
                preview: self.preview(n.id, 200),
                updated_rel: relative_time(now, n.updated),
                updated_abs: timefmt::local_datetime(n.updated),
                size: compact_count(n.text.chars().count()),
            })
            .collect();
        listwin::populate(h, rows);
        let count = self.store.list("").len();
        let title = if filter.trim().is_empty() {
            format!("Notes ({count}) — nmi-notes")
        } else {
            format!("Notes (filtered of {count}) — nmi-notes")
        };
        editor::set_caption(h, &title);
    }
}

fn ms(t0: Instant) -> f64 {
    t0.elapsed().as_secs_f64() * 1000.0
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn message_box(text: &str, icon: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) {
    let t = win::wide(text);
    unsafe {
        MessageBoxW(None, PCWSTR(t.as_ptr()), w!("nmi-notes"), MB_OK | icon);
    }
}

pub fn fatal(text: &str) {
    message_box(text, MB_ICONERROR);
}

fn shell_open(file: &str, params: Option<&str>) {
    let f = win::wide(file);
    let p = params.map(win::wide);
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(f.as_ptr()),
            p.as_ref().map(|p| PCWSTR(p.as_ptr())).unwrap_or(PCWSTR::null()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}
