//! The Help page of the settings window: what deskfolds can do, and the keys
//! that change what a drag does. Text only, built into deskfolds.exe, and
//! drawn here as two columns with keys as small key caps. The page scrolls
//! with the mouse wheel and its scrollbar.
//!
//! In the texts, a key is written in brackets: "[Alt]".

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::SetScrollInfo;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::util::wide;

pub const CLASS: &str = "deskfolds.help";
pub const README_URL: &str = "https://github.com/Matthieu1999/deskfolds#use";

pub struct Topic {
    /// Its button at the top of the page.
    pub button: &'static str,
    pub title: &'static str,
    /// (what, how).
    pub rows: &'static [(&'static str, &'static str)],
}

pub const TOPICS: &[Topic] = &[
    Topic {
        button: "Keys and mouse",
        title: "Keys and mouse",
        rows: &[
            ("[Alt] while you drag a folder", "Place it freely: it does not snap to the other folders or to the screen edges."),
            ("[Alt] while you drag a folder over the frame", "Drop it over the arrangement's frame and keep it out of the arrangement."),
            ("[Alt] while you drag the frame", "Place the arrangement freely, not snapped to the corners, edges or center of the screen."),
            ("[Esc] while you drag", "Put the folder, or the frame, back where it was."),
            ("[Ctrl] while you drop files on a folder", "Add only a link: the files stay where they are."),
            ("[Ctrl] while you drag a shortcut out", "Copy it, and keep it in the folder."),
            ("[Shift] + right-click on the desktop", "On Windows 11, the full menu at once, with \"New deskfolds folder\" if you added it."),
            ("[Enter] / [Esc] while you type a name", "Keep the new name, or the old one."),
            ("Double-click the tray icon", "Open Settings."),
            ("[↑] [↓] in a number box", "One step at a time; with [Shift], ten steps."),
        ],
    },
    Topic {
        button: "On the desktop",
        title: "Folders on the desktop",
        rows: &[
            ("Open a folder", "Hover it; it closes when the mouse leaves. With \"Open folders on hover\" off (Behavior), click its name."),
            ("Keep it open", "Click its name: a blue dot shows it is pinned, and it stays pinned after a restart. Click it again to unpin it."),
            ("Launch a shortcut", "Click it, or double-click it if you chose so in Behavior."),
            ("See a full name", "Hover a shortcut for a moment: its full name and what it opens."),
            ("The menu", "Right-click a folder (not one of its shortcuts) or the tray icon, or click the tray icon once."),
        ],
    },
    Topic {
        button: "Folders",
        title: "Making and changing folders",
        rows: &[
            (
                "Make one",
                "Right-click the desktop > New deskfolds folder (once added in Windows integration), or tray menu > New folder, then type its name. Or Settings > Folders > New folder.",
            ),
            ("Rename it", "Right-click it > Rename, and type the new name. Or Settings > Folders."),
            ("Delete it", "Right-click it > Delete. The files you moved into it go back to the desktop."),
            ("Unlock one folder", "Right-click it > Unlock. It gets a blue border and stays open; right-click it > Lock when you are done."),
            ("Unlock them all", "Tray menu > Unlock all folders, then Lock all folders when you are done."),
            ("Move or resize it", "While it is unlocked, drag it by its name, or drag its right edge."),
        ],
    },
    Topic {
        button: "Shortcuts",
        title: "Shortcuts",
        rows: &[
            ("Add some", "Drag files, folders or shortcuts onto a folder. They are moved into its storage folder; with [Ctrl], only a link is added."),
            ("Make a new file in a folder", "Right-click the folder > New: a folder, a shortcut, a document… Then type its name."),
            (
                "New desktop shortcuts",
                "They go into a folder by themselves if you choose one in Windows integration. For a shortcut for all users, click the notification, then answer Windows.",
            ),
            ("Reorder them", "While the folder is unlocked, drag them within it, or onto another folder."),
            ("Take one out", "Right-click it > Remove from folder: back on the desktop. Or, while the folder is unlocked, drag it onto the desktop."),
            ("Its menu", "Right-click it: Windows' menu for its file, then Remove from folder and Folder settings. With [Shift], Windows' extended entries."),
            (
                "Delete and Rename",
                "For a file moved into the folder: Delete sends it to the Recycle Bin, Rename renames it. For a link: Delete only takes it out of the folder and Rename only changes its name here; neither touches the original. Windows' other entries act on the original, as on the desktop.",
            ),
        ],
    },
    Topic {
        button: "Arranging",
        title: "Arranging",
        rows: &[
            ("Snapping", "A folder you drag snaps to the others and to the screen edges, keeping the gap; the folder it snaps to lights up."),
            ("Room for a bigger folder", "Snapped folders move aside when one gets bigger, and go to a new line rather than off the screen (Layout)."),
            (
                "An arrangement",
                "Settings > Folders > Arrange all… (row, column or grid). While all folders are unlocked, drag its frame to move it, a folder inside it to change its place, or out of it to take it out.",
            ),
            ("When a folder opens", "The folders under or above it move aside, or it covers them (Layout)."),
        ],
    },
    Topic {
        button: "This window",
        title: "This window",
        rows: &[
            ("Saving", "Every change is saved at once. There is no Save button."),
            (
                "Keys",
                "[Tab] moves between fields, [Enter] saves the one you are typing in, [Esc] puts back its previous value, or closes the window when you are not typing.",
            ),
            ("Sliders", "A click on the track puts the slider there."),
            ("The selected folder", "It stays open and outlined on the desktop, so you can see what you change."),
        ],
    },
    Topic {
        button: "Good to know",
        title: "Good to know",
        rows: &[
            ("Your files", "deskfolds never deletes a file. A name already taken becomes \"name (2)\"."),
            ("A mistake?", "About & advanced > Previous versions > Restore…"),
            ("Find deskfolds", "In the Start menu and Windows search, if you chose so in Windows integration."),
            ("Quit", "Tray menu > Quit."),
        ],
    },
];

