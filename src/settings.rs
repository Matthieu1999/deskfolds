//! The settings window, opened from the tray menu or by double-clicking the
//! tray icon: folders and their shortcuts, appearance, behavior, layout,
//! backups.
//!
//! It uses native Windows controls in dark mode. It is built when opened and
//! destroyed when closed, so it costs nothing while closed. A change applies
//! when a field is left or Enter is pressed, a slider is released or a button
//! is clicked. It goes through the store, which saves it at once: there is no
//! Save button.

use std::cell::Cell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{SS_CENTERIMAGE, SS_ENDELLIPSIS, SS_NOPREFIX, SS_PATHELLIPSIS};
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, EM_SETSEL, HIMAGELIST, MEASUREITEMSTRUCT, ODS_DISABLED, ODS_SELECTED, ODT_BUTTON, ODT_LISTBOX, I_IMAGENONE, ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, ILC_COLOR32, ILC_MASK, INITCOMMONCONTROLSEX,
    ImageList_Create, ImageList_Destroy, ImageList_ReplaceIcon, InitCommonControlsEx, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW,
    LVIF_IMAGE, LVIF_PARAM, LVIF_STATE, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETEALLITEMS,
    LVM_ENSUREVISIBLE, LVM_GETITEMW, LVM_GETNEXTITEM, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETBKCOLOR,
    LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETIMAGELIST, LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVM_SETTEXTBKCOLOR,
    LVM_SETTEXTCOLOR, LVN_ITEMCHANGED, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_EX_LABELTIP,
    LVS_NOCOLUMNHEADER, LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, LVSIL_SMALL, NM_CLICK, NMHDR, NMLISTVIEW, TB_THUMBTRACK,
    TBM_SETLINESIZE, TBM_SETPAGESIZE, TBM_SETPOS, TBM_SETRANGEMAX, TBM_SETRANGEMIN, TBS_NOTICKS, ICC_UPDOWN_CLASS, NMUPDOWN,
    UDM_SETBUDDY, UDM_SETRANGE32, UDN_DELTAPOS, UDS_ALIGNRIGHT, UDS_ARROWKEYS, UDS_NOTHOUSANDS,
};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetFocus, GetKeyState, SetFocus, VK_SHIFT};
use windows_sys::Win32::UI::Shell::ExtractIconExW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::arrange;
use crate::config::{self, ANCHORS, MIN_WIDTH, Settings, TitleStyle, anchor_label, format_color, parse_color};
use crate::layout::Shape;
use crate::dragdrop::{self, DropHandler, storage_root};
use crate::panel::{self, DEFAULT_PANEL_ICON, LOCKED};
use crate::store;
use crate::ui::{self, Ask, MenuEntry};
use crate::util::{self, error_box, launch, load_icon, resolve, wide};

const CLASS: &str = "deskfolds.settings";
/// Posted to itself: refresh the controls from the store, once, later.
const WM_APP_SYNC: u32 = WM_APP + 20;
const CLIENT_W: i32 = 880;
const CLIENT_H: i32 = 620;
const SIDE_W: i32 = 188;

const BG: u32 = 0x00202020;
const PANE: u32 = 0x001B1B1B;
const FIELD: u32 = 0x002B2B2B;
const TEXT: u32 = 0x00F2F2F2;
const DIM: u32 = 0x009E9E9E;
const SELECTED: u32 = 0x00383838;
/// #4CC2FF, the accent of the panels.
const ACCENT: u32 = 0x00FFC24C;

const SS_NOTIFY: u32 = 0x0100;
const TBM_GETPOS: u32 = WM_USER;
const BN_CLICKED: u32 = 0;
const LBN_SELCHANGE: u32 = 1;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Folders,
    Appearance,
    Behavior,
    Layout,
    About,
}

const PAGES: [(Page, &str); 5] = [
    (Page::Folders, "Folders"),
    (Page::Appearance, "Appearance"),
    (Page::Behavior, "Behavior"),
    (Page::Layout, "Layout"),
    (Page::About, "About & advanced"),
];

// Control ids. A slider's number box is `id + 1`; a color swatch is its
// "Choose…" button's `id - 1`. Ids 1 and 2 are IDOK and IDCANCEL.
const NAV: i32 = 100;

const F_LIST: i32 = 200;
const F_NEW: i32 = 201;
const F_UP: i32 = 202;
const F_DOWN: i32 = 203;
const F_NAME: i32 = 204;
const F_ICON: i32 = 205;
const F_DELETE: i32 = 206;
const F_WIDTH: i32 = 207;
const F_COLS: i32 = 209;
const F_X: i32 = 211;
const F_Y: i32 = 212;
const F_PLACE: i32 = 213;
const F_STORAGE: i32 = 214;
const F_OPEN: i32 = 215;
const S_LIST: i32 = 216;
const S_ADD: i32 = 217;
const S_REMOVE: i32 = 218;
const S_UP: i32 = 219;
const S_DOWN: i32 = 220;
const S_LOCATE: i32 = 221;
const S_NAME: i32 = 222;
const S_ARGS: i32 = 223;
const S_ICON_V: i32 = 224;
const S_ICON: i32 = 225;
const F_FIT: i32 = 226;
const F_ARRANGE: i32 = 227;
const F_ARRANGED: i32 = 228;

const A_LIGHT: i32 = 300;
const A_DARK: i32 = 301;
const A_SOLID: i32 = 302;
const A_TINT: i32 = 304;
const A_OPACITY: i32 = 305;
const A_ACCENT: i32 = 308;
const A_TEXT: i32 = 310;
const A_HOVER: i32 = 312;
const A_BORDER: i32 = 314;
const A_FONT: i32 = 315;
const A_ICONS: i32 = 317;
const A_ANIM: i32 = 319;
const A_STYLE_CENTERED: i32 = 330;
const A_STYLE_LEFT: i32 = 331;
const A_STYLE_ICON: i32 = 332;
const A_STYLE_FIT: i32 = 333;
const A_LINES: i32 = 334;
const A_COUNT: i32 = 336;
const A_NAMES_1: i32 = 337;
const A_NAMES_2: i32 = 338;

const B_HOVER: i32 = 400;
const B_OPEN: i32 = 401;
const B_CLOSE: i32 = 403;
const B_COLLAPSE: i32 = 405;
const B_LOCK: i32 = 406;
const B_AUTOSTART: i32 = 407;
const B_STORAGE: i32 = 408;
const B_BROWSE: i32 = 409;
const B_STORAGE_DEFAULT: i32 = 410;

const V_CONFIG: i32 = 500;
const V_NOTEPAD: i32 = 501;
const V_RELOAD: i32 = 502;
const V_BACKUPS: i32 = 503;
const V_RESTORE: i32 = 504;
const V_OPEN_BACKUPS: i32 = 505;
const V_RESET: i32 = 506;

const L_SNAP: i32 = 600;
const L_GAP: i32 = 601;
const L_PUSH: i32 = 603;
const L_COVER: i32 = 604;
const L_AUTO: i32 = 605;
const L_DOWN: i32 = 606;
const L_CLOSED: i32 = 607;
const L_OPEN_ALL: i32 = 608;
const L_CELL: i32 = 610;
const L_NEW_W: i32 = 612;
const L_SHAPE_ROW: i32 = 620;
const L_SHAPE_COLUMN: i32 = 621;
const L_SHAPE_GRID: i32 = 622;
const L_COUNT: i32 = 623;
const L_BY: i32 = 625;
/// The 9 anchor buttons, L_ANCHOR + vertical × 3 + horizontal.
const L_ANCHOR: i32 = 630;
const L_OFFSET_X: i32 = 640;
const L_OFFSET_Y: i32 = 642;
const L_SCREEN: i32 = 644;
const L_NOW: i32 = 645;
const L_MEMBERS: i32 = 646;
const L_CHOOSE: i32 = 647;
const L_JOIN: i32 = 648;
const L_ARRANGE_ALL: i32 = 649;
const L_FREE_ALL: i32 = 650;
const L_LINE: i32 = 651;

/// The clickable text next to a check box or a radio button: its id + this.
const CAPTION: i32 = 1000;
/// The up and down arrows of a number box: the box's id + this.
const UPDOWN: i32 = 5000;

const COLOR_BUTTONS: [i32; 5] = [A_TINT, A_ACCENT, A_TEXT, A_HOVER, A_BORDER];
/// Check boxes that have a caption.
const CAPTIONED_CHECKS: [i32; 5] = [F_FIT, F_ARRANGED, A_COUNT, L_SNAP, L_JOIN];
const TITLE_STYLES: [(i32, TitleStyle); 4] = [
    (A_STYLE_CENTERED, TitleStyle::Centered),
    (A_STYLE_LEFT, TitleStyle::Left),
    (A_STYLE_ICON, TitleStyle::IconAbove),
    (A_STYLE_FIT, TitleStyle::Fit),
];

/// Number boxes: (id, smallest, largest, arrow step). Typed values may go past
/// the slider next to it, up to what the config file accepts.
const NUMBERS: [(i32, i32, i32, i32); 17] = [
    (F_WIDTH + 1, MIN_WIDTH, 4000, 10),
    (F_COLS + 1, 1, 20, 1),
    (F_X, -32000, 32000, 1),
    (F_Y, -32000, 32000, 1),
    (A_OPACITY + 1, 0, 100, 5),
    (A_FONT + 1, 8, 40, 1),
    (A_ICONS + 1, 16, 128, 4),
    (A_ANIM + 1, 0, 2000, 50),
    (A_LINES, 1, 3, 1),
    (B_OPEN + 1, 0, 5000, 50),
    (B_CLOSE + 1, 1, 10000, 50),
    (L_GAP, 0, 40, 1),
    (L_CELL + 1, 40, 200, 2),
    (L_NEW_W + 1, MIN_WIDTH, 4000, 10),
    (L_COUNT, 1, 20, 1),
    (L_OFFSET_X, -20000, 20000, 1),
    (L_OFFSET_Y, -20000, 20000, 1),
];

fn number_def(id: i32) -> Option<(i32, i32, i32)> {
    NUMBERS.iter().find(|n| n.0 == id).map(|n| (n.1, n.2, n.3))
}

struct SliderDef {
    id: i32,
    min: i32,
    max: i32,
    step: i32,
    unit: &'static str,
}

const SLIDERS: [SliderDef; 10] = [
    SliderDef { id: F_WIDTH, min: 120, max: 900, step: 10, unit: " px" },
    SliderDef { id: F_COLS, min: 1, max: 10, step: 1, unit: "" },
    SliderDef { id: A_OPACITY, min: 0, max: 100, step: 5, unit: " %" },
    SliderDef { id: A_FONT, min: 8, max: 24, step: 1, unit: " px" },
    SliderDef { id: A_ICONS, min: 16, max: 96, step: 4, unit: " px" },
    SliderDef { id: A_ANIM, min: 0, max: 1000, step: 50, unit: " ms" },
    SliderDef { id: B_OPEN, min: 0, max: 1000, step: 50, unit: " ms" },
    SliderDef { id: B_CLOSE, min: 100, max: 2000, step: 50, unit: " ms" },
    SliderDef { id: L_CELL, min: 56, max: 120, step: 2, unit: " px" },
    SliderDef { id: L_NEW_W, min: 120, max: 900, step: 10, unit: " px" },
];

