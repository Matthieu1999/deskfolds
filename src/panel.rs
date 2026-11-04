//! A panel is a small glass folder glued to the desktop. Closed, it is a label
//! as wide as the folder, with its icon and name (a long name wraps onto more
//! lines). Hovering it, or clicking it, unrolls a card of shortcuts: downwards,
//! or upwards when there is no room below. It stays below every other window,
//! never takes focus, and cannot move while panels are locked.
//!
//! A panel shows a copy of its data from the store. Every change it makes
//! (a drop, a move, a resize, a rename) goes through the store, which then
//! updates the panel in place.

use std::cell::Cell;
use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON};
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent, VK_MENU,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{MIN_WIDTH, PANEL_PAD, Panel, Settings, TitleStyle, parse_color};
use crate::dragdrop::{self, DropHandler};
use crate::layout;
use crate::store::{self, WM_APP_REFRESH};
use crate::ui::{self, MenuEntry};
use crate::util::{error_box, launch, load_icon, prompt_text, resolve, wide};

const CLASS_NAME: &str = "deskfolds.panel";
/// The window that shows a folder's label while the folder opens or closes upwards.
const LABEL_CLASS: &str = "deskfolds.label";
const OPEN_TIMER: usize = 1;
const CLOSE_TIMER: usize = 2;
const ANIM_TIMER: usize = 3;
const DRAG_OPEN_TIMER: usize = 4;
const ANIM_FRAME_MS: u32 = 15;
/// Dragging a file over a closed panel opens it after this delay.
const DRAG_OPEN_DELAY_MS: u32 = 300;
/// Width of the resize grip on the right edge of an unlocked panel.
const GRIP: i32 = 7;
/// Size of the folder icon above the name, in the "icon above" title style.
const BIG_ICON: i32 = 24;
/// Room kept at the right of the title for the pin marker.
const PIN_ROOM: i32 = 14;
/// A moved folder snaps to an edge closer than this.
const SNAP_REACH: i32 = 10;
const ID_RENAME: usize = 1;
const ID_REMOVE: usize = 2;
const ID_SETTINGS: usize = 3;
/// Icon shown next to the name when the panel sets none: any folder works.
pub const DEFAULT_PANEL_ICON: &str = "%SystemRoot%";

/// Shared by all panels: while false, panels can be moved, resized and edited.
pub static LOCKED: AtomicBool = AtomicBool::new(true);

/// Posted to the main window to open the menu at the cursor. wparam: the panel id, or 0.
pub const WM_APP_SHOW_MENU: u32 = WM_APP + 2;
/// Posted to the main window when a panel grew, shrank or moved: the folders
/// stacked under open ones are pushed down again.
pub const WM_APP_PUSH: u32 = WM_APP + 3;
/// Posted to the main window when a panel changed size outside the store (a
/// new DPI): the arrangement is placed again.
pub const WM_APP_ARRANGE: u32 = WM_APP + 4;

thread_local! {
    /// The shortcut being dragged out of a panel, by id.
    static INTERNAL_DRAG: Cell<Option<u32>> = const { Cell::new(None) };
    /// Set when that drag was dropped on one of our windows.
    static INTERNAL_DROP_DONE: Cell<bool> = const { Cell::new(false) };
    /// A WM_APP_PUSH is waiting in the queue.
    static PUSH_POSTED: Cell<bool> = const { Cell::new(false) };
    /// A panel is being raised above the others on purpose: it may leave the bottom.
    static RAISING: Cell<bool> = const { Cell::new(false) };
}

/// Asks the main window to update the pushes, once however often it is asked.
fn request_pushes(main: HWND) {
    if !PUSH_POSTED.replace(true) {
        unsafe { PostMessageW(main, WM_APP_PUSH, 0, 0) };
    }
}

/// The main window is handling WM_APP_PUSH.
pub fn pushes_handled() {
    PUSH_POSTED.set(false);
}

/// The shortcut being dragged out of a panel right now, if any.
pub fn internal_drag() -> Option<u32> {
    INTERNAL_DRAG.get()
}

/// Tells the drag source that one of our windows handled the drop itself.
pub fn finish_internal_drop() {
    INTERNAL_DROP_DONE.set(true);
}

/// True while a shortcut is being dragged out of a panel. Panels must not be
/// rebuilt then: the drag loop still uses the one it started from.
pub fn dragging() -> bool {
    INTERNAL_DRAG.get().is_some()
}

static DROP_HANDLER: DropHandler = DropHandler { over: on_drag_over, leave: on_drag_leave, drop: on_drop };

pub struct PanelState {
    cfg: Panel,
    settings: Settings,
    main: HWND,
    /// Where the label is shown: its saved position, or the nearest visible
    /// spot if no monitor shows that position anymore.
    pos: (i32, i32),
    /// Pushed down by an open folder above it, in pixels.
    push: i32,
    /// Where the window is: the label position, pushed down, or moved up while
    /// it opens upwards.
    shown: (i32, i32),
    /// Opening upwards: chosen when it starts opening, kept until it is closed.
    up: bool,
    /// While it opens or closes upwards, the label is shown by `label_window`
    /// and the shortcuts keep to the top of the folder. Windows shows a
    /// window's new size one frame before its new picture, with the old picture
    /// at the top: a label drawn at the bottom of a folder growing upwards then
    /// jumps up every other frame, and the shortcuts look doubled. Kept to the
    /// top, the shortcuts are right in that frame too, and the label window is
    /// moved and drawn in one go, so it never jumps.
    lifted: bool,
    /// The folder stops drawing its label at the next animation step, once the
    /// label window, shown first, is on the screen (so the label never disappears).
    lift_next: bool,
    /// The label window goes away at the next animation step (after the folder
    /// shows its own label again).
    unlift_next: bool,
    label_window: HWND,
    /// What the main window last heard about: position, size, upwards.
    last_reach: Option<((i32, i32), i32, i32, bool)>,
    /// The window's position and size as last given to Windows.
    applied: Option<(i32, i32, i32, i32)>,
    /// Being moved by its name: pushes leave it alone.
    moving: bool,
    /// The window rectangle last given while moving: a different one at the
    /// end means the move was cancelled (Esc).
    last_moving: Option<RECT>,
    /// Lit up as the folder that a moved folder snaps to.
    snap_target: bool,
    /// Target state: open or closed. `progress` animates towards it.
    open: bool,
    pinned: bool,
    /// Selected in the settings window: kept open and outlined.
    highlighted: bool,
    progress: f32,
    anim_from: f32,
    anim_start: u64,
    open_pending: bool,
    /// Opened because a file was dragged over it: close again if the drag leaves.
    drag_opened: bool,
    drag_open_pending: bool,
    /// Where a dragged file would be inserted, shown as a bar.
    drop_insert: Option<usize>,
    /// Mouse pressed on an item while unlocked: (x, y, item index).
    press: Option<(i32, i32, usize)>,
    /// Resizing with the grip: (cursor x when it started, width then).
    resizing: Option<(i32, i32)>,
    hover: Option<usize>,
    tracking_mouse: bool,
    /// Also tracking the mouse over the name, which is the move handle while unlocked.
    tracking_nc: bool,
    dpi: u32,
    font: HFONT,
    title_font: HFONT,
    /// The name on one line, and split into lines for the folder's width.
    title_text_w: i32,
    title_lines: Vec<String>,
    /// Line heights of the title font and of the shortcut font.
    line_h: i32,
    item_line_h: i32,
    /// Width of the shortcut count, drawn in a small badge.
    count_w: i32,
    title_icon: HICON,
    icons: Vec<HICON>,
}

/// Where a panel's label is and how far the panel reaches past it now.
pub struct Geometry {
    pub label: layout::Rect,
    /// Width of the panel when open (wider than the label in the "fit" style).
    pub open_w: i32,
    /// Height of the panel when fully open.
    pub open_h: i32,
    /// How far it reaches past its label now: below it, or above it (`up`).
    pub extra: i32,
    /// Opening upwards (or closing, after it opened upwards).
    pub up: bool,
    /// Open or opening (not closing).
    pub open: bool,
    /// In the arrangement.
    pub arranged: bool,
    pub dpi: u32,
    pub moving: bool,
}

enum Hit {
    Title,
    Item(usize),
    Grip,
    Nothing,
}

struct Layout {
    title_h: i32,
    pad: i32,
    label_icon: i32,
    cell_w: i32,
    cell_h: i32,
    icon: i32,
    rows: i32,
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn lerp(a: i32, b: i32, t: f32) -> i32 {
    a + ((b - a) as f32 * t).round() as i32
}

fn locked() -> bool {
    LOCKED.load(Ordering::Relaxed)
}

impl PanelState {
    fn scale(&self, v: i32) -> i32 {
        (v as f32 * self.dpi as f32 / 96.0).round() as i32
    }

    fn style(&self) -> TitleStyle {
        match self.settings.title_style() {
            // With no icon, "icon above" is a centered name.
            TitleStyle::IconAbove if !self.settings.show_folder_icon => TitleStyle::Centered,
            style => style,
        }
    }

