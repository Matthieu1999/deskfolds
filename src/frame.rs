//! The frame around the arrangement, shown while folders are unlocked: a
//! dashed outline with a tab. Dragging it moves every folder of the
//! arrangement; it snaps to the corners, edges and center of the screen.
//!
//! It is a layered window drawn once per change, kept just under the folders,
//! so a folder above it still gets the mouse. It exists only while unlocked.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_MENU};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::arrange::{self, FRAME_PAD};
use crate::config::parse_color;
use crate::layout::Rect;
use crate::panel::{LOCKED, WM_APP_SHOW_MENU};
use crate::store;
use crate::util::wide;

const CLASS: &str = "deskfolds.frame";
const TAB_H: i32 = 22;

thread_local! {
    static FRAME: Cell<HWND> = const { Cell::new(null_mut()) };
}

struct State {
    /// The arrangement's folders, in screen pixels.
    block: Rect,
    text: String,
    /// Where a dragged folder would go.
    slot: Option<Rect>,
    dpi: u32,
    /// The tab goes under the frame when there is no room above it.
    tab_below: bool,
    moving: bool,
    /// The window rectangle last given while moving: a different one at the
    /// end means the move was cancelled (Esc).
    last_rect: Option<RECT>,
    /// What was drawn last, so an unchanged frame is not drawn again.
    drawn: Option<(Rect, String, Option<Rect>, u32, bool, u32)>,
}

impl State {
    fn s(&self, v: i32) -> i32 {
        (v as f32 * self.dpi as f32 / 96.0).round() as i32
    }

    /// The window: the block, the padding around it, and half the tab outside.
    fn window_rect(&self) -> Rect {
        let (pad, half) = (self.s(FRAME_PAD), self.s(TAB_H) / 2);
        let b = self.block;
        let top = b.y - pad - if self.tab_below { 0 } else { half };
        Rect { x: b.x - pad, y: top, w: b.w + 2 * pad, h: b.h + 2 * pad + half }
    }

    /// The block's top left corner for a window put at (x, y).
    fn block_at(&self, x: i32, y: i32) -> (i32, i32) {
        let (pad, half) = (self.s(FRAME_PAD), self.s(TAB_H) / 2);
        (x + pad, y + pad + if self.tab_below { 0 } else { half })
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
        hCursor: unsafe { LoadCursorW(null_mut(), IDC_SIZEALL) },
        hbrBackground: null_mut(),
        lpszMenuName: null(),
        lpszClassName: class.as_ptr(),
    };
    unsafe { RegisterClassW(&wc) };
}

fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut State;
    (!ptr.is_null()).then(|| unsafe { &mut *ptr })
}

/// Shows the frame around `block` (while unlocked), with a dashed slot where a
/// dragged folder would go.
pub fn show(block: Rect, text: &str, slot: Option<Rect>, dpi: u32) {
    if LOCKED.load(std::sync::atomic::Ordering::Relaxed) {
        hide();
        return;
    }
    let mut hwnd = FRAME.get();
    // Gone with its owner (Explorer restarted): make a new one.
    if !hwnd.is_null() && unsafe { IsWindow(hwnd) } == 0 {
        hwnd = null_mut();
        FRAME.set(null_mut());
    }
    if hwnd.is_null() {
        let desktop = unsafe { FindWindowW(wide("Progman").as_ptr(), null()) };
        let st = Box::new(State { block, text: String::new(), slot: None, dpi, tab_below: false, moving: false, last_rect: None, drawn: None });
        let class = wide(CLASS);
        hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                desktop,
                null_mut(),
                GetModuleHandleW(null()),
                Box::into_raw(st) as *const c_void,
            )
        };
        if hwnd.is_null() {
            return;
        }
        FRAME.set(hwnd);
    }
    let Some(st) = state(hwnd) else { return };
    if st.moving {
        return;
    }
    st.block = block;
    st.text = text.to_string();
    st.slot = slot;
    st.dpi = dpi;
    // No room above for the tab: put it under the frame.
    let area = crate::panel::work_area_at(block.x + block.w / 2, block.y + block.h / 2);
    st.tab_below = block.y - st.s(FRAME_PAD) - st.s(TAB_H) / 2 < area.y;
    let key = (block, st.text.clone(), slot, dpi, st.tab_below, accent());
    if st.drawn.as_ref() != Some(&key) {
        render(hwnd, st, true);
        st.drawn = Some(key);
    }
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
    keep_below();
}

