//! The settings window's sliders: the native trackbar control, kept for its
//! keyboard, wheel and accessibility, but drawn in the Windows 11 style (a thin
//! track filled with the accent up to a round thumb). A click on the track puts
//! the thumb there at once, and the drag goes on from there.

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Controls::{
    CDDS_PREPAINT, CDRF_DODEFAULT, CDRF_SKIPDEFAULT, NMCUSTOMDRAW, TBM_GETCHANNELRECT, TBM_GETRANGEMAX, TBM_GETRANGEMIN,
    TBM_GETTHUMBRECT, TBM_SETPOS, TBM_SETTHUMBLENGTH, TB_THUMBTRACK, WM_MOUSELEAVE,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, GetFocus, IsWindowEnabled, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::wide;

const SUBCLASS_ID: usize = 0x5D51;
const TBM_GETPOS: u32 = WM_USER;
/// Marks a slider, and holds 1 while the mouse is over its thumb.
const PROP: &str = "deskfolds.slider";
/// The track left of the thumb is the accent; right of it, this gray.
const TRACK: u32 = 0x009A9A9A;
const THUMB: u32 = 0x00454545;
const THUMB_EDGE: u32 = 0x005C5C5C;
const DISABLED: u32 = 0x00606060;
const FOCUS: u32 = 0x00F2F2F2;

fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

/// Makes a new trackbar one of these sliders.
pub fn prepare(hwnd: HWND) {
    unsafe {
        let s = scale(hwnd);
        // A thumb as tall as the drawn one, so it is easy to take.
        SendMessageW(hwnd, TBM_SETTHUMBLENGTH, (20.0 * s).round() as usize, 0);
        let prop = wide(PROP);
        SetPropW(hwnd, prop.as_ptr(), 2 as _);
        SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0);
    }
}

fn is_slider(hwnd: HWND) -> bool {
    let prop = wide(PROP);
    unsafe { !GetPropW(hwnd, prop.as_ptr()).is_null() }
}

fn thumb_hot(hwnd: HWND) -> bool {
    let prop = wide(PROP);
    unsafe { GetPropW(hwnd, prop.as_ptr()) as usize == 1 }
}

fn set_thumb_hot(hwnd: HWND, hot: bool) {
    if thumb_hot(hwnd) != hot {
        let prop = wide(PROP);
        unsafe {
            SetPropW(hwnd, prop.as_ptr(), if hot { 1 } else { 2 } as _);
            InvalidateRect(hwnd, std::ptr::null(), 0);
        }
    }
}

fn rect_of(hwnd: HWND, msg: u32) -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { SendMessageW(hwnd, msg, 0, &mut r as *mut RECT as LPARAM) };
    r
}

/// Center of the thumb, and the radius it is drawn with.
fn thumb(hwnd: HWND) -> (f32, f32, f32) {
    let t = rect_of(hwnd, TBM_GETTHUMBRECT);
    ((t.left + t.right) as f32 / 2.0, (t.top + t.bottom) as f32 / 2.0, 10.0 * scale(hwnd))
}

/// Handles NM_CUSTOMDRAW from a slider: draws all of it. None for other controls.
pub fn custom_draw(lparam: LPARAM, bg: u32, accent: u32) -> Option<LRESULT> {
    let nm = unsafe { &*(lparam as *const NMCUSTOMDRAW) };
    let hwnd = nm.hdr.hwndFrom;
    if !is_slider(hwnd) {
        return None;
    }
    if nm.dwDrawStage != CDDS_PREPAINT {
        return Some(CDRF_DODEFAULT as LRESULT);
    }
    unsafe {
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        GetClientRect(hwnd, &mut client);
        let ch = rect_of(hwnd, TBM_GETCHANNELRECT);
        let (cx, cy, r) = thumb(hwnd);
        let s = scale(hwnd);
        let enabled = IsWindowEnabled(hwnd) != 0;
        let pressed = enabled && GetCapture() == hwnd;
        let hot = enabled && (pressed || thumb_hot(hwnd));
        let focused = GetFocus() == hwnd && SendMessageW(hwnd, WM_QUERYUISTATE, 0, 0) as u32 & UISF_HIDEFOCUS == 0;
        let picture = Picture {
            w: client.right,
            h: client.bottom,
            bg,
            track: (ch.left as f32 + s, ch.right as f32 - s, cy, 2.0 * s),
            thumb: (cx, cy, r),
            // The dot in the thumb grows under the mouse and shrinks while pressed.
            dot: if pressed { 5.0 * s } else if hot { 7.0 * s } else { 6.0 * s },
            fill: if enabled { accent } else { DISABLED },
            focus: focused,
            scale: s,
        };
        picture.blit(nm.hdc);
    }
    Some(CDRF_SKIPDEFAULT as LRESULT)
}

/// The slider's picture, worked out pixel by pixel so its edges are smooth.
struct Picture {
    w: i32,
    h: i32,
    bg: u32,
    /// Left and right ends of the track, its center line and half its height.
    track: (f32, f32, f32, f32),
    thumb: (f32, f32, f32),
    dot: f32,
    fill: u32,
    focus: bool,
    scale: f32,
}

