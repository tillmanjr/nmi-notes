// SPDX-License-Identifier: AGPL-3.0-or-later
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod selftest;
mod win;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use nmi_notes::model::store::Store;
use nmi_notes::persist::{self, Job, Writer};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetMessageW,
    PostMessageW, RegisterClassExW, RegisterWindowMessageW, TranslateMessage, ASFW_ANY, MSG, WINDOW_EX_STYLE,
    WM_CLOSE, WM_CONTEXTMENU, WM_ENDSESSION, WM_HOTKEY, WM_LBUTTONDBLCLK, WM_QUERYENDSESSION, WM_RBUTTONUP, WM_TIMER,
    WNDCLASSEXW, WS_OVERLAPPED,
};

use app::{Action, Command};

const MAIN_CLASS: PCWSTR = w!("NmiNotesMain");
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn main() {
    let data_dir = env_dir("LOCALAPPDATA").join("nmi-notes");
    let config_path = env_dir("APPDATA").join("nmi-notes").join("config.toml");
    win::logging::init(data_dir.join("logs"));

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_LISTVIEW_CLASSES | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    win::theme::init();

    if std::env::args().any(|a| a == "--version" || a == "-V") {
        // Release builds are GUI-subsystem: borrow the shell's console unless
        // stdout is already redirected.
        unsafe {
            use windows::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
            if GetStdHandle(STD_OUTPUT_HANDLE).map_or(true, |h| h.is_invalid() || h.0.is_null()) {
                let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
        println!("{}", nmi_notes::about::text());
        return;
    }
    // Installer hook: `--autostart on|off` records the choice in config.toml
    // (the app re-applies it at every launch) and updates the Run key now.
    if let Some(i) = std::env::args().position(|a| a == "--autostart") {
        let enabled = match std::env::args().nth(i + 1).as_deref() {
            Some("on") => true,
            Some("off") => false,
            _ => std::process::exit(2),
        };
        if let Err(e) = nmi_notes::config::set_start_with_windows(&config_path, enabled) {
            log!("--autostart: cannot write {}: {e}", config_path.display());
            std::process::exit(1);
        }
        win::autostart::apply(enabled);
        return;
    }
    if std::env::args().any(|a| a == "--selftest") {
        std::process::exit(selftest::run());
    }

    // Remote control: `--cmd new|append|show|copy|list|help`, `--exit`.
    let args: Vec<String> = std::env::args().collect();
    let remote = if args.iter().any(|a| a == "--exit") {
        Some(0)
    } else if let Some(i) = args.iter().position(|a| a == "--cmd") {
        match args.get(i + 1).and_then(|n| Command::from_name(n)) {
            Some(c) => Some(c.id() as usize),
            None => {
                eprintln!("usage: nmi-notes [--cmd new|append|show|copy|list|help] [--exit] [--autostart on|off] [--version]");
                std::process::exit(2);
            }
        }
    } else {
        None
    };

    // Single instance: a second launch forwards its request (default: list).
    let _mutex = unsafe { CreateMutexW(None, true, w!("Local\\nmi-notes-single-instance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            if let Ok(h) = FindWindowW(MAIN_CLASS, PCWSTR::null()) {
                let _ = AllowSetForegroundWindow(ASFW_ANY);
                let _ = match remote {
                    Some(w) => PostMessageW(Some(h), app::WM_APP_REMOTE, WPARAM(w), LPARAM(0)),
                    None => PostMessageW(Some(h), app::WM_APP_SHOW_LIST, WPARAM(0), LPARAM(0)),
                };
            }
        }
        return;
    }
    if remote == Some(0) {
        return; // --exit with nothing running
    }

    let (config, config_error) = nmi_notes::config::load_or_create(&config_path);
    let db_path = data_dir.join("notes.db");
    let (conn, loaded) = match persist::open(&db_path) {
        Ok(v) => v,
        Err(e) => {
            log!("cannot open database {}: {e}", db_path.display());
            app::fatal(&format!(
                "nmi-notes cannot open its database:\n{}\n\n{e}\n\nThe app will exit without changing anything.",
                db_path.display()
            ));
            std::process::exit(1);
        }
    };

    win::toast::init();
    win::editor::register_class();
    win::listwin::register_class();
    win::help::register_class();
    win::prompt::register_class();
    let main = match create_main_window() {
        Some(h) => h,
        None => {
            app::fatal("nmi-notes could not create its main window.");
            std::process::exit(1);
        }
    };

    let events = Arc::new(Mutex::new(Vec::new()));
    let ev2 = events.clone();
    let main_key = win::key(main);
    let writer = Writer::spawn(
        conn,
        db_path,
        Box::new(move |s| {
            ev2.lock().unwrap().push(s);
            unsafe {
                let _ = PostMessageW(Some(win::hwnd(main_key)), app::WM_APP_WRITER, WPARAM(0), LPARAM(0));
            }
        }),
    );
    install_panic_hook(writer.sender());

    let store = Store::new(loaded.notes, loaded.groups, loaded.current);
    let list_view = store.valid_view(loaded.list_view.unwrap_or_default());
    let paths = app::Paths { backups: data_dir.join("backups"), data_dir, config: config_path };
    app::install(app::App::new(main, store, writer, config, config_error, paths, events, list_view));
    app::post(Action::Startup);

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if app::pre_translate(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    app::post(Action::EndSession);
}

/// On panic: log, then give the writer a bounded chance to commit.
fn install_panic_hook(tx: mpsc::Sender<Job>) {
    let tx = Mutex::new(tx);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log!("PANIC: {info}");
        if let Ok(tx) = tx.lock() {
            let (ack, done) = mpsc::channel();
            if tx.send(Job::Flush(ack)).is_ok() {
                let _ = done.recv_timeout(Duration::from_secs(2));
            }
        }
        default(info);
    }));
}