    /// The panel's width in pixels: its own, or columns × cell width.
    fn width_px(&self) -> i32 {
        self.scale(self.cfg.shown_width(&self.settings))
    }

    /// Size of the folder's icon next to its name: 0 when icons are off.
    fn label_icon(&self) -> i32 {
        if !self.settings.show_folder_icon {
            return 0;
        }
        self.scale(if self.style() == TitleStyle::IconAbove { BIG_ICON } else { 16 })
    }

    /// Space between the folder's icon and its name: none without an icon.
    fn icon_gap(&self) -> i32 {
        if self.settings.show_folder_icon { self.scale(8) } else { 0 }
    }

    /// Width of the shortcut count badge, or 0 when it is not shown.
    fn badge_w(&self) -> i32 {
        if self.settings.show_count { self.count_w + self.scale(12) } else { 0 }
    }

    /// Room for the name, which wraps inside it.
    fn title_wrap_width(&self) -> i32 {
        let w = self.width_px();
        let badge = if self.settings.show_count { self.badge_w() + self.scale(6) } else { 0 };
        let room = match self.style() {
            TitleStyle::IconAbove => w - self.scale(24),
            _ => w - self.scale(12 + 12 + PIN_ROOM) - self.label_icon() - self.icon_gap() - badge,
        };
        room.max(self.scale(24))
    }

    fn layout(&self) -> Layout {
        let s = &self.settings;
        let pad = self.scale(PANEL_PAD);
        let cols = self.cfg.columns.max(1);
        let icon = self.scale(s.icon_size);
        let line = self.scale(s.font_size * 3 / 2);
        let lines = self.title_lines.len().max(1) as i32;
        let title_h = match self.style() {
            TitleStyle::Fit => self.scale(s.font_size + 20),
            TitleStyle::IconAbove => self.scale(10 + BIG_ICON + 4 + 8) + lines * self.line_h,
            _ => self.scale(s.font_size + 20) + (lines - 1) * self.line_h,
        };
        Layout {
            title_h,
            pad,
            label_icon: self.label_icon(),
            cell_w: (self.width_px() - pad * 2) / cols,
            cell_h: self.scale(8) + icon + self.scale(4) + line + (s.item_lines.clamp(1, 2) - 1) * self.item_line_h + self.scale(4),
            icon,
            // An empty panel keeps one row for its "drop shortcuts here" hint.
            rows: ((self.cfg.items.len() as i32 + cols - 1) / cols).max(1),
        }
    }

    /// Top of the name in the window: at the bottom while it opens upwards.
    fn title_top(&self, l: &Layout) -> i32 {
        if self.up { self.size().1 - l.title_h } else { 0 }
    }

    /// Top of the first row of shortcuts: they unroll from the name.
    fn content_top(&self, l: &Layout) -> i32 {
        if self.lifted {
            // Where they are once open, counted from the top of the folder.
            self.open_size().1 - l.title_h - l.rows * l.cell_h
        } else if self.up {
            self.size().1 - l.title_h - l.rows * l.cell_h
        } else {
            l.title_h
        }
    }

    /// Where a file dropped at this client point would be inserted.
    fn insert_index(&self, x: i32, y: i32) -> usize {
        let count = self.cfg.items.len();
        if self.progress < 1.0 {
            return count;
        }
        let l = self.layout();
        (0..count)
            .find(|&i| {
                let r = self.item_rect(&l, i);
                y < r.top || (y < r.bottom && x < r.left + l.cell_w / 2)
            })
            .unwrap_or(count)
    }

    /// Size of the closed label: as wide as the folder, or as the name in the "fit" style.
    fn closed_size(&self) -> (i32, i32) {
        let l = self.layout();
        if self.style() != TitleStyle::Fit {
            return (self.width_px(), l.title_h);
        }
        let badge = if self.settings.show_count { self.badge_w() + self.scale(6) } else { 0 };
        let w = self.scale(12) + l.label_icon + self.icon_gap() + self.title_text_w + badge + self.scale(14);
        (w.min(self.width_px()), l.title_h)
    }

    fn open_size(&self) -> (i32, i32) {
        let l = self.layout();
        (self.width_px(), l.title_h + l.rows * l.cell_h + l.pad)
    }

    /// Where the window goes now.
    fn window_pos(&self) -> (i32, i32) {
        let y = self.pos.1 + self.push;
        if self.up {
            let (h, closed_h) = (self.size().1, self.closed_size().1);
            (self.pos.0, y + closed_h - h)
        } else {
            (self.pos.0, y)
        }
    }

    /// An open folder that moved or changed size picks its direction again.
    fn choose_direction(&mut self, hwnd: HWND) {
        if self.open {
            self.up = self.settings.open_upwards && self.needs_up(hwnd);
        }
    }

    /// It opens upwards: the folders it opens with (its arrangement, or the
    /// folders stacked with it) have no room below, see `layout::group_opens_up`.
    fn needs_up(&self, hwnd: HWND) -> bool {
        let (w, closed_h) = self.closed_size();
        let label = layout::Rect { x: self.pos.0, y: self.pos.1, w, h: closed_h };
        crate::opens_up(hwnd, label, self.open_size().1 - closed_h, self.cfg.arranged, self.dpi)
    }

    fn size(&self) -> (i32, i32) {
        let (cw, ch) = self.closed_size();
        let (ow, oh) = self.open_size();
        let t = ease_out_cubic(self.progress);
        (lerp(cw, ow, t), lerp(ch, oh, t))
    }

    fn item_rect(&self, l: &Layout, i: usize) -> RECT {
        let cols = self.cfg.columns.max(1);
        let col = i as i32 % cols;
        let row = i as i32 / cols;
        let left = l.pad + col * l.cell_w;
        let top = self.content_top(l) + row * l.cell_h;
        RECT { left, top, right: left + l.cell_w, bottom: top + l.cell_h }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let l = self.layout();
        if !locked() && self.progress >= 1.0 && !self.cfg.fit_columns && x >= self.size().0 - self.scale(GRIP) {
            return Hit::Grip;
        }
        let top = self.title_top(&l);
        if y >= top && y < top + l.title_h {
            return Hit::Title;
        }
        if self.progress >= 1.0 {
            for i in 0..self.cfg.items.len() {
                let r = self.item_rect(&l, i);
                if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                    return Hit::Item(i);
                }
            }
        }
        Hit::Nothing
    }

    /// Shows the panel at its saved position, or on the nearest screen if that
    /// position is on no monitor anymore. The saved position is not changed.
    fn place(&mut self) {
        let (w, h) = self.closed_size();
        self.pos = visible_position(self.cfg.x, self.cfg.y, w, h);
    }

    /// (Re)creates fonts and icons for the current DPI.
    fn load_resources(&mut self) {
        self.free_resources();
        let face = wide("Segoe UI");
        let height = -self.scale(self.settings.font_size);
        // Grayscale antialiasing: ClearType fringes look wrong on glass.
        let make_font = |weight: i32| unsafe {
            CreateFontW(
                height,
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
                ANTIALIASED_QUALITY as _,
                DEFAULT_PITCH as _,
                face.as_ptr(),
            )
        };
        self.font = make_font(FW_NORMAL as i32);
        self.title_font = make_font(FW_SEMIBOLD as i32);
        self.title_icon = if self.settings.show_folder_icon {
            load_icon(self.cfg.icon.as_deref().unwrap_or(DEFAULT_PANEL_ICON), self.label_icon())
        } else {
            null_mut()
        };
        let size = self.scale(self.settings.icon_size);
        self.icons = self
            .cfg
            .items
            .iter()
            .map(|item| load_icon(item.icon.as_deref().unwrap_or(&item.path), size))
            .collect();
        self.measure();
    }

    /// Measures the text and splits the name into lines for the current width.
    fn measure(&mut self) {
        unsafe {
            let dc = GetDC(null_mut());
            let old = SelectObject(dc, self.font);
            let mut tm: TEXTMETRICW = std::mem::zeroed();
            GetTextMetricsW(dc, &mut tm);
            self.item_line_h = tm.tmHeight;
            self.count_w = text_width(dc, &self.cfg.items.len().to_string());
            SelectObject(dc, self.title_font);
            GetTextMetricsW(dc, &mut tm);
            self.line_h = tm.tmHeight;
            self.title_text_w = text_width(dc, &self.cfg.title);
            let room = self.title_wrap_width();
            self.title_lines = if self.style() == TitleStyle::Fit {
                vec![self.cfg.title.clone()]
            } else {
                layout::wrap(&self.cfg.title, room, self.settings.title_lines.clamp(1, 3) as usize, |t| text_width(dc, t))
            };
            SelectObject(dc, old);
            ReleaseDC(null_mut(), dc);
        }
    }

    fn free_resources(&mut self) {
        unsafe {
            for icon in self.icons.drain(..).chain(Some(self.title_icon)) {
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
            }
            for font in [self.font, self.title_font] {
                if !font.is_null() {
                    DeleteObject(font);
                }
            }
        }
        self.font = null_mut();
        self.title_font = null_mut();
        self.title_icon = null_mut();
    }
}

