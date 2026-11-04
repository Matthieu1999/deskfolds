#![windows_subsystem = "windows"]

mod config;
mod dragdrop;
mod panel;
mod settings;
mod store;
mod ui;
mod util;

use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Ole::OleInitialize;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use panel::{LOCKED, WM_APP_SHOW_MENU};
use store::{Change, Store, WM_APP_CONFLICT, WM_APP_REFRESH};
use ui::{Ask, MenuEntry};
use util::{error_box, wide};

const MAIN_CLASS: &str = "deskfolds.main";
const WM_APP_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;
const DESKTOP_RETRY_TIMER: usize = 1;
/// A single click on the tray icon opens the menu once a double-click is ruled out.
const TRAY_CLICK_TIMER: usize = 2;

const ID_EDIT_FOLDER: usize = 100;
const ID_SETTINGS: usize = 101;
const ID_NEW_FOLDER: usize = 102;
const ID_LOCK: usize = 103;
const ID_AUTOSTART: usize = 104;
const ID_QUIT: usize = 105;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static MAIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static PANELS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    /// The "changed outside deskfolds" question is on screen.
    static ASKING: Cell<bool> = const { Cell::new(false) };
    /// Ignore the button release that ends a double-click on the tray icon.
    static SKIP_CLICK: Cell<bool> = const { Cell::new(false) };
}

fn main_window() -> HWND {
    MAIN.get()
}

/// A copy of the panel windows, safe to use while they handle messages.
fn panels() -> Vec<HWND> {
    PANELS.with(|p| p.borrow().clone())
}