const BG: u32 = 0x00202020;
const STRIPE: u32 = 0x00262626;
const TEXT: u32 = 0x00F2F2F2;
const DIM: u32 = 0x00CFCFCF;
const KEY_FILL: u32 = 0x00333333;
const KEY_EDGE: u32 = 0x005A5A5A;
const LINK: u32 = 0x00FFC24C;

/// A piece of text: a word (with the space after it) or a key.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Word(&'a str),
    Key(&'a str),
}

/// Splits a text into words and keys.
fn pieces(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(inner) = rest.strip_prefix('[')
            && let Some(end) = inner.find(']')
        {
            out.push(Piece::Key(&inner[..end]));
            rest = &inner[end + 1..];
            continue;
        }
        // A word runs to the next space (kept with it) or key.
        let first = rest.chars().next().map_or(1, char::len_utf8);
        let stop = rest[first..].find(['[', ' ']).map_or(rest.len(), |i| i + first);
        let end = if rest[stop..].starts_with(' ') { stop + 1 } else { stop };
        out.push(Piece::Word(&rest[..end]));
        rest = &rest[end..];
    }
    out
}

struct State {
    font: HFONT,
    strong: HFONT,
    dpi: u32,
    scroll: i32,
    /// The whole page's height, and where each topic starts.
    height: i32,
    topic_tops: Vec<i32>,
    link: RECT,
}

impl State {
    fn s(&self, v: i32) -> i32 {
        (v as f32 * self.dpi as f32 / 96.0).round() as i32
    }
}

fn state<'a>(hwnd: HWND) -> Option<&'a mut State> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut State;
    (!ptr.is_null()).then(|| unsafe { &mut *ptr })
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

/// Gives the page its fonts (the settings window's) and DPI.
pub fn set_fonts(hwnd: HWND, font: HFONT, strong: HFONT, dpi: u32) {
    if let Some(st) = state(hwnd) {
        st.font = font;
        st.strong = strong;
        st.dpi = dpi;
        layout(hwnd, st, false);
    }
}

/// Scrolls to topic `i`.
pub fn show_topic(hwnd: HWND, i: usize) {
    if let Some(st) = state(hwnd) {
        let top = st.topic_tops.get(i).copied().unwrap_or(0);
        scroll_to(hwnd, st, top);
        unsafe { SetFocus(hwnd) };
    }
}

fn text_size(dc: HDC, text: &str) -> SIZE {
    let w = wide(text);
    let mut size = SIZE { cx: 0, cy: 0 };
    unsafe { GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32 - 1, &mut size) };
    size
}

