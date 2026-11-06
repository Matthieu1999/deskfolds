#![windows_subsystem = "windows"]

mod arrange;
mod config;
mod dragdrop;
mod frame;
mod help;
mod integration;
mod layout;
mod panel;
mod settings;
mod shellmenu;
mod slider;
mod store;
mod ui;
mod util;
mod watch;
mod welcome;

use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, UpdateWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Ole::OleInitialize;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem, SetProcessDpiAwarenessContext};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetDoubleClickTime, IsWindowEnabled};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use panel::{LOCKED, WM_APP_ARRANGE, WM_APP_PUSH, WM_APP_SHOW_MENU};
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
const ID_DELETE_FOLDER: usize = 106;
const ID_RENAME_FOLDER: usize = 107;
const ID_UNLOCK_ONE: usize = 108;
const ID_ITEM_MORE: usize = 109;
/// The « New » submenu's entries: NEW_BASE + their index.
const NEW_BASE: usize = 0x800;

/// Posted to the main window: make a folder at (wparam, lparam), the place
/// the desktop menu was used, and name it there.
pub const WM_APP_NEW_FOLDER_AT: u32 = WM_APP + 5;
/// Posted to the main window: type the name of folder `wparam` in place.
pub const WM_APP_RENAME: u32 = WM_APP + 6;
/// NIN_BALLOONUSERCLICK, from the tray icon.
const BALLOON_CLICK: u32 = WM_USER + 5;

/// What a click on the tray icon's notification does.
#[derive(Clone, Copy, PartialEq)]
pub enum Balloon {
    Plain,
    /// Asks Windows to move the shortcuts for all users (see `watch`).
    Shared,
}

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// The first folder of a row, and the folders `make_room` put on new lines
/// under it, with where it put them.
type Wrapped = (u32, Vec<(u32, i32, i32)>);

thread_local! {
    static MAIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static PANELS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    /// The "changed outside deskfolds" question is on screen.
    static ASKING: Cell<bool> = const { Cell::new(false) };
    /// Ignore the button release that ends a double-click on the tray icon.
    static SKIP_CLICK: Cell<bool> = const { Cell::new(false) };
    /// (open folder, folder it pushes away), from the last push update.
    static STACKS: RefCell<Vec<(HWND, HWND)>> = const { RefCell::new(Vec::new()) };
    /// The notification on screen.
    static BALLOON: Cell<Balloon> = const { Cell::new(Balloon::Plain) };
    /// Folders put on a new line by `make_room`, by the first folder of their
    /// row: (folder, where it was put). Kept while they stay there.
    static WRAPS: RefCell<Vec<Wrapped>> = const { RefCell::new(Vec::new()) };
    /// Every folder's label as last seen after a change, to tell which one
    /// changed size (a resize with the grip shows the new size before saving it).
    static LAST_LABELS: RefCell<Vec<(u32, layout::Rect, bool, u32)>> = const { RefCell::new(Vec::new()) };
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
    // Pinned folders opened before the others existed: now the direction they
    // open in can take the others into account.
    for hwnd in panels() {
        panel::reposition(hwnd);
    }
    LAST_LABELS.with(|l| *l.borrow_mut() = labels_now());
    // The frame belonged to the old desktop window (Explorer restarted).
    frame::hide();
    arrange::rearrange();
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
    apply_panel_changes(main, changes);
    // A folder joined, left, moved in the list or changed size: place the arrangement again.
    arrange::rearrange();
}

/// Every folder's label: (id, label, can be moved by `make_room`, DPI).
fn labels_now() -> Vec<(u32, layout::Rect, bool, u32)> {
    panels()
        .into_iter()
        .filter_map(|h| {
            let g = panel::geometry(h)?;
            Some((panel::id_of(h)?, g.label, !g.arranged && !g.moving, g.dpi))
        })
        .collect()
}

