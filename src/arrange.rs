//! The arrangement: the folders in it are placed by its shape, its anchor and
//! its offset after every change, and they move together with its frame.
//!
//! The positions are still saved in each folder's x and y, so the config file
//! always says where the folders are. While the frame or a folder is being
//! dragged, nothing is placed again: the drag shows where things would go, and
//! the drop saves it.

use std::cell::{Cell, RefCell};
use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::core::BOOL;

use crate::config::{ANCHORS, Settings, anchor_label};
use crate::layout::{self, Rect};
use crate::{frame, panel, store};

/// Space between the frame and its folders, at 100 % scaling.
pub const FRAME_PAD: i32 = 8;
/// A moved arrangement snaps to an anchor closer than this.
const SNAP_REACH: i32 = 12;
const MONITORINFOF_PRIMARY: u32 = 1;

thread_local! {
    /// The frame or a folder is being dragged: nothing is placed again meanwhile.
    static BUSY: Cell<bool> = const { Cell::new(false) };
    /// Where the dragged folder goes if dropped now: into the arrangement (its
    /// new order), or out of it (None). Unset when it is not over one.
    static DROP: RefCell<Option<Option<Vec<u32>>>> = const { RefCell::new(None) };
    /// The frame is being dragged: where the arrangement and each of its
    /// folders were when it started.
    static FRAME_DRAG: RefCell<Option<FrameDrag>> = const { RefCell::new(None) };
    /// Each folder of the arrangement and the height of its row.
    static ROWS: RefCell<Vec<(HWND, i32)>> = const { RefCell::new(Vec::new()) };
}

/// The height of the row a folder of the arrangement is in: pushing treats it
/// as that tall, so the folder under it in the same column is pushed down.
pub fn row_height(hwnd: HWND) -> Option<i32> {
    ROWS.with(|r| r.borrow().iter().find(|(h, _)| *h == hwnd).map(|(_, h)| *h))
}

/// Where the arrangement was, and each of its folders: (window, label position).
type FrameDrag = ((i32, i32), Vec<(HWND, (i32, i32))>);

fn scale(v: i32, dpi: u32) -> i32 {
    (v as f32 * dpi as f32 / 96.0).round() as i32
}

/// A monitor, for the list of screens in Settings.
pub struct Screen {
    /// Device name, as saved in arrange_screen; empty for the main screen.
    pub device: String,
    pub label: String,
    pub full: Rect,
    pub work: Rect,
    pub dpi: u32,
    pub primary: bool,
}

fn to_rect(r: RECT) -> Rect {
    Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let list = unsafe { &mut *(data as *mut Vec<Screen>) };
    let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) } != 0 {
        let (mut dx, mut dy) = (96, 96);
        unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
        let end = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        list.push(Screen {
            device: String::from_utf16_lossy(&info.szDevice[..end]),
            label: String::new(),
            full: to_rect(info.monitorInfo.rcMonitor),
            work: to_rect(info.monitorInfo.rcWork),
            dpi: dx.max(96),
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    1
}

/// The connected screens, the main one first: "Main screen", "Screen 2 (above)"...
pub fn screens() -> Vec<Screen> {
    let mut list: Vec<Screen> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), null(), Some(collect), &mut list as *mut Vec<Screen> as LPARAM) };
    list.sort_by_key(|s| !s.primary);
    let main = list.first().map(|s| s.full).unwrap_or_default();
    for (i, s) in list.iter_mut().enumerate() {
        if s.primary {
            s.label = "Main screen".into();
            s.device.clear();
            continue;
        }
        let side = if s.full.bottom() <= main.y {
            "above"
        } else if s.full.y >= main.bottom() {
            "below"
        } else if s.full.right() <= main.x {
            "left"
        } else {
            "right"
        };
        s.label = format!("Screen {} ({side})", i + 1);
    }
    list
}

/// The screen the arrangement is on: the chosen one if it is connected, else the main one.
fn screen_for(device: &str) -> Option<Screen> {
    let mut all = screens();
    let i = all.iter().position(|s| !device.is_empty() && s.device.eq_ignore_ascii_case(device)).unwrap_or(0);
    (i < all.len()).then(|| all.swap_remove(i))
}

/// The screen showing this point, or the nearest one.
fn screen_at(x: i32, y: i32) -> Option<Screen> {
    let mut all = screens();
    let dist = |s: &Screen| {
        let dx = (s.full.x - x).max(x - s.full.right()).max(0) as i64;
        let dy = (s.full.y - y).max(y - s.full.bottom()).max(0) as i64;
        dx * dx + dy * dy
    };
    let i = (0..all.len()).min_by_key(|&i| dist(&all[i]))?;
    Some(all.swap_remove(i))
}

/// Where an arrangement of these folder sizes goes: its block, each folder's
/// position, the screen and the gap in pixels.
struct Placed {
    block: Rect,
    pos: Vec<(i32, i32)>,
    heights: Vec<i32>,
    screen: Screen,
}