fn panel_window(id: u32) -> Option<HWND> {
    panels().into_iter().find(|&h| panel::id_of(h) == Some(id))
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

/// Recreates every panel from the store.
fn rebuild_panels(main: HWND) {
    destroy_panels();
    let desktop = desktop_window();
    if desktop.is_null() {
        // Explorer is not ready yet (early at logon): try again shortly.
        unsafe { SetTimer(main, DESKTOP_RETRY_TIMER, 1000, None) };
        return;
    }
    let (settings, list) = store::snapshot();
    let created: Vec<HWND> =
        list.iter().map(|p| panel::create(p, &settings, desktop, main)).filter(|h| !h.is_null()).collect();
    PANELS.with(|p| *p.borrow_mut() = created);
}

/// Redraws only what the store says changed.
fn apply_changes(main: HWND, changes: &[Change]) {
    if changes.is_empty() {
        return;
    }
    if changes.contains(&Change::All) {
        rebuild_panels(main);
        return;
    }
    let (settings, list) = store::snapshot();
    let find = |id: u32| list.iter().find(|p| p.id == id);
    for change in changes {
        match *change {
            Change::Removed(id) => {
                if let Some(hwnd) = panel_window(id) {
                    PANELS.with(|p| p.borrow_mut().retain(|&h| h != hwnd));
                    unsafe { DestroyWindow(hwnd) };
                }
            }
            Change::Added(id) => {
                let desktop = desktop_window();
                if let Some(p) = find(id)
                    && !desktop.is_null()
                    && panel_window(id).is_none()
                {
                    let hwnd = panel::create(p, &settings, desktop, main);
                    if !hwnd.is_null() {
                        PANELS.with(|list| list.borrow_mut().push(hwnd));
                    }
                }
            }
            Change::Panel(id) => {
                if let (Some(p), Some(hwnd)) = (find(id), panel_window(id)) {
                    panel::update(hwnd, p, &settings);
                }
            }
            Change::Settings | Change::All => {}
        }
    }
    if changes.contains(&Change::Settings) {
        for hwnd in panels() {
            if let Some(p) = panel::id_of(hwnd).and_then(find) {
                panel::update(hwnd, p, &settings);
            }
        }
    }
}

fn reposition_panels() {
    for hwnd in panels() {
        panel::reposition(hwnd);
    }
}

/// Keeps the folder selected in the settings window open and outlined.
pub fn highlight_panel(id: Option<u32>) {
    for hwnd in panels() {
        panel::set_highlight(hwnd, id.is_some() && panel::id_of(hwnd) == id);
    }
}

/// Unlocked, every panel opens with a blue border and can be moved, resized
/// and edited. Positions are saved as soon as a panel is moved.
pub fn set_locked(locked: bool) {
    LOCKED.store(locked, Ordering::Relaxed);
    for hwnd in panels() {
        panel::set_editing(hwnd, !locked);
    }
    settings::on_store_changed();
}

pub fn toggle_lock() {
    set_locked(!LOCKED.load(Ordering::Relaxed));
}

/// Creates "New folder" at a free spot and returns its id.
pub fn new_folder() -> u32 {
    let taken: Vec<(i32, i32)> = store::with(|s| s.panels.iter().map(|p| (p.x, p.y)).collect());
    let (x, y) = panel::free_spot(&taken);
    store::change(|s| {
        let title = s.unique_title("New folder");
        s.add_panel(&title, x, y)
    })
}

/// Loads config.toml again, after it was edited by hand.
pub fn reload_config() {
    if let Err(e) = store::with(|s| s.load_file()) {
        error_box(&e);
    }
}

/// If config.toml was edited by hand, asks which version to keep before
/// anything else can overwrite it.
fn check_config_file() {
    if store::with(|s| s.has_conflict() || s.changed_outside()) {
        resolve_conflict();
    }
}

fn resolve_conflict() {
    const LOAD: i32 = 1000;
    const KEEP: i32 = 1001;
    if ASKING.replace(true) {
        return;
    }
    let owner = settings::owner().unwrap_or_else(main_window);
    unsafe { SetForegroundWindow(owner) };
    let (choice, _) = ui::ask(
        owner,
        &Ask {
            instruction: "config.toml was changed outside deskfolds",
            content: "The folders on screen no longer match the file. Which version do you want to keep? \
                      The other one is saved as a backup, so nothing is lost.",
            buttons: &[
                (LOAD, "Load the file\nThe folders are rebuilt from the file. If it has an error, nothing changes."),
                (KEEP, "Keep the folders on screen\nThe file is replaced by what you see now."),
            ],
            default_button: LOAD,
            links: true,
            cancel: false,
            warning: true,
            ..Default::default()
        },
    );
    let result = if choice == KEEP { store::with(|s| s.keep_screen()) } else { store::with(|s| s.load_file()) };
    ASKING.set(false);
    if let Err(e) = result {
        error_box(&e);
    }
}

fn open_settings(panel: Option<u32>) {
    check_config_file();
    settings::open(panel, false);
}

fn quit(main: HWND) {
    if store::with(|s| s.has_conflict()) {
        resolve_conflict();
    }
    unsafe { DestroyWindow(main) };
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let text: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..text.len()].copy_from_slice(&text);
    dst[text.len()] = 0;
}

fn add_tray_icon(main: HWND) {
    let mut nid = tray_data(main);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_APP_TRAY;
    let shell32 = wide("shell32.dll");
    unsafe { ExtractIconExW(shell32.as_ptr(), 3, null_mut(), &mut nid.hIcon, 1) };
    copy_wide(&mut nid.szTip, "deskfolds");
    unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
}

fn remove_tray_icon(main: HWND) {
    let nid = tray_data(main);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
}