fn slider_def(id: i32) -> Option<&'static SliderDef> {
    SLIDERS.iter().find(|s| s.id == id)
}

/// Glass presets: (tint, tint opacity, text, hover, border).
fn preset(id: i32) -> (&'static str, f32, &'static str, &'static str, &'static str) {
    match id {
        A_DARK => ("#000000", 0.45, "#F2F2F2", "#3A3A3A", "#444444"),
        A_SOLID => ("#202020", 1.0, "#F2F2F2", "#3D3D3D", "#3A3A3A"),
        _ => ("#FFFFFF", 0.10, "#F2F2F2", "#2E2E2E", "#3A3A3A"),
    }
}

fn color_field(s: &mut Settings, id: i32) -> Option<&mut String> {
    match id {
        A_TINT => Some(&mut s.tint),
        A_ACCENT => Some(&mut s.accent),
        A_TEXT => Some(&mut s.text),
        A_HOVER => Some(&mut s.hover),
        A_BORDER => Some(&mut s.border),
        _ => None,
    }
}

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
}

/// One row of a list: column texts, the file whose icon is shown, and an id.
struct Row {
    texts: Vec<String>,
    icon: String,
    param: isize,
}

struct Win {
    hwnd: HWND,
    dpi: u32,
    font: HFONT,
    heading: HFONT,
    /// Semibold, for the title of a group of settings.
    strong: HFONT,
    bg: HBRUSH,
    /// The line above a group of settings.
    line: HBRUSH,
    pane: HBRUSH,
    field: HBRUSH,
    icons: [HICON; 2],
    page: Page,
    nav: HWND,
    footer: HWND,
    /// Controls of the current page, destroyed when the page changes.
    controls: Vec<HWND>,
    dim: Vec<HWND>,
    swatches: Vec<(HWND, HBRUSH)>,
    /// What each list shows, so it is only refilled when that changes.
    list_sigs: [String; 2],
    /// The folder and shortcut being edited.
    panel: Option<u32>,
    item: Option<u32>,
    /// Set while the controls are filled from the store: their notifications are ignored.
    syncing: bool,
    sync_posted: bool,
    /// The edit box typed in since it was last saved.
    dirty: Option<i32>,
    /// Focus the folder name once the page shows the new folder.
    select_name: bool,
    drop_list: HWND,
    backups: Vec<PathBuf>,
    /// The screens in the Layout page's list, by device name ("" = main).
    screens: Vec<String>,
    /// The arrangement's anchor, for drawing the anchor buttons.
    anchor: (usize, usize),
}

fn win<'a>(hwnd: HWND) -> Option<&'a mut Win> {
    if hwnd.is_null() {
        return None;
    }
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
    (!ptr.is_null()).then(|| unsafe { &mut *ptr })
}

fn send(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    unsafe { SendMessageW(hwnd, msg, wparam, lparam) }
}