fn place(settings: &Settings, sizes: &[(i32, i32)], screen: Screen) -> Placed {
    let gap = scale(settings.gap, screen.dpi);
    let spec = settings.arrange_spec();
    let slots = layout::slots(sizes, &spec, screen.work, gap);
    let (x, y) = layout::origin(slots.w, slots.h, &spec, screen.work, gap);
    Placed {
        block: Rect { x, y, w: slots.w, h: slots.h },
        pos: slots.pos.iter().map(|&(px, py)| (x + px, y + py)).collect(),
        heights: slots.heights,
        screen,
    }
}

/// The folders of the arrangement, in the order of the Folders list:
/// (id, window, label), leaving `skip` out.
fn members(skip: Option<u32>) -> (Settings, Vec<(u32, HWND, Rect)>, Vec<crate::config::Panel>) {
    let (settings, list) = store::snapshot();
    let members = list
        .iter()
        .filter(|p| p.arranged && Some(p.id) != skip)
        .filter_map(|p| {
            let hwnd = crate::panel_window(p.id)?;
            Some((p.id, hwnd, panel::geometry(hwnd)?.label))
        })
        .collect();
    (settings, members, list)
}

/// The frame's tab: "Arrangement · Top left · drag to move".
fn tab_text(anchor: (usize, usize), offset: (i32, i32), dragging: bool) -> String {
    let moved = if offset != (0, 0) { format!(", {:+}, {:+}", offset.0, offset.1) } else { String::new() };
    let hint = if dragging { "" } else { " · drag to move" };
    format!("Arrangement · {}{moved}{hint}", anchor_label(anchor))
}

/// Places the folders of the arrangement again and saves where they went.
/// Shows its frame while unlocked.
pub fn rearrange() {
    if BUSY.get() {
        return;
    }
    let (settings, members, list) = members(None);
    if members.is_empty() {
        ROWS.with(|r| r.borrow_mut().clear());
        frame::hide();
        return;
    }
    let Some(screen) = screen_for(&settings.arrange_screen) else { return };
    let sizes: Vec<(i32, i32)> = members.iter().map(|m| (m.2.w, m.2.h)).collect();
    let placed = place(&settings, &sizes, screen);
    let moves: Vec<(u32, i32, i32)> = members
        .iter()
        .zip(&placed.pos)
        .filter(|(m, p)| list.iter().find(|q| q.id == m.0).is_some_and(|q| (q.x, q.y) != **p))
        .map(|(m, &(x, y))| (m.0, x, y))
        .collect();
    if !moves.is_empty() {
        store::change(|s| s.set_positions(&moves));
    }
    ROWS.with(|r| *r.borrow_mut() = members.iter().map(|m| m.1).zip(placed.heights.iter().copied()).collect());
    let spec = settings.arrange_spec();
    frame::show(placed.block, &tab_text(spec.anchor, spec.offset, false), None, placed.screen.dpi);
}

/// Puts every folder in the arrangement; with a shape, it gets that shape.
pub fn arrange_all(shape: Option<&str>) {
    store::with(|s| s.backup_now());
    store::change(|s| {
        if let Some(shape) = shape {
            s.update_settings(|st| st.arrange_shape = shape.into());
        }
        let ids: Vec<u32> = s.panels.iter().map(|p| p.id).collect();
        s.set_arranged(&ids, true);
    });
}

/// Frees every folder: they stay where they are.
pub fn free_all() {
    store::change(|s| {
        let ids: Vec<u32> = s.panels.iter().map(|p| p.id).collect();
        s.set_arranged(&ids, false);
    });
}

/// The arrangement has folders (so a new folder can join it).
pub fn has_members() -> bool {
    store::with(|s| s.panels.iter().any(|p| p.arranged))
}

// ---- Dragging the frame ----

/// The frame is about to be moved from `block`: the folders fold, and their places are kept.
pub fn begin_frame_drag(block: Rect) {
    BUSY.set(true);
    crate::fold_for_move(std::ptr::null_mut());
    let (_, members, _) = members(None);
    let start = members.iter().filter_map(|m| panel::geometry(m.1).map(|g| (m.1, (g.label.x, g.label.y)))).collect();
    FRAME_DRAG.set(Some(((block.x, block.y), start)));
}

/// The frame is being moved to `block`: snaps it to an anchor of the screen it
/// is over (unless `free`), moves the folders with it, and returns where it
/// goes and the text of its tab.
pub fn frame_moving(block: Rect, free: bool) -> ((i32, i32), String) {
    let settings = store::with(|s| s.settings.clone());
    let Some(screen) = screen_at(block.x + block.w / 2, block.y + block.h / 2) else { return ((block.x, block.y), String::new()) };
    let gap = scale(settings.gap, screen.dpi);
    let (mut x, mut y) = (block.x, block.y);
    if settings.snap && !free {
        let snapped = layout::snap_block(x, y, block.w, block.h, screen.work, gap, scale(SNAP_REACH, screen.dpi));
        (x, y) = (snapped.0, snapped.1);
    }
    FRAME_DRAG.with(|d| {
        if let Some(((sx, sy), folders)) = &*d.borrow() {
            for &(hwnd, (fx, fy)) in folders {
                panel::set_label(hwnd, fx + x - sx, fy + y - sy);
            }
        }
    });
    let (anchor, offset) = layout::anchor_of(x, y, block.w, block.h, screen.work, gap);
    ((x, y), tab_text(anchor, offset, true))
}