/// Lays out a text in `width`, from (x, y), and draws it when `draw`. Returns its height.
#[allow(clippy::too_many_arguments)]
fn flow(dc: HDC, st: &State, text: &str, x: i32, y: i32, width: i32, color: u32, draw: bool) -> i32 {
    let line_h = text_size(dc, "Ag").cy + st.s(4);
    let (pad, gap) = (st.s(5), st.s(3));
    let mut cx = x;
    let mut cy = y;
    for piece in pieces(text) {
        let (label, key) = match piece {
            Piece::Word(w) => (w, false),
            Piece::Key(k) => (k, true),
        };
        // A key is drawn in its cap; a word's trailing space takes no room at the end of a line.
        let w = if key { text_size(dc, label).cx + 2 * pad } else { text_size(dc, label).cx };
        let room = if key { w } else { text_size(dc, label.trim_end()).cx };
        if cx > x && cx + room > x + width {
            cx = x;
            cy += line_h;
        }
        if draw {
            let t = wide(label);
            unsafe {
                if key {
                    let r = RECT { left: cx, top: cy + st.s(1), right: cx + w, bottom: cy + line_h - st.s(2) };
                    let brush = CreateSolidBrush(KEY_FILL);
                    let pen = CreatePen(PS_SOLID, 1, KEY_EDGE);
                    let (ob, op) = (SelectObject(dc, brush), SelectObject(dc, pen));
                    RoundRect(dc, r.left, r.top, r.right, r.bottom, st.s(6), st.s(6));
                    SelectObject(dc, ob);
                    SelectObject(dc, op);
                    DeleteObject(brush);
                    DeleteObject(pen);
                    SetTextColor(dc, TEXT);
                    TextOutW(dc, cx + pad, cy + st.s(2), t.as_ptr(), t.len() as i32 - 1);
                } else {
                    SetTextColor(dc, color);
                    TextOutW(dc, cx, cy + st.s(2), t.as_ptr(), t.len() as i32 - 1);
                }
            }
        }
        cx += if key { w + gap } else { w };
    }
    cy + line_h - y
}

/// Measures the page (and draws it on `dc` when given), from the top of the page.
fn render(hwnd: HWND, st: &mut State, target: Option<HDC>) {
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let width = client.right - st.s(8);
    let what_w = st.s(250);
    let how_x = what_w + st.s(16);
    let how_w = (width - how_x).max(st.s(120));
    let dc = target.unwrap_or_else(|| unsafe { GetDC(hwnd) });
    let draw = target.is_some();
    let top = -st.scroll;
    let mut y = 0;
    let mut tops = Vec::new();
    unsafe {
        let old_font = SelectObject(dc, st.font);
        SetBkMode(dc, TRANSPARENT as _);
        for topic in TOPICS {
            tops.push(y);
            SelectObject(dc, st.strong);
            if draw {
                SetTextColor(dc, TEXT);
                let t = wide(topic.title);
                TextOutW(dc, st.s(4), top + y, t.as_ptr(), t.len() as i32 - 1);
            }
            y += text_size(dc, topic.title).cy + st.s(8);
            SelectObject(dc, st.font);
            for (k, (what, how)) in topic.rows.iter().enumerate() {
                let h_what = flow(dc, st, what, st.s(8), 0, what_w - st.s(8), TEXT, false);
                let h_how = flow(dc, st, how, how_x, 0, how_w, DIM, false);
                let h = h_what.max(h_how) + st.s(8);
                if draw {
                    if k % 2 == 0 {
                        let r = RECT { left: 0, top: top + y, right: width, bottom: top + y + h };
                        let brush = CreateSolidBrush(STRIPE);
                        FillRect(dc, &r, brush);
                        DeleteObject(brush);
                    }
                    flow(dc, st, what, st.s(8), top + y + st.s(4), what_w - st.s(8), TEXT, true);
                    flow(dc, st, how, how_x, top + y + st.s(4), how_w, DIM, true);
                }
                y += h;
            }
            y += st.s(16);
        }
        // The README link.
        let text = "More in the README on GitHub";
        let size = text_size(dc, text);
        st.link = RECT { left: st.s(4), top: y, right: st.s(4) + size.cx, bottom: y + size.cy };
        if draw {
            SetTextColor(dc, LINK);
            let t = wide(text);
            TextOutW(dc, st.s(4), top + y, t.as_ptr(), t.len() as i32 - 1);
        }
        y += size.cy + st.s(12);
        SelectObject(dc, old_font);
        if target.is_none() {
            ReleaseDC(hwnd, dc);
        }
    }
    st.height = y;
    st.topic_tops = tops;
}

/// Measures again and sets the scrollbar.
fn layout(hwnd: HWND, st: &mut State, keep: bool) {
    render(hwnd, st, None);
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    if !keep {
        st.scroll = 0;
    }
    let page = client.bottom.max(1);
    st.scroll = st.scroll.clamp(0, (st.height - page).max(0));
    let info = SCROLLINFO {
        cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
        fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
        nMin: 0,
        nMax: st.height.max(0),
        nPage: page as u32,
        nPos: st.scroll,
        nTrackPos: 0,
    };
    unsafe {
        SetScrollInfo(hwnd, SB_VERT, &info, 1);
        InvalidateRect(hwnd, null(), 0);
    }
}

