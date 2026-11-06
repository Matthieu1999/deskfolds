//! The welcome window, shown once, the first time deskfolds starts (there was
//! no config yet). deskfolds has no installer, so this is where it offers what
//! it can add to Windows: the Start menu shortcut (checked), Start with
//! Windows and the desktop menu entry (not checked). Nothing is added unless
//! the user clicks Start; the same choices are in Settings > Windows integration.

use std::cell::Cell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::{SS_CENTERIMAGE, SS_NOPREFIX};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::{error_box, wide};

const CLASS: &str = "deskfolds.welcome";
const BG: u32 = 0x00202020;
const TEXT: u32 = 0x00F2F2F2;
const DIM: u32 = 0x00CFCFCF;
const W: i32 = 500;
const H: i32 = 268;

const START_MENU: i32 = 10;
const AUTOSTART: i32 = 11;
const DESKTOP_MENU: i32 = 12;
const HELP: i32 = 20;
const START: i32 = 21;
/// The clickable caption of a check box: its id + this.
const CAPTION: i32 = 100;
const DIM_TEXT: i32 = 30;

thread_local! {
    static WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
    static BRUSH: Cell<HBRUSH> = const { Cell::new(null_mut()) };
    static FONTS: Cell<(HFONT, HFONT)> = const { Cell::new((null_mut(), null_mut())) };
    static ICONS: Cell<(HICON, HICON)> = const { Cell::new((null_mut(), null_mut())) };
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v as f32 * dpi as f32 / 96.0).round() as i32
}

/// Shows the welcome window.
pub fn show() {
    if !WINDOW.get().is_null() {
        return;
    }
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
    let title = wide("Welcome to deskfolds");
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    unsafe {
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style, 0, 0, 100, 100, null_mut(), null_mut(), GetModuleHandleW(null()), null());
        if hwnd.is_null() {
            return;
        }
        WINDOW.set(hwnd);
        crate::ui::dark_window(hwnd);
        // The folder icon of the tray icon and of the settings window.
        let (mut big, mut small) = (null_mut(), null_mut());
        let shell32 = wide("shell32.dll");
        windows_sys::Win32::UI::Shell::ExtractIconExW(shell32.as_ptr(), 3, &mut big, &mut small, 1);
        ICONS.set((big, small));
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, big as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, small as isize);
        let dpi = GetDpiForWindow(hwnd).max(96);
        let s = |v: i32| scale(v, dpi);
        let face = wide("Segoe UI");
        let font = |h: i32, weight: i32| {
            CreateFontW(-s(h), 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as _, OUT_DEFAULT_PRECIS as _, CLIP_DEFAULT_PRECIS as _, CLEARTYPE_QUALITY as _, DEFAULT_PITCH as _, face.as_ptr())
        };
        let (normal, strong) = (font(12, FW_NORMAL as i32), font(16, FW_SEMIBOLD as i32));
        FONTS.set((normal, strong));
        BRUSH.set(CreateSolidBrush(BG));
        let mut r = RECT { left: 0, top: 0, right: s(W), bottom: s(H) };
        AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi);
        // In the middle of the main screen's work area.
        let area = crate::panel::primary_work_area();
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        SetWindowPos(hwnd, null_mut(), area.x + (area.w - w) / 2, area.y + (area.h - h) / 2, w, h, SWP_NOZORDER);

        let add = |class: &str, text: &str, style: u32, (x, y, cw, ch): (i32, i32, i32, i32), id: i32, f: HFONT| {
            let (c, t) = (wide(class), wide(text));
            let child = CreateWindowExW(0, c.as_ptr(), t.as_ptr(), WS_CHILD | WS_VISIBLE | style, s(x), s(y), s(cw), s(ch), hwnd, id as usize as HMENU, GetModuleHandleW(null()), null());
            SendMessageW(child, WM_SETFONT, f as usize, 0);
            child
        };
        add("STATIC", "Welcome to deskfolds", SS_NOPREFIX, (20, 16, 460, 26), -1, strong);
        add(
            "STATIC",
            "deskfolds needs no installation. Choose what it may add to Windows, for you only. You can change it later in Settings > Windows integration.",
            SS_NOPREFIX,
            (20, 48, 460, 36),
            DIM_TEXT,
            normal,
        );
        let checks = [
            (START_MENU, "Show deskfolds in the Start menu and in Windows search", true),
            (AUTOSTART, "Start deskfolds with Windows", false),
            (DESKTOP_MENU, "Add \"New deskfolds folder\" to the desktop's right-click menu", false),
        ];
        for (k, (id, text, on)) in checks.iter().enumerate() {
            let y = 96 + k as i32 * 30;
            let check = add("BUTTON", "", WS_TABSTOP | BS_AUTOCHECKBOX as u32, (20, y, 22, 24), *id, normal);
            crate::ui::dark_control(check, false);
            SendMessageW(check, BM_SETCHECK, *on as usize, 0);
            add("STATIC", text, SS_NOPREFIX | SS_CENTERIMAGE | 0x0100, (42, y, 430, 24), id + CAPTION, normal);
        }
        let help = add("BUTTON", "See what deskfolds can do", WS_TABSTOP | BS_PUSHBUTTON as u32, (20, 218, 200, 30), HELP, normal);
        crate::ui::dark_control(help, false);
        let start = add("BUTTON", "Start", WS_TABSTOP | BS_DEFPUSHBUTTON as u32, (380, 218, 100, 30), START, normal);
        crate::ui::dark_control(start, false);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}