fn text_of(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

fn set_window_text(hwnd: HWND, text: &str) {
    if text_of(hwnd) != text {
        let w = wide(text);
        unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
    }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn work_area_at_cursor() -> RECT {
    let mut pt = POINT { x: 0, y: 0 };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    unsafe {
        GetCursorPos(&mut pt);
        if GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut info) == 0 {
            return RECT { left: 0, top: 0, right: 1280, bottom: 800 };
        }
    }
    info.rcWork
}

/// The selected row's id in a list view.
fn selected_param(lv: HWND) -> Option<isize> {
    let index = send(lv, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    if index < 0 {
        return None;
    }
    let mut item: LVITEMW = unsafe { std::mem::zeroed() };
    item.mask = LVIF_PARAM;
    item.iItem = index as i32;
    send(lv, LVM_GETITEMW, 0, &mut item as *mut LVITEMW as isize);
    Some(item.lParam)
}

fn select_row(lv: HWND, index: Option<usize>) {
    let mut state: LVITEMW = unsafe { std::mem::zeroed() };
    state.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
    send(lv, LVM_SETITEMSTATE, usize::MAX, &state as *const LVITEMW as isize);
    if let Some(i) = index {
        state.state = LVIS_SELECTED | LVIS_FOCUSED;
        send(lv, LVM_SETITEMSTATE, i, &state as *const LVITEMW as isize);
        send(lv, LVM_ENSUREVISIBLE, i, 0);
    }
}

pub fn register_class() {
    let class = wide(CLASS);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: unsafe { GetModuleHandleW(null()) },
        hIcon: null_mut(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class.as_ptr(),
    };
    unsafe { RegisterClassW(&wc) };
}

/// Opens the settings window, or brings it to the front. With a folder, shows
/// it in the Folders page; with `rename`, its name is selected for typing.
pub fn open(panel: Option<u32>, rename: bool) {
    let existing = WINDOW.get();
    if let Some(w) = win(existing) {
        if panel.is_some() {
            w.commit_dirty();
            w.panel = panel;
            w.item = None;
            w.select_name = rename;
            if w.page == Page::Folders {
                w.request_sync();
            } else {
                w.show_page(Page::Folders);
            }
        }
        unsafe {
            if IsIconic(existing) != 0 {
                ShowWindow(existing, SW_RESTORE);
            }
            SetForegroundWindow(existing);
        }
        return;
    }

    let init = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES | ICC_UPDOWN_CLASS,
    };
    unsafe { InitCommonControlsEx(&init) };

    let state = Box::new(Win {
        hwnd: null_mut(),
        dpi: 96,
        font: null_mut(),
        heading: null_mut(),
        strong: null_mut(),
        bg: unsafe { CreateSolidBrush(BG) },
        line: unsafe { CreateSolidBrush(0x00383838) },
        pane: unsafe { CreateSolidBrush(PANE) },
        field: unsafe { CreateSolidBrush(FIELD) },
        icons: [null_mut(); 2],
        page: Page::Folders,
        nav: null_mut(),
        footer: null_mut(),
        controls: Vec::new(),
        dim: Vec::new(),
        swatches: Vec::new(),
        list_sigs: Default::default(),
        panel,
        item: None,
        syncing: false,
        sync_posted: false,
        dirty: None,
        select_name: rename,
        drop_list: null_mut(),
        backups: Vec::new(),
        screens: Vec::new(),
        anchor: (0, 0),
    });
    let area = work_area_at_cursor();
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
    let class = wide(CLASS);
    let title = wide("deskfolds — Settings");
    unsafe {
        // Created on the monitor under the cursor, so it gets that monitor's DPI.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            style,
            area.left + 40,
            area.top + 40,
            200,
            200,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            Box::into_raw(state) as *const c_void,
        );
        let Some(w) = win(hwnd) else { return };
        WINDOW.set(hwnd);
        w.hwnd = hwnd;
        w.dpi = GetDpiForWindow(hwnd).max(96);
        ui::dark_window(hwnd);
        w.make_fonts();
        let shell32 = wide("shell32.dll");
        ExtractIconExW(shell32.as_ptr(), 3, &mut w.icons[0], &mut w.icons[1], 1);
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, w.icons[0] as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, w.icons[1] as isize);

        let mut r = RECT { left: 0, top: 0, right: w.s(CLIENT_W), bottom: w.s(CLIENT_H) };
        AdjustWindowRectExForDpi(&mut r, style, 0, 0, w.dpi);
        let (width, height) = (r.right - r.left, r.bottom - r.top);
        let x = area.left + ((area.right - area.left - width) / 2).max(0);
        let y = area.top + ((area.bottom - area.top - height) / 2).max(0);
        SetWindowPos(hwnd, null_mut(), x, y, width, height, SWP_NOZORDER | SWP_NOACTIVATE);

        w.build_frame();
        w.show_page(Page::Folders);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

pub fn close() {
    let hwnd = WINDOW.get();
    if !hwnd.is_null() {
        unsafe { DestroyWindow(hwnd) };
    }
}

/// The settings window, to own dialogs while it is open.
pub fn owner() -> Option<HWND> {
    let hwnd = WINDOW.get();
    (!hwnd.is_null()).then_some(hwnd)
}

/// Something changed (the store, the lock, autostart): refresh the window.
pub fn on_store_changed() {
    if let Some(w) = win(WINDOW.get()) {
        w.request_sync();
    }
}

/// Keyboard navigation (Tab, Enter, Esc) for the settings window.
pub fn translate(msg: &MSG) -> bool {
    let hwnd = WINDOW.get();
    !hwnd.is_null() && unsafe { IsDialogMessageW(hwnd, msg) } != 0
}

static LIST_DROP: DropHandler = DropHandler { over: list_over, leave: list_leave, drop: list_drop };

fn selected_panel() -> Option<u32> {
    win(WINDOW.get()).and_then(|w| w.panel)
}

fn list_over(_: HWND, _: POINT, keys: u32, allowed: u32) -> u32 {
    if selected_panel().is_some() { panel::choose_effect(keys, allowed) } else { 0 }
}

fn list_leave(_: HWND) {}

/// Files dropped on the shortcut list go to the end of the selected folder.
fn list_drop(_: HWND, files: Vec<String>, _: POINT, keys: u32, allowed: u32) -> u32 {
    let Some(panel_id) = selected_panel() else { return 0 };
    let effect = panel::drop_files(panel_id, usize::MAX, files, panel::choose_effect(keys, allowed));
    on_store_changed();
    effect
}

impl Win {
    fn s(&self, v: i32) -> i32 {
        (v as f32 * self.dpi as f32 / 96.0).round() as i32
    }

    fn get(&self, id: i32) -> HWND {
        unsafe { GetDlgItem(self.hwnd, id) }
    }

    fn make_fonts(&mut self) {
        let face = wide("Segoe UI");
        let make = |height: i32, weight: i32| unsafe {
            CreateFontW(
                -height,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET as _,
                OUT_DEFAULT_PRECIS as _,
                CLIP_DEFAULT_PRECIS as _,
                CLEARTYPE_QUALITY as _,
                DEFAULT_PITCH as _,
                face.as_ptr(),
            )
        };
        let (font, heading, strong) =
            (make(self.s(12), FW_NORMAL as i32), make(self.s(22), FW_SEMIBOLD as i32), make(self.s(13), FW_SEMIBOLD as i32));
        for old in [
            std::mem::replace(&mut self.font, font),
            std::mem::replace(&mut self.heading, heading),
            std::mem::replace(&mut self.strong, strong),
        ] {
            if !old.is_null() {
                unsafe { DeleteObject(old) };
            }
        }
    }

    // ---- Creating controls (positions in pixels at 100 % scaling) ----

    fn create(&mut self, class: &str, text: &str, style: u32, ex_style: u32, rect: (i32, i32, i32, i32), id: i32) -> HWND {
        let (class, text) = (wide(class), wide(text));
        let (x, y, w, h) = rect;
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                class.as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                self.s(x),
                self.s(y),
                self.s(w),
                self.s(h),
                self.hwnd,
                id as usize as HMENU,
                GetModuleHandleW(null()),
                null(),
            )
        };
        send(hwnd, WM_SETFONT, self.font as usize, 0);
        self.controls.push(hwnd);
        hwnd
    }

    fn heading(&mut self, text: &str) {
        let h = self.create("STATIC", text, SS_NOPREFIX, 0, (200, 12, 600, 34), -1);
        send(h, WM_SETFONT, self.heading as usize, 0);
    }

    fn label(&mut self, text: &str, x: i32, y: i32, w: i32) -> HWND {
        self.create("STATIC", text, SS_NOPREFIX | SS_CENTERIMAGE | SS_ENDELLIPSIS, 0, (x, y, w, 24), -1)
    }

    fn help(&mut self, text: &str, x: i32, y: i32, w: i32) -> HWND {
        let h = self.create("STATIC", text, SS_NOPREFIX | SS_ENDELLIPSIS, 0, (x, y, w, 18), -1);
        self.dim.push(h);
        h
    }

    /// A dim one-line path, shortened in the middle when too long.
    fn path(&mut self, x: i32, y: i32, w: i32, id: i32) -> HWND {
        let h = self.create("STATIC", "", SS_NOPREFIX | SS_CENTERIMAGE | SS_PATHELLIPSIS, 0, (x, y, w, 24), id);
        self.dim.push(h);
        h
    }

    fn button(&mut self, text: &str, x: i32, y: i32, w: i32, id: i32) -> HWND {
        let h = self.create("BUTTON", text, WS_TABSTOP | BS_PUSHBUTTON as u32, 0, (x, y, w, 28), id);
        ui::dark_control(h, false);
        h
    }

    fn edit(&mut self, x: i32, y: i32, w: i32, id: i32) -> HWND {
        let h = self.create("EDIT", "", WS_TABSTOP | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE, (x, y, w, 24), id);
        ui::dark_control(h, true);
        h
    }

    fn check(&mut self, x: i32, y: i32, id: i32) -> HWND {
        let h = self.create("BUTTON", "", WS_TABSTOP | BS_AUTOCHECKBOX as u32, 0, (x, y, 22, 24), id);
        ui::dark_control(h, false);
        h
    }

    /// A slider, its number box to the right of it, and the unit.
    fn slider(&mut self, x: i32, y: i32, w: i32, id: i32) {
        let h = self.create("msctls_trackbar32", "", WS_TABSTOP | TBS_NOTICKS, 0, (x, y, w, 28), id);
        if let Some(d) = slider_def(id) {
            send(h, TBM_SETRANGEMIN, 0, d.min as isize);
            send(h, TBM_SETRANGEMAX, 1, d.max as isize);
            send(h, TBM_SETLINESIZE, 0, d.step as isize);
            send(h, TBM_SETPAGESIZE, 0, (d.step * 5) as isize);
        }
        self.number(x + w + 8, y + 2, 64, id + 1);
        let unit = slider_def(id).map_or("", |d| d.unit.trim());
        if !unit.is_empty() {
            self.label(unit, x + w + 76, y + 2, 34);
        }
    }

    /// An edit box for a number, with up and down arrows. Typing, the arrows and
    /// the Up/Down keys all save at once (Shift: ten steps).
    fn number(&mut self, x: i32, y: i32, w: i32, id: i32) {
        let edit = self.edit(x, y, w, id);
        let style = UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_NOTHOUSANDS;
        let arrows = self.create("msctls_updown32", "", style, 0, (x + w, y, 16, 24), id + UPDOWN);
        // The position never moves (each step is handled in UDN_DELTAPOS), so a small range is enough.
        send(arrows, UDM_SETRANGE32, (-10isize) as usize, 10);
        send(arrows, UDM_SETBUDDY, edit as usize, 0);
        ui::dark_control(arrows, false);
    }

    /// A drop-down list of choices.
    fn combo(&mut self, x: i32, y: i32, w: i32, id: i32, items: &[&str]) {
        let h = self.create("COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST as u32, 0, (x, y, w, 200), id);
        ui::dark_control(h, true);
        for item in items {
            let text = wide(item);
            send(h, CB_ADDSTRING, 0, text.as_ptr() as isize);
        }
    }

    /// The 3 × 3 buttons that pick the arrangement's anchor.
    fn anchor_buttons(&mut self, x: i32, y: i32) {
        for v in 0..3 {
            for h in 0..3 {
                let style = WS_TABSTOP | BS_OWNERDRAW as u32;
                self.create("BUTTON", "", style, 0, (x + h * 24, y + v * 20, 22, 18), L_ANCHOR + v * 3 + h);
            }
        }
    }

    /// A check box with its caption, which can be clicked too.
    fn check_text(&mut self, x: i32, y: i32, id: i32, text: &str, w: i32) {
        self.check(x, y, id);
        self.create("STATIC", text, SS_NOPREFIX | SS_CENTERIMAGE | SS_NOTIFY, 0, (x + 22, y, w, 24), id + CAPTION);
    }

    /// A radio button with its caption. The window sets which one is checked.
    fn radio(&mut self, x: i32, y: i32, id: i32, text: &str, w: i32) {
        let h = self.create("BUTTON", "", WS_TABSTOP | BS_RADIOBUTTON as u32, 0, (x, y, 22, 24), id);
        ui::dark_control(h, false);
        self.create("STATIC", text, SS_NOPREFIX | SS_CENTERIMAGE | SS_NOTIFY, 0, (x + 22, y, w, 24), id + CAPTION);
    }

    /// A color swatch and its "Choose…" button.
    fn color(&mut self, x: i32, y: i32, id: i32) {
        let swatch = self.create("STATIC", "", SS_NOTIFY, WS_EX_CLIENTEDGE, (x, y, 44, 24), id - 1);
        self.swatches.push((swatch, null_mut()));
        self.button("Choose…", x + 52, y - 2, 96, id);
    }

    fn list(&mut self, rect: (i32, i32, i32, i32), id: i32, columns: &[i32]) -> HWND {
        let style = WS_TABSTOP | WS_VSCROLL | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOCOLUMNHEADER;
        let lv = self.create("SysListView32", "", style, 0, rect, id);
        ui::dark_control(lv, false);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_LABELTIP;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        send(lv, LVM_SETBKCOLOR, 0, PANE as isize);
        send(lv, LVM_SETTEXTBKCOLOR, 0, PANE as isize);
        send(lv, LVM_SETTEXTCOLOR, 0, TEXT as isize);
        let empty = wide("");
        for (i, width) in columns.iter().enumerate() {
            let mut col: LVCOLUMNW = unsafe { std::mem::zeroed() };
            col.mask = LVCF_WIDTH | LVCF_TEXT;
            col.cx = self.s(*width);
            col.pszText = empty.as_ptr() as *mut u16;
            send(lv, LVM_INSERTCOLUMNW, i, &col as *const LVCOLUMNW as isize);
        }
        lv
    }

    fn list_box(&mut self, rect: (i32, i32, i32, i32), id: i32) -> HWND {
        // Drawn by `draw_list_item`: the themed selection is unreadable in dark mode.
        let style = WS_TABSTOP | WS_VSCROLL | (LBS_NOTIFY | LBS_NOINTEGRALHEIGHT | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS) as u32;
        let h = self.create("LISTBOX", "", style, 0, rect, id);
        ui::dark_control(h, false);
        h
    }

    /// The sidebar: page list and a note at the bottom.
    fn build_frame(&mut self) {
        for old in [self.nav, self.footer] {
            if !old.is_null() {
                unsafe { DestroyWindow(old) };
            }
        }
        let before = self.controls.len();
        self.nav = self.list_box((8, 12, SIDE_W - 16, PAGES.len() as i32 * 34 + 4), NAV);
        for (_, name) in PAGES {
            let name = wide(name);
            send(self.nav, LB_ADDSTRING, 0, name.as_ptr() as isize);
        }
        self.footer = self.create("STATIC", "Changes are saved as soon as you make them.", SS_NOPREFIX, 0, (16, CLIENT_H - 52, SIDE_W - 28, 40), -1);
        // Not page controls: they stay when the page changes.
        self.controls.truncate(before);
    }

    // ---- Pages ----

    fn clear_page(&mut self) {
        self.syncing = true;
        if !self.drop_list.is_null() {
            dragdrop::revoke(self.drop_list);
            self.drop_list = null_mut();
        }
        for hwnd in std::mem::take(&mut self.controls) {
            unsafe { DestroyWindow(hwnd) };
        }
        for (_, brush) in std::mem::take(&mut self.swatches) {
            if !brush.is_null() {
                unsafe { DeleteObject(brush) };
            }
        }
        self.dim.clear();
        self.list_sigs = Default::default();
        self.dirty = None;
        self.syncing = false;
    }

    fn show_page(&mut self, page: Page) {
        self.commit_dirty();
        self.clear_page();
        self.page = page;
        let index = PAGES.iter().position(|(p, _)| *p == page).unwrap_or(0);
        self.syncing = true;
        send(self.nav, LB_SETCURSEL, index, 0);
        match page {
            Page::Folders => self.build_folders(),
            Page::Appearance => self.build_appearance(),
            Page::Behavior => self.build_behavior(),
            Page::Layout => self.build_layout(),
            Page::About => self.build_about(),
        }
        self.syncing = false;
        self.sync();
    }

    fn build_folders(&mut self) {
        self.heading("Folders");
        self.list((200, 56, 180, 470), F_LIST, &[132, 28]);
        self.button("New folder", 200, 534, 96, F_NEW);
        self.button("▲", 300, 534, 38, F_UP);
        self.button("▼", 342, 534, 38, F_DOWN);
        self.button("Arrange all…", 200, 568, 180, F_ARRANGE);

        let (rx, cx) = (396, 492);
        self.label("Name", rx, 56, 92);
        self.edit(cx, 56, 196, F_NAME);
        self.button("Icon…", 694, 54, 80, F_ICON);
        self.button("Delete…", 780, 54, 84, F_DELETE);
        self.label("Width", rx, 92, 92);
        self.slider(cx, 90, 230, F_WIDTH);
        self.label("Columns", rx, 124, 92);
        self.slider(cx, 122, 230, F_COLS);
        self.label("Options", rx, 156, 92);
        self.check_text(cx, 156, F_FIT, "Width follows the columns", 160);
        self.check_text(690, 156, F_ARRANGED, "In the arrangement", 150);
        self.label("Position", rx, 190, 92);
        self.label("X", cx, 190, 14);
        self.number(cx + 16, 190, 64, F_X);
        self.label("Y", cx + 90, 190, 14);
        self.number(cx + 106, 190, 64, F_Y);
        self.button("Move on the desktop", 680, 188, 184, F_PLACE);
        self.label("Storage", rx, 226, 92);
        self.path(cx, 226, 290, F_STORAGE);
        self.button("Open", 788, 224, 76, F_OPEN);

        self.label("Shortcuts", rx, 264, 92);
        self.help("Drop files here or on the folder. Ctrl keeps the original.", cx, 267, 290);
        self.button("Add…", 788, 262, 76, S_ADD);
        let lv = self.list((rx, 296, 468, 150), S_LIST, &[160, 70, 214]);
        dragdrop::register(lv, &LIST_DROP);
        self.drop_list = lv;
        self.button("Remove", rx, 452, 90, S_REMOVE);
        self.button("▲", rx + 96, 452, 38, S_UP);
        self.button("▼", rx + 138, 452, 38, S_DOWN);
        self.button("Open file location", rx + 182, 452, 150, S_LOCATE);
        self.label("Shortcut name", rx, 490, 92);
        self.edit(cx, 490, 372, S_NAME);
        self.label("Arguments", rx, 522, 92);
        self.edit(cx, 522, 372, S_ARGS);
        self.label("Icon", rx, 554, 92);
        self.path(cx, 554, 206, S_ICON_V);
        self.button("Change…", 704, 552, 90, S_ICON);
    }

    fn build_appearance(&mut self) {
        self.heading("Appearance");
        let (lx, cx) = (200, 410);
        self.label("Presets", lx, 56, 200);
        self.button("Light glass", cx, 54, 110, A_LIGHT);
        self.button("Dark glass", cx + 116, 54, 110, A_DARK);
        self.button("Solid", cx + 232, 54, 110, A_SOLID);

        self.label("Glass tint", lx, 100, 200);
        self.color(cx, 100, A_TINT);
        self.label("Tint strength", lx, 134, 200);
        self.help("0 = clear glass, 100 = solid color", lx, 156, 200);
        self.slider(cx, 134, 330, A_OPACITY);
        self.label("Accent", lx, 180, 200);
        self.help("Pin marker, edit border, insertion bar", lx, 202, 205);
        self.color(cx, 180, A_ACCENT);
        self.label("Text", lx, 226, 200);
        self.color(cx, 226, A_TEXT);
        self.label("Hover highlight", lx, 260, 200);
        self.help("Grays act like light on the glass", lx, 282, 200);
        self.color(cx, 260, A_HOVER);
        self.label("Border", lx, 306, 200);
        self.color(cx, 306, A_BORDER);

        self.label("Text size", lx, 344, 200);
        self.slider(cx, 342, 330, A_FONT);
        self.label("Icon size", lx, 378, 200);
        self.slider(cx, 376, 330, A_ICONS);
        self.label("Open / close animation", lx, 412, 200);
        self.help("0 = no animation", lx, 434, 200);
        self.slider(cx, 410, 330, A_ANIM);

        self.label("Folder titles", lx, 466, 200);
        self.help("How a closed folder shows its name", lx, 488, 205);
        self.radio(cx, 466, A_STYLE_CENTERED, "Centered", 70);
        self.radio(cx + 94, 466, A_STYLE_LEFT, "Left", 46);
        self.radio(cx + 164, 466, A_STYLE_ICON, "Icon above", 84);
        self.radio(cx + 274, 466, A_STYLE_FIT, "Fit the name", 96);
        self.label("Title lines", lx, 512, 200);
        self.help("A longer name ends with \u{2026}", lx, 534, 205);
        self.number(cx, 514, 64, A_LINES);
        self.label("lines at most", cx + 72, 514, 100);
        self.check_text(cx + 210, 514, A_COUNT, "Show the number of shortcuts", 220);
        self.label("Shortcut names", lx, 558, 200);
        self.radio(cx, 558, A_NAMES_1, "One line", 70);
        self.radio(cx + 94, 558, A_NAMES_2, "Up to two lines", 120);
    }

    fn build_behavior(&mut self) {
        self.heading("Behavior");
        let (lx, cx) = (200, 410);
        self.label("Open folders on hover", lx, 56, 200);
        self.help("Off: open and close a folder by clicking its name", lx, 78, 300);
        self.check(cx + 100, 56, B_HOVER);
        self.label("Hover time before opening", lx, 104, 200);
        self.slider(cx + 100, 102, 230, B_OPEN);
        self.label("Time before closing", lx, 138, 200);
        self.help("After the mouse leaves the folder", lx, 160, 200);
        self.slider(cx + 100, 136, 230, B_CLOSE);
        self.label("Close after launching a shortcut", lx, 186, 250);
        self.help("Pinned folders stay open", lx, 208, 200);
        self.check(cx + 100, 186, B_COLLAPSE);
        self.label("Lock folders in place", lx, 234, 200);
        self.help("Unlocked: move and resize folders, drag shortcuts", lx, 256, 300);
        self.check(cx + 100, 234, B_LOCK);
        self.label("Start with Windows", lx, 282, 200);
        self.check(cx + 100, 282, B_AUTOSTART);

        self.label("Storage folder for new folders", lx, 322, 300);
        self.path(lx, 344, 420, B_STORAGE);
        self.help("Each folder keeps the storage folder it was created with.", lx, 368, 420);
        self.button("Browse…", 640, 322, 110, B_BROWSE);
        self.button("Default", 756, 322, 108, B_STORAGE_DEFAULT);
    }

    fn build_layout(&mut self) {
        self.heading("Layout");
        let (lx, cx) = (200, 410);
        self.label("Snap when moving", lx, 56, 200);
        self.check_text(cx, 56, L_SNAP, "Snap to other folders and to the screen edges", 300);
        self.help("Hold Alt while dragging to place a folder freely.", lx, 78, 420);
        self.label("Gap between folders", lx, 104, 200);
        self.help("For snapping, pushing and arranging", lx, 126, 205);
        self.number(cx, 106, 64, L_GAP);
        self.label("px", cx + 72, 106, 30);
        self.label("When a folder opens", lx, 150, 200);
        self.radio(cx, 150, L_PUSH, "Push the folders below", 150);
        self.radio(cx + 184, 150, L_COVER, "Cover them", 90);
        self.help("Folders stacked right under it slide down while it is open, and come back when it closes.", lx, 172, 600);
        self.label("Open direction", lx, 196, 200);
        self.radio(cx, 196, L_AUTO, "Automatic", 80);
        self.radio(cx + 114, 196, L_DOWN, "Always down", 96);
        self.help("Automatic opens a folder upwards when there is no room below it.", lx, 218, 600);
        self.label("While unlocked", lx, 242, 200);
        self.radio(cx, 242, L_CLOSED, "Folders stay closed, open on hover", 210);
        self.radio(cx + 244, 242, L_OPEN_ALL, "All folders open", 120);
        self.help("Closed folders are easier to arrange.", lx, 264, 420);
        self.label("Cell width", lx, 288, 200);
        self.help("For folders sized by columns", lx, 310, 205);
        self.slider(cx, 288, 330, L_CELL);
        self.label("Width of new folders", lx, 334, 200);
        self.slider(cx, 332, 330, L_NEW_W);
        self.create("STATIC", "", SS_NOPREFIX, 0, (lx, 372, 664, 1), L_LINE);
        let title = self.label("Arrangement", lx, 380, 200);
        send(title, WM_SETFONT, self.strong as usize, 0);
        self.help("The folders in it move and stay together.", cx, 383, 454);
        self.label("Shape", lx, 412, 200);
        self.radio(cx, 412, L_SHAPE_ROW, "Row", 46);
        self.radio(cx + 70, 412, L_SHAPE_COLUMN, "Column", 62);
        self.radio(cx + 156, 412, L_SHAPE_GRID, "Grid", 46);
        self.label("of", 640, 412, 18);
        self.number(660, 412, 64, L_COUNT);
        self.combo(732, 412, 132, L_BY, &["columns", "rows"]);
        self.label("Place on the screen", lx, 448, 200);
        self.help("Or drag its frame while unlocked", lx, 470, 205);
        self.anchor_buttons(cx, 446);
        self.label("Offset X", 490, 448, 52);
        self.number(544, 448, 64, L_OFFSET_X);
        self.label("Y", 618, 448, 14);
        self.number(634, 448, 64, L_OFFSET_Y);
        let screens = arrange::screens();
        let labels: Vec<&str> = screens.iter().map(|s| s.label.as_str()).collect();
        self.combo(710, 448, 154, L_SCREEN, &labels);
        self.screens = screens.iter().map(|s| s.device.clone()).collect();
        self.path(490, 474, 374, L_NOW);
        self.label("Folders in it", lx, 512, 200);
        self.create("STATIC", "", SS_NOPREFIX | SS_CENTERIMAGE, 0, (cx, 512, 60, 24), L_MEMBERS);
        self.button("Choose…", 474, 510, 90, L_CHOOSE);
        self.check_text(580, 512, L_JOIN, "New folders join it", 150);
        self.button("Arrange all folders", cx, 548, 160, L_ARRANGE_ALL);
        self.button("Free all folders", cx + 166, 548, 140, L_FREE_ALL);
        self.help("Drag a folder out of the frame to free it, or into the frame to add it.", lx, 584, 664);
    }

    fn build_about(&mut self) {
        self.heading("About & advanced");
        let lx = 200;
        self.label(&format!("deskfolds {}", env!("CARGO_PKG_VERSION")), lx, 56, 400);
        self.help("Native Rust · MIT license · github.com/Matthieu1999/deskfolds", lx, 78, 420);

        self.label("Configuration file", lx, 116, 300);
        self.path(lx, 138, 420, V_CONFIG);
        self.help("Reload applies changes you made to the file by hand.", lx, 160, 420);
        self.button("Open in Notepad", 640, 116, 130, V_NOTEPAD);
        self.button("Reload", 776, 116, 88, V_RELOAD);

        self.label("Previous versions", lx, 196, 300);
        self.help("deskfolds keeps the last 5 versions of your settings automatically.", lx, 218, 420);
        self.list_box((lx, 242, 420, 120), V_BACKUPS);
        self.button("Restore…", 640, 242, 130, V_RESTORE);
        self.button("Open folder", 640, 276, 130, V_OPEN_BACKUPS);

        self.label("Reset appearance, behavior and layout", lx, 382, 300);
        self.help("Folders, shortcuts and the storage folder are kept. A backup is made first.", lx, 404, 420);
        self.button("Reset…", 640, 382, 130, V_RESET);
    }

    // ---- Filling the controls from the store ----

    fn request_sync(&mut self) {
        if !self.sync_posted {
            self.sync_posted = true;
            unsafe { PostMessageW(self.hwnd, WM_APP_SYNC, 0, 0) };
        }
    }

    fn sync(&mut self) {
        self.sync_posted = false;
        self.syncing = true;
        let (settings, panels) = store::snapshot();
        if self.panel.is_none_or(|id| !panels.iter().any(|p| p.id == id)) {
            self.panel = panels.first().map(|p| p.id);
            self.item = None;
        }
        match self.page {
            Page::Folders => self.sync_folders(&panels, &settings),
            Page::Appearance => self.sync_appearance(&settings),
            Page::Behavior => self.sync_behavior(&settings),
            Page::Layout => self.sync_layout(&settings),
            Page::About => self.sync_about(),
        }
        self.syncing = false;
        crate::highlight_panel(self.panel);
        if self.select_name && self.page == Page::Folders {
            self.select_name = false;
            let name = self.get(F_NAME);
            unsafe { SetFocus(name) };
            send(name, EM_SETSEL, 0, -1);
        }
    }

    fn enable(&self, id: i32, on: bool) {
        unsafe { EnableWindow(self.get(id), on as i32) };
        if number_def(id).is_some() {
            unsafe { EnableWindow(self.get(id + UPDOWN), on as i32) };
        }
    }

    fn set_text(&self, id: i32, text: &str) {
        set_window_text(self.get(id), text);
    }

    /// Leaves an edit box alone while the user is typing in it.
    fn set_edit(&self, id: i32, text: &str) {
        let h = self.get(id);
        if self.dirty == Some(id) && unsafe { GetFocus() } == h {
            return;
        }
        set_window_text(h, text);
    }

    fn set_slider(&self, id: i32, value: i32) {
        send(self.get(id), TBM_SETPOS, 1, value as isize);
        self.set_edit(id + 1, &value.to_string());
    }

    /// Shows a slider's value in its number box while it is dragged.
    fn set_value_label(&self, id: i32, value: i32) {
        self.set_text(id + 1, &value.to_string());
    }

    fn set_radio(&self, id: i32, on: bool) {
        send(self.get(id), BM_SETCHECK, on as usize, 0);
    }

    fn set_check(&self, id: i32, on: bool) {
        send(self.get(id), BM_SETCHECK, on as usize, 0);
    }

    fn checked(&self, id: i32) -> bool {
        send(self.get(id), BM_GETCHECK, 0, 0) == 1
    }

    fn set_swatch(&mut self, id: i32, color: &str) {
        let h = self.get(id - 1);
        if let Some(slot) = self.swatches.iter_mut().find(|(s, _)| *s == h) {
            let brush = unsafe { CreateSolidBrush(parse_color(color, 0)) };
            let old = std::mem::replace(&mut slot.1, brush);
            if !old.is_null() {
                unsafe { DeleteObject(old) };
            }
            unsafe { InvalidateRect(h, null(), 1) };
        }
    }

    /// Fills a list view, only when its rows changed, and selects a row by id.
    fn fill_list(&mut self, id: i32, rows: &[Row], selected: Option<isize>) {
        let lv = self.get(id);
        let slot = if id == F_LIST { 0 } else { 1 };
        let sig: String = rows.iter().map(|r| format!("{}\u{1}{}\u{1}{}\u{2}", r.texts.join("\u{1}"), r.icon, r.param)).collect();
        if self.list_sigs[slot] != sig {
            self.list_sigs[slot] = sig;
            let size = self.s(16);
            unsafe {
                send(lv, WM_SETREDRAW, 0, 0);
                send(lv, LVM_DELETEALLITEMS, 0, 0);
                let images = ImageList_Create(size, size, ILC_COLOR32 | ILC_MASK, rows.len() as i32, 1);
                let old = send(lv, LVM_SETIMAGELIST, LVSIL_SMALL as usize, images as isize);
                if old != 0 {
                    ImageList_Destroy(old as HIMAGELIST);
                }
                for (i, row) in rows.iter().enumerate() {
                    let icon = load_icon(&row.icon, size);
                    let image = if icon.is_null() {
                        I_IMAGENONE
                    } else {
                        let index = ImageList_ReplaceIcon(images, -1, icon);
                        DestroyIcon(icon);
                        index
                    };
                    let text = wide(&row.texts[0]);
                    let mut item: LVITEMW = std::mem::zeroed();
                    item.mask = LVIF_TEXT | LVIF_PARAM | LVIF_IMAGE;
                    item.iItem = i as i32;
                    item.pszText = text.as_ptr() as *mut u16;
                    item.iImage = image;
                    item.lParam = row.param;
                    let index = send(lv, LVM_INSERTITEMW, 0, &item as *const LVITEMW as isize);
                    for (column, text) in row.texts.iter().enumerate().skip(1) {
                        let text = wide(text);
                        let mut sub: LVITEMW = std::mem::zeroed();
                        sub.iSubItem = column as i32;
                        sub.pszText = text.as_ptr() as *mut u16;
                        send(lv, LVM_SETITEMTEXTW, index.max(0) as usize, &sub as *const LVITEMW as isize);
                    }
                }
                send(lv, WM_SETREDRAW, 1, 0);
                InvalidateRect(lv, null(), 1);
            }
        }
        let want = selected.and_then(|p| rows.iter().position(|r| r.param == p));
        let have = selected_param(lv).and_then(|p| rows.iter().position(|r| r.param == p));
        if want != have {
            select_row(lv, want);
        }
    }

    fn sync_folders(&mut self, panels: &[config::Panel], settings: &Settings) {
        let rows: Vec<Row> = panels
            .iter()
            .map(|p| Row {
                texts: vec![p.title.clone(), p.items.len().to_string()],
                icon: p.icon.clone().unwrap_or_else(|| DEFAULT_PANEL_ICON.into()),
                param: p.id as isize,
            })
            .collect();
        self.fill_list(F_LIST, &rows, self.panel.map(|id| id as isize));

        let index = self.panel.and_then(|id| panels.iter().position(|p| p.id == id));
        let panel = index.map(|i| &panels[i]);
        self.enable(F_UP, index.is_some_and(|i| i > 0));
        self.enable(F_DOWN, index.is_some_and(|i| i + 1 < panels.len()));
        for id in [F_NAME, F_ICON, F_DELETE, F_WIDTH, F_WIDTH + 1, F_COLS, F_COLS + 1, F_FIT, F_ARRANGED, F_X, F_Y, F_OPEN, S_ADD, S_LIST] {
            self.enable(id, panel.is_some());
        }
        self.enable(F_ARRANGE, !panels.is_empty());
        self.set_text(F_PLACE, if LOCKED.load(std::sync::atomic::Ordering::Relaxed) { "Move on the desktop" } else { "Done" });

        let Some(p) = panel else {
            for id in [F_NAME, F_X, F_Y, S_NAME, S_ARGS] {
                self.set_edit(id, "");
            }
            self.set_text(F_STORAGE, "No folder yet. Click New folder to create one.");
            self.set_text(S_ICON_V, "");
            self.fill_list(S_LIST, &[], None);
            for id in [S_REMOVE, S_UP, S_DOWN, S_LOCATE, S_NAME, S_ARGS, S_ICON] {
                self.enable(id, false);
            }
            return;
        };
        self.set_edit(F_NAME, &p.title);
        // Following the columns, the width shows the result and cannot be changed.
        self.set_slider(F_WIDTH, p.shown_width(settings));
        self.enable(F_WIDTH, !p.fit_columns);
        self.enable(F_WIDTH + 1, !p.fit_columns);
        self.set_check(F_FIT, p.fit_columns);
        self.set_slider(F_COLS, p.columns);
        self.set_edit(F_X, &p.x.to_string());
        self.set_edit(F_Y, &p.y.to_string());
        // In the arrangement, it places the folder.
        self.set_check(F_ARRANGED, p.arranged);
        self.enable(F_X, !p.arranged);
        self.enable(F_Y, !p.arranged);
        let (storage, stored) =
            store::with(|s| (s.storage_of(p.id), p.items.iter().map(|it| s.is_stored(p.id, it)).collect::<Vec<_>>()));
        self.set_text(F_STORAGE, &storage.display().to_string());

        if self.item.is_some_and(|id| !p.items.iter().any(|it| it.id == id)) {
            self.item = None;
        }
        let rows: Vec<Row> = p
            .items
            .iter()
            .zip(&stored)
            .map(|(it, &moved)| Row {
                texts: vec![
                    it.name.clone(),
                    if moved { "Moved in" } else { "Link" }.into(),
                    if it.args.is_empty() { it.path.clone() } else { format!("{} {}", it.path, it.args) },
                ],
                icon: it.icon.clone().unwrap_or_else(|| it.path.clone()),
                param: it.id as isize,
            })
            .collect();
        self.fill_list(S_LIST, &rows, self.item.map(|id| id as isize));

        let item_index = self.item.and_then(|id| p.items.iter().position(|it| it.id == id));
        let item = item_index.map(|i| &p.items[i]);
        for id in [S_REMOVE, S_LOCATE, S_NAME, S_ARGS, S_ICON] {
            self.enable(id, item.is_some());
        }
        self.enable(S_UP, item_index.is_some_and(|i| i > 0));
        self.enable(S_DOWN, item_index.is_some_and(|i| i + 1 < p.items.len()));
        self.set_edit(S_NAME, item.map_or("", |it| it.name.as_str()));
        self.set_edit(S_ARGS, item.map_or("", |it| it.args.as_str()));
        let icon = match item {
            Some(it) => it.icon.clone().unwrap_or_else(|| "The shortcut's own icon".into()),
            None => String::new(),
        };
        self.set_text(S_ICON_V, &icon);
    }

    fn sync_appearance(&mut self, s: &Settings) {
        for (id, color) in [(A_TINT, &s.tint), (A_ACCENT, &s.accent), (A_TEXT, &s.text), (A_HOVER, &s.hover), (A_BORDER, &s.border)] {
            self.set_swatch(id, color);
        }
        self.set_slider(A_OPACITY, (s.tint_opacity * 100.0).round() as i32);
        self.set_slider(A_FONT, s.font_size);
        self.set_slider(A_ICONS, s.icon_size);
        self.set_slider(A_ANIM, s.animation_ms as i32);
        let style = s.title_style();
        for (id, st) in TITLE_STYLES {
            self.set_radio(id, st == style);
        }
        self.set_edit(A_LINES, &s.title_lines.to_string());
        // The "fit" style always shows one line.
        self.enable(A_LINES, style != TitleStyle::Fit);
        self.set_check(A_COUNT, s.show_count);
        self.set_radio(A_NAMES_1, s.item_lines < 2);
        self.set_radio(A_NAMES_2, s.item_lines >= 2);
    }

    fn sync_behavior(&mut self, s: &Settings) {
        self.set_check(B_HOVER, s.hover_open);
        self.set_slider(B_OPEN, s.open_delay_ms as i32);
        self.set_slider(B_CLOSE, s.close_delay_ms as i32);
        self.enable(B_OPEN, s.hover_open);
        self.enable(B_CLOSE, s.hover_open);
        self.set_check(B_COLLAPSE, s.collapse_after_launch);
        self.set_check(B_LOCK, LOCKED.load(std::sync::atomic::Ordering::Relaxed));
        self.set_check(B_AUTOSTART, util::autostart_enabled());
        let root = storage_root(&s.storage_dir).display().to_string();
        let shown = if s.storage_dir.trim().is_empty() { format!("{root}  (default)") } else { root };
        self.set_text(B_STORAGE, &shown);
        self.enable(B_STORAGE_DEFAULT, !s.storage_dir.trim().is_empty());
    }

    fn sync_layout(&mut self, s: &Settings) {
        self.set_check(L_SNAP, s.snap);
        self.set_edit(L_GAP, &s.gap.to_string());
        self.set_radio(L_PUSH, s.push_below);
        self.set_radio(L_COVER, !s.push_below);
        self.set_radio(L_AUTO, s.open_upwards);
        self.set_radio(L_DOWN, !s.open_upwards);
        self.set_radio(L_CLOSED, !s.unlocked_open_all);
        self.set_radio(L_OPEN_ALL, s.unlocked_open_all);
        self.set_slider(L_CELL, s.cell_width);
        self.set_slider(L_NEW_W, s.new_folder_width);

        let spec = s.arrange_spec();
        let grid = spec.shape == Shape::Grid;
        self.set_radio(L_SHAPE_ROW, spec.shape == Shape::Row);
        self.set_radio(L_SHAPE_COLUMN, spec.shape == Shape::Column);
        self.set_radio(L_SHAPE_GRID, grid);
        self.set_edit(L_COUNT, &s.arrange_count.to_string());
        self.enable(L_COUNT, grid);
        send(self.get(L_BY), CB_SETCURSEL, spec.by_rows as usize, 0);
        self.enable(L_BY, grid);
        self.anchor = spec.anchor;
        for i in 0..9 {
            unsafe { InvalidateRect(self.get(L_ANCHOR + i), null(), 0) };
        }
        self.set_edit(L_OFFSET_X, &s.arrange_offset_x.to_string());
        self.set_edit(L_OFFSET_Y, &s.arrange_offset_y.to_string());
        let screen = self.screens.iter().position(|d| d.eq_ignore_ascii_case(&s.arrange_screen)).unwrap_or(0);
        send(self.get(L_SCREEN), CB_SETCURSEL, screen, 0);
        let moved = if spec.offset != (0, 0) { format!(", moved by {}, {}", spec.offset.0, spec.offset.1) } else { String::new() };
        self.set_text(L_NOW, &format!("Now: {}{moved}", anchor_label(spec.anchor)));
        let (n, all) = store::with(|st| (st.panels.iter().filter(|p| p.arranged).count(), st.panels.len()));
        self.set_text(L_MEMBERS, &format!("{n} of {all}"));
        self.enable(L_CHOOSE, all > 0);
        self.enable(L_ARRANGE_ALL, all > 0);
        self.enable(L_FREE_ALL, n > 0);
        self.set_check(L_JOIN, s.new_folders_join);
    }

    fn sync_about(&mut self) {
        let (path, backups) = store::with(|s| (s.path().display().to_string(), s.backups()));
        self.set_text(V_CONFIG, &path);
        let list = self.get(V_BACKUPS);
        let selected = usize::try_from(send(list, LB_GETCURSEL, 0, 0)).ok().and_then(|i| self.backups.get(i).cloned());
        self.backups = backups.iter().map(|b| b.path.clone()).collect();
        send(list, LB_RESETCONTENT, 0, 0);
        for b in &backups {
            let label = wide(&b.label);
            send(list, LB_ADDSTRING, 0, label.as_ptr() as isize);
        }
        if backups.is_empty() {
            let none = wide("No backup yet");
            send(list, LB_ADDSTRING, 0, none.as_ptr() as isize);
        }
        let index = selected.and_then(|p| self.backups.iter().position(|b| *b == p));
        send(list, LB_SETCURSEL, index.unwrap_or(usize::MAX), 0);
        self.enable(V_BACKUPS, !backups.is_empty());
        self.enable(V_RESTORE, index.is_some());
    }

    // ---- Applying changes ----

    fn commit_dirty(&mut self) {
        if let Some(id) = self.dirty {
            self.commit_edit(id);
        }
    }

    fn commit_edit(&mut self, id: i32) {
        if self.dirty != Some(id) {
            return;
        }
        self.dirty = None;
        let value = text_of(self.get(id)).trim().to_string();
        match (id, self.panel, self.item) {
            (F_NAME, Some(p), _) if !value.is_empty() => store::change(|s| s.update_panel(p, |panel| panel.title = value)),
            (id, _, _) if number_def(id).is_some() => {
                if let Ok(v) = value.parse::<i32>() {
                    self.commit_number(id, v);
                }
            }
            (S_NAME, _, Some(i)) if !value.is_empty() => store::change(|s| s.update_item(i, |it| it.name = value)),
            (S_ARGS, _, Some(i)) => store::change(|s| s.update_item(i, |it| it.args = value)),
            _ => {}
        }
        // Also puts back the old value after an empty or invalid entry.
        self.request_sync();
    }

    /// Saves a number box, brought into its range.
    fn commit_number(&mut self, id: i32, v: i32) {
        let Some((min, max, _)) = number_def(id) else { return };
        let v = v.clamp(min, max);
        match (id, self.panel) {
            (F_X, Some(p)) => store::change(|s| s.update_panel(p, |panel| panel.x = v)),
            (F_Y, Some(p)) => store::change(|s| s.update_panel(p, |panel| panel.y = v)),
            (A_LINES, _) => store::change(|s| s.update_settings(|st| st.title_lines = v)),
            (L_GAP, _) => store::change(|s| s.update_settings(|st| st.gap = v)),
            (L_COUNT, _) => store::change(|s| s.update_settings(|st| st.arrange_count = v)),
            (L_OFFSET_X, _) => store::change(|s| s.update_settings(|st| st.arrange_offset_x = v)),
            (L_OFFSET_Y, _) => store::change(|s| s.update_settings(|st| st.arrange_offset_y = v)),
            // A slider's box.
            _ => self.commit_slider(id - 1, v),
        }
        self.request_sync();
    }

    /// One click on the arrows of a number box, or Up/Down in it.
    fn step_number(&mut self, id: i32, delta: i32) {
        let Some((_, _, step)) = number_def(id) else { return };
        let Ok(current) = text_of(self.get(id)).trim().parse::<i32>() else {
            self.dirty = None;
            self.request_sync();
            return;
        };
        let times = if unsafe { GetKeyState(VK_SHIFT as i32) } < 0 { 10 } else { 1 };
        self.dirty = None;
        self.commit_number(id, current + delta * step * times);
    }

    fn commit_slider(&mut self, id: i32, v: i32) {
        match (id, self.panel) {
            (F_WIDTH, Some(p)) => store::change(|s| s.update_panel(p, |panel| panel.width = v)),
            (F_COLS, Some(p)) => store::change(|s| s.update_panel(p, |panel| panel.columns = v)),
            (A_OPACITY, _) => store::change(|s| s.update_settings(|st| st.tint_opacity = v as f32 / 100.0)),
            (A_FONT, _) => store::change(|s| s.update_settings(|st| st.font_size = v)),
            (A_ICONS, _) => store::change(|s| s.update_settings(|st| st.icon_size = v)),
            (A_ANIM, _) => store::change(|s| s.update_settings(|st| st.animation_ms = v.max(0) as u32)),
            (B_OPEN, _) => store::change(|s| s.update_settings(|st| st.open_delay_ms = v.max(0) as u32)),
            (B_CLOSE, _) => store::change(|s| s.update_settings(|st| st.close_delay_ms = v.max(0) as u32)),
            (L_CELL, _) => store::change(|s| s.update_settings(|st| st.cell_width = v)),
            (L_NEW_W, _) => store::change(|s| s.update_settings(|st| st.new_folder_width = v)),
            _ => {}
        }
        self.request_sync();
    }

    /// The Arrange menu, under the button that opens it.
    fn arrange_menu(&self, under: i32) {
        let entries: [MenuEntry; 3] = [(1, "In a row", 0), (2, "In a column", 0), (3, "As a grid", 0)];
        match ui::popup_menu(self.hwnd, &entries, self.below(under)) {
            1 => arrange::arrange_all(Some("row")),
            2 => arrange::arrange_all(Some("column")),
            3 => arrange::arrange_all(Some("grid")),
            _ => {}
        }
    }

    /// The folders, checked when they are in the arrangement: picking one puts
    /// it in or takes it out.
    fn members_menu(&self) {
        let list: Vec<(u32, String, bool)> = store::with(|s| s.panels.iter().map(|p| (p.id, ui::menu_text(&p.title), p.arranged)).collect());
        let entries: Vec<MenuEntry> =
            list.iter().enumerate().map(|(i, (_, title, on))| (i + 1, title.as_str(), if *on { MF_CHECKED } else { MF_UNCHECKED })).collect();
        let pick = ui::popup_menu(self.hwnd, &entries, self.below(L_CHOOSE));
        if let Some(&(id, _, on)) = pick.checked_sub(1).and_then(|i| list.get(i)) {
            store::change(|s| s.set_arranged(&[id], !on));
        }
    }

    /// Screen point under a button, to open its menu there.
    fn below(&self, id: i32) -> POINT {
        let mut r: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(self.get(id), &mut r) };
        POINT { x: r.left, y: r.bottom }
    }

    fn pick_icon(&self) -> Option<String> {
        ui::open_files(self.hwnd, "Choose an icon", &[("Icons and programs", "*.ico;*.exe;*.lnk"), ("All files", "*.*")], false)
            .into_iter()
            .next()
    }

    fn on_click(&mut self, id: i32) {
        // A click on a color swatch is a click on its "Choose…" button.
        let id = if COLOR_BUTTONS.contains(&(id + 1)) { id + 1 } else { id };
        // A click on a caption is a click on its check box or radio button.
        let id = if (CAPTION..UPDOWN).contains(&id) {
            let target = id - CAPTION;
            if CAPTIONED_CHECKS.contains(&target) {
                self.set_check(target, !self.checked(target));
            }
            target
        } else {
            id
        };
        let (panel, item) = (self.panel, self.item);
        match id {
            F_NEW => {
                self.panel = Some(crate::new_folder());
                self.item = None;
                self.select_name = true;
            }
            F_UP | F_DOWN => {
                if let Some(p) = panel {
                    store::change(|s| s.move_panel(p, if id == F_UP { -1 } else { 1 }));
                }
            }
            F_ICON => {
                let entries: [MenuEntry; 2] = [(1, "Choose an icon file…", 0), (2, "Default folder icon", 0)];
                let icon = match ui::popup_menu(self.hwnd, &entries, self.below(F_ICON)) {
                    1 => self.pick_icon().map(Some),
                    2 => Some(None),
                    _ => None,
                };
                if let (Some(icon), Some(p)) = (icon, panel) {
                    store::change(|s| s.update_panel(p, |panel| panel.icon = icon));
                }
            }
            F_DELETE => {
                if let Some(p) = panel {
                    self.delete_folder(p);
                }
            }
            F_PLACE => crate::toggle_lock(),
            F_FIT => {
                let on = self.checked(F_FIT);
                if let Some(p) = panel {
                    store::change(|s| {
                        let settings = s.settings.clone();
                        s.update_panel(p, |panel| {
                            // Back to its own width: keep the size it had.
                            panel.width = panel.shown_width(&settings);
                            panel.fit_columns = on;
                        })
                    });
                }
            }
            F_ARRANGE => self.arrange_menu(F_ARRANGE),
            A_STYLE_CENTERED | A_STYLE_LEFT | A_STYLE_ICON | A_STYLE_FIT => {
                if let Some(&(_, style)) = TITLE_STYLES.iter().find(|(i, _)| *i == id) {
                    store::change(|s| s.update_settings(|st| st.title_style = style.name().into()));
                }
            }
            A_COUNT => {
                let on = self.checked(A_COUNT);
                store::change(|s| s.update_settings(|st| st.show_count = on));
            }
            A_NAMES_1 | A_NAMES_2 => store::change(|s| s.update_settings(|st| st.item_lines = if id == A_NAMES_2 { 2 } else { 1 })),
            L_SNAP => {
                let on = self.checked(L_SNAP);
                store::change(|s| s.update_settings(|st| st.snap = on));
            }
            L_PUSH | L_COVER => store::change(|s| s.update_settings(|st| st.push_below = id == L_PUSH)),
            L_AUTO | L_DOWN => store::change(|s| s.update_settings(|st| st.open_upwards = id == L_AUTO)),
            L_CLOSED | L_OPEN_ALL => store::change(|s| s.update_settings(|st| st.unlocked_open_all = id == L_OPEN_ALL)),
            F_ARRANGED => {
                let on = self.checked(F_ARRANGED);
                if let Some(p) = panel {
                    store::change(|s| s.set_arranged(&[p], on));
                }
            }
            L_SHAPE_ROW | L_SHAPE_COLUMN | L_SHAPE_GRID => {
                let shape = match id {
                    L_SHAPE_ROW => "row",
                    L_SHAPE_COLUMN => "column",
                    _ => "grid",
                };
                store::change(|s| s.update_settings(|st| st.arrange_shape = shape.into()));
            }
            id if (L_ANCHOR..L_ANCHOR + 9).contains(&id) => {
                let i = (id - L_ANCHOR) as usize;
                store::change(|s| {
                    s.update_settings(|st| {
                        st.arrange_anchor = ANCHORS[i / 3][i % 3].into();
                        st.arrange_offset_x = 0;
                        st.arrange_offset_y = 0;
                    })
                });
            }
            L_CHOOSE => self.members_menu(),
            L_JOIN => {
                let on = self.checked(L_JOIN);
                store::change(|s| s.update_settings(|st| st.new_folders_join = on));
            }
            L_ARRANGE_ALL => arrange::arrange_all(None),
            L_FREE_ALL => arrange::free_all(),
            F_OPEN => {
                if let Some(p) = panel {
                    let dir = store::with(|s| s.storage_of(p));
                    // The folder may not exist yet: nothing was dropped in it so far.
                    let _ = std::fs::create_dir_all(&dir);
                    launch(&dir.to_string_lossy(), "");
                }
            }
            S_ADD => {
                if let Some(p) = panel {
                    let files = ui::open_files(self.hwnd, "Add shortcuts (the files stay where they are)", &[("All files", "*.*")], true);
                    if !files.is_empty() {
                        store::change(|s| s.add_files(p, usize::MAX, &files, true));
                    }
                }
            }
            S_REMOVE => {
                if let Some(i) = item {
                    // A file that was moved in goes back to the desktop; a link is just removed.
                    if let Err(e) = store::change(|s| s.remove_item(i, true)) {
                        error_box(&e);
                    }
                    self.item = None;
                }
            }
            S_UP | S_DOWN => {
                if let Some(i) = item {
                    store::change(|s| s.shift_item(i, if id == S_UP { -1 } else { 1 }));
                }
            }
            S_LOCATE => {
                if let Some(path) = item.and_then(|i| store::with(|s| s.item(i).map(|(_, it)| it.path.clone()))) {
                    let target = resolve(&path);
                    if Path::new(&target).exists() {
                        launch("explorer.exe", &format!("/select,\"{target}\""));
                    } else {
                        error_box(&format!("This shortcut has no file to show:\n\n{path}"));
                    }
                }
            }
            S_ICON => {
                let entries: [MenuEntry; 2] = [(1, "Choose an icon file…", 0), (2, "Use the shortcut's own icon", 0)];
                let icon = match ui::popup_menu(self.hwnd, &entries, self.below(S_ICON)) {
                    1 => self.pick_icon().map(Some),
                    2 => Some(None),
                    _ => None,
                };
                if let (Some(icon), Some(i)) = (icon, item) {
                    store::change(|s| s.update_item(i, |it| it.icon = icon));
                }
            }
            A_LIGHT | A_DARK | A_SOLID => {
                let (tint, opacity, text, hover, border) = preset(id);
                store::change(|s| {
                    s.update_settings(|st| {
                        st.tint = tint.into();
                        st.tint_opacity = opacity;
                        st.text = text.into();
                        st.hover = hover.into();
                        st.border = border.into();
                    })
                });
            }
            A_TINT | A_ACCENT | A_TEXT | A_HOVER | A_BORDER => {
                let mut current = store::with(|s| s.settings.clone());
                let initial = color_field(&mut current, id).map_or(0, |c| parse_color(c, 0));
                if let Some(color) = ui::choose_color(self.hwnd, initial) {
                    store::change(|s| {
                        s.update_settings(|st| {
                            if let Some(field) = color_field(st, id) {
                                *field = format_color(color);
                            }
                        })
                    });
                }
            }
            B_HOVER => {
                let on = self.checked(B_HOVER);
                store::change(|s| s.update_settings(|st| st.hover_open = on));
            }
            B_COLLAPSE => {
                let on = self.checked(B_COLLAPSE);
                store::change(|s| s.update_settings(|st| st.collapse_after_launch = on));
            }
            B_LOCK => crate::set_locked(self.checked(B_LOCK)),
            B_AUTOSTART => util::set_autostart(self.checked(B_AUTOSTART)),
            B_BROWSE => {
                if let Some(dir) = ui::choose_folder(self.hwnd, "Where new folders keep the files dropped into them") {
                    store::change(|s| s.update_settings(|st| st.storage_dir = dir));
                }
            }
            B_STORAGE_DEFAULT => store::change(|s| s.update_settings(|st| st.storage_dir.clear())),
            V_NOTEPAD => util::open_in_notepad(&config::path()),
            V_RELOAD => crate::reload_config(),
            V_RESTORE => self.restore_backup(),
            V_OPEN_BACKUPS => {
                let dir = store::with(|s| s.backup_dir());
                let _ = std::fs::create_dir_all(&dir);
                launch(&dir.to_string_lossy(), "");
            }
            V_RESET => {
                let reset = ui::confirm(
                    self.hwnd,
                    "Reset appearance, behavior and layout?",
                    "Colors, sizes, delays, the animation and the layout settings go back to their defaults. Folders, shortcuts and the storage folder are kept. The current version is saved as a backup first.",
                    "Reset",
                );
                if reset {
                    store::with(|s| s.backup_now());
                    store::change(|s| {
                        s.update_settings(|st| {
                            let storage_dir = std::mem::take(&mut st.storage_dir);
                            *st = Settings { storage_dir, ..Settings::default() };
                        })
                    });
                }
            }
            _ => return,
        }
        self.request_sync();
    }

    fn delete_folder(&mut self, id: u32) {
        const DELETE: i32 = 1000;
        const TO_DESKTOP: i32 = 2000;
        const KEEP: i32 = 2001;
        let Some((title, total, moved, storage)) = store::with(|s| {
            let p = s.panel(id)?;
            let moved = p.items.iter().filter(|it| s.is_stored(id, it)).count();
            Some((p.title.clone(), p.items.len(), moved, s.storage_of(id)))
        }) else {
            return;
        };
        let instruction = format!("Delete the folder '{title}'?");
        let content = if moved > 0 {
            format!("It holds {total} shortcut(s); {moved} of them are files you moved into it. What should happen to those files?")
        } else if total > 0 {
            format!("Its {total} shortcut(s) are links: the files they open are not touched.")
        } else {
            "This folder is empty.".to_string()
        };
        let keep = format!("Keep them in {}", storage.display());
        let radios = [(TO_DESKTOP, "Move them back to the desktop"), (KEEP, keep.as_str())];
        let (button, radio) = ui::ask(
            self.hwnd,
            &Ask {
                instruction: &instruction,
                content: &content,
                buttons: &[(DELETE, "Delete folder")],
                default_button: DELETE,
                radios: if moved > 0 { &radios[..] } else { &[] },
                default_radio: TO_DESKTOP,
                warning: true,
                footer: Some("deskfolds never deletes a file."),
                ..Default::default()
            },
        );
        if button != DELETE {
            return;
        }
        let errors = store::change(|s| s.delete_panel(id, moved > 0 && radio != KEEP));
        if !errors.is_empty() {
            error_box(&format!(
                "Some files could not be moved back to the desktop. They are still in {}:\n\n{}",
                storage.display(),
                errors.join("\n")
            ));
        }
        self.panel = None;
        self.item = None;
    }

    fn restore_backup(&mut self) {
        let index = usize::try_from(send(self.get(V_BACKUPS), LB_GETCURSEL, 0, 0)).ok();
        let Some(path) = index.and_then(|i| self.backups.get(i).cloned()) else { return };
        let label = store::with(|s| s.backups()).into_iter().find(|b| b.path == path).map_or_else(String::new, |b| b.label);
        let restore = ui::confirm(
            self.hwnd,
            &format!("Restore the version from {label}?"),
            "The folders, shortcuts and settings on screen are replaced by this version. The current version is saved as a backup first.",
            "Restore",
        );
        if restore {
            if let Err(e) = store::with(|s| s.restore(&path)) {
                error_box(&e);
            }
            self.panel = None;
            self.item = None;
        }
    }

    fn on_command(&mut self, id: i32, code: u32) {
        if self.syncing {
            return;
        }
        match (id, code) {
            (IDOK, _) => self.on_enter(),
            (IDCANCEL, _) => self.on_escape(),
            (NAV, LBN_SELCHANGE) => {
                let index = send(self.nav, LB_GETCURSEL, 0, 0);
                if let Some(&(page, _)) = usize::try_from(index).ok().and_then(|i| PAGES.get(i))
                    && page != self.page
                {
                    self.show_page(page);
                }
            }
            (L_BY, CBN_SELCHANGE) => {
                let rows = send(self.get(L_BY), CB_GETCURSEL, 0, 0) == 1;
                store::change(|s| s.update_settings(|st| st.arrange_by = if rows { "rows" } else { "columns" }.into()));
                self.request_sync();
            }
            (L_SCREEN, CBN_SELCHANGE) => {
                let index = usize::try_from(send(self.get(L_SCREEN), CB_GETCURSEL, 0, 0)).ok();
                if let Some(device) = index.and_then(|i| self.screens.get(i)).cloned() {
                    store::change(|s| s.update_settings(|st| st.arrange_screen = device));
                }
                self.request_sync();
            }
            (V_BACKUPS, LBN_SELCHANGE) => {
                let index = usize::try_from(send(self.get(V_BACKUPS), LB_GETCURSEL, 0, 0)).ok();
                self.enable(V_RESTORE, index.is_some_and(|i| i < self.backups.len()));
            }
            (_, EN_CHANGE) => self.dirty = Some(id),
            (_, EN_KILLFOCUS) => self.commit_edit(id),
            (_, BN_CLICKED) => self.on_click(id),
            _ => {}
        }
    }

    /// Enter clicks the focused button, or saves the focused edit box.
    fn on_enter(&mut self) {
        let focus = unsafe { GetFocus() };
        let class = class_of(focus);
        if class.eq_ignore_ascii_case("Button") {
            send(focus, BM_CLICK, 0, 0);
        } else if class.eq_ignore_ascii_case("Edit") {
            self.commit_edit(unsafe { GetDlgCtrlID(focus) });
            send(focus, EM_SETSEL, 0, -1);
        }
    }

    /// Esc puts back the value of the edit box being typed in, or closes the window.
    fn on_escape(&mut self) {
        if self.dirty.take().is_some() {
            self.request_sync();
        } else {
            unsafe { PostMessageW(self.hwnd, WM_CLOSE, 0, 0) };
        }
    }

    fn on_notify(&mut self, hdr: &NMHDR, lparam: LPARAM) -> LRESULT {
        if self.syncing {
            return 0;
        }
        let id = hdr.idFrom as i32;
        let selected = || selected_param(hdr.hwndFrom).map(|p| p as u32);
        match hdr.code {
            UDN_DELTAPOS => {
                let change = unsafe { &*(lparam as *const NMUPDOWN) };
                self.step_number(id - UPDOWN, change.iDelta.signum());
                // The arrows' own position never moves.
                return 1;
            }
            // A click on a row first unselects the old one: only the new selection
            // counts. Syncing in between would select the old row again while the
            // list is still handling the click.
            LVN_ITEMCHANGED => {
                let change = unsafe { &*(lparam as *const NMLISTVIEW) };
                if change.uChanged & LVIF_STATE == 0 || change.uNewState & LVIS_SELECTED == 0 {
                    return 0;
                }
                let Some(selected) = selected() else { return 0 };
                if id == F_LIST && Some(selected) != self.panel {
                    self.commit_dirty();
                    self.panel = Some(selected);
                    self.item = None;
                    self.request_sync();
                } else if id == S_LIST && Some(selected) != self.item {
                    self.commit_dirty();
                    self.item = Some(selected);
                    self.request_sync();
                }
            }
            // The click is over. On empty space: the folder stays selected (the list
            // shows it again), the shortcut is unselected.
            NM_CLICK if selected().is_none() => {
                if id == S_LIST {
                    self.commit_dirty();
                    self.item = None;
                }
                self.request_sync();
            }
            _ => {}
        }
        0
    }

    fn on_slider(&mut self, slider: HWND, code: u32) {
        if self.syncing {
            return;
        }
        let id = unsafe { GetDlgCtrlID(slider) };
        let value = send(slider, TBM_GETPOS, 0, 0) as i32;
        self.set_value_label(id, value);
        // Applied when released, not while dragging.
        if code != TB_THUMBTRACK {
            self.commit_slider(id, value);
        }
    }

    fn list_item_height(&self, id: u32) -> u32 {
        self.s(if id == NAV as u32 { 34 } else { 24 }) as u32
    }

    /// Draws a list box row: gray when selected, with an accent bar in the page list.
    fn draw_list_item(&self, d: &DRAWITEMSTRUCT) {
        if d.CtlType != ODT_LISTBOX || d.itemID == u32::MAX {
            return;
        }
        let selected = d.itemState & ODS_SELECTED != 0;
        let nav = d.CtlID == NAV as u32;
        unsafe {
            let brush = CreateSolidBrush(if selected { SELECTED } else { PANE });
            FillRect(d.hDC, &d.rcItem, brush);
            DeleteObject(brush);
            if selected && nav {
                let bar = RECT {
                    left: d.rcItem.left + self.s(2),
                    top: d.rcItem.top + self.s(9),
                    right: d.rcItem.left + self.s(5),
                    bottom: d.rcItem.bottom - self.s(9),
                };
                let accent = CreateSolidBrush(ACCENT);
                FillRect(d.hDC, &bar, accent);
                DeleteObject(accent);
            }
            let len = send(d.hwndItem, LB_GETTEXTLEN, d.itemID as usize, 0).max(0);
            let mut text = vec![0u16; len as usize + 1];
            send(d.hwndItem, LB_GETTEXT, d.itemID as usize, text.as_mut_ptr() as isize);
            SetBkMode(d.hDC, TRANSPARENT as _);
            SetTextColor(d.hDC, if d.itemState & ODS_DISABLED != 0 { DIM } else { TEXT });
            let old = SelectObject(d.hDC, self.font);
            let mut r = RECT { left: d.rcItem.left + self.s(if nav { 14 } else { 8 }), ..d.rcItem };
            DrawTextW(d.hDC, text.as_ptr(), len as i32, &mut r, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
            SelectObject(d.hDC, old);
        }
    }

    fn paint_background(&self, hdc: HDC) {
        let mut rc: RECT = unsafe { std::mem::zeroed() };
        unsafe {
            GetClientRect(self.hwnd, &mut rc);
            let side = RECT { right: self.s(SIDE_W), ..rc };
            FillRect(hdc, &side, self.pane);
            let rest = RECT { left: side.right, ..rc };
            FillRect(hdc, &rest, self.bg);
        }
    }

    /// Draws an anchor button: filled with the accent when it is the anchor.
    fn draw_anchor(&self, d: &DRAWITEMSTRUCT) {
        let i = d.CtlID as i32 - L_ANCHOR;
        let on = (i % 3, i / 3) == (self.anchor.0 as i32, self.anchor.1 as i32);
        unsafe {
            let fill = CreateSolidBrush(if on { ACCENT } else { FIELD });
            FillRect(d.hDC, &d.rcItem, fill);
            DeleteObject(fill);
            let edge = CreateSolidBrush(if on { ACCENT } else { 0x004D4D4D });
            FrameRect(d.hDC, &d.rcItem, edge);
            DeleteObject(edge);
        }
    }

    fn static_color(&self, hdc: HDC, ctl: HWND) -> LRESULT {
        if unsafe { GetDlgCtrlID(ctl) } == L_LINE {
            return self.line as LRESULT;
        }
        if let Some(&(_, brush)) = self.swatches.iter().find(|(h, _)| *h == ctl)
            && !brush.is_null()
        {
            return brush as LRESULT;
        }
        unsafe {
            if ctl == self.footer {
                SetTextColor(hdc, DIM);
                SetBkColor(hdc, PANE);
                return self.pane as LRESULT;
            }
            SetTextColor(hdc, if self.dim.contains(&ctl) { DIM } else { TEXT });
            SetBkColor(hdc, BG);
        }
        self.bg as LRESULT
    }
}

