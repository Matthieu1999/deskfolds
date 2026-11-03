#![windows_subsystem = "windows"]

mod config;
mod panel;
mod util;

use std::cell::RefCell;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use panel::{LOCKED, WM_APP_SHOW_MENU};
use util::wide;

const MAIN_CLASS: &str = "deskfolds.main";
const WM_APP_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;
const DESKTOP_RETRY_TIMER: usize = 1;

const ID_LOCK: usize = 100;
const ID_EDIT: usize = 101;
const ID_RELOAD: usize = 102;
const ID_AUTOSTART: usize = 103;
const ID_QUIT: usize = 104;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

thread_local! {
    static PANELS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
}

fn config_path() -> &'static PathBuf {
    CONFIG_PATH.get_or_init(config::config_path)
}

fn error_box(text: &str) {
    let text = wide(text);
    let title = wide("deskfolds");
    unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
}

/// The window that draws the desktop icons. Panels owned by it survive "Show desktop".
fn desktop_window() -> HWND {
    let class = wide("Progman");
    unsafe { FindWindowW(class.as_ptr(), null()) }
}

fn destroy_panels() {
    let old = PANELS.with(|p| std::mem::take(&mut *p.borrow_mut()));
    for hwnd in old {
        unsafe {
            if IsWindow(hwnd) != 0 {
                DestroyWindow(hwnd);
            }
        }
    }
}

fn rebuild_panels(main: HWND) {
    destroy_panels();
    let desktop = desktop_window();
    if desktop.is_null() {
        // Explorer is not ready yet (early at logon): try again shortly.
        unsafe { SetTimer(main, DESKTOP_RETRY_TIMER, 1000, None) };
        return;
    }
    match config::load(config_path()) {
        Ok(cfg) => {
            let settings = Rc::new(cfg.settings);
            let created: Vec<HWND> = cfg
                .panels
                .into_iter()
                .map(|p| panel::create(p, settings.clone(), desktop, main))
                .filter(|h| !h.is_null())
                .collect();
            PANELS.with(|p| *p.borrow_mut() = created);
        }
        Err(e) => error_box(&e),
    }
}

fn panel_positions() -> Vec<(i32, i32)> {
    PANELS.with(|p| {
        p.borrow()
            .iter()
            .map(|&h| {
                let st = panel::state_ptr(h);
                if st.is_null() { (0, 0) } else { unsafe { ((*st).x, (*st).y) } }
            })
            .collect()
    })
}

fn redraw_panels() {
    PANELS.with(|p| {
        for &h in p.borrow().iter() {
            unsafe { InvalidateRect(h, null(), 0) };
        }
    });
}

fn toggle_lock() {
    let locked = LOCKED.load(Ordering::Relaxed);
    if !locked && let Err(e) = config::save_positions(config_path(), &panel_positions()) {
        error_box(&format!("Positions were not saved.\n\n{e}"));
    }
    LOCKED.store(!locked, Ordering::Relaxed);
    redraw_panels();
}

fn add_tray_icon(main: HWND) {
    let mut nid = tray_data(main);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    let shell32 = wide("shell32.dll");
    unsafe { ExtractIconExW(shell32.as_ptr(), 3, null_mut(), &mut nid.hIcon, 1) };
    let tip = wide("deskfolds");
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
}

fn remove_tray_icon(main: HWND) {
    let nid = tray_data(main);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}

fn tray_data(main: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = main;
    nid.uID = TRAY_ID;
    nid
}

fn show_menu(main: HWND) {
    let locked = LOCKED.load(Ordering::Relaxed);
    let lock_label = if locked { "Unlock panels (move them)" } else { "Lock panels (save positions)" };
    let autostart = if util::autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    let entries: [(u32, usize, &str); 6] = [
        (MF_STRING, ID_LOCK, lock_label),
        (MF_STRING, ID_EDIT, "Edit config"),
        (MF_STRING, ID_RELOAD, "Reload config"),
        (MF_STRING | autostart, ID_AUTOSTART, "Start with Windows"),
        (MF_SEPARATOR, 0, ""),
        (MF_STRING, ID_QUIT, "Quit"),
    ];
    unsafe {
        let menu = CreatePopupMenu();
        for (flags, id, label) in entries {
            let label = wide(label);
            AppendMenuW(menu, flags, id, label.as_ptr());
        }
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(main);
        TrackPopupMenu(menu, TPM_RIGHTBUTTON, pt.x, pt.y, 0, main, null());
        PostMessageW(main, WM_NULL, 0, 0);
        DestroyMenu(menu);
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) {
            // Explorer restarted: the tray icon and the desktop window are new.
            add_tray_icon(hwnd);
            rebuild_panels(hwnd);
            return 0;
        }
        match msg {
            WM_APP_TRAY => {
                let event = (lparam & 0xFFFF) as u32;
                if event == WM_LBUTTONUP || event == WM_RBUTTONUP {
                    show_menu(hwnd);
                }
                0
            }
            WM_APP_SHOW_MENU => {
                show_menu(hwnd);
                0
            }
            WM_TIMER if wparam == DESKTOP_RETRY_TIMER => {
                KillTimer(hwnd, DESKTOP_RETRY_TIMER);
                rebuild_panels(hwnd);
                0
            }
            WM_COMMAND => {
                match wparam & 0xFFFF {
                    ID_LOCK => toggle_lock(),
                    ID_EDIT => util::open_in_notepad(config_path()),
                    ID_RELOAD => rebuild_panels(hwnd),
                    ID_AUTOSTART => util::set_autostart(!util::autostart_enabled()),
                    ID_QUIT => {
                        if !LOCKED.load(Ordering::Relaxed) {
                            toggle_lock();
                        }
                        DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                0
            }
            WM_DESTROY => {
                destroy_panels();
                remove_tray_icon(hwnd);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let mutex_name = wide("Local\\deskfolds.single-instance");
        let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        CoInitializeEx(null(), COINIT_APARTMENTTHREADED as _);

        let taskbar_created = wide("TaskbarCreated");
        TASKBAR_CREATED.store(RegisterWindowMessageW(taskbar_created.as_ptr()), Ordering::Relaxed);

        let class = wide(MAIN_CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(main_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(null()),
            hIcon: null_mut(),
            hCursor: null_mut(),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        panel::register_class();

        // Hidden top-level window: receives tray and menu messages.
        let main = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            class.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        add_tray_icon(main);
        rebuild_panels(main);

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