/// A folder's label changed size: the folders snapped to it move aside, or
/// back, and go to a new line rather than off the screen (see `layout::make_room`).
/// `before`: the labels as they are, with that folder's old size.
fn make_room(grown: u32, before: &[(u32, layout::Rect, bool, u32)], size: (i32, i32)) {
    let Some(gi) = before.iter().position(|b| b.0 == grown) else { return };
    if !before[gi].2 {
        return;
    }
    let labels: Vec<layout::Rect> = before.iter().map(|b| b.1).collect();
    let movable: Vec<bool> = before.iter().map(|b| b.2).collect();
    let gap = scale(store::with(|s| s.settings.gap), before[gi].3);
    let row = layout::snapped_row(&labels, &movable, gi, gap);
    let head = before[row[0]].0;
    // The folders this row put on new lines, if they are still where they were put.
    let wrapped: Vec<usize> = WRAPS.with(|w| {
        w.borrow()
            .iter()
            .find(|(h, _)| *h == head)
            .map(|(_, list)| {
                list.iter().filter_map(|&(id, x, y)| before.iter().position(|b| b.0 == id && (b.1.x, b.1.y) == (x, y))).collect()
            })
            .unwrap_or_default()
    });
    let g = labels[gi];
    let area = panel::work_area_at(g.x + g.w / 2, g.y + g.h / 2);
    let (moves, lines) = layout::make_room(&labels, &movable, gi, size, &row, &wrapped, area, gap);
    let at = |i: usize| moves.iter().find(|m| m.0 == i).map_or((labels[i].x, labels[i].y), |m| (m.1, m.2));
    let kept: Vec<(u32, i32, i32)> = lines.iter().map(|&i| (before[i].0, at(i).0, at(i).1)).collect();
    WRAPS.with(|w| {
        let mut w = w.borrow_mut();
        w.retain(|(h, _)| *h != head);
        if !kept.is_empty() {
            w.push((head, kept));
        }
    });
    let positions: Vec<(u32, i32, i32)> = moves.iter().map(|&(i, x, y)| (before[i].0, x, y)).collect();
    if !positions.is_empty() {
        store::change(|s| s.set_positions(&positions));
    }
}

fn apply_panel_changes(main: HWND, changes: &[Change]) {
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
        // The gap or the push setting may have changed.
        update_pushes();
    }
    // A folder that changed size makes room for itself (not for a change of
    // the shared settings, which changes them all).
    let after = labels_now();
    let last = LAST_LABELS.with(|l| std::mem::replace(&mut *l.borrow_mut(), after.clone()));
    if settings.snap_make_room {
        for change in changes {
            if let Change::Panel(id) = *change
                && let (Some(old), Some(gi)) = (last.iter().find(|b| b.0 == id), after.iter().position(|a| a.0 == id))
                && (old.1.w, old.1.h) != (after[gi].1.w, after[gi].1.h)
                && (old.1.x, old.1.y) == (after[gi].1.x, after[gi].1.y)
            {
                // The others as they are now, this one with its old size.
                let mut before = after.clone();
                before[gi].1.w = old.1.w;
                before[gi].1.h = old.1.h;
                make_room(id, &before, (after[gi].1.w, after[gi].1.h));
            }
        }
    }
}

fn reposition_panels() {
    for hwnd in panels() {
        panel::reposition(hwnd);
    }
    arrange::rearrange();
}

/// Keeps the folder selected in the settings window open and outlined.
pub fn highlight_panel(id: Option<u32>) {
    for hwnd in panels() {
        panel::set_highlight(hwnd, id.is_some() && panel::id_of(hwnd) == id);
    }
}

/// Unlocked, every panel opens with a blue border and can be moved, resized
/// and edited. Positions are saved as soon as a panel is moved. Locking or
/// unlocking them all also ends the folders unlocked one by one.
pub fn set_locked(locked: bool) {
    LOCKED.store(locked, Ordering::Relaxed);
    for hwnd in panels() {
        panel::clear_solo(hwnd);
        panel::set_editing(hwnd, !locked);
    }
    // Shows or hides the arrangement's frame.
    arrange::rearrange();
    settings::on_store_changed();
}

/// Some folder can be moved now: all of them, or one unlocked by itself.
pub fn any_unlocked() -> bool {
    !LOCKED.load(Ordering::Relaxed) || panels().into_iter().any(panel::is_solo)
}

/// The arrangement's frame shows while folders are unlocked, or while one of
/// its folders is unlocked by itself (it can then take another place in it).
pub fn frame_wanted() -> bool {
    !LOCKED.load(Ordering::Relaxed) || panels().into_iter().any(|h| panel::is_solo(h) && panel::geometry(h).is_some_and(|g| g.arranged))
}