impl Drop for PanelState {
    fn drop(&mut self) {
        self.free_resources();
    }
}

fn monitor_work_area(monitor: HMONITOR) -> Option<RECT> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some(info.rcWork)
}

/// Where a label of size (w, h) saved at (x, y) is shown: there if a monitor
/// shows its middle, otherwise inside the nearest monitor's work area.
pub fn visible_position(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    let middle = POINT { x: x + w / 2, y: y + h / 2 };
    unsafe {
        if !MonitorFromPoint(middle, MONITOR_DEFAULTTONULL).is_null() {
            return (x, y);
        }
        let Some(area) = monitor_work_area(MonitorFromPoint(middle, MONITOR_DEFAULTTONEAREST)) else { return (x, y) };
        (x.clamp(area.left, (area.right - w).max(area.left)), y.clamp(area.top, (area.bottom - h).max(area.top)))
    }
}

fn to_rect(r: RECT) -> layout::Rect {
    layout::Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

/// The work area (the screen without the taskbar) of the monitor at this point.
pub fn work_area_at(x: i32, y: i32) -> layout::Rect {
    let area = unsafe { monitor_work_area(MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST)) };
    to_rect(area.unwrap_or(RECT { left: 0, top: 0, right: 1920, bottom: 1080 }))
}

/// The work area of the main monitor.
pub fn primary_work_area() -> layout::Rect {
    let area = unsafe { monitor_work_area(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)) };
    to_rect(area.unwrap_or(RECT { left: 0, top: 0, right: 1920, bottom: 1080 }))
}

unsafe fn text_width(dc: HDC, text: &str) -> i32 {
    let w = wide(text);
    let mut size = SIZE { cx: 0, cy: 0 };
    unsafe { GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32 - 1, &mut size) };
    size.cx
}

