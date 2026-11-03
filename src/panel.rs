//! A panel is a small glass folder glued to the desktop. Closed, it is a
//! compact label (icon + name). Hovering it, or clicking it, unrolls a card of
//! shortcuts. It stays below every other window, never takes focus, and cannot
//! move while panels are locked.

use std::cell::Cell;
use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows_sys::Win32::System::SystemServices::{MK_CONTROL, MK_LBUTTON};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::config::{self, Panel, Settings, item_path, new_item, parse_color};
use crate::dragdrop::{self, display_name, is_stored, move_into, panel_dir, storage_root};
use crate::util::{error_box, expand_env, launch, load_icon, prompt_text, resolve, wide};

const CLASS_NAME: &str = "deskfolds.panel";
const OPEN_TIMER: usize = 1;
const CLOSE_TIMER: usize = 2;
const ANIM_TIMER: usize = 3;
const DRAG_OPEN_TIMER: usize = 4;
const ANIM_FRAME_MS: u32 = 15;
/// Dragging a file over a closed panel opens it after this delay.
const DRAG_OPEN_DELAY_MS: u32 = 300;
const ID_RENAME: usize = 1;
const ID_REMOVE: usize = 2;

thread_local! {
    /// Set while one of our shortcuts is being dragged: (panel index, item index).
    static INTERNAL_DRAG: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
    /// Set when that drag was dropped on one of our panels.
    static INTERNAL_DROP_DONE: Cell<bool> = const { Cell::new(false) };
}
/// Icon shown next to the name when the panel sets none: any folder works.
const DEFAULT_PANEL_ICON: &str = "%SystemRoot%";

/// Shared by all panels: while false, panels can be dragged by their title bar.
pub static LOCKED: AtomicBool = AtomicBool::new(true);

/// Posted to the main window to open the tray menu at the cursor.
pub const WM_APP_SHOW_MENU: u32 = WM_APP + 2;
/// Posted to the main window after the config file changed: rebuild the panels.
pub const WM_APP_RELOAD: u32 = WM_APP + 3;

pub struct PanelState {
    /// Position of this panel in the config file.
    index: usize,
    cfg: Panel,
    settings: Rc<Settings>,
    menu_target: HWND,
    pub x: i32,
    pub y: i32,
    /// Target state: open or closed. `progress` animates towards it.
    open: bool,
    pinned: bool,
    progress: f32,
    anim_from: f32,
    anim_start: u64,
    open_pending: bool,
    /// Opened because a file was dragged over it: close again if the drag leaves.
    drag_opened: bool,
    drag_open_pending: bool,
    /// Where a dragged file would be inserted, shown as a bar.
    drop_insert: Option<usize>,
    /// Mouse pressed on an item while unlocked: (x, y, item) of the press.
    press: Option<(i32, i32, usize)>,
    hover: Option<usize>,
    tracking_mouse: bool,
    dpi: u32,
    font: HFONT,
    title_font: HFONT,
    title_text_w: i32,
    title_icon: HICON,
    icons: Vec<HICON>,
}

enum Hit {
    Title,
    Item(usize),
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

impl PanelState {
    fn scale(&self, v: i32) -> i32 {
        (v as f32 * self.dpi as f32 / 96.0).round() as i32
    }