pub fn toggle_lock() {
    set_locked(any_unlocked());
}

/// Unlocks one folder by itself, or locks it again.
fn toggle_solo(id: u32) {
    if let Some(hwnd) = panel_window(id) {
        panel::set_solo(hwnd, !panel::is_solo(hwnd));
    }
    arrange::rearrange();
    settings::on_store_changed();
}

fn scale(v: i32, dpi: u32) -> i32 {
    (v as f32 * dpi as f32 / 96.0).round() as i32
}

/// Creates "New folder" at a free spot on the main screen and returns its id.
pub fn new_folder() -> u32 {
    make_folder(None, None)
}

/// Creates a folder of this name at a free spot (for new desktop shortcuts).
pub fn new_folder_named(title: &str) -> u32 {
    make_folder(Some(title), None)
}

/// Creates a folder (named `title`, or "New folder", "New folder (2)"...) with
/// its label at `at` if there is room there, otherwise at a free spot.
fn make_folder(title: Option<&str>, at: Option<POINT>) -> u32 {
    let (width, font, gap) = store::with(|s| (s.settings.new_folder_width, s.settings.font_size, s.settings.gap));
    let taken: Vec<layout::Rect> = panels().into_iter().filter_map(panel::geometry).map(|g| g.label).collect();
    let dpi = unsafe { GetDpiForSystem() }.max(96);
    let (w, h, gap) = (scale(width, dpi), scale(font + 20, dpi), scale(gap.max(8), dpi));
    let (x, y) = match at {
        Some(pt) => {
            let area = panel::work_area_at(pt.x, pt.y);
            let x = pt.x.clamp(area.x + gap, (area.right() - w - gap).max(area.x + gap));
            let y = pt.y.clamp(area.y + gap, (area.bottom() - h - gap).max(area.y + gap));
            let spot = layout::Rect { x, y, w, h };
            let free = !taken.iter().any(|t| t.x < spot.right() + gap && spot.x < t.right() + gap && t.y < spot.bottom() + gap && spot.y < t.bottom() + gap);
            if free { (x, y) } else { layout::free_spot(&taken, w, h, area, gap) }
        }
        None => layout::free_spot(&taken, w, h, panel::primary_work_area(), gap),
    };
    // It joins the arrangement, if there is one and the settings say so.
    let join = arrange::has_members() && store::with(|s| s.settings.new_folders_join);
    store::change(|s| {
        let title = match title {
            Some(t) => t.to_string(),
            None => s.unique_title("New folder"),
        };
        let id = s.add_panel(&title, x, y, width);
        if join {
            s.set_arranged(&[id], true);
        }
        id
    })
}

/// The desktop menu's « New deskfolds folder »: a folder where it was used,
/// named in place.
fn new_folder_at(main: HWND, at: POINT) {
    let id = make_folder(None, Some(at));
    // Once its window is made (WM_APP_REFRESH comes first).
    unsafe { PostMessageW(main, WM_APP_RENAME, id as usize, 0) };
}

fn update_pushes() {
    relayout();
}