/// The label window lets the mouse through and is never activated.
unsafe extern "system" fn label_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub fn register_class() {
    let label = wide(LABEL_CLASS);
    let lc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(label_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: unsafe { GetModuleHandleW(null()) },
        hIcon: null_mut(),
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: label.as_ptr(),
    };
    unsafe { RegisterClassW(&lc) };
    let class = wide(CLASS_NAME);
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

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct CompositionAttribData {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

type SetWindowCompositionAttributeFn = unsafe extern "system" fn(HWND, *mut CompositionAttribData) -> i32;

/// Puts a blurred "acrylic" glass behind the window, the effect used by the Start menu.
/// The API is undocumented but stable since Windows 10 1803; if it is missing, the
/// panel simply has a dark background.
fn enable_glass(hwnd: HWND, settings: &Settings) {
    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;
    let alpha = ((settings.tint_opacity * 255.0).round() as u32).max(1);
    let mut policy = AccentPolicy {
        state: ACCENT_ENABLE_ACRYLICBLURBEHIND,
        flags: 0,
        gradient_color: (alpha << 24) | parse_color(&settings.tint, 0xFFFFFF),
        animation_id: 0,
    };
    let mut data = CompositionAttribData {
        attribute: WCA_ACCENT_POLICY,
        data: &mut policy as *mut _ as *mut c_void,
        size: std::mem::size_of::<AccentPolicy>(),
    };
    let user32 = wide("user32.dll");
    unsafe {
        let module = GetModuleHandleW(user32.as_ptr());
        if let Some(f) = GetProcAddress(module, c"SetWindowCompositionAttribute".as_ptr() as *const u8) {
            let set: SetWindowCompositionAttributeFn = std::mem::transmute(f);
            set(hwnd, &mut data);
        }
    }
}

/// Creates a panel owned by the desktop window, so "Show desktop" leaves it visible.
/// While panels are unlocked, it starts open if every folder opens then.
pub fn create(cfg: &Panel, settings: &Settings, desktop: HWND, main: HWND) -> HWND {
    let editing = !locked() && settings.unlocked_open_all;
    let state = Box::new(PanelState {
        pos: (cfg.x, cfg.y),
        push: 0,
        shown: (cfg.x, cfg.y),
        up: false,
        lifted: false,
        lift_next: false,
        unlift_next: false,
        label_window: null_mut(),
        last_reach: None,
        applied: None,
        moving: false,
        last_moving: None,
        snap_target: false,
        cfg: cfg.clone(),
        settings: settings.clone(),
        main,
        open: editing,
        pinned: false,
        highlighted: false,
        progress: if editing { 1.0 } else { 0.0 },
        anim_from: 0.0,
        anim_start: 0,
        open_pending: false,
        drag_opened: false,
        drag_open_pending: false,
        drop_insert: None,
        press: None,
        resizing: None,
        hover: None,
        tracking_mouse: false,
        tracking_nc: false,
        dpi: 96,
        font: null_mut(),
        title_font: null_mut(),
        title_text_w: 0,
        title_lines: Vec::new(),
        line_h: 0,
        item_line_h: 0,
        count_w: 0,
        title_icon: null_mut(),
        icons: Vec::new(),
    });
    let title = wide(&state.cfg.title);
    let class = wide(CLASS_NAME);
    unsafe {
        let hwnd = CreateWindowExW(
            // No WS_EX_NOACTIVATE: it makes Windows skip the panel as a drop target for
            // other programs. WM_MOUSEACTIVATE keeps clicks from taking the focus instead.
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            cfg.x,
            cfg.y,
            100,
            30,
            desktop,
            null_mut(),
            GetModuleHandleW(null()),
            Box::into_raw(state) as *const c_void,
        );
        if hwnd.is_null() {
            return hwnd;
        }
        let st = &mut *state_ptr(hwnd);
        enable_glass(hwnd, &st.settings);
        let corner = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &corner as *const _ as *const c_void,
            std::mem::size_of_val(&corner) as u32,
        );
        st.dpi = GetDpiForWindow(hwnd).max(96);
        st.load_resources();
        st.place();
        apply_size(hwnd, st);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        // A new window starts above the others: put it at the bottom, under
        // every program. Later moves leave the z-order alone.
        SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
        dragdrop::register(hwnd, &DROP_HANDLER);
        hwnd
    }
}

/// Shows new data for this panel, keeping its open and pinned state.
pub fn update(hwnd: HWND, cfg: &Panel, settings: &Settings) {
    let Some(st) = state(hwnd) else { return };
    let resources = st.cfg.title != cfg.title
        || st.cfg.icon != cfg.icon
        || st.cfg.items.len() != cfg.items.len()
        || st.cfg.items.iter().zip(&cfg.items).any(|(a, b)| a.path != b.path || a.icon != b.icon)
        || st.settings.font_size != settings.font_size
        || st.settings.icon_size != settings.icon_size
        || st.settings.title_style() != settings.title_style()
        || st.settings.show_folder_icon != settings.show_folder_icon;
    let glass = st.settings.tint != settings.tint || st.settings.tint_opacity != settings.tint_opacity;
    let open_all_changed = st.settings.unlocked_open_all != settings.unlocked_open_all;
    if st.cfg.title != cfg.title {
        let title = wide(&cfg.title);
        unsafe { SetWindowTextW(hwnd, title.as_ptr()) };
    }
    st.cfg = cfg.clone();
    st.settings = settings.clone();
    if st.hover.is_some_and(|i| i >= st.cfg.items.len()) {
        st.hover = None;
    }
    // A shortcut pressed before the change may be gone.
    if st.press.is_some_and(|(_, _, i)| i >= st.cfg.items.len()) {
        st.press = None;
    }
    if resources {
        st.load_resources();
    } else {
        st.measure();
    }
    if glass {
        enable_glass(hwnd, &st.settings);
    }
    st.place();
    st.choose_direction(hwnd);
    apply_size(hwnd, st);
    // Switched between "all open" and "open on hover" while unlocked.
    if open_all_changed && !locked() {
        if st.settings.unlocked_open_all {
            set_open(hwnd, st, true);
        } else if !st.highlighted {
            set_open(hwnd, st, false);
        }
    }
}

/// Moves the panel back on screen after the monitors changed, or back to its
/// saved position once its monitor is there again.
pub fn reposition(hwnd: HWND) {
    let Some(st) = state(hwnd) else { return };
    st.place();
    st.choose_direction(hwnd);
    apply_size(hwnd, st);
}

/// The id of the panel shown by this window.
pub fn id_of(hwnd: HWND) -> Option<u32> {
    state(hwnd).map(|st| st.cfg.id)
}

/// Unlocking opens every panel if the settings say so (otherwise they open on
/// hover, as when locked); locking closes them.
pub fn set_editing(hwnd: HWND, editing: bool) {
    let Some(st) = state(hwnd) else { return };
    if editing {
        if st.settings.unlocked_open_all {
            set_open(hwnd, st, true);
        }
    } else if !st.highlighted {
        set_open(hwnd, st, false);
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

pub fn geometry(hwnd: HWND) -> Option<Geometry> {
    let st = state(hwnd)?;
    let (w, h) = st.closed_size();
    let now_h = st.size().1;
    let (open_w, open_h) = st.open_size();
    Some(Geometry {
        label: layout::Rect { x: st.pos.0, y: st.pos.1, w, h },
        open_w,
        open_h,
        extra: (now_h - h).max(0),
        up: st.up,
        open: st.open,
        arranged: st.cfg.arranged,
        dpi: st.dpi,
        moving: st.moving,
    })
}

/// Where a window goes: (x, y, width, height, resized).
pub type Placement = (i32, i32, i32, i32, bool);

/// Where the window goes in the next batch of moves, pushed down by `push`
/// pixels. None when it does not change, or while it is being moved by the user.
pub fn place_for(hwnd: HWND, push: i32) -> Option<Placement> {
    let st = state(hwnd)?;
    if st.moving {
        return None;
    }
    st.push = push;
    let (w, h) = st.size();
    let (x, y) = st.window_pos();
    st.shown = (x, y);
    // The pushes were just worked out with this size: no need to ask again.
    st.last_reach = Some((st.pos, w, h, st.up));
    let rect = (x, y, w, h);
    if st.applied == Some(rect) {
        return None;
    }
    let resized = st.applied.is_none_or(|a| (a.2, a.3) != (w, h));
    st.applied = Some(rect);
    Some((x, y, w, h, resized))
}

/// Shows the label at (x, y) without saving it: the arrangement it is in is
/// being moved, or makes room for a dragged folder.
pub fn set_label(hwnd: HWND, x: i32, y: i32) {
    let Some(st) = state(hwnd) else { return };
    if st.moving || st.pos == (x, y) {
        return;
    }
    st.pos = (x, y);
    let (wx, wy) = st.window_pos();
    st.shown = (wx, wy);
    let (w, h) = st.size();
    st.applied = Some((wx, wy, w, h));
    let main = st.main;
    unsafe { SetWindowPos(hwnd, null_mut(), wx, wy, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER) };
    request_pushes(main);
}

/// Closes the panel at once, without animation, before a folder is moved. The
/// one selected in Settings stays open unless it is the one being moved.
pub fn close_now(hwnd: HWND, moving: bool) {
    let Some(st) = state(hwnd) else { return };
    if (st.highlighted && !moving) || (!st.open && st.progress <= 0.0) {
        return;
    }
    unsafe {
        KillTimer(hwnd, OPEN_TIMER);
        KillTimer(hwnd, CLOSE_TIMER);
        KillTimer(hwnd, ANIM_TIMER);
    }
    st.open = false;
    st.open_pending = false;
    st.pinned = false;
    st.hover = None;
    st.progress = 0.0;
    st.up = false;
    hide_label_window(st);
    apply_size(hwnd, st);
}

/// Lights the panel up as the one a moved folder snaps to.
pub fn set_snap_target(hwnd: HWND, on: bool) {
    let Some(st) = state(hwnd) else { return };
    if st.snap_target != on {
        st.snap_target = on;
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

/// The folder selected in the settings window stays open and outlined.
pub fn set_highlight(hwnd: HWND, on: bool) {
    let Some(st) = state(hwnd) else { return };
    if st.highlighted == on {
        return;
    }
    st.highlighted = on;
    if on {
        set_open(hwnd, st, true);
    } else if (locked() || !st.settings.unlocked_open_all) && !st.pinned && !st.tracking_mouse && !st.tracking_nc {
        set_open(hwnd, st, false);
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

/// The effect a drop would have: our own shortcuts move between panels; files
/// from elsewhere move in, unless Ctrl is held (or moving is not offered), which
/// adds a link and leaves the original where it is.
pub fn choose_effect(keys: u32, allowed: u32) -> u32 {
    if INTERNAL_DRAG.get().is_some() {
        return allowed & (DROPEFFECT_MOVE | DROPEFFECT_LINK);
    }
    if keys & MK_CONTROL == 0 && allowed & DROPEFFECT_MOVE != 0 {
        DROPEFFECT_MOVE
    } else if allowed & DROPEFFECT_LINK != 0 {
        DROPEFFECT_LINK
    } else {
        allowed & DROPEFFECT_COPY
    }
}

/// Adds dropped files to a panel at `index`, or moves our own dragged shortcut
/// there. Returns the effect for the drag source: none when deskfolds moved the
/// file itself, so the source never deletes anything.
pub fn drop_files(panel_id: u32, index: usize, files: Vec<String>, effect: u32) -> u32 {
    if effect == 0 {
        return 0;
    }
    if let Some(item) = internal_drag() {
        // One of our shortcuts: the panels are updated once the drag loop has ended.
        finish_internal_drop();
        if let Err(e) = store::change(|s| s.move_item(item, panel_id, index)) {
            error_box(&e);
        }
        return 0;
    }
    let link = effect != DROPEFFECT_MOVE;
    let errors = store::change(|s| s.add_files(panel_id, index, &files, link));
    if !errors.is_empty() {
        error_box(&format!("Some files could not be moved, so they were added as links:\n\n{}", errors.join("\n")));
    }
    if link { effect } else { 0 }
}

fn client_point(hwnd: HWND, screen: POINT) -> (i32, i32) {
    let mut pt = screen;
    unsafe { ScreenToClient(hwnd, &mut pt) };
    (pt.x, pt.y)
}

fn on_drag_over(hwnd: HWND, screen: POINT, keys: u32, allowed: u32) -> u32 {
    let Some(st) = state(hwnd) else { return 0 };
    unsafe { KillTimer(hwnd, CLOSE_TIMER) };
    // OLE calls this repeatedly even when the mouse is still: start the timer once.
    if !st.open && !st.drag_open_pending {
        st.drag_open_pending = true;
        unsafe { SetTimer(hwnd, DRAG_OPEN_TIMER, DRAG_OPEN_DELAY_MS, None) };
    }
    let (x, y) = client_point(hwnd, screen);
    let insert = Some(st.insert_index(x, y));
    if insert != st.drop_insert {
        st.drop_insert = insert;
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
    choose_effect(keys, allowed)
}

fn on_drag_leave(hwnd: HWND) {
    let Some(st) = state(hwnd) else { return };
    st.drop_insert = None;
    unsafe {
        KillTimer(hwnd, DRAG_OPEN_TIMER);
        st.drag_open_pending = false;
        if st.drag_opened && !st.pinned && !st.highlighted && (locked() || !st.settings.unlocked_open_all) {
            SetTimer(hwnd, CLOSE_TIMER, st.settings.close_delay_ms, None);
        }
        InvalidateRect(hwnd, null(), 0);
    }
}

fn on_drop(hwnd: HWND, files: Vec<String>, screen: POINT, keys: u32, allowed: u32) -> u32 {
    let Some(st) = state(hwnd) else { return 0 };
    let effect = choose_effect(keys, allowed);
    let (x, y) = client_point(hwnd, screen);
    let index = st.insert_index(x, y);
    let panel_id = st.cfg.id;
    st.drop_insert = None;
    st.drag_opened = false;
    st.drag_open_pending = false;
    unsafe {
        KillTimer(hwnd, DRAG_OPEN_TIMER);
        InvalidateRect(hwnd, null(), 0);
    }
    drop_files(panel_id, index, files, effect)
}

/// Drags a shortcut out of a panel with the shell's drag loop. Must be the last
/// thing the caller does with the panel: it may be updated right after.
fn drag_item_out(hwnd: HWND, st: &PanelState, index: usize) {
    let item = st.cfg.items[index].clone();
    let path = resolve(&item.path);
    if !Path::new(&path).exists() {
        return;
    }
    let panel_id = st.cfg.id;
    let main = st.main;
    let stored = store::with(|s| s.is_stored(panel_id, &item));
    // Files we keep can be moved anywhere. A link must never move the original
    // (it could be a program in Program Files), unless it sits on the desktop.
    let on_desktop = dragdrop::desktop_dir().is_some_and(|d| Path::new(&path).parent() == Some(d.as_path()));
    let allowed = if stored || on_desktop { DROPEFFECT_MOVE | DROPEFFECT_COPY | DROPEFFECT_LINK } else { DROPEFFECT_LINK };

    INTERNAL_DRAG.set(Some(item.id));
    INTERNAL_DROP_DONE.set(false);
    let dropped = dragdrop::drag_file_out(hwnd, &path, allowed);
    INTERNAL_DRAG.set(None);
    if !INTERNAL_DROP_DONE.replace(false)
        && let Some(effect) = dropped
    {
        // Out of the panel: the file went where it was dropped, or a shortcut to it
        // was created there. A copy (Ctrl) leaves the shortcut in the panel.
        let gone = !Path::new(&path).exists();
        let out = gone || effect & DROPEFFECT_LINK != 0 || (on_desktop && !stored && dragdrop::cursor_over_desktop());
        if out && let Err(e) = store::change(|s| s.remove_item(item.id, false)) {
            error_box(&e);
        }
    }
    // Updates were held back during the drag: apply them now.
    unsafe { PostMessageW(main, WM_APP_REFRESH, 0, 0) };
}

/// Right-click menu on a shortcut while unlocked.
fn item_menu(hwnd: HWND, st: &PanelState, index: usize) {
    let item = st.cfg.items[index].clone();
    let (panel_id, main, font) = (st.cfg.id, st.main, st.font);
    // Where the rename box goes: over the shortcut's name.
    let l = st.layout();
    let r = st.item_rect(&l, index);
    let mut origin = POINT { x: r.left, y: r.top + st.scale(8) + l.icon + st.scale(2) };
    unsafe { ClientToScreen(hwnd, &mut origin) };
    let (height, width) = (st.scale(st.settings.font_size + 10), l.cell_w.max(st.scale(120)));
    let rect = RECT {
        left: origin.x - (width - l.cell_w) / 2,
        top: origin.y,
        right: origin.x + (width + l.cell_w) / 2,
        bottom: origin.y + height,
    };

    let entries: [MenuEntry; 4] = [
        (ID_RENAME, "Rename", 0),
        (ID_REMOVE, "Remove from folder", 0),
        ui::SEPARATOR,
        (ID_SETTINGS, "Folder settings…", 0),
    ];
    let mut at = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut at) };
    // The panel may be updated while the menu or the rename box is open: from
    // here on, only the copied values are used.
    match ui::popup_menu(main, &entries, at) {
        ID_RENAME => {
            if let Some(name) = prompt_text(rect, &item.name, font) {
                store::change(|s| s.update_item(item.id, |it| it.name = name));
            }
        }
        ID_REMOVE => {
            // A file we keep goes back to the desktop; a link is just removed.
            if let Err(e) = store::change(|s| s.remove_item(item.id, true)) {
                error_box(&e);
            }
        }
        ID_SETTINGS => crate::settings::open(Some(panel_id), false),
        _ => {}
    }
}

fn state<'a>(hwnd: HWND) -> Option<&'a mut PanelState> {
    let ptr = state_ptr(hwnd);
    (!ptr.is_null()).then(|| unsafe { &mut *ptr })
}

fn state_ptr(hwnd: HWND) -> *mut PanelState {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PanelState }
}

fn apply_size(hwnd: HWND, st: &mut PanelState) {
    let (w, h) = st.size();
    let (x, y) = st.window_pos();
    st.shown = (x, y);
    st.applied = Some((x, y, w, h));
    unsafe {
        // The z-order is left alone: changing it makes Windows redraw the glass
        // of the other folders.
        SetWindowPos(hwnd, null_mut(), x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
        InvalidateRect(hwnd, null(), 0);
        UpdateWindow(hwnd);
    }
    // Folders stacked under this one follow how far it reaches.
    let reach = (st.pos, w, h, st.up);
    if st.last_reach != Some(reach) {
        st.last_reach = Some(reach);
        request_pushes(st.main);
    }
}

/// The label window of this folder, made the first time it is needed.
fn label_window(hwnd: HWND, st: &mut PanelState) -> HWND {
    if st.label_window.is_null() || unsafe { IsWindow(st.label_window) } == 0 {
        let class = wide(LABEL_CLASS);
        st.label_window = unsafe {
            CreateWindowExW(
                // Clicks go through it to the folder, which keeps the mouse.
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                // Owned by the folder: always just above it.
                hwnd,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            )
        };
    }
    st.label_window
}

/// Shows the label in the label window, at the label's place on the screen.
fn draw_label_window(hwnd: HWND, st: &mut PanelState) {
    let win = label_window(hwnd, st);
    if win.is_null() {
        return;
    }
    let l = st.layout();
    let (w, h) = (st.size().0.max(1), l.title_h.max(1));
    let (x, y) = (st.pos.0, st.pos.1 + st.push);
    let s = &st.settings;
    let (text, hover, border, accent) =
        (parse_color(&s.text, 0xF2F2F2), parse_color(&s.hover, 0x2E2E2E), parse_color(&s.border, 0x3A3A3A), parse_color(&s.accent, 0xFFC24C));
    let outlined = !locked() || st.highlighted;
    unsafe {
        let screen = GetDC(null_mut());
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if mem.is_null() || bmp.is_null() || bits.is_null() {
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            ReleaseDC(null_mut(), screen);
            return;
        }
        let old_bmp = SelectObject(mem, bmp);
        SetBkMode(mem, TRANSPARENT as _);
        SetTextColor(mem, text);
        // The bottom of the folder's border: its sides, corners and bottom edge.
        let radius = st.scale(8);
        let pen = CreatePen(PS_SOLID, if outlined { st.scale(2) } else { 1 }, if outlined { accent } else { border });
        let old_pen = SelectObject(mem, pen);
        let old_brush = SelectObject(mem, GetStockObject(NULL_BRUSH));
        RoundRect(mem, 0, -radius * 4, w, h, radius * 2, radius * 2);
        SelectObject(mem, old_brush);
        SelectObject(mem, old_pen);
        DeleteObject(pen);
        draw_title(mem, st, &l, 0, w, accent, hover);
        GdiFlush();
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, w as usize * h as usize);
        fix_alpha(pixels, w as usize, 0..h as usize, 1.0);
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let (at, size, origin) = (POINT { x, y }, SIZE { cx: w, cy: h }, POINT { x: 0, y: 0 });
        UpdateLayeredWindow(win, screen, &at, &size, mem, &origin, 0, &blend, ULW_ALPHA);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        if IsWindowVisible(win) == 0 {
            // Just above the folder, under every window that is above it. The
            // window right above the folder may be this one, hidden.
            let mut above = GetWindow(hwnd, GW_HWNDPREV);
            while above == win {
                above = GetWindow(above, GW_HWNDPREV);
            }
            let after = if above.is_null() { HWND_TOP } else { above };
            SetWindowPos(win, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER);
            if IsWindowVisible(win) == 0 {
                ShowWindow(win, SW_SHOWNOACTIVATE);
            }
        }
    }
}

/// Shows the label window over the label; from the next animation step on, it
/// shows the label instead of the folder (see `PanelState::lifted`).
fn lift_label(hwnd: HWND, st: &mut PanelState) {
    st.unlift_next = false;
    draw_label_window(hwnd, st);
    if !st.lifted {
        st.lift_next = true;
    }
}

/// The folder shows its own label again; the label window goes away at the
/// next step, once that is on the screen.
fn drop_label(hwnd: HWND, st: &mut PanelState) {
    if !st.lifted {
        return;
    }
    st.lifted = false;
    st.unlift_next = true;
    unsafe {
        InvalidateRect(hwnd, null(), 0);
        UpdateWindow(hwnd);
    }
}

fn hide_label_window(st: &mut PanelState) {
    st.lifted = false;
    st.lift_next = false;
    st.unlift_next = false;
    if !st.label_window.is_null() {
        unsafe { ShowWindow(st.label_window, SW_HIDE) };
    }
}

fn set_open(hwnd: HWND, st: &mut PanelState, open: bool) {
    unsafe {
        KillTimer(hwnd, OPEN_TIMER);
        KillTimer(hwnd, CLOSE_TIMER);
    }
    st.open_pending = false;
    if !open {
        st.pinned = false;
    }
    if st.open == open {
        return;
    }
    if open && st.progress <= 0.0 {
        st.up = st.settings.open_upwards && st.needs_up(hwnd);
    }
    st.open = open;
    st.hover = None;
    if open {
        raise_above_panels(hwnd);
    }
    if st.settings.animation_ms == 0 {
        st.progress = if open { 1.0 } else { 0.0 };
        st.up &= open;
        hide_label_window(st);
        apply_size(hwnd, st);
    } else {
        if st.up {
            lift_label(hwnd, st);
        }
        st.anim_from = st.progress;
        st.anim_start = unsafe { GetTickCount64() };
        unsafe { SetTimer(hwnd, ANIM_TIMER, ANIM_FRAME_MS, None) };
    }
}

fn animate(hwnd: HWND, st: &mut PanelState) {
    if st.unlift_next {
        // The folder shows its own label since the last step.
        hide_label_window(st);
        unsafe { KillTimer(hwnd, ANIM_TIMER) };
        return;
    }
    if st.lift_next {
        // The label window is on the screen since the last step: the folder stops
        // drawing its label, and the animation starts now. If it could not be
        // shown, the folder keeps drawing its label.
        st.lift_next = false;
        st.lifted = !st.label_window.is_null() && unsafe { IsWindowVisible(st.label_window) } != 0;
        st.anim_from = st.progress;
        st.anim_start = unsafe { GetTickCount64() };
        unsafe {
            InvalidateRect(hwnd, null(), 0);
            UpdateWindow(hwnd);
        }
        return;
    }
    let target = if st.open { 1.0 } else { 0.0 };
    // A full open takes animation_ms; a reversed half-way animation takes less.
    let duration = st.settings.animation_ms as f32 * (target - st.anim_from).abs();
    let elapsed = unsafe { GetTickCount64() } - st.anim_start;
    let p = if duration <= 0.0 { 1.0 } else { (elapsed as f32 / duration).min(1.0) };
    st.progress = st.anim_from + (target - st.anim_from) * p;
    let done = p >= 1.0;
    if done {
        st.progress = target;
        st.up &= st.open;
        if st.lifted {
            // One more step takes the label window away (see drop_label).
            drop_label(hwnd, st);
        } else {
            unsafe { KillTimer(hwnd, ANIM_TIMER) };
        }
    }
    // This folder's new size and the folders it pushes move together, in one
    // batch: otherwise some are drawn moved and others not, and the glass flickers.
    crate::relayout();
    if st.lifted {
        draw_label_window(hwnd, st);
    }
}

/// Puts an opening panel above the other panels, still under every other
/// window, so it covers the neighbours it overlaps. Only this window changes
/// place, and only if a panel is above it: moving the others in the z-order
/// makes Windows redraw their glass, which flickers.
fn raise_above_panels(hwnd: HWND) {
    let ours = crate::panels();
    let mut top = null_mut();
    let mut h = hwnd;
    loop {
        h = unsafe { GetWindow(h, GW_HWNDPREV) };
        if h.is_null() {
            break;
        }
        if ours.contains(&h) {
            top = h;
        }
    }
    if top.is_null() {
        return;
    }
    let above = unsafe { GetWindow(top, GW_HWNDPREV) };
    RAISING.set(true);
    // NOOWNERZORDER: the desktop window owns the panels and must stay where it is.
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
    unsafe { SetWindowPos(hwnd, if above.is_null() { HWND_TOP } else { above }, 0, 0, 0, 0, flags) };
    RAISING.set(false);
}

fn cursor_x() -> i32 {
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt) };
    pt.x
}

/// The cursor is over the window (its name or its shortcuts).
/// The mouse is on the panel itself: the panel is the window under it. A
/// window over the panel covers it, even inside its rectangle. A menu of
/// deskfolds over the panel (its right-click menu) still counts as the panel.
fn cursor_inside(hwnd: HWND) -> bool {
    let mut pt = POINT { x: 0, y: 0 };
    let mut r: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetCursorPos(&mut pt) == 0 || GetWindowRect(hwnd, &mut r) == 0 } {
        return false;
    }
    let in_rect = pt.x >= r.left && pt.x < r.right && pt.y >= r.top && pt.y < r.bottom;
    let under = unsafe { WindowFromPoint(pt) };
    if under.is_null() || !in_rect {
        return false;
    }
    unsafe { GetAncestor(under, GA_ROOT) == hwnd || is_own_menu(under) }
}

/// A popup menu (window class "#32768") of this thread.
fn is_own_menu(hwnd: HWND) -> bool {
    let mut class = [0u16; 16];
    let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
    let ours = unsafe { GetWindowThreadProcessId(hwnd, null_mut()) == GetCurrentThreadId() };
    ours && String::from_utf16_lossy(&class[..n.max(0) as usize]) == "#32768"
}

/// A menu of deskfolds closed: an open folder the mouse is no longer on closes
/// after the usual delay, as when the mouse leaves it.
pub fn after_menu() {
    let locked = locked();
    for hwnd in crate::panels() {
        if let Some(st) = state(hwnd)
            && st.open
            && !cursor_inside(hwnd)
        {
            on_leave(hwnd, st, locked);
        }
    }
}

/// The mouse is over the panel: stop closing, and open after the hover delay.
fn on_hover(hwnd: HWND, st: &mut PanelState, locked: bool) {
    unsafe { KillTimer(hwnd, CLOSE_TIMER) };
    if (locked || !st.settings.unlocked_open_all) && st.settings.hover_open && !st.open && !st.open_pending && !st.moving {
        st.open_pending = true;
        unsafe { SetTimer(hwnd, OPEN_TIMER, st.settings.open_delay_ms, None) };
    }
}

/// The mouse left the panel: close after the delay, unless it is held open.
fn on_leave(hwnd: HWND, st: &mut PanelState, locked: bool) {
    st.open_pending = false;
    unsafe { KillTimer(hwnd, OPEN_TIMER) };
    let hover_closes = (locked || !st.settings.unlocked_open_all) && st.settings.hover_open;
    if hover_closes && st.open && !st.pinned && !st.highlighted {
        unsafe { SetTimer(hwnd, CLOSE_TIMER, st.settings.close_delay_ms, None) };
    }
}

/// While a folder is moved: snaps its label to the nearest edge and lights up
/// the folders it snaps to. Alt moves it freely.
fn snap_moving(hwnd: HWND, st: &PanelState, r: &mut RECT) {
    let dy = st.shown.1 - st.pos.1;
    let (w, h) = st.closed_size();
    // Over the arrangement's frame, it takes a place there instead of snapping.
    let mut cursor = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut cursor) };
    let free = unsafe { GetKeyState(VK_MENU as i32) } < 0;
    if crate::arrange::folder_moving(st.cfg.id, layout::Rect { x: r.left, y: r.top - dy, w, h }, cursor, free) {
        crate::show_snap_targets(hwnd, &[]);
        return;
    }
    let mut targets = Vec::new();
    if st.settings.snap && unsafe { GetKeyState(VK_MENU as i32) } >= 0 {
        let label = layout::Rect { x: r.left, y: r.top - dy, w, h };
        let others = crate::other_labels(hwnd);
        let rects: Vec<layout::Rect> = others.iter().map(|o| o.1).collect();
        let area = work_area_at(label.x + w / 2, label.y + h / 2);
        let snapped = layout::snap(label, &rects, area, st.scale(st.settings.gap), st.scale(SNAP_REACH));
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        r.left = snapped.x;
        r.top = snapped.y + dy;
        r.right = r.left + ww;
        r.bottom = r.top + wh;
        targets = snapped.with.iter().map(|&i| others[i].0).collect();
    }
    crate::show_snap_targets(hwnd, &targets);
}