/// Removes the frame (locked, or no arrangement).
pub fn hide() {
    let hwnd = FRAME.replace(null_mut());
    if !hwnd.is_null() && unsafe { IsWindow(hwnd) } != 0 {
        unsafe { DestroyWindow(hwnd) };
    }
}

/// Puts the frame back under the folders after one of them was raised.
pub fn keep_below() {
    let hwnd = FRAME.get();
    if !hwnd.is_null() {
        unsafe { SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER) };
    }
}

fn accent() -> u32 {
    store::with(|s| parse_color(&s.settings.accent, 0xFFC24C))
}

/// Draws the frame into a 32-bit bitmap and hands it to the layered window.
/// With `place`, the window also moves to where the frame goes.
fn render(hwnd: HWND, st: &State, place: bool) {
    let win = st.window_rect();
    let (w, h) = (win.w.max(1), win.h.max(1));
    let accent = accent();
    let (half, tab_h) = (st.s(TAB_H) / 2, st.s(TAB_H));
    // The outline, inside the window.
    let top = if st.tab_below { 0 } else { half };
    let outline = RECT { left: 0, top, right: w, bottom: top + st.block.h + 2 * st.s(FRAME_PAD) };
    unsafe {
        let screen = GetDC(null_mut());
        let mem = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let bmp = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if mem.is_null() || bmp.is_null() || bits.is_null() {
            // Out of memory or GDI handles: no frame this time.
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

        // Dashed outline in the accent color.
        let brush = LOGBRUSH { lbStyle: BS_SOLID, lbColor: accent, lbHatch: 0 };
        let pen = ExtCreatePen((PS_GEOMETRIC | PS_DASH | PS_ENDCAP_FLAT) as u32, st.s(2) as u32, &brush, 0, null());
        let old_pen = SelectObject(mem, pen);
        let old_brush = SelectObject(mem, GetStockObject(NULL_BRUSH));
        let radius = st.s(14);
        let inset = st.s(1);
        RoundRect(mem, outline.left + inset, outline.top + inset, outline.right - inset, outline.bottom - inset, radius, radius);
        // The slot where a dragged folder would go: dashed and white.
        if let Some(slot) = st.slot {
            let white = LOGBRUSH { lbStyle: BS_SOLID, lbColor: 0xF0F0F0, lbHatch: 0 };
            let slot_pen = ExtCreatePen((PS_GEOMETRIC | PS_DASH | PS_ENDCAP_FLAT) as u32, st.s(2) as u32, &white, 0, null());
            SelectObject(mem, slot_pen);
            let (x, y) = (slot.x - win.x, slot.y - win.y);
            RoundRect(mem, x, y, x + slot.w, y + slot.h, st.s(11), st.s(11));
            SelectObject(mem, pen);
            DeleteObject(slot_pen);
        }
        SelectObject(mem, old_brush);
        SelectObject(mem, old_pen);
        DeleteObject(pen);

        // The tab: the accent color, dark text.
        let font = CreateFontW(
            -st.s(12),
            0,
            0,
            0,
            FW_SEMIBOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as _,
            OUT_DEFAULT_PRECIS as _,
            CLIP_DEFAULT_PRECIS as _,
            ANTIALIASED_QUALITY as _,
            DEFAULT_PITCH as _,
            wide("Segoe UI").as_ptr(),
        );
        let old_font = SelectObject(mem, font);
        let text = wide(&st.text);
        let mut size = SIZE { cx: 0, cy: 0 };
        GetTextExtentPoint32W(mem, text.as_ptr(), text.len() as i32 - 1, &mut size);
        let tab_top = if st.tab_below { outline.bottom - half } else { 0 };
        let tab = RECT { left: st.s(12), top: tab_top, right: (st.s(12) + size.cx + st.s(20)).min(w), bottom: tab_top + tab_h };
        let fill = CreateSolidBrush(accent);
        let old = SelectObject(mem, fill);
        let old_pen = SelectObject(mem, GetStockObject(NULL_PEN));
        RoundRect(mem, tab.left, tab.top, tab.right, tab.bottom, tab_h, tab_h);
        SelectObject(mem, old_pen);
        SelectObject(mem, old);
        DeleteObject(fill);
        SetBkMode(mem, TRANSPARENT as _);
        SetTextColor(mem, 0x00332000);
        let mut text_rect = tab;
        DrawTextW(mem, text.as_ptr(), -1, &mut text_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        SelectObject(mem, old_font);
        DeleteObject(font);
        GdiFlush();

        // Alpha: the tab and the lines are solid, the inside a faint tint that
        // still takes the mouse, the corners outside the outline clear.
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, w as usize * h as usize);
        let tint = 0x16u32;
        let (r, g, b) = (accent & 0xFF, (accent >> 8) & 0xFF, (accent >> 16) & 0xFF);
        let faint = (tint << 24) | ((r * tint / 255) << 16) | ((g * tint / 255) << 8) | (b * tint / 255);
        for (y, row) in pixels.chunks_mut(w as usize).enumerate() {
            let y = y as i32;
            for (x, p) in row.iter_mut().enumerate() {
                let x = x as i32;
                let in_tab = x >= tab.left && x < tab.right && y >= tab.top && y < tab.bottom;
                let in_outline = x >= outline.left && x < outline.right && y >= outline.top && y < outline.bottom;
                *p = if in_tab && *p != 0 || (*p & 0xFFFFFF) != 0 {
                    0xFF00_0000 | (*p & 0xFFFFFF)
                } else if in_outline {
                    faint
                } else {
                    0
                };
            }
        }

        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let at = POINT { x: win.x, y: win.y };
        let size = SIZE { cx: w, cy: h };
        let origin = POINT { x: 0, y: 0 };
        UpdateLayeredWindow(hwnd, screen, if place { &at } else { null() }, &size, mem, &origin, 0, &blend, ULW_ALPHA);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        // Messages that need no state first: some arrive while the state is in use
        // (moving the folders puts the frame back under them).
        match msg {
            WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
            // The whole frame is a handle to move it.
            WM_NCHITTEST => return HTCAPTION as LRESULT,
            WM_WINDOWPOSCHANGING => {
                let pos = &mut *(lparam as *mut WINDOWPOS);
                pos.hwndInsertAfter = HWND_BOTTOM;
                pos.flags &= !SWP_NOZORDER;
                return 0;
            }
            WM_NCRBUTTONUP => {
                PostMessageW(crate::main_window(), WM_APP_SHOW_MENU, 0, 0);
                return 0;
            }
            WM_SYSCOMMAND => {
                let cmd = (wparam & 0xFFF0) as u32;
                if cmd == SC_MINIMIZE || cmd == SC_MAXIMIZE || cmd == SC_CLOSE {
                    return 0;
                }
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            WM_NCDESTROY | WM_ENTERSIZEMOVE | WM_MOVING | WM_EXITSIZEMOVE => {}
            // Everything else needs no state (and may arrive while it is in use).
            _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let st = &mut *ptr;
        match msg {
            WM_ENTERSIZEMOVE => {
                st.moving = true;
                st.slot = None;
                st.last_rect = None;
                arrange::begin_frame_drag(st.block);
                return 0;
            }
            WM_MOVING => {
                let r = &mut *(lparam as *mut RECT);
                let (bx, by) = st.block_at(r.left, r.top);
                let free = GetKeyState(VK_MENU as i32) < 0;
                let ((x, y), text) = arrange::frame_moving(Rect { x: bx, y: by, ..st.block }, free);
                let (dx, dy) = (x - bx, y - by);
                r.left += dx;
                r.right += dx;
                r.top += dy;
                r.bottom += dy;
                st.block.x = x;
                st.block.y = y;
                st.last_rect = Some(*r);
                if text != st.text {
                    st.text = text;
                    st.drawn = None;
                    render(hwnd, st, false);
                }
                return 1;
            }
            WM_EXITSIZEMOVE => {
                st.moving = false;
                st.drawn = None;
                let mut now: RECT = std::mem::zeroed();
                GetWindowRect(hwnd, &mut now);
                let moved = st.last_rect.take().is_some_and(|r| (r.left, r.top) == (now.left, now.top));
                let block = st.block;
                // Esc puts the window back, and a click does not move it: save nothing then.
                if moved {
                    arrange::end_frame_drag(block);
                } else {
                    arrange::cancel_drag();
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