/// A notification from the tray icon, for problems that need no answer.
fn tray_notice(main: HWND, text: &str) {
    let mut nid = tray_data(main);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = NIIF_WARNING;
    copy_wide(&mut nid.szInfoTitle, "deskfolds");
    copy_wide(&mut nid.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
}

fn tray_data(main: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = main;
    nid.uID = TRAY_ID;
    nid
}

/// The tray menu. Opened from a panel, it starts with that folder's settings.
fn show_menu(main: HWND, panel: Option<u32>) {
    check_config_file();
    let title = panel.and_then(|id| store::with(|s| s.panel(id).map(|p| p.title.clone())));
    let edit_label = title.map(|t| format!("Edit '{}'…", ui::menu_text(&t)));
    let locked = LOCKED.load(Ordering::Relaxed);
    let autostart = if util::autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    let mut entries: Vec<MenuEntry> = Vec::new();
    if let Some(label) = &edit_label {
        entries.extend([(ID_EDIT_FOLDER, label.as_str(), 0), ui::SEPARATOR]);
    }
    entries.extend([
        (ID_SETTINGS, "Settings…", 0),
        (ID_NEW_FOLDER, "New folder", 0),
        (ID_LOCK, if locked { "Unlock panels (move and edit)" } else { "Lock panels" }, 0),
        ui::SEPARATOR,
        (ID_AUTOSTART, "Start with Windows", autostart),
        ui::SEPARATOR,
        (ID_QUIT, "Quit", 0),
    ]);
    let mut at = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut at) };
    match ui::popup_menu(main, &entries, at) {
        ID_EDIT_FOLDER => open_settings(panel),
        ID_SETTINGS => open_settings(None),
        ID_NEW_FOLDER => {
            let id = new_folder();
            settings::open(Some(id), true);
        }
        ID_LOCK => toggle_lock(),
        ID_AUTOSTART => {
            util::set_autostart(!util::autostart_enabled());
            settings::on_store_changed();
        }
        ID_QUIT => quit(main),
        _ => {}
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) {
            // Explorer restarted: the tray icon and the desktop window are new.
            add_tray_icon(hwnd);
            rebuild_panels(hwnd);
            settings::on_store_changed();
            return 0;
        }
        match msg {
            WM_APP_TRAY => {
                match (lparam & 0xFFFF) as u32 {
                    WM_LBUTTONUP => {
                        if !SKIP_CLICK.replace(false) {
                            SetTimer(hwnd, TRAY_CLICK_TIMER, GetDoubleClickTime(), None);
                        }
                    }
                    WM_LBUTTONDBLCLK => {
                        KillTimer(hwnd, TRAY_CLICK_TIMER);
                        SKIP_CLICK.set(true);
                        open_settings(None);
                    }
                    WM_RBUTTONUP => show_menu(hwnd, None),
                    _ => {}
                }
                0
            }
            WM_APP_SHOW_MENU => {
                show_menu(hwnd, (wparam != 0).then_some(wparam as u32));
                0
            }
            WM_APP_REFRESH => {
                // While a shortcut is dragged out of a panel, the drag's end posts this again.
                if !panel::dragging() {
                    let changes = store::with(|s| s.take_changes());
                    apply_changes(hwnd, &changes);
                    settings::on_store_changed();
                }
                0
            }
            WM_APP_CONFLICT => {
                check_config_file();
                0
            }
            WM_DISPLAYCHANGE => {
                reposition_panels();
                0
            }
            WM_SETTINGCHANGE if wparam == SPI_SETWORKAREA as usize => {
                reposition_panels();
                0
            }
            WM_TIMER => {
                KillTimer(hwnd, wparam);
                match wparam {
                    DESKTOP_RETRY_TIMER => {
                        rebuild_panels(hwnd);
                        settings::on_store_changed();
                    }
                    TRAY_CLICK_TIMER => show_menu(hwnd, None),
                    _ => {}
                }
                0
            }
            WM_DESTROY => {
                settings::close();
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
        // Also set by the manifest; kept for builds without it.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let mutex_name = wide("Local\\deskfolds.single-instance");
        let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // OLE, not just COM: drag and drop needs it.
        OleInitialize(null());
        ui::enable_dark_mode();

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
        settings::register_class();

        // Hidden top-level window: receives tray, menu and store messages.
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
        MAIN.set(main);
        let (data, notices) = Store::open(config::path(), main);
        store::init(data);
        add_tray_icon(main);
        rebuild_panels(main);
        if !notices.is_empty() {
            tray_notice(main, &notices.join("\n"));
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if settings::translate(&msg) {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