/// Ends a resize with the grip and saves the new width.
fn finish_resize(st: &mut PanelState) {
    if st.resizing.take().is_some() {
        let (id, width) = (st.cfg.id, st.cfg.width);
        store::change(|s| s.update_panel(id, |p| p.width = width));
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let ptr = state_ptr(hwnd);
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let st = &mut *ptr;
        let locked = locked();
        let mouse = || ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32);

        match msg {
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            WM_WINDOWPOSCHANGING => {
                // Stay at the bottom of the z-order, and pinned while locked. A
                // move that leaves the z-order alone keeps it alone.
                let pos = &mut *(lparam as *mut WINDOWPOS);
                if pos.flags & SWP_NOZORDER == 0 && !RAISING.get() {
                    pos.hwndInsertAfter = HWND_BOTTOM;
                }
                if locked {
                    pos.x = st.shown.0;
                    pos.y = st.shown.1;
                }
                return 0;
            }
            WM_NCHITTEST => {
                if !locked {
                    let mut pt = POINT { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
                    ScreenToClient(hwnd, &mut pt);
                    if let Hit::Title = st.hit(pt.x, pt.y) {
                        return HTCAPTION as LRESULT;
                    }
                }
                return HTCLIENT as LRESULT;
            }
            WM_NCLBUTTONDOWN if !locked && wparam == HTCAPTION as usize => {
                // About to be moved by its name: the folders fold, so it moves by its
                // label and nothing is pushed around meanwhile. DefWindowProc moves it.
                crate::fold_for_move(hwnd);
            }
            WM_ENTERSIZEMOVE => {
                st.moving = !locked;
                st.last_moving = None;
                return 0;
            }
            WM_MOVING if st.moving => {
                let r = &mut *(lparam as *mut RECT);
                snap_moving(hwnd, st, r);
                st.last_moving = Some(*r);
                return 1;
            }
            WM_EXITSIZEMOVE => {
                // Moved by its name while unlocked: save where its label was put.
                let mut r: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut r);
                let dy = st.shown.1 - st.pos.1;
                let moved = st.last_moving.take().is_some_and(|m| (m.left, m.top) == (r.left, r.top));
                st.moving = false;
                st.pos = (r.left, r.top - dy);
                st.shown = (r.left, r.top);
                let (id, (x, y), main) = (st.cfg.id, st.pos, st.main);
                crate::show_snap_targets(hwnd, &[]);
                if moved {
                    // Saves it, and puts it into the arrangement or out of it.
                    crate::arrange::folder_dropped(id, x, y);
                } else {
                    // Esc put it back, or it was only clicked.
                    crate::arrange::cancel_drag();
                }
                request_pushes(main);
                return 0;
            }
            WM_WINDOWPOSCHANGED => {
                // Sent to the bottom of the z-order: the arrangement's frame goes back under it.
                if (*(lparam as *const WINDOWPOS)).flags & SWP_NOZORDER == 0 {
                    crate::frame::keep_below();
                }
            }
            WM_NCMOUSEMOVE => {
                // While unlocked the name is the move handle, outside the client area.
                if !st.tracking_nc {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme);
                    st.tracking_nc = true;
                }
                on_hover(hwnd, st, locked);
            }
            WM_NCMOUSELEAVE => {
                st.tracking_nc = false;
                if !cursor_inside(hwnd) {
                    on_leave(hwnd, st, locked);
                }
                return 0;
            }
            WM_SETCURSOR => {
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                ScreenToClient(hwnd, &mut pt);
                let cursor = match st.hit(pt.x, pt.y) {
                    _ if st.resizing.is_some() => IDC_SIZEWE,
                    Hit::Grip => IDC_SIZEWE,
                    Hit::Title if !locked => IDC_SIZEALL,
                    Hit::Title | Hit::Item(_) => IDC_HAND,
                    Hit::Nothing => IDC_ARROW,
                };
                SetCursor(LoadCursorW(null_mut(), cursor));
                return 1;
            }
            WM_MOUSEMOVE => {
                if let Some((start_x, start_width)) = st.resizing {
                    let delta = ((cursor_x() - start_x) as f32 * 96.0 / st.dpi as f32).round() as i32;
                    let width = (start_width + delta).clamp(MIN_WIDTH, 4000);
                    if width != st.cfg.width {
                        st.cfg.width = width;
                        apply_size(hwnd, st);
                    }
                    return 0;
                }
                // Unlocked: pressing a shortcut and moving starts dragging it out.
                if let Some((px, py, item)) = st.press
                    && !locked
                    && wparam as u32 & MK_LBUTTON != 0
                {
                    let (x, y) = mouse();
                    if (x - px).abs() > GetSystemMetrics(SM_CXDRAG) || (y - py).abs() > GetSystemMetrics(SM_CYDRAG) {
                        st.press = None;
                        drag_item_out(hwnd, st, item);
                        return 0;
                    }
                }
                if !st.tracking_mouse {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme);
                    st.tracking_mouse = true;
                }
                on_hover(hwnd, st, locked);
                let (x, y) = mouse();
                let hover = match st.hit(x, y) {
                    Hit::Item(i) => Some(i),
                    _ => None,
                };
                if hover != st.hover {
                    st.hover = hover;
                    InvalidateRect(hwnd, null(), 0);
                }
                return 0;
            }
            WM_MOUSELEAVE => {
                st.tracking_mouse = false;
                st.hover = None;
                InvalidateRect(hwnd, null(), 0);
                // Moving onto the name while unlocked is not leaving.
                if !cursor_inside(hwnd) {
                    on_leave(hwnd, st, locked);
                }
                return 0;
            }
            WM_TIMER => {
                match wparam {
                    OPEN_TIMER => set_open(hwnd, st, true),
                    CLOSE_TIMER => {
                        // Held open while the mouse is on a folder it pushed down: the
                        // folder under the mouse never moves.
                        if !crate::cursor_in_stack(hwnd) {
                            set_open(hwnd, st, false);
                        }
                    }
                    ANIM_TIMER => animate(hwnd, st),
                    DRAG_OPEN_TIMER => {
                        KillTimer(hwnd, DRAG_OPEN_TIMER);
                        st.drag_open_pending = false;
                        if !st.open {
                            set_open(hwnd, st, true);
                            st.drag_opened = true;
                        }
                    }
                    _ => {}
                }
                return 0;
            }
            WM_LBUTTONDOWN => {
                let (x, y) = mouse();
                st.press = None;
                match st.hit(x, y) {
                    Hit::Grip => {
                        st.resizing = Some((cursor_x(), st.cfg.width));
                        SetCapture(hwnd);
                    }
                    Hit::Item(i) if !locked => st.press = Some((x, y, i)),
                    _ => {}
                }
                return 0;
            }
            WM_LBUTTONUP => {
                st.press = None;
                if st.resizing.is_some() {
                    // Sends WM_CAPTURECHANGED, which saves the new width.
                    ReleaseCapture();
                    return 0;
                }
                let (x, y) = mouse();
                if !locked {
                    // Editing: shortcuts are dragged, not launched.
                    return 0;
                }
                match st.hit(x, y) {
                    Hit::Title if !st.settings.hover_open => set_open(hwnd, st, !st.open),
                    Hit::Title => {
                        // Clicking the name pins the panel open, clicking again unpins it.
                        if st.open {
                            st.pinned = !st.pinned;
                        } else {
                            set_open(hwnd, st, true);
                            st.pinned = true;
                        }
                        InvalidateRect(hwnd, null(), 0);
                    }
                    Hit::Item(i) => {
                        let item = st.cfg.items[i].clone();
                        if st.settings.collapse_after_launch && !st.pinned && !st.highlighted {
                            set_open(hwnd, st, false);
                        }
                        // Last, and nothing of the panel is used after it: ShellExecute can
                        // run a message loop (a security prompt) in which the panel is destroyed.
                        launch(&item.path, &item.args);
                    }
                    Hit::Grip | Hit::Nothing => {}
                }
                return 0;
            }
            WM_CAPTURECHANGED => {
                finish_resize(st);
                return 0;
            }
            WM_RBUTTONUP => {
                let (x, y) = mouse();
                match st.hit(x, y) {
                    Hit::Item(i) if !locked => {
                        item_menu(hwnd, st, i);
                        // The mouse may have gone elsewhere while the menu was open.
                        if let Some(st) = state(hwnd)
                            && !cursor_inside(hwnd)
                        {
                            on_leave(hwnd, st, locked);
                        }
                    }
                    _ => {
                        PostMessageW(st.main, WM_APP_SHOW_MENU, st.cfg.id as usize, 0);
                    }
                }
                return 0;
            }
            WM_NCRBUTTONUP => {
                PostMessageW(st.main, WM_APP_SHOW_MENU, st.cfg.id as usize, 0);
                return 0;
            }
            WM_DESTROY => dragdrop::revoke(hwnd),
            WM_DPICHANGED => {
                st.dpi = ((wparam >> 16) & 0xFFFF) as u32;
                st.load_resources();
                if !locked {
                    let suggested = &*(lparam as *const RECT);
                    st.pos = (suggested.left, suggested.top);
                }
                apply_size(hwnd, st);
                // Its size changed: the arrangement it may be in is placed again.
                PostMessageW(st.main, WM_APP_ARRANGE, 0, 0);
                return 0;
            }
            WM_SYSCOMMAND => {
                // Never minimize, maximize or close from the system menu.
                let cmd = (wparam & 0xFFF0) as u32;
                if cmd == SC_MINIMIZE || cmd == SC_MAXIMIZE || cmd == SC_CLOSE {
                    return 0;
                }
            }
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                paint(hwnd, st, locked);
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