/// Places every folder for this moment of the animations: the folders stacked
/// under (or above) open ones are pushed down (or up) by how far those reach,
/// the others are put back. A folder that would push one past the edge of the
/// screen covers them instead. All the moves go to Windows in one batch, so no
/// frame shows some folders moved and others not.
pub fn relayout() {
    panel::pushes_handled();
    let (push_below, gap) = store::with(|s| (s.settings.push_below, s.settings.gap));
    let list: Vec<(HWND, panel::Geometry)> =
        panels().into_iter().filter_map(|h| panel::geometry(h).map(|g| (h, g))).filter(|(_, g)| !g.moving).collect();
    let mut offsets = vec![0; list.len()];
    let mut stacks = Vec::new();
    if push_below {
        // A folder of the arrangement counts as tall as its row.
        let labels: Vec<layout::Rect> = list
            .iter()
            .map(|(h, g)| layout::Rect { h: g.label.h.max(arrange::row_height(*h).unwrap_or(0)), ..g.label })
            .collect();
        for (i, (opener, g)) in list.iter().enumerate() {
            if g.extra <= 0 && !g.open {
                continue;
            }
            // An open folder of the "fit" style is wider than its label.
            let mut column = labels.clone();
            column[i].w = g.open_w;
            let gap = scale(gap, g.dpi);
            let pushed = if g.up { layout::stacked_above(&column, i, gap) } else { layout::stacked_below(&column, i, gap) };
            let sign = if g.up { -1 } else { 1 };
            // Checked with the full height, so the pushed folders do not jump
            // back half-way through the animation.
            let now: Vec<layout::Rect> = labels.iter().zip(&offsets).map(|(l, o)| layout::Rect { y: l.y + o, ..*l }).collect();
            let area = panel::work_area_at(g.label.x + g.label.w / 2, g.label.y + g.label.h / 2);
            if !layout::pushes_fit(&now, &pushed, sign * (g.open_h - g.label.h), area) {
                continue;
            }
            for j in pushed {
                offsets[j] += sign * g.extra;
                if g.open {
                    stacks.push((*opener, list[j].0));
                }
            }
        }
    }
    let moves: Vec<(HWND, panel::Placement)> =
        list.iter().zip(offsets).filter_map(|((hwnd, _), push)| panel::place_for(*hwnd, push).map(|p| (*hwnd, p))).collect();
    STACKS.with(|s| *s.borrow_mut() = stacks);
    if moves.is_empty() {
        return;
    }
    let flags = |resized: bool| SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | if resized { 0 } else { SWP_NOSIZE };
    unsafe {
        let mut batch = BeginDeferWindowPos(moves.len() as i32);
        for &(hwnd, (x, y, w, h, resized)) in &moves {
            if !batch.is_null() {
                batch = DeferWindowPos(batch, hwnd, null_mut(), x, y, w, h, flags(resized));
            }
        }
        if batch.is_null() || EndDeferWindowPos(batch) == 0 {
            // The batch could not be made: move them one by one.
            for &(hwnd, (x, y, w, h, resized)) in &moves {
                SetWindowPos(hwnd, null_mut(), x, y, w, h, flags(resized));
            }
        }
        for &(hwnd, (.., resized)) in &moves {
            if resized {
                InvalidateRect(hwnd, null(), 0);
                UpdateWindow(hwnd);
            }
        }
    }
}

/// The cursor is on a folder that this open folder pushes away: that folder
/// is the window under it (a window over the folders covers them).
pub fn cursor_in_stack(opener: HWND) -> bool {
    let mut pt = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt) } == 0 {
        return false;
    }
    let under = unsafe { GetAncestor(WindowFromPoint(pt), GA_ROOT) };
    !under.is_null() && STACKS.with(|s| s.borrow().iter().any(|&(o, p)| o == opener && p == under))
}

/// Whether the folder in `hwnd` opens upwards: its label `label` grows by
/// `extra` when open. It opens like the folders of its group: its
/// arrangement, or the folders stacked with it (see `layout::group_opens_up`).
pub fn opens_up(hwnd: HWND, label: layout::Rect, extra: i32, arranged: bool, dpi: u32) -> bool {
    let gap = scale(store::with(|s| s.settings.gap), dpi);
    // A folder of the arrangement counts as tall as its row, as for pushing.
    let row = |h: HWND, l: layout::Rect| layout::Rect { h: l.h.max(arrange::row_height(h).unwrap_or(0)), ..l };
    let mut labels = vec![row(hwnd, label)];
    let mut extras = vec![extra];
    let mut in_arrangement = vec![arranged];
    for h in panels().into_iter().filter(|&h| h != hwnd) {
        if let Some(g) = panel::geometry(h)
            && !g.moving
        {
            labels.push(row(h, layout::Rect { w: g.label.w.max(g.open_w), ..g.label }));
            extras.push(g.open_h - g.label.h);
            in_arrangement.push(g.arranged);
        }
    }
    let area = panel::work_area_at(label.x + label.w / 2, label.y + label.h / 2);
    let group = layout::group(&labels, &in_arrangement, 0, gap);
    layout::group_opens_up(&labels, &extras, &group, 0, area)
}

/// The labels of every other folder, for snapping a moved one.
pub fn other_labels(except: HWND) -> Vec<(HWND, layout::Rect)> {
    panels().into_iter().filter(|&h| h != except).filter_map(|h| panel::geometry(h).map(|g| (h, g.label))).collect()
}