fn scroll_to(hwnd: HWND, st: &mut State, pos: i32) {
    let mut client: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut client) };
    let pos = pos.clamp(0, (st.height - client.bottom).max(0));
    if pos != st.scroll {
        st.scroll = pos;
        let info = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_POS, nPos: pos, ..unsafe { std::mem::zeroed() } };
        unsafe {
            SetScrollInfo(hwnd, SB_VERT, &info, 1);
            InvalidateRect(hwnd, null(), 0);
        }
    }
}

fn paint(hwnd: HWND, st: &mut State) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        // Drawn off screen first, so scrolling never flickers.
        let mem = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, client.right.max(1), client.bottom.max(1));
        if mem.is_null() || bmp.is_null() {
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
        let brush = CreateSolidBrush(BG);
        FillRect(mem, &client, brush);
        DeleteObject(brush);
        let old_font = SelectObject(mem, st.font);
        render(hwnd, st, Some(mem));
        SelectObject(mem, old_font);
        BitBlt(hdc, 0, 0, client.right, client.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

fn over_link(st: &State, x: i32, y: i32) -> bool {
    let y = y + st.scroll;
    x >= st.link.left && x < st.link.right && y >= st.link.top && y < st.link.bottom
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let st = Box::new(State { font: null_mut(), strong: null_mut(), dpi: 96, scroll: 0, height: 0, topic_tops: Vec::new(), link: std::mem::zeroed() });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(st) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
        let line = st.s(40);
        match msg {
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                if st.font.is_null() {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                paint(hwnd, st);
                return 0;
            }
            WM_SIZE => {
                if !st.font.is_null() {
                    layout(hwnd, st, true);
                }
                return 0;
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam >> 16) & 0xFFFF) as i16 as i32;
                let pos = st.scroll - delta * line * 3 / 120 / 2;
                scroll_to(hwnd, st, pos);
                return 0;
            }
            WM_VSCROLL => {
                let mut client: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut client);
                let page = client.bottom;
                let pos = match (wparam & 0xFFFF) as i32 {
                    SB_LINEUP => st.scroll - line,
                    SB_LINEDOWN => st.scroll + line,
                    SB_PAGEUP => st.scroll - page,
                    SB_PAGEDOWN => st.scroll + page,
                    SB_TOP => 0,
                    SB_BOTTOM => st.height,
                    SB_THUMBTRACK | SB_THUMBPOSITION => {
                        let mut info = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_TRACKPOS, ..std::mem::zeroed() };
                        GetScrollInfo(hwnd, SB_VERT, &mut info);
                        info.nTrackPos
                    }
                    _ => st.scroll,
                };
                scroll_to(hwnd, st, pos);
                return 0;
            }
            WM_KEYDOWN => {
                let pos = match wparam as u16 {
                    0x26 => st.scroll - line,         // VK_UP
                    0x28 => st.scroll + line,         // VK_DOWN
                    0x21 => st.scroll - line * 8,     // VK_PRIOR
                    0x22 => st.scroll + line * 8,     // VK_NEXT
                    0x24 => 0,                        // VK_HOME
                    0x23 => st.height,                // VK_END
                    _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
                };
                scroll_to(hwnd, st, pos);
                return 0;
            }
            WM_GETDLGCODE => return DLGC_WANTARROWS as LRESULT,
            WM_SETCURSOR => {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                ScreenToClient(hwnd, &mut pt);
                SetCursor(LoadCursorW(null_mut(), if over_link(st, pt.x, pt.y) { IDC_HAND } else { IDC_ARROW }));
                return 1;
            }
            WM_LBUTTONDOWN => {
                SetFocus(hwnd);
                return 0;
            }
            WM_LBUTTONUP => {
                let (x, y) = ((lparam & 0xFFFF) as i16 as i32, ((lparam >> 16) & 0xFFFF) as i16 as i32);
                if over_link(st, x, y) {
                    crate::util::launch(README_URL, "");
                }
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_told_from_words() {
        assert_eq!(
            pieces("One step; with [Shift], ten"),
            [Piece::Word("One "), Piece::Word("step; "), Piece::Word("with "), Piece::Key("Shift"), Piece::Word(", "), Piece::Word("ten")]
        );
        assert_eq!(pieces("[↑] [↓] in"), [Piece::Key("↑"), Piece::Word(" "), Piece::Key("↓"), Piece::Word(" in")]);
        // A lone bracket is text.
        assert_eq!(pieces("a [b"), [Piece::Word("a "), Piece::Word("[b")]);
    }

    #[test]
    fn every_topic_has_a_button_and_rows() {
        assert_eq!(TOPICS.len(), 7);
        assert!(TOPICS.iter().all(|t| !t.button.is_empty() && !t.rows.is_empty()));
    }
}