unsafe fn fill_round(hdc: HDC, r: &RECT, radius: i32, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let old_brush = SelectObject(hdc, brush);
        let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
        RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_brush);
        DeleteObject(brush);
    }
}

unsafe fn draw_text(hdc: HDC, text: &str, r: &RECT, flags: u32) {
    let w = wide(text);
    let mut r = *r;
    unsafe { DrawTextW(hdc, w.as_ptr(), -1, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS) };
}

/// GDI draws with alpha 0. On the glass that would be invisible, so every
/// pixel GDI touched gets an alpha equal to its brightness: grays become
/// light on the glass. Icons already carry their own alpha. Rows outside the
/// title fade in with the opening animation.
fn fix_alpha(pixels: &mut [u32], w: usize, title: std::ops::Range<usize>, fade: f32) {
    let f = (fade.clamp(0.0, 1.0) * 255.0) as u32;
    for (y, row) in pixels.chunks_mut(w).enumerate() {
        let scale = if title.contains(&y) { 255 } else { f };
        for p in row {
            let (mut a, r, g, b) = (*p >> 24, (*p >> 16) & 0xFF, (*p >> 8) & 0xFF, *p & 0xFF);
            if a == 0 {
                a = r.max(g).max(b);
            }
            let s = |c: u32| c * scale / 255;
            *p = (s(a) << 24) | (s(r) << 16) | (s(g) << 8) | s(b);
        }
    }
}