    fn layout(&self) -> Layout {
        let s = &self.settings;
        let pad = self.scale(6);
        let cols = self.cfg.columns;
        let icon = self.scale(s.icon_size);
        let line = self.scale(s.font_size * 3 / 2);
        Layout {
            title_h: self.scale(s.font_size + 20),
            pad,
            label_icon: self.scale(16),
            cell_w: (self.scale(self.cfg.width) - pad * 2) / cols,
            cell_h: self.scale(8) + icon + self.scale(4) + line + self.scale(4),
            icon,
            // An empty panel keeps one row for its "drop shortcuts here" hint.
            rows: ((self.cfg.items.len() as i32 + cols - 1) / cols).max(1),
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

    /// Size of the closed label: icon + name, like a small tag.
    fn closed_size(&self) -> (i32, i32) {
        let l = self.layout();
        let w = self.scale(12) + l.label_icon + self.scale(8) + self.title_text_w + self.scale(14);
        (w.min(self.scale(self.cfg.width)), l.title_h)
    }

    fn open_size(&self) -> (i32, i32) {
        let l = self.layout();
        (self.scale(self.cfg.width), l.title_h + l.rows * l.cell_h + l.pad)
    }

    fn size(&self) -> (i32, i32) {
        let (cw, ch) = self.closed_size();
        let (ow, oh) = self.open_size();
        let t = ease_out_cubic(self.progress);
        (lerp(cw, ow, t), lerp(ch, oh, t))
    }

    fn item_rect(&self, l: &Layout, i: usize) -> RECT {
        let col = i as i32 % self.cfg.columns;
        let row = i as i32 / self.cfg.columns;
        let left = l.pad + col * l.cell_w;
        let top = l.title_h + row * l.cell_h;
        RECT { left, top, right: left + l.cell_w, bottom: top + l.cell_h }
    }

    fn hit(&self, x: i32, y: i32) -> Hit {
        let l = self.layout();
        if y < l.title_h {
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
        self.title_text_w = unsafe {
            let dc = GetDC(null_mut());
            let old = SelectObject(dc, self.title_font);
            let text = wide(&self.cfg.title);
            let mut size = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32 - 1, &mut size);
            SelectObject(dc, old);
            ReleaseDC(null_mut(), dc);
            size.cx
        };
        let label_icon = self.scale(16);
        self.title_icon = load_icon(self.cfg.icon.as_deref().unwrap_or(DEFAULT_PANEL_ICON), label_icon);
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

pub fn register_class() {
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
/// While panels are unlocked, it starts open so its shortcuts can be edited.
pub fn create(index: usize, cfg: Panel, settings: Rc<Settings>, desktop: HWND, menu_target: HWND) -> HWND {
    let editing = !LOCKED.load(Ordering::Relaxed);
    let state = Box::new(PanelState {
        index,
        x: cfg.x,
        y: cfg.y,
        cfg,
        settings,
        menu_target,
        open: editing,
        pinned: false,
        progress: if editing { 1.0 } else { 0.0 },
        anim_from: 0.0,
        anim_start: 0,
        open_pending: false,
        drag_opened: false,
        drag_open_pending: false,
        drop_insert: None,
        press: None,
        hover: None,
        tracking_mouse: false,
        dpi: 96,
        font: null_mut(),
        title_font: null_mut(),
        title_text_w: 0,
        title_icon: null_mut(),
        icons: Vec::new(),
    });
    let (x, y) = (state.x, state.y);
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
        apply_size(hwnd, st);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        dragdrop::register(hwnd);
        hwnd
    }
}

/// Unlocked panels stay open so their shortcuts can be dragged; locking closes them.
pub fn set_editing(hwnd: HWND, editing: bool) {
    let ptr = state_ptr(hwnd);
    if ptr.is_null() {
        return;
    }
    let st = unsafe { &mut *ptr };
    set_open(hwnd, st, editing);
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

fn request_reload(st: &PanelState) {
    unsafe { PostMessageW(st.menu_target, WM_APP_RELOAD, 0, 0) };
}

/// The effect a drop would have: our own shortcuts move between panels; files
/// from elsewhere move in, unless Ctrl is held (or moving is not offered), which
/// adds a link and leaves the original where it is.
fn choose_effect(keys: u32, allowed: u32) -> u32 {
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

fn client_point(hwnd: HWND, screen: POINT) -> (i32, i32) {
    let mut pt = screen;
    unsafe { ScreenToClient(hwnd, &mut pt) };
    (pt.x, pt.y)
}

pub fn on_drag_over(hwnd: HWND, screen: POINT, keys: u32, allowed: u32) -> u32 {
    let ptr = state_ptr(hwnd);
    if ptr.is_null() {
        return 0;
    }
    let st = unsafe { &mut *ptr };
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

pub fn on_drag_leave(hwnd: HWND) {
    let ptr = state_ptr(hwnd);
    if ptr.is_null() {
        return;
    }
    let st = unsafe { &mut *ptr };
    st.drop_insert = None;
    unsafe {
        KillTimer(hwnd, DRAG_OPEN_TIMER);
        st.drag_open_pending = false;
        if st.drag_opened && !st.pinned && LOCKED.load(Ordering::Relaxed) {
            SetTimer(hwnd, CLOSE_TIMER, st.settings.close_delay_ms, None);
        }
        InvalidateRect(hwnd, null(), 0);
    }
}

/// Handles files dropped on a panel and returns the effect for the drag source.
pub fn on_drop(hwnd: HWND, files: Vec<String>, screen: POINT, keys: u32, allowed: u32) -> u32 {
    let ptr = state_ptr(hwnd);
    if ptr.is_null() {
        return 0;
    }
    let st = unsafe { &mut *ptr };
    let effect = choose_effect(keys, allowed);
    let (x, y) = client_point(hwnd, screen);
    let index = st.insert_index(x, y);
    st.drop_insert = None;
    st.drag_opened = false;
    unsafe {
        KillTimer(hwnd, DRAG_OPEN_TIMER);
        st.drag_open_pending = false;
        InvalidateRect(hwnd, null(), 0);
    }
    if effect == 0 {
        return 0;
    }
    let panel = st.index;
    let dir = panel_dir(&storage_root(&st.settings.storage_dir), &st.cfg.title);

    if let Some((from_panel, from_item)) = INTERNAL_DRAG.get() {
        // One of our shortcuts: reorder it, or move it to this panel. The panels
        // are rebuilt once the drag loop has ended, not while it runs.
        INTERNAL_DROP_DONE.set(true);
        let root = storage_root(&st.settings.storage_dir);
        let result = config::edit_items(config::path(), |items| {
            if from_panel >= items.len() || from_item >= items[from_panel].len() || panel >= items.len() {
                return Err("The config file changed. Reload it and try again.".into());
            }
            let mut table = items[from_panel].remove(from_item);
            let path = item_path(&table);
            if from_panel != panel && is_stored(&path, &root) {
                let moved = move_into(Path::new(&expand_env(&path)), &dir)?;
                table["path"] = toml_edit::value(moved.to_string_lossy().as_ref());
            }
            let mut at = index;
            if from_panel == panel && from_item < at {
                at -= 1;
            }
            let at = at.min(items[panel].len());
            items[panel].insert(at, table);
            Ok(())
        });
        if let Err(e) = result {
            error_box(&e);
        }
        return 0;
    }

    let link = effect != DROPEFFECT_MOVE;
    let mut errors = Vec::new();
    let result = config::edit_items(config::path(), |items| {
        let list = items.get_mut(panel).ok_or("The config file changed. Reload it and try again.")?;
        let start = index.min(list.len());
        for (at, file) in (start..).zip(&files) {
            let src = Path::new(file);
            let name = display_name(src);
            let path = if link {
                file.clone()
            } else {
                match move_into(src, &dir) {
                    Ok(dest) => dest.to_string_lossy().to_string(),
                    // Could not move it (e.g. a shared desktop file): keep a link instead.
                    Err(e) => {
                        errors.push(e);
                        file.clone()
                    }
                }
            };
            list.insert(at, new_item(&name, &path));
        }
        Ok(())
    });
    match result {
        Err(e) => error_box(&e),
        Ok(()) if !errors.is_empty() => {
            error_box(&format!("Some files could not be moved, so they were added as links:\n\n{}", errors.join("\n")))
        }
        Ok(()) => {}
    }
    request_reload(st);
    // We moved the files ourselves: the source must not delete anything.
    if link { effect } else { 0 }
}

/// Drags a shortcut out of a panel with the shell's drag loop. Must be the last
/// thing the caller does: the panels may be rebuilt right after.
fn drag_item_out(hwnd: HWND, st: &mut PanelState, item: usize) {
    let path = resolve(&st.cfg.items[item].path);
    if !Path::new(&path).exists() {
        return;
    }
    let root = storage_root(&st.settings.storage_dir);
    let stored = is_stored(&path, &root);
    // Files we keep can be moved anywhere. A link must never move the original
    // (it could be a program in Program Files), unless it sits on the desktop.
    let on_desktop = dragdrop::desktop_dir().is_some_and(|d| Path::new(&path).parent() == Some(d.as_path()));
    let allowed = if stored || on_desktop { DROPEFFECT_MOVE | DROPEFFECT_COPY | DROPEFFECT_LINK } else { DROPEFFECT_LINK };
    let panel = st.index;
    let menu_target = st.menu_target;

    INTERNAL_DRAG.set(Some((panel, item)));
    INTERNAL_DROP_DONE.set(false);
    let dropped = dragdrop::drag_file_out(hwnd, &path, allowed);
    INTERNAL_DRAG.set(None);
    let reload = || unsafe { PostMessageW(menu_target, WM_APP_RELOAD, 0, 0) };
    if INTERNAL_DROP_DONE.replace(false) {
        reload();
        return;
    }
    let Some(effect) = dropped else { return };
    // Out of the panel: the file went where it was dropped, or a shortcut to it
    // was created there. A copy (Ctrl) leaves the shortcut in the panel.
    let gone = !Path::new(&path).exists();
    let out = gone || effect & DROPEFFECT_LINK != 0 || (on_desktop && !stored && dragdrop::cursor_over_desktop());
    if out {
        if let Err(e) = remove_item(panel, item) {
            error_box(&e);
        }
        reload();
    }
}

fn remove_item(panel: usize, item: usize) -> Result<(), String> {
    config::edit_items(config::path(), |items| {
        if let Some(list) = items.get_mut(panel)
            && item < list.len()
        {
            list.remove(item);
        }
        Ok(())
    })
}

/// Right-click menu on a shortcut while unlocked.
fn item_menu(hwnd: HWND, st: &mut PanelState, item: usize) {
    let rename = wide("Rename");
    let remove = wide("Remove from folder");
    let choice = unsafe {
        let menu = CreatePopupMenu();
        AppendMenuW(menu, MF_STRING, ID_RENAME, rename.as_ptr());
        AppendMenuW(menu, MF_STRING, ID_REMOVE, remove.as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(st.menu_target);
        let choice = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, 0, st.menu_target, null());
        DestroyMenu(menu);
        choice as usize
    };
    let panel = st.index;
    match choice {
        ID_RENAME => {
            let l = st.layout();
            let r = st.item_rect(&l, item);
            let mut origin = POINT { x: r.left, y: r.top + st.scale(8) + l.icon + st.scale(2) };
            unsafe { ClientToScreen(hwnd, &mut origin) };
            let height = st.scale(st.settings.font_size + 10);
            let width = l.cell_w.max(st.scale(120));
            let rect = RECT { left: origin.x - (width - l.cell_w) / 2, top: origin.y, right: origin.x + (width + l.cell_w) / 2, bottom: origin.y + height };
            let Some(name) = prompt_text(rect, &st.cfg.items[item].name, st.font) else { return };
            let result = config::edit_items(config::path(), |items| {
                if let Some(table) = items.get_mut(panel).and_then(|l| l.get_mut(item)) {
                    table["name"] = toml_edit::value(name.as_str());
                }
                Ok(())
            });
            if let Err(e) = result {
                error_box(&e);
            }
            request_reload(st);
        }
        ID_REMOVE => {
            // A file we keep goes back to the desktop; a link is just removed.
            let path = expand_env(&st.cfg.items[item].path);
            if is_stored(&path, &storage_root(&st.settings.storage_dir)) && Path::new(&path).exists() {
                let Some(desktop) = dragdrop::desktop_dir() else { return };
                if let Err(e) = move_into(Path::new(&path), &desktop) {
                    error_box(&e);
                    return;
                }
            }
            if let Err(e) = remove_item(panel, item) {
                error_box(&e);
            }
            request_reload(st);
        }
        _ => {}
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
        UpdateWindow(hwnd);
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
    st.open = open;
    st.hover = None;
    if open {
        push_other_panels_down(hwnd);
    }
    if st.settings.animation_ms == 0 {
        st.progress = if open { 1.0 } else { 0.0 };
        apply_size(hwnd, st);
    } else {
        st.anim_from = st.progress;
        st.anim_start = unsafe { GetTickCount64() };
        unsafe { SetTimer(hwnd, ANIM_TIMER, ANIM_FRAME_MS, None) };
    }
}

fn animate(hwnd: HWND, st: &mut PanelState) {
    let target = if st.open { 1.0 } else { 0.0 };
    // A full open takes animation_ms; a reversed half-way animation takes less.
    let duration = st.settings.animation_ms as f32 * (target - st.anim_from).abs();
    let elapsed = unsafe { GetTickCount64() } - st.anim_start;
    let p = if duration <= 0.0 { 1.0 } else { (elapsed as f32 / duration).min(1.0) };
    st.progress = st.anim_from + (target - st.anim_from) * p;
    if p >= 1.0 {
        st.progress = target;
        unsafe { KillTimer(hwnd, ANIM_TIMER) };
    }
    apply_size(hwnd, st);
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
                KillTimer(hwnd, CLOSE_TIMER);
                if locked && st.settings.hover_open && !st.open && !st.open_pending {
                    st.open_pending = true;
                    SetTimer(hwnd, OPEN_TIMER, st.settings.open_delay_ms, None);
                }
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
                st.open_pending = false;
                KillTimer(hwnd, OPEN_TIMER);
                InvalidateRect(hwnd, null(), 0);
                if locked && st.open && !st.pinned && st.settings.hover_open {
                    SetTimer(hwnd, CLOSE_TIMER, st.settings.close_delay_ms, None);
                }
                return 0;
            }
            WM_TIMER => {
                match wparam {
                    OPEN_TIMER => set_open(hwnd, st, true),
                    CLOSE_TIMER => set_open(hwnd, st, false),
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
                st.press = match st.hit(x, y) {
                    Hit::Item(i) if !locked => Some((x, y, i)),
                    _ => None,
                };
                return 0;
            }
            WM_LBUTTONUP => {
                st.press = None;
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
                        launch(&item.path, &item.args);
                        if st.settings.collapse_after_launch && !st.pinned {
                            set_open(hwnd, st, false);
                        }
                    }
                    Hit::Nothing => {}
                }
                return 0;
            }
            WM_RBUTTONUP => {
                let (x, y) = mouse();
                match st.hit(x, y) {
                    Hit::Item(i) if !locked => item_menu(hwnd, st, i),
                    _ => {
                        PostMessageW(st.menu_target, WM_APP_SHOW_MENU, 0, 0);
                    }
                }
                return 0;
            }
            WM_NCRBUTTONUP => {
                PostMessageW(st.menu_target, WM_APP_SHOW_MENU, 0, 0);
                return 0;
            }
            WM_DESTROY => dragdrop::revoke(hwnd),
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
/// light on the glass. Icons already carry their own alpha. Rows below the
/// title fade in with the opening animation.
fn fix_alpha(pixels: &mut [u32], w: usize, title_h: usize, fade: f32) {
    let f = (fade.clamp(0.0, 1.0) * 255.0) as u32;
    for (y, row) in pixels.chunks_mut(w).enumerate() {
        let scale = if y < title_h { 255 } else { f };
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

fn paint(hwnd: HWND, st: &PanelState, locked: bool) {
    let s = &st.settings;
    let text = parse_color(&s.text, 0xF2F2F2);
    let hover = parse_color(&s.hover, 0x2E2E2E);
    let border = parse_color(&s.border, 0x3A3A3A);
    let accent = parse_color(&s.accent, 0xFFC24C);
    let l = st.layout();

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
        let old_bmp = SelectObject(mem, bmp);
        SetBkMode(mem, TRANSPARENT as _);
        SetTextColor(mem, text);
        let radius = st.scale(8);

        // Border, in the accent color while panels can be moved.
        let pen = CreatePen(PS_SOLID, if locked { 1 } else { st.scale(2) }, if locked { border } else { accent });
        let old_pen = SelectObject(mem, pen);
        let old_brush = SelectObject(mem, GetStockObject(NULL_BRUSH));
        RoundRect(mem, 0, 0, w, h, radius * 2, radius * 2);
        SelectObject(mem, old_brush);
        SelectObject(mem, old_pen);
        DeleteObject(pen);

        // Title: folder icon + name, and a pin marker while pinned.
        let icon_left = st.scale(12);
        if !st.title_icon.is_null() {
            let top = (l.title_h - l.label_icon) / 2;
            DrawIconEx(mem, icon_left, top, st.title_icon, l.label_icon, l.label_icon, 0, null_mut(), DI_NORMAL);
        }
        let old_font = SelectObject(mem, st.title_font);
        let text_left = icon_left + l.label_icon + st.scale(8);
        let pin_space = if st.pinned { st.scale(18) } else { 0 };
        let title_rect = RECT { left: text_left, top: 0, right: w - st.scale(8) - pin_space, bottom: l.title_h };
        draw_text(mem, &st.cfg.title, &title_rect, DT_LEFT | DT_VCENTER);
        if st.pinned {
            let d = st.scale(7);
            let cx = w - st.scale(16);
            let cy = l.title_h / 2;
            let dot = RECT { left: cx - d / 2, top: cy - d / 2, right: cx + d / 2 + 1, bottom: cy + d / 2 + 1 };
            fill_round(mem, &dot, d, accent);
        }

        if st.progress > 0.0 {
            SelectObject(mem, st.font);
            for (i, item) in st.cfg.items.iter().enumerate() {
                let r = st.item_rect(&l, i);
                if r.top >= h {
                    break;
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
                draw_text(mem, &item.name, &label, DT_CENTER | DT_TOP);
            }
            if st.cfg.items.is_empty() {
                let hint = RECT { left: l.pad, top: l.title_h, right: w - l.pad, bottom: l.title_h + l.cell_h };
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
        }

        GdiFlush();
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
        fix_alpha(pixels, w as usize, l.title_h as usize, ease_out_cubic(st.progress));

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
        fix_alpha(&mut px, 3, 1, 1.0);
        assert_eq!(px, [0xF0F0F0F0, 0x80402010, 0x00000000]);
    }

    #[test]
    fn rows_below_title_fade() {
        let mut px = [0x00FFFFFF, 0x00FFFFFF];
        fix_alpha(&mut px, 1, 1, 0.0);
        assert_eq!(px, [0xFFFFFFFF, 0x00000000]);
    }
}