/// Lights up the folders a moved folder snaps to, and only those.
pub fn show_snap_targets(moving: HWND, targets: &[HWND]) {
    for hwnd in panels().into_iter().filter(|&h| h != moving) {
        panel::set_snap_target(hwnd, targets.contains(&hwnd));
    }
}

/// A move of a folder or of the frame ended: with "all folders open while
/// unlocked", the folders folded for the move open again.
pub fn after_move() {
    if !LOCKED.load(Ordering::Relaxed) && store::with(|s| s.settings.unlocked_open_all) {
        for hwnd in panels() {
            panel::set_editing(hwnd, true);
        }
    }
    // Pinned folders, and the ones unlocked by themselves, open again.
    for hwnd in panels() {
        panel::reopen_held(hwnd);
    }
}

/// A folder (or the arrangement's frame, with a null `moving`) is about to be
/// moved: fold every folder but the one selected in Settings, so it moves by
/// its label and nothing is pushed meanwhile.
pub fn fold_for_move(moving: HWND) {
    for hwnd in panels() {
        panel::close_now(hwnd, hwnd == moving);
    }
    update_pushes();
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
    // A dialog of the settings window is open: its code still uses the window
    // once the dialog returns, so quitting now would pull it away. Show it instead.
    if let Some(w) = settings::owner()
        && unsafe { IsWindowEnabled(w) } == 0
    {
        unsafe { SetForegroundWindow(w) };
        return;
    }
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
    show_balloon(main, "deskfolds", text, NIIF_WARNING, Balloon::Plain);
}

/// A Windows notification from the tray icon. With `Balloon::Shared`, a click
/// on it asks Windows to move the shortcuts for all users.
pub fn notify(main: HWND, title: &str, text: &str, kind: Option<Balloon>) {
    show_balloon(main, title, text, NIIF_INFO, kind.unwrap_or(Balloon::Plain));
}

