//! A panel is a small borderless window glued to the desktop: a title bar that
//! folds and unfolds a grid of shortcuts. It stays below every other window,
//! never takes focus, and cannot move while panels are locked.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{Panel, Settings, parse_color};
use crate::util::{launch, load_icon, wide};

const CLASS_NAME: &str = "deskfolds.panel";
const COLLAPSE_TIMER: usize = 1;
const COLLAPSE_DELAY_MS: u32 = 700;

/// Shared by all panels: while false, panels can be dragged by their title bar.
pub static LOCKED: AtomicBool = AtomicBool::new(true);

/// Posted to the main window to open the tray menu at the cursor.
pub const WM_APP_SHOW_MENU: u32 = WM_APP + 2;

pub struct PanelState {
    cfg: Panel,
    settings: Rc<Settings>,
    menu_target: HWND,
    pub x: i32,
    pub y: i32,
    expanded: bool,
    hover: Option<usize>,
    tracking_mouse: bool,
    dpi: u32,
    font: HFONT,
    title_font: HFONT,
    icons: Vec<HICON>,
}

enum Hit {
    Title,
    Item(usize),
    Nothing,
}

struct Layout {
    width: i32,
    title_h: i32,
    pad: i32,
    cell_w: i32,
    cell_h: i32,
    icon: i32,
    rows: i32,
}

impl PanelState {
    fn scale(&self, v: i32) -> i32 {
        (v as f32 * self.dpi as f32 / 96.0).round() as i32
    }

    fn layout(&self) -> Layout {
        let s = &self.settings;
        let width = self.scale(self.cfg.width);
        let pad = self.scale(6);
        let cols = self.cfg.columns;
        let icon = self.scale(s.icon_size);
        let line = self.scale(s.font_size * 3 / 2);
        Layout {
            width,
            title_h: self.scale(s.font_size + 16),
            pad,
            cell_w: (width - pad * 2) / cols,
            cell_h: self.scale(8) + icon + self.scale(4) + line + self.scale(4),
            icon,
            rows: (self.cfg.items.len() as i32 + cols - 1) / cols,
        }
    }

    fn size(&self) -> (i32, i32) {
        let l = self.layout();
        let h = if self.expanded && l.rows > 0 { l.title_h + l.pad * 2 + l.rows * l.cell_h } else { l.title_h };
        (l.width, h)
    }