/// Coverage of a pixel by a shape at signed distance `d` (negative inside).
fn cover(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn blend(under: u32, over: u32, a: f32) -> u32 {
    if a <= 0.0 {
        return under;
    }
    let mix = |shift: u32| {
        let (u, o) = (((under >> shift) & 0xFF) as f32, ((over >> shift) & 0xFF) as f32);
        ((u + (o - u) * a).round() as u32) << shift
    };
    mix(0) | mix(8) | mix(16)
}

/// Signed distance to a capsule (a bar with round ends) from x0 to x1 on line cy.
fn capsule(px: f32, py: f32, x0: f32, x1: f32, cy: f32, half: f32) -> f32 {
    let x = px.clamp(x0 + half, (x1 - half).max(x0 + half));
    ((px - x).powi(2) + (py - cy).powi(2)).sqrt() - half
}

impl Picture {
    fn color_at(&self, px: f32, py: f32) -> u32 {
        let (x0, x1, cy, half) = self.track;
        let (tx, ty, r) = self.thumb;
        let mut c = self.bg;
        c = blend(c, TRACK, cover(capsule(px, py, x0, x1, cy, half)));
        c = blend(c, self.fill, cover(capsule(px, py, x0, tx.max(x0 + 2.0 * half), cy, half)));
        let d = ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
        c = blend(c, THUMB_EDGE, cover(d - r));
        c = blend(c, THUMB, cover(d - (r - self.scale)));
        c = blend(c, self.fill, cover(d - self.dot));
        if self.focus {
            c = blend(c, FOCUS, cover((d - (r + 2.5 * self.scale)).abs() - 0.75 * self.scale));
        }
        c
    }

    unsafe fn blit(&self, hdc: HDC) {
        let (w, h) = (self.w.max(1), self.h.max(1));
        let mut pixels: Vec<u32> = Vec::with_capacity(w as usize * h as usize);
        for y in 0..h {
            for x in 0..w {
                let c = self.color_at(x as f32 + 0.5, y as f32 + 0.5);
                // COLORREF is 0x00BBGGRR; a 32-bit DIB pixel is 0x00RRGGBB.
                pixels.push(((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF));
            }
        }
        let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        unsafe {
            SetDIBitsToDevice(hdc, 0, 0, w as u32, h as u32, 0, 0, 0, h as u32, pixels.as_ptr() as *const _, &info, DIB_RGB_COLORS);
        }
    }
}

/// The slider value for a click at x on the track.
fn value_at(hwnd: HWND, x: i32) -> isize {
    let ch = rect_of(hwnd, TBM_GETCHANNELRECT);
    let t = rect_of(hwnd, TBM_GETTHUMBRECT);
    let half = (t.right - t.left) / 2;
    let (lo, hi) = unsafe { (SendMessageW(hwnd, TBM_GETRANGEMIN, 0, 0), SendMessageW(hwnd, TBM_GETRANGEMAX, 0, 0)) };
    let span = (ch.right - ch.left - 2 * half).max(1) as f32;
    let at = ((x - ch.left - half) as f32 / span).clamp(0.0, 1.0);
    lo + (at * (hi - lo) as f32).round() as isize
}

unsafe extern "system" fn subclass_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_LBUTTONDOWN if IsWindowEnabled(hwnd) != 0 => {
                let (x, y) = ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32);
                let (tx, ty, r) = thumb(hwnd);
                let on_thumb = ((x as f32 - tx).powi(2) + (y as f32 - ty).powi(2)).sqrt() <= r + 2.0;
                if !on_thumb {
                    // A click on the track: the thumb goes there now, and the parent hears of
                    // it as of a drag...
                    let v = value_at(hwnd, x);
                    SendMessageW(hwnd, TBM_SETPOS, 1, v);
                    let v = SendMessageW(hwnd, TBM_GETPOS, 0, 0);
                    let parent = GetParent(hwnd);
                    SendMessageW(parent, WM_HSCROLL, (TB_THUMBTRACK as usize) | ((v as usize & 0xFFFF) << 16), hwnd as LPARAM);
                    // ...which goes on from the thumb: the press is passed on at its center.
                    let (tx, ty, _) = thumb(hwnd);
                    let at = ((tx.round() as i32) & 0xFFFF) as isize | (((ty.round() as i32) & 0xFFFF) as isize) << 16;
                    return DefSubclassProc(hwnd, msg, wparam, at);
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = ((lparam & 0xFFFF) as i16 as f32, ((lparam >> 16) & 0xFFFF) as i16 as f32);
                let (tx, ty, r) = thumb(hwnd);
                set_thumb_hot(hwnd, ((x - tx).powi(2) + (y - ty).powi(2)).sqrt() <= r + 2.0);
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TrackMouseEvent(&mut tme);
            }
            WM_MOUSELEAVE => set_thumb_hot(hwnd, false),
            // The control repaints only where its thumb was and is: the picture is
            // the whole slider (the filled track changes too), so all of it is repainted.
            WM_PAINT => {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED | WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => {
                InvalidateRect(hwnd, std::ptr::null(), 0);
            }
            WM_NCDESTROY => {
                let prop = wide(PROP);
                RemovePropW(hwnd, prop.as_ptr());
                RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }
}