/// Keyboard navigation (Tab, Enter, Esc) for the welcome window.
pub fn translate(msg: &MSG) -> bool {
    let hwnd = WINDOW.get();
    !hwnd.is_null() && unsafe { IsDialogMessageW(hwnd, msg) } != 0
}

fn checked(hwnd: HWND, id: i32) -> bool {
    unsafe { SendMessageW(GetDlgItem(hwnd, id), BM_GETCHECK, 0, 0) == 1 }
}

/// Start: adds what is checked, and closes.
fn apply(hwnd: HWND) {
    let mut errors = Vec::new();
    if checked(hwnd, START_MENU)
        && let Err(e) = crate::integration::set_start_menu(true)
    {
        errors.push(e);
    }
    if checked(hwnd, AUTOSTART) {
        crate::util::set_autostart(true);
    }
    if checked(hwnd, DESKTOP_MENU)
        && let Err(e) = crate::integration::set_desktop_menu(true)
    {
        errors.push(e);
    }
    unsafe { DestroyWindow(hwnd) };
    crate::settings::on_store_changed();
    if !errors.is_empty() {
        error_box(&errors.join("\n"));
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => {
                let mut r: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut r);
                FillRect(wparam as HDC, &r, BRUSH.get());
                return 1;
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let dc = wparam as HDC;
                let dim = GetDlgCtrlID(lparam as HWND) == DIM_TEXT;
                SetTextColor(dc, if dim { DIM } else { TEXT });
                SetBkColor(dc, BG);
                return BRUSH.get() as LRESULT;
            }
            WM_COMMAND => {
                let id = (wparam & 0xFFFF) as i32;
                match id {
                    START | 1 => apply(hwnd),
                    // Esc or the close button: nothing is added.
                    2 => {
                        DestroyWindow(hwnd);
                    }
                    HELP => crate::settings::open_help(),
                    id if (START_MENU + CAPTION..=DESKTOP_MENU + CAPTION).contains(&id) => {
                        let check = GetDlgItem(hwnd, id - CAPTION);
                        let on = SendMessageW(check, BM_GETCHECK, 0, 0) == 1;
                        SendMessageW(check, BM_SETCHECK, (!on) as usize, 0);
                    }
                    _ => {}
                }
                return 0;
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                WINDOW.set(null_mut());
                let (normal, strong) = FONTS.replace((null_mut(), null_mut()));
                for f in [normal, strong] {
                    if !f.is_null() {
                        DeleteObject(f);
                    }
                }
                let brush = BRUSH.replace(null_mut());
                if !brush.is_null() {
                    DeleteObject(brush);
                }
                let (big, small) = ICONS.replace((null_mut(), null_mut()));
                for icon in [big, small] {
                    if !icon.is_null() {
                        DestroyIcon(icon);
                    }
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}
