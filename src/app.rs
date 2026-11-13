// SPDX-License-Identifier: AGPL-3.0-or-later

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
use nmi_notes::model::store::{Change, GroupId, ListView, NoteId, Op, Store, StoreError};
use nmi_notes::model::text::{compact_count, ellipsize, first_line, line_count, relative_time, utf16_len};
use nmi_notes::persist::{Job, Status, Writer};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetTextFaceW, ReleaseDC, SelectObject, FW_BOLD, FW_NORMAL, HFONT, HGDIOBJ,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_TAB};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetAncestor, IsWindow, IsWindowVisible, MessageBoxW, PostMessageW, PostQuitMessage, SetTimer,
    GA_ROOT, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MSG, SW_SHOWNORMAL, WM_APP, WM_KEYDOWN,
};

use crate::log;
use crate::win::{self, autostart, clipboard, editor, help, hotkeys, listwin, menu, monitor, prompt, timefmt, toast, tray::Tray, vdesktop::VDesk};

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
    Help,
}

impl Command {
    pub const ALL: [Command; 6] =
        [Command::NewNote, Command::Append, Command::ShowCurrent, Command::CopyCurrent, Command::NoteList, Command::Help];

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
            "help" => Command::Help,
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
            Command::Help => "Hot key help",
        }
    }

    fn binding(self, c: &Config) -> &str {
        match self {
            Command::NewNote => &c.hotkeys.new_note,
            Command::Append => &c.hotkeys.append,
            Command::ShowCurrent => &c.hotkeys.show_current,
            Command::CopyCurrent => &c.hotkeys.copy_current,
            Command::NoteList => &c.hotkeys.note_list,
            Command::Help => &c.hotkeys.help,
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
    /// Esc: close, and return to the list if the note was opened from it.
    EditorBack(isize),
    EditorMakeCurrent(isize),
    /// Protect checkbox clicked: toggle.
    EditorProtect(isize),
    /// Groups button clicked: show the groups menu.
    EditorGroups(isize),
    ListFilter(isize),
    /// The user picked an entry in the group dropdown.
    ListGroup(isize),
    ListOpen(isize),
    ListMakeCurrent(isize),
    ListDelete(isize),
    ListUndo,
    ListClose(isize),
    /// Context menu; `None`: opened from the keyboard.
    ListMenu(isize, Option<POINT>),
    PromptSubmit(isize),
    PromptCancel(isize),
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
            if vk == VK_ESCAPE.0 {
                post(Action::EditorBack(k));
                return true;
            }
            if ctrl && !shift && vk == b'W' as u16 {
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
            let in_group = listwin::group_has_focus(root);
            match vk {
                v if v == VK_ESCAPE.0 => post(Action::ListClose(k)),
                v if v == VK_RETURN.0 && ctrl => post(Action::ListMakeCurrent(k)),
                v if v == VK_RETURN.0 => post(Action::ListOpen(k)),
                v if v == VK_DOWN.0 && !in_list && !in_group => listwin::focus_list(root),
                // Tab cycles filter -> list -> group -> filter.
                v if v == VK_TAB.0 => {
                    if in_list {
                        listwin::focus_group(root)
                    } else if in_group {
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
        "NmiPrompt" => {
            if vk == VK_RETURN.0 {
                post(Action::PromptSubmit(k));
                return true;
            }
            if vk == VK_ESCAPE.0 {
                post(Action::PromptCancel(k));
                return true;
            }
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
    /// Opened from the Note List: Esc returns there.
    from_list: bool,
}

/// An open text prompt.
struct Prompt {
    key: isize,
    /// Window it belongs to; gets focus back when the prompt closes.
    owner: isize,
    purpose: PromptPurpose,
}

/// What an open text prompt is for.
enum PromptPurpose {
    /// Create a group, then add these notes to it.
    NewGroup { add: Vec<NoteId> },
    RenameGroup(GroupId),
}

// Context-menu command ids.
const MENU_NEW_GROUP: u32 = 1;
const MENU_NEW_GROUP_ADD: u32 = 2;
const MENU_RENAME_GROUP: u32 = 3;
const MENU_DELETE_GROUP: u32 = 4;
const MENU_GROUP: u32 = 1000;
const MENU_ADD: u32 = 2000;
const MENU_REMOVE: u32 = 3000;

/// Menu and button labels treat `&` as a mnemonic marker.
fn menu_text(s: &str) -> String {
    s.replace('&', "&&")
}

fn quoted(s: &str) -> String {
    format!("“{s}”")
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
    /// Filter and view of the list that last opened notes, restored on return.
    list_filter: String,
    back_view: ListView,
    /// What the list opens on (persisted).
    list_view: ListView,
    /// The group last chosen by hand in the dropdown.
    last_picked: Option<GroupId>,
    /// Note to select the next time the list opens (after New Note / Append).
    list_select: Option<NoteId>,
    prompt: Option<Prompt>,
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
        list_view: ListView,
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
            list_filter: String::new(),
            back_view: ListView::Ungrouped,
            list_view,
            last_picked: match list_view {
                ListView::Group(g) => Some(g),
                _ => None,
            },
            list_select: None,
            prompt: None,
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
            Action::EditorBack(k) => {
                let back = self.editors.get(&k).filter(|s| s.from_list).map(|s| s.note);
                self.close_editor(k);
                if let Some(id) = back {
                    let filter = std::mem::take(&mut self.list_filter);
                    self.show_list_with(Some((&filter, id)));
                    self.list_filter = filter;
                }
            }
            Action::EditorMakeCurrent(k) => {
                if let Some(id) = self.editors.get(&k).map(|s| s.note) {
                    self.flush_editor(k);
                    self.apply(Op::SetCurrent { id: Some(id) }, None);
                    toast::show(&format!("Current: {}", self.preview(id, 60)));
                }
            }
            Action::EditorProtect(k) => {
                if let Some(n) = self.editors.get(&k).and_then(|s| self.store.live(s.note)) {
                    let (id, protected) = (n.id, !n.protected);
                    self.apply(Op::SetProtected { id, protected }, None);
                }
                editor::focus(win::hwnd(k));
            }
            Action::EditorGroups(k) => self.editor_groups_menu(k),
            Action::ListFilter(k) => self.refresh_list(win::hwnd(k)),
            Action::ListGroup(k) => {
                let h = win::hwnd(k);
                self.refresh_list(h);
                let v = listwin::view(h);
                if let ListView::Group(g) = v {
                    self.last_picked = Some(g);
                }
                self.set_list_view(v);
            }
            Action::ListOpen(k) => {
                let h = win::hwnd(k);
                let mut ids = listwin::selected_ids(h);
                if ids.is_empty() {
                    ids.extend(listwin::first_id(h));
                }
                if ids.is_empty() {
                    return;
                }
                self.list_filter = listwin::filter_text(h);
                self.back_view = listwin::view(h);
                unsafe {
                    let _ = DestroyWindow(h);
                }
                for (i, id) in ids.into_iter().enumerate() {
                    self.show_note(id, i as i32, true);
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
                let selected = listwin::selected_ids(win::hwnd(k));
                if selected.is_empty() {
                    return;
                }
                let (kept, ids): (Vec<NoteId>, Vec<NoteId>) =
                    selected.into_iter().partition(|id| self.store.note(*id).is_some_and(|n| n.protected));
                if ids.is_empty() {
                    let n = kept.len();
                    toast::show(&if n == 1 { "Protected — not deleted".to_owned() } else { format!("{n} protected notes — not deleted") });
                    return;
                }
                for id in &ids {
                    self.flush_note_editors(*id);
                }
                if self.apply(Op::Delete { ids: ids.clone() }, None).is_some() {
                    let n = ids.len();
                    self.last_deleted = ids;
                    let kept = if kept.is_empty() { String::new() } else { format!(", kept {} protected", kept.len()) };
                    toast::show(&format!("Deleted {n} note{}{kept} — Ctrl+Z in list to undo", plural(n)));
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
            Action::ListMenu(k, at) => self.list_menu(win::hwnd(k), at),
            Action::PromptSubmit(k) => self.prompt_submit(k),
            Action::PromptCancel(k) => {
                self.close_prompt(k);
            }
            Action::WindowDpi(k) => self.apply_fonts(win::hwnd(k)),
            Action::WindowDestroyed(k) => {
                self.editors.remove(&k);
                self.lists.retain(|l| *l != k);
                if self.prompt.as_ref().is_some_and(|p| p.key == k) {
                    self.prompt = None;
                }
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
            toast::show(&format!("nmi-notes ready — {} notes", self.store.list("", ListView::All).len()));
        }
        log!("started with {} notes", self.store.list("", ListView::All).len());
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
        const ABOUT: u32 = 102;
        const EXIT: u32 = 199;
        let labels: Vec<(u32, String)> = Command::ALL
            .iter()
            .map(|c| (c.id() as u32, format!("{}\t{}", c.label(), c.binding(&self.config))))
            .collect();
        let mut items: Vec<Option<(u32, &str)>> = labels.iter().map(|(id, l)| Some((*id, l.as_str()))).collect();
        items.push(None);
        items.push(Some((OPEN_CONFIG, "Open config")));
        items.push(Some((OPEN_DATA, "Open data folder")));
        items.push(Some((ABOUT, "About")));
        items.push(None);
        items.push(Some((EXIT, "Exit")));
        let Some(choice) = self.tray.as_ref().and_then(|t| t.show_menu(&items)) else { return };
        match choice {
            OPEN_CONFIG => shell_open("notepad.exe", Some(&self.paths.config.display().to_string())),
            OPEN_DATA => shell_open(&self.paths.data_dir.display().to_string(), None),
            // Own thread: a modal box here would hold the app borrow and stall hotkeys.
            ABOUT => {
                std::thread::spawn(|| message_box(&nmi_notes::about::text(), MB_ICONINFORMATION));
            }
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
                            self.set_list_view(self.store.view_for_note(id, self.last_picked));
                            self.list_select = Some(id);
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
                Some(id) => self.show_note(id, 0, false),
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
            Command::Help => {
                let rows = Command::ALL.iter().map(|c| (c.binding(&self.config).to_owned(), c.label().to_owned())).collect();
                help::show(rows);
            }
        }
    }

    fn create_current(&mut self, text: String) -> Option<NoteId> {
        let changes = self.apply(Op::Create { text }, None)?;
        let id = changes.iter().find_map(|c| match c {
            Change::Created { id } => Some(*id),
            _ => None,
        })?;
        self.apply(Op::SetCurrent { id: Some(id) }, None);
        self.set_list_view(ListView::Ungrouped);
        self.list_select = Some(id);
        Some(id)
    }

    /// Remembers what the list opens on next.
    fn set_list_view(&mut self, v: ListView) {
        if self.list_view != v {
            self.list_view = v;
            self.writer.send(Job::SetListView(v));
        }
    }

    /// Keeps the remembered views in step with group changes: the Current
    /// note leaving Ungrouped moves the view to its group, and a deleted
    /// group stops being remembered.
    fn track_views(&mut self, changes: &[Change], current_was_ungrouped: bool) {
        let current = self.store.current();
        for ch in changes {
            match ch {
                Change::Membership { ids } => {
                    if let Some(cur) = current.filter(|c| current_was_ungrouped && ids.contains(c)) {
                        let v = self.store.view_for_note(cur, None);
                        if v != ListView::Ungrouped {
                            self.set_list_view(v);
                        }
                    }
                }
                Change::GroupDeleted { id, .. } => {
                    if self.list_view == ListView::Group(*id) {
                        self.set_list_view(ListView::Ungrouped);
                    }
                    if self.back_view == ListView::Group(*id) {
                        self.back_view = ListView::Ungrouped;
                    }
                    if self.last_picked == Some(*id) {
                        self.last_picked = None;
                    }
                }
                _ => {}
            }
        }
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
        let current_was_ungrouped =
            self.store.current().and_then(|c| self.store.note(c)).is_some_and(|n| n.groups.is_empty());
        match self.store.dispatch(op, now_ms()) {
            Ok(changes) => {
                self.persist(&changes);
                self.track_views(&changes, current_was_ungrouped);
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
            match ch {
                Change::Current { new, .. } => self.writer.send(Job::SetCurrent(*new)),
                Change::GroupSaved { id } => {
                    if let Some(g) = self.store.group(*id) {
                        self.writer.send(Job::UpsertGroup(g.clone()));
                    }
                }
                Change::GroupDeleted { id, .. } => self.writer.send(Job::DeleteGroup(*id)),
                _ => {}
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
                Change::Protected { id, .. } => self.refresh_editor_chrome(*id),
                Change::Membership { ids } => {
                    for id in ids {
                        self.refresh_editor_chrome(*id);
                    }
                }
                // Group names show on every editor's Groups button.
                Change::GroupSaved { .. } | Change::GroupDeleted { .. } => {
                    for k in self.editors.keys().copied().collect::<Vec<_>>() {
                        self.refresh_editor_chrome_for(k);
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
            // Group errors can't come from a text edit.
            Err(StoreError::StaleRevision { .. })
            | Err(StoreError::OutOfRange)
            | Err(StoreError::NoSuchGroup(_))
            | Err(StoreError::EmptyGroupName)
            | Err(StoreError::GroupNameTaken(_)) => {
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
                            let from_list = self.editors.get(&k).is_some_and(|s| s.from_list);
                            self.editors.insert(k, EditorState { note: *id, rev: n.revision, synced: current, from_list });
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
        if st.from_list {
            status.push_str("   ·   Esc: back to list");
        }
        editor::set_status(h, &status);
        editor::set_protected(h, n.protected);
        let names = self.store.group_names(n.id);
        let label = if names.is_empty() { "Groups ▾".to_owned() } else { format!("Groups: {} ▾", ellipsize(&names, 22)) };
        editor::set_groups_label(h, &menu_text(&label));
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
    /// opens one here. `cascade` offsets multiple windows opened together;
    /// `from_list` makes Esc in that window return to the list.
    fn show_note(&mut self, id: NoteId, cascade: i32, from_list: bool) {
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
            if let Some(st) = self.editors.get_mut(&k) {
                st.from_list = from_list;
            }
            self.refresh_editor_chrome_for(k);
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
        self.editors.insert(win::key(h), EditorState { note: id, rev, synced: text.encode_utf16().collect(), from_list });
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
        self.show_list_with(None);
    }

    /// `back`: returning from an editor — restore the filter and select the
    /// note that was being edited.
    fn show_list_with(&mut self, back: Option<(&str, NoteId)>) {
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
        let select = if back.is_none() { self.list_select.take() } else { None };
        if let Some(k) = existing {
            let h = win::hwnd(k);
            if let Some((filter, id)) = back {
                listwin::select_view(h, self.back_view);
                listwin::set_filter(h, filter);
                self.refresh_list(h);
                listwin::select_only(h, id);
            } else if listwin::view(h) != self.list_view || select.is_some() {
                listwin::select_view(h, self.list_view);
                self.refresh_list(h);
                if let Some(id) = select {
                    listwin::select_only(h, id);
                }
            }
            listwin::show_at(h, r);
            self.log_visible();
            listwin::focus(h);
            if back.is_some() {
                listwin::focus_list(h);
            }
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
        self.refresh_list(h); // loads the group dropdown
        match back {
            Some((filter, _)) => {
                listwin::select_view(h, self.back_view);
                listwin::set_filter(h, filter);
            }
            None => listwin::select_view(h, self.list_view),
        }
        self.refresh_list(h);
        if let Some(id) = back.map(|(_, id)| id).or(select) {
            listwin::select_only(h, id);
        }
        listwin::show_at(h, r);
        self.log_visible();
        listwin::focus(h);
        if back.is_some() {
            listwin::focus_list(h);
        }
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
        listwin::set_groups(h, self.store.groups().iter().map(|g| (g.id, g.name.clone())).collect());
        let view = listwin::view(h);
        let filter = listwin::filter_text(h);
        let now = now_ms();
        let current = self.store.current();
        let rows: Vec<listwin::Row> = self
            .store
            .list(&filter, view)
            .into_iter()
            .filter_map(|id| self.store.note(id))
            .map(|n| listwin::Row {
                id: n.id,
                current: Some(n.id) == current,
                protected: n.protected,
                preview: self.preview(n.id, 200),
                groups: self.store.group_names(n.id),
                updated_rel: relative_time(now, n.updated),
                updated_abs: timefmt::local_datetime(n.updated),
                size: compact_count(n.text.chars().count()),
            })
            .collect();
        let shown = rows.len();
        listwin::populate(h, rows);
        let count = self.store.list("", ListView::All).len();
        let title = match view {
            ListView::All if filter.trim().is_empty() => format!("All notes ({count})"),
            ListView::All => format!("All notes ({shown} of {count})"),
            ListView::Ungrouped => format!("Ungrouped notes ({shown} of {count})"),
            ListView::Group(g) => {
                let name = self.store.group(g).map_or(String::new(), |g| quoted(&g.name));
                format!("Notes in {name} ({shown} of {count})")
            }
        };
        let title = format!("{title} — nmi-notes");
        editor::set_caption(h, &title);
    }

    // ---- groups ---------------------------------------------------------------

    fn editor_groups_menu(&mut self, k: isize) {
        let h = win::hwnd(k);
        let Some(id) = self.editors.get(&k).map(|s| s.note) else { return };
        let Some(note) = self.store.live(id) else { return };
        let groups: Vec<(GroupId, String)> = self.store.groups().iter().map(|g| (g.id, g.name.clone())).collect();
        let mut items: Vec<menu::Item> = groups
            .iter()
            .enumerate()
            .map(|(i, (g, name))| menu::Item::Cmd {
                id: MENU_GROUP + i as u32,
                label: menu_text(name),
                checked: note.groups.contains(g),
                enabled: true,
            })
            .collect();
        if !items.is_empty() {
            items.push(menu::Item::Sep);
        }
        items.push(menu::Item::cmd(MENU_NEW_GROUP, "New group…"));
        match menu::popup(h, &items, Some(editor::groups_menu_anchor(h))) {
            Some(MENU_NEW_GROUP) => {
                let purpose = PromptPurpose::NewGroup { add: vec![id] };
                self.open_prompt(h, "New group", "Name for the new group (this note joins it):", "", purpose);
                return;
            }
            Some(c) if c >= MENU_GROUP => {
                if let Some((g, _)) = groups.get((c - MENU_GROUP) as usize) {
                    let member = self.store.note(id).is_some_and(|n| n.groups.contains(g));
                    let op = if member {
                        Op::RemoveFromGroup { ids: vec![id], group: *g }
                    } else {
                        Op::AddToGroup { ids: vec![id], group: *g }
                    };
                    self.apply(op, None);
                }
            }
            _ => {}
        }
        editor::focus(h);
    }

    fn list_menu(&mut self, h: HWND, at: Option<POINT>) {
        let sel = listwin::selected_ids(h);
        let groups: Vec<(GroupId, String)> = self.store.groups().iter().map(|g| (g.id, g.name.clone())).collect();
        let in_group =
            |g: GroupId| sel.iter().filter(|id| self.store.note(**id).is_some_and(|n| n.groups.contains(&g))).count();
        let mut add: Vec<menu::Item> = groups
            .iter()
            .enumerate()
            .map(|(i, (g, name))| menu::Item::Cmd {
                id: MENU_ADD + i as u32,
                label: menu_text(name),
                checked: false,
                enabled: in_group(*g) < sel.len(),
            })
            .collect();
        if !add.is_empty() {
            add.push(menu::Item::Sep);
        }
        add.push(menu::Item::cmd(MENU_NEW_GROUP_ADD, "New group…"));
        let remove: Vec<menu::Item> = groups
            .iter()
            .enumerate()
            .filter(|(_, (g, _))| in_group(*g) > 0)
            .map(|(i, (_, name))| menu::Item::cmd(MENU_REMOVE + i as u32, menu_text(name)))
            .collect();
        let filtered = match listwin::view(h) {
            ListView::Group(g) => self.store.group(g).map(|g| (g.id, g.name.clone())),
            _ => None,
        };
        let mut items = vec![
            menu::Item::Sub { label: "Add to group".into(), enabled: !sel.is_empty(), items: add },
            menu::Item::Sub { label: "Remove from group".into(), enabled: !remove.is_empty(), items: remove },
            menu::Item::Sep,
            menu::Item::cmd(MENU_NEW_GROUP, "New group…"),
        ];
        if let Some((_, name)) = &filtered {
            items.push(menu::Item::cmd(MENU_RENAME_GROUP, format!("Rename group {}…", menu_text(&quoted(name)))));
            items.push(menu::Item::cmd(MENU_DELETE_GROUP, format!("Delete group {}", menu_text(&quoted(name)))));
        }
        let Some(choice) = menu::popup(h, &items, at) else { return };
        let n = sel.len();
        match choice {
            MENU_NEW_GROUP => {
                self.open_prompt(h, "New group", "Name for the new group:", "", PromptPurpose::NewGroup { add: vec![] })
            }
            MENU_NEW_GROUP_ADD => {
                let hint = format!("Name for the new group ({n} note{} join{} it):", plural(n), if n == 1 { "s" } else { "" });
                self.open_prompt(h, "New group", &hint, "", PromptPurpose::NewGroup { add: sel });
            }
            MENU_RENAME_GROUP => {
                if let Some((g, name)) = filtered {
                    self.open_prompt(h, "Rename group", "New name for the group:", &name, PromptPurpose::RenameGroup(g));
                }
            }
            MENU_DELETE_GROUP => {
                if let Some((g, name)) = filtered {
                    if self.apply(Op::DeleteGroup { id: g }, None).is_some() {
                        toast::show(&format!("Deleted group {} — its notes are kept", quoted(&name)));
                    }
                }
            }
            c if (MENU_REMOVE..MENU_REMOVE + 1000).contains(&c) => {
                if let Some((g, name)) = groups.get((c - MENU_REMOVE) as usize) {
                    if self.apply(Op::RemoveFromGroup { ids: sel, group: *g }, None).is_some() {
                        toast::show(&format!("Removed {n} note{} from {}", plural(n), quoted(name)));
                    }
                }
            }
            c if (MENU_ADD..MENU_ADD + 1000).contains(&c) => {
                if let Some((g, name)) = groups.get((c - MENU_ADD) as usize) {
                    if self.apply(Op::AddToGroup { ids: sel, group: *g }, None).is_some() {
                        toast::show(&format!("Added {n} note{} to {}", plural(n), quoted(name)));
                    }
                }
            }
            _ => {}
        }
    }

    fn open_prompt(&mut self, owner: HWND, title: &str, hint: &str, initial: &str, purpose: PromptPurpose) {
        if let Some(p) = self.prompt.take() {
            unsafe {
                let _ = DestroyWindow(win::hwnd(p.key));
            }
        }
        let dpi = unsafe { GetDpiForWindow(owner) }.max(96);
        let font = self.fonts.get(1, dpi, &self.config);
        if let Some(h) = prompt::open(owner, title, hint, initial, font) {
            self.prompt = Some(Prompt { key: win::key(h), owner: win::key(owner), purpose });
        }
    }

    /// Closes the prompt; a list owner gets focus back on its rows (where
    /// the menu was used), an editor on its text.
    fn close_prompt(&mut self, k: isize) -> Option<Prompt> {
        let p = self.prompt.take_if(|p| p.key == k);
        unsafe {
            let _ = DestroyWindow(win::hwnd(k));
        }
        if let Some(p) = &p {
            let owner = win::hwnd(p.owner);
            if unsafe { IsWindow(Some(owner)).as_bool() } {
                if self.lists.contains(&p.owner) {
                    listwin::focus_list(owner);
                } else {
                    editor::focus(owner);
                }
            }
        }
        p
    }

    /// Validates the prompt's text; on success closes it and acts.
    fn prompt_submit(&mut self, k: isize) {
        let h = win::hwnd(k);
        let Some(p) = self.prompt.as_ref().filter(|p| p.key == k) else { return };
        let existing = match p.purpose {
            PromptPurpose::NewGroup { .. } => None,
            PromptPurpose::RenameGroup(g) => Some(g),
        };
        let name = match self.store.check_group_name(&prompt::text(h), existing) {
            Ok(name) => name,
            Err(StoreError::GroupNameTaken(other)) => {
                prompt::set_error(h, &format!("A group named {} already exists", quoted(&other)));
                return;
            }
            Err(_) => {
                prompt::set_error(h, "Enter a name for the group");
                return;
            }
        };
        let Some(Prompt { purpose, .. }) = self.close_prompt(k) else { return };
        match purpose {
            PromptPurpose::NewGroup { add } => {
                let Some(changes) = self.apply(Op::CreateGroup { name: name.clone() }, None) else { return };
                let Some(Change::GroupSaved { id }) = changes.first() else { return };
                let n = add.len();
                if n > 0 {
                    self.apply(Op::AddToGroup { ids: add, group: *id }, None);
                    toast::show(&format!("Created group {} — added {n} note{}", quoted(&name), plural(n)));
                } else {
                    toast::show(&format!("Created group {}", quoted(&name)));
                }
            }
            PromptPurpose::RenameGroup(g) => {
                if self.apply(Op::RenameGroup { id: g, name: name.clone() }, None).is_some() {
                    toast::show(&format!("Renamed group to {}", quoted(&name)));
                }
            }
        }
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