/// The frame was dropped with the arrangement at `block`: its anchor, offset
/// and screen are saved, and the folders placed there.
pub fn end_frame_drag(block: Rect) {
    BUSY.set(false);
    FRAME_DRAG.set(None);
    let settings = store::with(|s| s.settings.clone());
    if let Some(screen) = screen_at(block.x + block.w / 2, block.y + block.h / 2) {
        let gap = scale(settings.gap, screen.dpi);
        let ((h, v), (ox, oy)) = layout::anchor_of(block.x, block.y, block.w, block.h, screen.work, gap);
        store::change(|s| {
            s.update_settings(|st| {
                st.arrange_anchor = ANCHORS[v][h].into();
                st.arrange_offset_x = ox;
                st.arrange_offset_y = oy;
                st.arrange_screen = screen.device.clone();
            })
        });
    }
    rearrange();
}

// ---- Dragging a folder over the arrangement ----

/// A folder (`id`, its label at `label`) is being moved, the mouse at `cursor`.
/// Over the frame, it takes the nearest place in the arrangement and the
/// others make room; outside, the others close the gap. Returns true while it
/// is over the frame (then it does not snap to other folders).
pub fn folder_moving(id: u32, label: Rect, cursor: POINT) -> bool {
    let (settings, others, _) = members(Some(id));
    if others.is_empty() {
        return false;
    }
    let Some(screen) = screen_for(&settings.arrange_screen) else { return false };
    BUSY.set(true);
    let dpi = screen.dpi;
    let mut sizes: Vec<(i32, i32)> = others.iter().map(|m| (m.2.w, m.2.h)).collect();
    sizes.push((label.w, label.h));
    let with_it = place(&settings, &sizes, screen);
    let pad = scale(FRAME_PAD + 6, dpi);
    let b = with_it.block;
    let inside = cursor.x >= b.x - pad && cursor.x < b.right() + pad && cursor.y >= b.y - pad && cursor.y < b.bottom() + pad;
    let spec = settings.arrange_spec();
    let text = tab_text(spec.anchor, spec.offset, false);
    let Some(screen) = screen_for(&settings.arrange_screen) else { return false };
    if inside {
        let k = layout::nearest_slot(&with_it.pos, (label.x, label.y));
        let mut order: Vec<(u32, Option<HWND>, (i32, i32))> = others.iter().map(|m| (m.0, Some(m.1), (m.2.w, m.2.h))).collect();
        order.insert(k, (id, None, (label.w, label.h)));
        let sizes: Vec<(i32, i32)> = order.iter().map(|o| o.2).collect();
        let placed = place(&settings, &sizes, screen);
        for (o, &(x, y)) in order.iter().zip(&placed.pos) {
            if let Some(hwnd) = o.1 {
                panel::set_label(hwnd, x, y);
            }
        }
        let (sx, sy) = placed.pos[k];
        frame::show(placed.block, &text, Some(Rect { x: sx, y: sy, w: label.w, h: label.h }), dpi);
        DROP.set(Some(Some(order.iter().map(|o| o.0).collect())));
    } else {
        let sizes: Vec<(i32, i32)> = others.iter().map(|m| (m.2.w, m.2.h)).collect();
        let placed = place(&settings, &sizes, screen);
        for (m, &(x, y)) in others.iter().zip(&placed.pos) {
            panel::set_label(m.1, x, y);
        }
        frame::show(placed.block, &text, None, dpi);
        DROP.set(Some(None));
    }
    inside
}

/// The folder `id` was dropped with its label at (x, y): into the arrangement
/// at its new place, out of it where it is, or just moved.
pub fn folder_dropped(id: u32, x: i32, y: i32) {
    BUSY.set(false);
    match DROP.take() {
        Some(Some(order)) => store::change(|s| {
            s.set_arranged(&[id], true);
            // Its place in the list: just before the folder after it in the
            // arrangement, or just after the one before it.
            let k = order.iter().position(|&o| o == id).unwrap_or(0);
            let rest: Vec<u32> = s.panels.iter().map(|p| p.id).filter(|&p| p != id).collect();
            let at = match (order.get(k + 1), k.checked_sub(1).and_then(|i| order.get(i))) {
                (Some(next), _) => rest.iter().position(|p| p == next),
                (None, Some(prev)) => rest.iter().position(|p| p == prev).map(|i| i + 1),
                _ => None,
            };
            if let Some(at) = at {
                s.move_panel_to(id, at);
            }
        }),
        Some(None) => store::change(|s| {
            s.set_arranged(&[id], false);
            s.update_panel(id, |p| {
                p.x = x;
                p.y = y;
            })
        }),
        None => store::change(|s| {
            s.update_panel(id, |p| {
                p.x = x;
                p.y = y;
            })
        }),
    }
    // The others are already shown where the arrangement puts them: this saves it.
    rearrange();
}