    fn item_rect(&self, l: &Layout, i: usize) -> RECT {
        let col = i as i32 % self.cfg.columns;
        let row = i as i32 / self.cfg.columns;
        let left = l.pad + col * l.cell_w;
        let top = l.title_h + l.pad + row * l.cell_h;
        RECT { left, top, right: left + l.cell_w, bottom: top + l.cell_h }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let l = self.layout();
        if y < l.title_h {
            return Hit::Title;
        }
        if self.expanded {
            for i in 0..self.cfg.items.len() {
                let r = self.item_rect(&l, i);
                if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                    return Hit::Item(i);
                }
            }
        }
        Hit::Nothing
    }

    /// (Re)creates fonts and icons for the current DPI.
    fn load_resources(&mut self) {
        self.free_resources();
        let face = wide("Segoe UI");
        let height = -self.scale(self.settings.font_size);
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
                CLEARTYPE_QUALITY as _,
                DEFAULT_PITCH as _,
                face.as_ptr(),
            )
        };
        self.font = make_font(FW_NORMAL as i32);
        self.title_font = make_font(FW_SEMIBOLD as i32);
        let size = self.scale(self.settings.icon_size);
        self.icons = self
            .cfg
            .items
            .iter()
            .map(|item| load_icon(item.icon.as_deref().unwrap_or(&item.path), size))
            .collect();
    }

    fn free_resources(&mut self) {
        unsafe {
            for icon in self.icons.drain(..) {
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
    }
}

impl Drop for PanelState {
    fn drop(&mut self) {
        self.free_resources();
    }
}

pub fn register_class() {
    let class = wide(CLASS_NAME);
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
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

/// Creates a panel owned by the desktop window, so "Show desktop" leaves it visible.
pub fn create(cfg: Panel, settings: Rc<Settings>, desktop: HWND, menu_target: HWND) -> HWND {
    let opaque = settings.opacity >= 1.0;
    let alpha = (settings.opacity * 255.0).round() as u8;
    let state = Box::new(PanelState {
        x: cfg.x,
        y: cfg.y,
        cfg,
        settings,
        menu_target,
        expanded: false,
        hover: None,
        tracking_mouse: false,
        dpi: 96,
        font: null_mut(),
        title_font: null_mut(),
        icons: Vec::new(),
    });
    let (x, y) = (state.x, state.y);
    let title = wide(&state.cfg.title);
    let class = wide(CLASS_NAME);
    let mut ex_style = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
    if !opaque {
        ex_style |= WS_EX_LAYERED;
    }
    unsafe {
        let hwnd = CreateWindowExW(
            ex_style,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            x,
            y,
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
        if !opaque {
            SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
        }
        let corner = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &corner as *const _ as *const c_void,
            std::mem::size_of_val(&corner) as u32,
        );
        let st = &mut *state_ptr(hwnd);
        st.dpi = GetDpiForWindow(hwnd).max(96);
        st.load_resources();
        apply_size(hwnd, st);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        hwnd
    }
}

pub fn state_ptr(hwnd: HWND) -> *mut PanelState {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PanelState }
}

fn apply_size(hwnd: HWND, st: &PanelState) {
    let (w, h) = st.size();
    unsafe {
        SetWindowPos(hwnd, HWND_BOTTOM, st.x, st.y, w, h, SWP_NOACTIVATE);
        InvalidateRect(hwnd, null(), 0);
    }
}

fn set_expanded(hwnd: HWND, st: &mut PanelState, expanded: bool) {
    if st.expanded != expanded {
        st.expanded = expanded;
        st.hover = None;
        apply_size(hwnd, st);
        if expanded {
            push_other_panels_down(hwnd);
        }
    }
}

/// Sends every other panel to the bottom so an open panel overlaps its neighbours.
fn push_other_panels_down(keep: HWND) {
    let class = wide(CLASS_NAME);
    let mut other = null_mut();
    loop {
        other = unsafe { FindWindowExW(null_mut(), other, class.as_ptr(), null()) };
        if other.is_null() {
            break;
        }
        if other != keep {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            unsafe { SetWindowPos(other, HWND_BOTTOM, 0, 0, 0, 0, flags) };
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
        let locked = LOCKED.load(Ordering::Relaxed);
        let mouse = || ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32);

        match msg {
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            WM_WINDOWPOSCHANGING => {
                // Stay at the bottom of the z-order, and pinned while locked.
                let pos = &mut *(lparam as *mut WINDOWPOS);
                pos.hwndInsertAfter = HWND_BOTTOM;
                pos.flags &= !SWP_NOZORDER;
                if locked {
                    pos.x = st.x;
                    pos.y = st.y;
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
            WM_EXITSIZEMOVE => {
                let mut r: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut r);
                st.x = r.left;
                st.y = r.top;
                return 0;
            }
            WM_SETCURSOR => {
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                ScreenToClient(hwnd, &mut pt);
                let cursor = match st.hit(pt.x, pt.y) {
                    Hit::Title if !locked => IDC_SIZEALL,
                    Hit::Title | Hit::Item(_) => IDC_HAND,
                    Hit::Nothing => IDC_ARROW,
                };
                SetCursor(LoadCursorW(null_mut(), cursor));
                return 1;
            }
            WM_MOUSEMOVE => {
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
                KillTimer(hwnd, COLLAPSE_TIMER);
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
                if st.settings.collapse_on_leave && st.expanded {
                    SetTimer(hwnd, COLLAPSE_TIMER, COLLAPSE_DELAY_MS, None);
                }
                return 0;
            }
            WM_TIMER if wparam == COLLAPSE_TIMER => {
                KillTimer(hwnd, COLLAPSE_TIMER);
                set_expanded(hwnd, st, false);
                return 0;
            }
            WM_LBUTTONUP => {
                let (x, y) = mouse();
                match st.hit(x, y) {
                    Hit::Title => set_expanded(hwnd, st, !st.expanded),
                    Hit::Item(i) => {
                        let item = st.cfg.items[i].clone();
                        launch(&item.path, &item.args);
                        if st.settings.collapse_after_launch {
                            set_expanded(hwnd, st, false);
                        }
                    }
                    Hit::Nothing => {}
                }
                return 0;
            }
            WM_RBUTTONUP | WM_NCRBUTTONUP => {
                PostMessageW(st.menu_target, WM_APP_SHOW_MENU, 0, 0);
                return 0;
            }
            WM_DPICHANGED => {
                st.dpi = ((wparam >> 16) & 0xFFFF) as u32;
                st.load_resources();
                if !locked {
                    let suggested = &*(lparam as *const RECT);
                    st.x = suggested.left;
                    st.y = suggested.top;
                }
                apply_size(hwnd, st);
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

unsafe fn fill(hdc: HDC, r: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(hdc, r, brush);
        DeleteObject(brush);
    }
}

unsafe fn draw_text(hdc: HDC, text: &str, r: &RECT, flags: u32) {
    let w = wide(text);
    let mut r = *r;
    unsafe { DrawTextW(hdc, w.as_ptr(), -1, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS) };
}

fn paint(hwnd: HWND, st: &PanelState, locked: bool) {
    let s = &st.settings;
    let bg = parse_color(&s.background, 0x1F1F1F);
    let title_bg = parse_color(&s.title_background, 0x2B2B2B);
    let text = parse_color(&s.text, 0xF2F2F2);
    let hover = parse_color(&s.hover, 0x3D3D3D);
    let accent = parse_color(&s.accent, 0xFFC24C);
    let l = st.layout();

    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right, client.bottom);

        // Draw off-screen first to avoid flicker.
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mem, bmp);
        SetBkMode(mem, TRANSPARENT as _);
        SetTextColor(mem, text);

        fill(mem, &client, bg);
        let title = RECT { left: 0, top: 0, right: w, bottom: l.title_h };
        fill(mem, &title, title_bg);

        // Fold arrow: pointing right when folded, down when open.
        let cx = l.pad + st.scale(6);
        let cy = l.title_h / 2;
        let a = st.scale(4);
        let points = if st.expanded {
            [POINT { x: cx - a, y: cy - a / 2 }, POINT { x: cx + a, y: cy - a / 2 }, POINT { x: cx, y: cy + a }]
        } else {
            [POINT { x: cx - a / 2, y: cy - a }, POINT { x: cx - a / 2, y: cy + a }, POINT { x: cx + a, y: cy }]
        };
        let arrow_brush = CreateSolidBrush(text);
        let old_brush = SelectObject(mem, arrow_brush);
        let old_pen = SelectObject(mem, GetStockObject(NULL_PEN));
        Polygon(mem, points.as_ptr(), 3);
        SelectObject(mem, old_brush);
        DeleteObject(arrow_brush);

        let old_font = SelectObject(mem, st.title_font);
        let title_text = RECT { left: cx + a * 3, top: 0, right: w - l.pad, bottom: l.title_h };
        draw_text(mem, &st.cfg.title, &title_text, DT_LEFT | DT_VCENTER);

        if st.expanded {
            SelectObject(mem, st.font);
            let hover_brush = CreateSolidBrush(hover);
            let radius = st.scale(8);
            for (i, item) in st.cfg.items.iter().enumerate() {
                let r = st.item_rect(&l, i);
                if st.hover == Some(i) {
                    SelectObject(mem, hover_brush);
                    RoundRect(mem, r.left + 1, r.top + 1, r.right - 1, r.bottom - 1, radius, radius);
                }
                let icon_top = r.top + st.scale(8);
                let icon = st.icons.get(i).copied().unwrap_or(null_mut());
                if !icon.is_null() {
                    let icon_left = r.left + (l.cell_w - l.icon) / 2;
                    DrawIconEx(mem, icon_left, icon_top, icon, l.icon, l.icon, 0, null_mut(), DI_NORMAL);
                }
                let label_top = icon_top + l.icon + st.scale(4);
                let label = RECT { left: r.left + st.scale(2), top: label_top, right: r.right - st.scale(2), bottom: r.bottom - st.scale(2) };
                draw_text(mem, &item.name, &label, DT_CENTER | DT_TOP);
            }
            SelectObject(mem, old_brush);
            DeleteObject(hover_brush);
        }

        if !locked {
            // Accent border shows the panels can be dragged.
            let pen = CreatePen(PS_SOLID, st.scale(2), accent);
            SelectObject(mem, pen);
            SelectObject(mem, GetStockObject(NULL_BRUSH));
            Rectangle(mem, 1, 1, w, h);
            SelectObject(mem, old_brush);
            SelectObject(mem, old_pen);
            DeleteObject(pen);
        }

        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_pen);
        SelectObject(mem, old_font);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}