fn show_balloon(main: HWND, title: &str, text: &str, icon: u32, kind: Balloon) {
    // A new notification replaces the one about shortcuts for all users.
    if BALLOON.replace(kind) == Balloon::Shared && kind != Balloon::Shared {
        watch::forget_refused();
    }
    let mut nid = tray_data(main);
    nid.uFlags = NIF_INFO;
    nid.dwInfoFlags = icon;
    copy_wide(&mut nid.szInfoTitle, title);
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

/// The tray menu. Opened from a folder, it starts with Windows' « New » menu
/// for its storage folder and that folder's commands; from one of its
/// shortcuts while locked, with that shortcut's Windows menu.
fn show_menu(main: HWND, panel: Option<u32>, item: Option<u32>) {
    check_config_file();
    let title = panel.and_then(|id| store::with(|s| s.panel(id).map(|p| p.title.clone())));
    // The name from the config, on one line and not too long, for the menu.
    let name = title.as_ref().map(|t| ui::menu_text(&ui::short_name(t, 40)));
    let item_name = item.and_then(|id| store::with(|s| s.item(id).map(|(_, it)| ui::menu_text(&ui::short_name(&it.name, 40)))));
    let locked = LOCKED.load(Ordering::Relaxed);
    let hwnd = panel.and_then(panel_window);
    let solo = hwnd.is_some_and(panel::is_solo);
    let autostart = if util::autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    let labels = name.as_ref().map(|t| {
        (
            format!("Edit '{t}'…"),
            format!("Rename '{t}'"),
            format!("Delete '{t}'…"),
            if solo { format!("Lock '{t}'") } else { format!("Unlock '{t}' (move and edit)") },
        )
    });
    let more = item_name.as_ref().map(|n| format!("Show more options for '{n}'"));
    let mut entries: Vec<MenuEntry> = Vec::new();
    if let Some(more) = &more {
        entries.extend([(ID_ITEM_MORE, more.as_str(), 0), ui::SEPARATOR]);
    }
    if let Some((edit, rename, delete, unlock)) = &labels {
        entries.extend([(ID_EDIT_FOLDER, edit.as_str(), 0), (ID_RENAME_FOLDER, rename.as_str(), 0), (ID_DELETE_FOLDER, delete.as_str(), 0), ui::SEPARATOR]);
        // One folder by itself: only while the others are locked.
        if locked {
            entries.push((ID_UNLOCK_ONE, unlock.as_str(), 0));
        }
    }
    let lock_label = if any_unlocked() { "Lock all folders" } else { "Unlock all folders (move and edit)" };
    if labels.is_some() {
        entries.extend([(ID_LOCK, lock_label, 0), ui::SEPARATOR, (ID_SETTINGS, "Settings…", 0), (ID_NEW_FOLDER, "New folder", 0)]);
    } else {
        entries.extend([(ID_SETTINGS, "Settings…", 0), (ID_NEW_FOLDER, "New folder", 0), (ID_LOCK, lock_label, 0)]);
    }
    entries.extend([ui::SEPARATOR, (ID_AUTOSTART, "Start with Windows", autostart), ui::SEPARATOR, (ID_QUIT, "Quit", 0)]);
    let mut at = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut at) };
    // Windows' « New » menu for the folder's storage folder, at the top.
    let storage = panel.map(|id| store::with(|s| s.storage_of(id)));
    let kinds = if storage.is_some() { shellmenu::new_kinds() } else { Vec::new() };
    let chosen = shellmenu::track(main, at, None, |menu| unsafe {
        if !kinds.is_empty() {
            let sub = CreatePopupMenu();
            for (i, kind) in kinds.iter().enumerate() {
                // Folder and Shortcut, then the file types.
                if i == 2 {
                    AppendMenuW(sub, MF_SEPARATOR, 0, null());
                }
                let text = wide(&ui::menu_text(&ui::short_name(&kind.label, 60)));
                AppendMenuW(sub, MF_STRING, NEW_BASE + i, text.as_ptr());
            }
            let text = wide("New");
            AppendMenuW(menu, MF_POPUP, sub as usize, text.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
        }
        for (id, text, flags) in &entries {
            let text = wide(text);
            AppendMenuW(menu, MF_STRING | flags, *id, text.as_ptr());
        }
    }) as usize;
    // The mouse may have left the folder while the menu was open.
    panel::after_menu();
    if let (Some(kind), Some(id), Some(dir)) = (chosen.checked_sub(NEW_BASE).and_then(|i| kinds.get(i)), panel, &storage) {
        new_from_menu(main, kind, id, dir);
        return;
    }
    match chosen {
        ID_ITEM_MORE => {
            if let Some(id) = item {
                panel::item_windows_menu(main, id, at);
            }
        }
        ID_EDIT_FOLDER => open_settings(panel),
        ID_RENAME_FOLDER => {
            if let Some(h) = hwnd {
                panel::rename_in_place(h);
            }
        }
        ID_DELETE_FOLDER => {
            if let Some(id) = panel {
                settings::delete_folder(settings::owner().unwrap_or(main), id);
            }
        }
        ID_UNLOCK_ONE => {
            if let Some(id) = panel {
                toggle_solo(id);
            }
        }
        ID_SETTINGS => open_settings(None),
        ID_NEW_FOLDER => {
            let id = new_folder();
            // Named in place, once its window is made.
            unsafe { PostMessageW(main, WM_APP_RENAME, id as usize, 0) };
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

/// An entry of the « New » menu was chosen: the new file is made in the
/// folder's storage folder, and joins the folder, its name ready to type.
fn new_from_menu(main: HWND, kind: &shellmenu::NewKind, panel_id: u32, dir: &std::path::Path) {
    let before = shellmenu::listing(dir);
    match shellmenu::make_new(dir, kind) {
        Ok(made) => panel::adopt_new(main, panel_id, dir, before, made),
        Err(e) => error_box(&e),
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // Windows' menus on screen fill their submenus through their owner.
        if let Some(result) = shellmenu::handle_menu_message(msg, wparam, lparam) {
            return result;
        }
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
                    WM_RBUTTONUP => show_menu(hwnd, None, None),
                    // Also from the notification center later: the question stays
                    // until another notification replaces it.
                    BALLOON_CLICK if BALLOON.replace(Balloon::Plain) == Balloon::Shared => watch::ask_for_shared(hwnd),
                    _ => {}
                }
                0
            }
            WM_APP_SHOW_MENU => {
                show_menu(hwnd, (wparam != 0).then_some(wparam as u32), (lparam != 0).then_some(lparam as u32));
                0
            }
            WM_APP_NEW_FOLDER_AT => {
                new_folder_at(hwnd, POINT { x: wparam as i32, y: lparam as i32 });
                0
            }
            WM_APP_RENAME => {
                if let Some(h) = panel_window(wparam as u32) {
                    panel::rename_in_place(h);
                }
                0
            }
            panel::WM_APP_RENAME_ITEM => {
                let item = wparam as u32;
                if let Some(id) = store::with(|s| s.item(item).map(|(p, _)| p))
                    && let Some(h) = panel_window(id)
                {
                    panel::rename_item_in_place(h, item);
                }
                0
            }
            watch::WM_APP_DESKTOP_FILE => {
                watch::on_found(hwnd);
                0
            }
            WM_APP_REFRESH => {
                // While a shortcut is dragged out of a panel, the drag's end posts this again.
                if !panel::dragging() {
                    let changes = store::with(|s| s.take_changes());
                    apply_changes(hwnd, &changes);
                    settings::on_store_changed();
                    // New desktop shortcuts may have been turned on or off.
                    watch::sync(hwnd);
                }
                0
            }
            WM_APP_CONFLICT => {
                check_config_file();
                0
            }
            WM_APP_PUSH => {
                update_pushes();
                0
            }
            WM_APP_ARRANGE => {
                arrange::rearrange();
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
                    TRAY_CLICK_TIMER => show_menu(hwnd, None, None),
                    watch::PUT_AWAY_TIMER => watch::on_timer(hwnd),
                    panel::NEW_LINK_TIMER => panel::check_new_links(hwnd),
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

/// One deskfolds per config file. With the usual one it keeps the name older
/// versions use, so they never run together; another APPDATA (a test copy)
/// gets its own.
fn instance_name() -> String {
    const NAME: &str = "Local\\deskfolds.single-instance";
    // Compared after resolving links, mapped drives and short names.
    match integration::copy_suffix() {
        None => NAME.into(),
        Some(suffix) => format!("{NAME}.{suffix}"),
    }
}

/// The desktop menu started this second deskfolds: the one running makes the
/// folder, where the menu was used.
fn send_new_folder(at: POINT) {
    let (class, title) = (wide(MAIN_CLASS), wide(&instance_name()));
    unsafe {
        let running = FindWindowW(class.as_ptr(), title.as_ptr());
        if running.is_null() {
            return;
        }
        // Explorer let this deskfolds come to the front: the running one may
        // too, for the name box.
        let mut pid = 0;
        GetWindowThreadProcessId(running, &mut pid);
        AllowSetForegroundWindow(pid);
        PostMessageW(running, WM_APP_NEW_FOLDER_AT, at.x as isize as usize, at.y as isize);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A copy with its own APPDATA (a test copy), started by what it added to
    // Windows: it gives its folder, so it starts as itself.
    if let Some(i) = args.iter().position(|a| a == integration::APPDATA_ARG)
        && let Some(dir) = args.get(i + 1)
        && std::path::Path::new(dir).is_absolute()
    {
        // Nothing else runs yet: no other thread reads the environment.
        unsafe { std::env::set_var("APPDATA", dir) };
    }
    let new_folder_asked = args.iter().any(|a| a == integration::NEW_FOLDER_ARG);
    unsafe {
        // Also set by the manifest; kept for builds without it.
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Where the desktop menu was used, before anything else moves the mouse.
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);

        let mutex_name = wide(&instance_name());
        let _mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if new_folder_asked {
                send_new_folder(cursor);
            }
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
        frame::register_class();

        // Hidden top-level window: receives tray, menu and store messages. Its
        // title is the instance's name, for a second deskfolds to find it.
        let title = wide(&instance_name());
        let main = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
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
        let first_start = data.first_start;
        store::init(data);
        add_tray_icon(main);
        rebuild_panels(main);
        if !notices.is_empty() {
            tray_notice(main, &notices.join("\n"));
        }
        // What deskfolds added to Windows opens this deskfolds.exe.
        integration::refresh();
        watch::sync(main);
        if new_folder_asked {
            PostMessageW(main, WM_APP_NEW_FOLDER_AT, cursor.x as isize as usize, cursor.y as isize);
        } else if first_start {
            welcome::show();
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if settings::translate(&msg) || welcome::translate(&msg) {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