impl Drop for Win {
    fn drop(&mut self) {
        unsafe {
            for (_, brush) in self.swatches.drain(..) {
                if !brush.is_null() {
                    DeleteObject(brush);
                }
            }
            for object in [self.font, self.heading, self.strong, self.bg, self.line, self.pane, self.field] {
                if !object.is_null() {
                    DeleteObject(object);
                }
            }
            for icon in self.icons {
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
            }
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let w = &mut *ptr;
        match msg {
            WM_APP_SYNC => {
                w.sync();
                return 0;
            }
            WM_ERASEBKGND => {
                w.paint_background(wparam as HDC);
                return 1;
            }
            WM_MEASUREITEM => {
                let m = &mut *(lparam as *mut MEASUREITEMSTRUCT);
                if m.CtlType == ODT_LISTBOX {
                    m.itemHeight = w.list_item_height(m.CtlID);
                    return 1;
                }
            }
            WM_DRAWITEM => {
                let d = &*(lparam as *const DRAWITEMSTRUCT);
                if d.CtlType == ODT_BUTTON {
                    w.draw_anchor(d);
                } else {
                    w.draw_list_item(d);
                }
                return 1;
            }
            WM_CTLCOLORSTATIC => return w.static_color(wparam as HDC, lparam as HWND),
            WM_CTLCOLOREDIT => {
                SetTextColor(wparam as HDC, TEXT);
                SetBkColor(wparam as HDC, FIELD);
                return w.field as LRESULT;
            }
            WM_CTLCOLORLISTBOX => {
                SetTextColor(wparam as HDC, TEXT);
                SetBkColor(wparam as HDC, PANE);
                return w.pane as LRESULT;
            }
            WM_CTLCOLORBTN => {
                SetBkColor(wparam as HDC, BG);
                return w.bg as LRESULT;
            }
            WM_COMMAND => {
                w.on_command((wparam & 0xFFFF) as i32, ((wparam >> 16) & 0xFFFF) as u32);
                return 0;
            }
            WM_NOTIFY => return w.on_notify(&*(lparam as *const NMHDR), lparam),
            WM_HSCROLL => {
                if lparam != 0 {
                    w.on_slider(lparam as HWND, (wparam & 0xFFFF) as u32);
                }
                return 0;
            }
            WM_DPICHANGED => {
                w.commit_dirty();
                w.dpi = ((wparam >> 16) & 0xFFFF) as u32;
                w.make_fonts();
                let r = &*(lparam as *const RECT);
                SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                w.build_frame();
                w.show_page(w.page);
                return 0;
            }
            WM_CLOSE => {
                w.commit_dirty();
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                w.clear_page();
                // Notifications from the controls being destroyed are ignored.
                w.syncing = true;
                WINDOW.set(null_mut());
                crate::highlight_panel(None);
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