fn create_main_window() -> Option<HWND> {
    unsafe {
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(main_wndproc),
            hInstance: win::hinstance(),
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // A hidden top-level window (not message-only) so it receives the
        // TaskbarCreated broadcast.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            MAIN_CLASS,
            w!("nmi-notes"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(win::hinstance()),
            None,
        )
        .ok()
    }
}

unsafe extern "system" fn main_wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if let Some(c) = Command::from_id(wp.0 as i32) {
                post_now(Action::Command(c, Instant::now()));
            }
            LRESULT(0)
        }
        win::tray::WM_TRAY => {
            match win::loword(lp.0 as usize) {
                WM_RBUTTONUP | WM_CONTEXTMENU => post_now(Action::TrayMenu),
                WM_LBUTTONDBLCLK => post_now(Action::Command(Command::NoteList, Instant::now())),
                _ => {}
            }
            LRESULT(0)
        }
        app::WM_APP_SHOW_LIST => {
            post_now(Action::Command(Command::NoteList, Instant::now()));
            LRESULT(0)
        }
        app::WM_APP_REMOTE => {
            match Command::from_id(wp.0 as i32) {
                Some(c) => post_now(Action::Command(c, Instant::now())),
                None if wp.0 == 0 => post_now(Action::Exit),
                None => {}
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            post_now(Action::Exit);
            LRESULT(0)
        }
        app::WM_APP_SPARE => {
            post_now(Action::CreateSpare);
            LRESULT(0)
        }
        app::WM_APP_WRITER => {
            post_now(Action::WriterStatus);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == app::TIMER_BACKUP => {
            post_now(Action::BackupTick);
            LRESULT(0)
        }
        WM_QUERYENDSESSION => {
            post_now(Action::FlushAll);
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wp.0 != 0 {
                post_now(Action::EndSession);
            }
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            post_now(Action::TrayRecreate);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

fn post_now(a: Action) {
    app::post(a);
}