/// The number of shortcuts in a small rounded badge, starting at x, centered on cy.
unsafe fn draw_badge(dc: HDC, st: &PanelState, x: i32, cy: i32, color: u32) {
    let h = st.item_line_h + st.scale(2);
    let r = RECT { left: x, top: cy - h / 2, right: x + st.badge_w(), bottom: cy - h / 2 + h };
    unsafe {
        fill_round(dc, &r, h, color);
        let old = SelectObject(dc, st.font);
        draw_text(dc, &st.cfg.items.len().to_string(), &r, DT_CENTER | DT_VCENTER);
        SelectObject(dc, old);
    }
}

/// The folder's icon and name, the shortcut count and the pin marker, in the
/// title band starting at `top`.
unsafe fn draw_title(dc: HDC, st: &PanelState, l: &Layout, top: i32, w: i32, accent: u32, badge: u32) {
    let s = |v: i32| st.scale(v);
    let count = st.settings.show_count;
    let icon = l.label_icon;
    let draw_icon = |x: i32, y: i32| {
        if !st.title_icon.is_null() {
            unsafe { DrawIconEx(dc, x, y, st.title_icon, icon, icon, 0, null_mut(), DI_NORMAL) };
        }
    };
    unsafe {
        let old_font = SelectObject(dc, st.title_font);
        let pin_at = match st.style() {
            TitleStyle::IconAbove => {
                let (ix, iy) = ((w - icon) / 2, top + s(10));
                draw_icon(ix, iy);
                if count {
                    draw_badge(dc, st, ix + icon - s(6), iy + s(4), badge);
                }
                let y0 = iy + icon + s(4);
                for (k, line) in st.title_lines.iter().enumerate() {
                    let y = y0 + k as i32 * st.line_h;
                    let r = RECT { left: s(12), top: y, right: w - s(12), bottom: y + st.line_h };
                    draw_text(dc, line, &r, DT_CENTER | DT_TOP);
                }
                (w - s(14), top + s(14))
            }
            TitleStyle::Fit => {
                let cy = top + l.title_h / 2;
                draw_icon(s(12), cy - icon / 2);
                let text_left = s(12) + icon + st.icon_gap();
                let pin_space = if st.pinned { s(18) } else { 0 };
                let badge_space = if count { st.badge_w() + s(6) } else { 0 };
                let right = w - s(8) - pin_space - badge_space;
                let r = RECT { left: text_left, top, right, bottom: top + l.title_h };
                draw_text(dc, &st.cfg.title, &r, DT_LEFT | DT_VCENTER);
                if count {
                    draw_badge(dc, st, text_left + st.title_text_w.min(right - text_left) + s(6), cy, badge);
                }
                (w - s(16), cy)
            }
            style => {
                // Icon, lines and count as one group, centered or on the left.
                let room = st.title_wrap_width();
                let block_w = st.title_lines.iter().map(|t| text_width(dc, t)).max().unwrap_or(0).min(room);
                let badge_space = if count { s(6) + st.badge_w() } else { 0 };
                let group_w = icon + st.icon_gap() + block_w + badge_space;
                let min_x = s(12);
                let max_x = w - s(12 + PIN_ROOM) - group_w;
                let gx = if style == TitleStyle::Centered { ((w - group_w) / 2).min(max_x).max(min_x) } else { min_x };
                let cy = top + l.title_h / 2;
                draw_icon(gx, cy - icon / 2);
                let tx = gx + icon + st.icon_gap();
                let y0 = cy - st.title_lines.len() as i32 * st.line_h / 2;
                let align = if style == TitleStyle::Centered { DT_CENTER } else { DT_LEFT };
                for (k, line) in st.title_lines.iter().enumerate() {
                    let y = y0 + k as i32 * st.line_h;
                    let r = RECT { left: tx, top: y, right: tx + block_w, bottom: y + st.line_h };
                    draw_text(dc, line, &r, align | DT_TOP);
                }
                if count {
                    draw_badge(dc, st, tx + block_w + s(6), cy, badge);
                }
                (w - s(16), cy)
            }
        };
        if st.pinned {
            let d = s(7);
            let (cx, cy) = pin_at;
            let dot = RECT { left: cx - d / 2, top: cy - d / 2, right: cx + d / 2 + 1, bottom: cy + d / 2 + 1 };
            fill_round(dc, &dot, d, accent);
        }
        SelectObject(dc, old_font);
    }
}

fn paint(hwnd: HWND, st: &PanelState, locked: bool) {
    let s = &st.settings;
    let text = parse_color(&s.text, 0xF2F2F2);
    let hover = parse_color(&s.hover, 0x2E2E2E);
    let border = parse_color(&s.border, 0x3A3A3A);
    let accent = parse_color(&s.accent, 0xFFC24C);
    let l = st.layout();
    let outlined = !locked || st.highlighted;

    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right, client.bottom);
        if w <= 0 || h <= 0 {
            EndPaint(hwnd, &ps);
            return;
        }

        // Off-screen 32-bit buffer, starting fully transparent.
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if mem.is_null() || bmp.is_null() || bits.is_null() {
            // Out of memory or GDI handles: nothing drawn this time.
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            EndPaint(hwnd, &ps);
            return;
        }
        let old_bmp = SelectObject(mem, bmp);
        SetBkMode(mem, TRANSPARENT as _);
        SetTextColor(mem, text);
        let radius = st.scale(8);

        // Border: in the accent color while it can be edited or is selected in
        // Settings, and thicker on the folder a moved one snaps to.
        if st.snap_target {
            // A blue glow on the glass: half the accent's brightness.
            fill_round(mem, &RECT { left: 0, top: 0, right: w, bottom: h }, radius * 2, (accent >> 1) & 0x7F7F7F);
        }
        let pen_w = if st.snap_target { st.scale(4) } else if outlined { st.scale(2) } else { 1 };
        let pen = CreatePen(PS_SOLID, pen_w, if outlined || st.snap_target { accent } else { border });
        let old_pen = SelectObject(mem, pen);
        let old_brush = SelectObject(mem, GetStockObject(NULL_BRUSH));
        // While lifted, the label window draws the label and the bottom of the border.
        let above_label = if st.lifted { h - l.title_h } else { h };
        let clip = CreateRectRgn(0, 0, w, above_label);
        SelectClipRgn(mem, clip);
        RoundRect(mem, 0, 0, w, h, radius * 2, radius * 2);
        SelectObject(mem, old_brush);
        SelectObject(mem, old_pen);
        DeleteObject(pen);

        let title_top = st.title_top(&l);
        if !st.lifted {
            draw_title(mem, st, &l, title_top, w, accent, hover);
        }
        let old_font = SelectObject(mem, st.font);

        if st.progress > 0.0 {
            for (i, item) in st.cfg.items.iter().enumerate() {
                let r = st.item_rect(&l, i);
                if r.top >= h {
                    break;
                }
                if r.bottom <= 0 {
                    continue;
                }
                if st.hover == Some(i) {
                    let inset = RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom - 1 };
                    fill_round(mem, &inset, radius, hover);
                }
                let icon_top = r.top + st.scale(8);
                let icon = st.icons.get(i).copied().unwrap_or(null_mut());
                if !icon.is_null() {
                    let left = r.left + (l.cell_w - l.icon) / 2;
                    DrawIconEx(mem, left, icon_top, icon, l.icon, l.icon, 0, null_mut(), DI_NORMAL);
                }
                let label_top = icon_top + l.icon + st.scale(4);
                let label = RECT { left: r.left + st.scale(2), top: label_top, right: r.right - st.scale(2), bottom: r.bottom };
                if st.settings.item_lines >= 2 {
                    let lines = layout::wrap(&item.name, label.right - label.left, 2, |t| text_width(mem, t));
                    for (k, line) in lines.iter().enumerate() {
                        let top = label_top + k as i32 * st.item_line_h;
                        draw_text(mem, line, &RECT { top, bottom: top + st.item_line_h, ..label }, DT_CENTER | DT_TOP);
                    }
                } else {
                    draw_text(mem, &item.name, &label, DT_CENTER | DT_TOP);
                }
            }
            if st.cfg.items.is_empty() {
                let top = st.content_top(&l);
                let hint = RECT { left: l.pad, top, right: w - l.pad, bottom: top + l.cell_h };
                SetTextColor(mem, 0x9A9A9A);
                draw_text(mem, "Drop shortcuts here", &hint, DT_CENTER | DT_VCENTER);
                SetTextColor(mem, text);
            }
            // Insertion bar while a file is dragged over the panel.
            if let Some(at) = st.drop_insert
                && st.progress >= 1.0
                && !st.cfg.items.is_empty()
            {
                let count = st.cfg.items.len();
                let r = st.item_rect(&l, at.min(count - 1));
                let x = if at < count { r.left } else { r.right };
                let bar = RECT { left: x - st.scale(1), top: r.top + st.scale(6), right: x + st.scale(2), bottom: r.bottom - st.scale(6) };
                fill_round(mem, &bar, st.scale(2), accent);
            }
            // Resize grip on the right edge while unlocked.
            if !locked && st.progress >= 1.0 && !st.cfg.fit_columns {
                let mid = if st.up { (h - l.title_h) / 2 } else { (l.title_h + h) / 2 };
                let grip = RECT { left: w - st.scale(5), top: mid - st.scale(12), right: w - st.scale(3), bottom: mid + st.scale(12) };
                fill_round(mem, &grip, st.scale(2), accent);
            }
        }

        SelectClipRgn(mem, null_mut());
        DeleteObject(clip);
        GdiFlush();
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, w as usize * h as usize);
        let band = if st.lifted { 0..0 } else { title_top.max(0) as usize..(title_top + l.title_h).max(0) as usize };
        fix_alpha(pixels, w as usize, band, ease_out_cubic(st.progress));

        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_font);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdi_pixels_get_alpha_from_brightness() {
        let mut px = [0x00F0F0F0, 0x80402010, 0x00000000];
        fix_alpha(&mut px, 3, 0..1, 1.0);
        assert_eq!(px, [0xF0F0F0F0, 0x80402010, 0x00000000]);
    }

    #[test]
    fn rows_below_title_fade() {
        let mut px = [0x00FFFFFF, 0x00FFFFFF];
        fix_alpha(&mut px, 1, 0..1, 0.0);
        assert_eq!(px, [0xFFFFFFFF, 0x00000000]);
        // Opening upwards, the title is the bottom row.
        let mut px = [0x00FFFFFF, 0x00FFFFFF];
        fix_alpha(&mut px, 1, 1..2, 0.0);
        assert_eq!(px, [0x00000000, 0xFFFFFFFF]);
    }
}
