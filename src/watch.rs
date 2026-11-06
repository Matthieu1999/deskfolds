//! New desktop shortcuts go into a folder by themselves, when the user chose
//! one (Settings > Windows integration).
//!
//! Windows tells a sleeping thread when a file appears on the desktop
//! (ReadDirectoryChangesW): nothing is checked again and again, and nothing
//! runs at all while the option is off. A new .lnk or .url file is moved into
//! the folder 2 seconds later, once its installer is done with it.
//!
//! Shortcuts installed « for all users » are on the shared desktop, which
//! usually only an administrator can change. deskfolds tries to move them like
//! the others; when Windows refuses, a click on the notification asks Windows
//! for permission, each time: a copy is made in the folder, and Windows' own
//! file operation sends the original to the Recycle Bin, with Windows' own
//! permission prompt. No code of deskfolds ever runs as administrator.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, HANDLE, HWND, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_RENAMED_NEW_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows_sys::Win32::UI::Shell::FOLDERID_PublicDesktop;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SetTimer, WM_APP};

use crate::util::wide;

/// Posted to the main window: new files appeared on a desktop.
pub const WM_APP_DESKTOP_FILE: u32 = WM_APP + 30;
/// The main window's timer that puts the new shortcuts away.
pub const PUT_AWAY_TIMER: usize = 30;
/// How long after a shortcut appears it is moved.
const WAIT: Duration = Duration::from_secs(2);

/// New files found by the watching thread, for the main thread.
static FOUND: Mutex<Vec<(PathBuf, bool)>> = Mutex::new(Vec::new());
/// Files deskfolds itself put on the desktop: never taken again.
static PUT_BACK: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Watcher {
    stop: HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe { SetEvent(self.stop) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe { CloseHandle(self.stop) };
    }
}

/// A new shortcut waiting to be put away, and since when.
struct Waiting {
    path: PathBuf,
    shared: bool,
    since: Instant,
}

/// A shortcut of the shared desktop copied into a folder while Windows is
/// asked to move the original to the Recycle Bin.
struct Asking {
    shared: PathBuf,
    copy: PathBuf,
    panel: u32,
}

thread_local! {
    static WATCHER: RefCell<Option<Watcher>> = const { RefCell::new(None) };
    static WAITING: RefCell<Vec<Waiting>> = const { RefCell::new(Vec::new()) };
    /// Shortcuts of the shared desktop Windows refused to move: (file, folder).
    static REFUSED: RefCell<Vec<(PathBuf, u32)>> = const { RefCell::new(Vec::new()) };
    /// Windows is asking for permission right now.
    static ASKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// While a shortcut is dragged out of a folder (and a moment after), what
    /// appears on the desktop is that shortcut.
    static OUTBOUND: RefCell<Option<(Instant, Option<Instant>)>> = const { RefCell::new(None) };
}

/// The desktops watched: the user's and the shared one. A test copy (another
/// APPDATA) watches folders of its own, so it never takes the real shortcuts.
pub fn desktops() -> (Option<PathBuf>, Option<PathBuf>) {
    if crate::integration::usual_copy() {
        (crate::dragdrop::desktop_dir(), shared_desktop())
    } else {
        let root = crate::config::app_dir().parent().map(Path::to_path_buf);
        (root.as_ref().map(|r| r.join("Desktop")), root.map(|r| r.join("Public Desktop")))
    }
}

fn shared_desktop() -> Option<PathBuf> {
    crate::dragdrop::known_folder(&FOLDERID_PublicDesktop)
}

/// The file is on the shared desktop (of this copy, see `desktops`).
pub fn on_shared_desktop(path: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).map(|c| c.to_string_lossy().to_lowercase()).unwrap_or_default();
    match (desktops().1, path.parent()) {
        (Some(shared), Some(parent)) => !canon(&shared).is_empty() && canon(&shared) == canon(parent),
        _ => false,
    }
}

/// Shortcuts of the shared desktop dropped on a folder: moved like the
/// others when Windows allows it; otherwise Windows is asked at once, as when
/// the notification is clicked. Returns the errors. Only shortcuts are given.
pub fn move_shared_now(main: HWND, panel: u32, files: &[String]) -> Vec<String> {
    let dir = crate::store::with(|s| s.storage_of(panel));
    let (mut moved, mut refused, mut errors) = (Vec::new(), Vec::new(), Vec::new());
    for file in files.iter().filter(|f| is_shortcut(Path::new(f))) {
        match crate::dragdrop::move_file(Path::new(file), &dir) {
            Ok(dest) => moved.push(dest.to_string_lossy().to_string()),
            Err((_, code)) if code == ERROR_ACCESS_DENIED => refused.push((PathBuf::from(file), panel)),
            Err((e, _)) => errors.push(e),
        }
    }
    if !moved.is_empty() {
        crate::store::change(|s| s.add_files(panel, usize::MAX, &moved, true));
    }
    if !refused.is_empty() {
        REFUSED.with(|r| *r.borrow_mut() = refused);
        ask_for_shared(main);
    }
    errors
}

pub fn is_shortcut(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk") || e.eq_ignore_ascii_case("url"))
}

/// deskfolds put this file on the desktop: it is not a new shortcut.
pub fn note_put_back(path: &Path) {
    if let Ok(mut list) = PUT_BACK.lock() {
        list.push(path.to_string_lossy().to_lowercase());
        // Only the last few matter: they are new files a moment ago.
        let extra = list.len().saturating_sub(64);
        list.drain(..extra);
    }
}

fn was_put_back(path: &Path) -> bool {
    let key = path.to_string_lossy().to_lowercase();
    PUT_BACK.lock().is_ok_and(|l| l.contains(&key))
}

/// A shortcut is being dragged out of a folder (`true`), or that drag ended.
pub fn dragging_out(on: bool) {
    OUTBOUND.with(|o| {
        let mut o = o.borrow_mut();
        if on {
            *o = Some((Instant::now(), None));
        } else if let Some((start, _)) = *o {
            *o = Some((start, Some(Instant::now())));
        }
    });
}

fn during_drag_out(at: Instant) -> bool {
    OUTBOUND.with(|o| match *o.borrow() {
        Some((start, None)) => at >= start,
        Some((start, Some(end))) => at >= start && at <= end + Duration::from_secs(3),
        None => false,
    })
}

/// Starts or stops watching, as the settings say.
pub fn sync(main: HWND) {
    let wanted = crate::store::with(|s| !s.settings.new_shortcuts_folder.is_empty());
    let running = WATCHER.with(|w| w.borrow().is_some());
    if wanted == running {
        return;
    }
    if !wanted {
        WATCHER.with(|w| w.borrow_mut().take());
        WAITING.with(|w| w.borrow_mut().clear());
        return;
    }
    let (mine, shared) = desktops();
    let dirs: Vec<(PathBuf, bool)> = [(mine, false), (shared, true)].into_iter().filter_map(|(d, s)| d.map(|d| (d, s))).collect();
    let stop = unsafe { CreateEventW(null(), 1, 0, null()) };
    if stop.is_null() {
        return;
    }
    let (main_raw, stop_raw) = (main as isize, stop as isize);
    let thread = std::thread::Builder::new()
        .name("desktop watcher".into())
        .spawn(move || watch_loop(&dirs, main_raw as HWND, stop_raw as HANDLE))
        .ok();
    WATCHER.with(|w| *w.borrow_mut() = Some(Watcher { stop, thread }));
}

/// One watched folder of the thread.
struct Watched {
    dir: PathBuf,
    shared: bool,
    handle: HANDLE,
    event: HANDLE,
    /// Boxed: Windows writes into it while the read is pending, so it must
    /// never move.
    overlapped: Box<OVERLAPPED>,
    buffer: Vec<u32>,
}

impl Watched {
    fn read(&mut self) -> bool {
        *self.overlapped = unsafe { std::mem::zeroed() };
        self.overlapped.hEvent = self.event;
        unsafe {
            ReadDirectoryChangesW(
                self.handle,
                self.buffer.as_mut_ptr() as *mut c_void,
                (self.buffer.len() * 4) as u32,
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME,
                null_mut(),
                &mut *self.overlapped,
                None,
            ) != 0
        }
    }
}

fn watch_loop(dirs: &[(PathBuf, bool)], main: HWND, stop: HANDLE) {
    let mut watched: Vec<Watched> = Vec::new();
    for (dir, shared) in dirs {
        let path = wide(&dir.to_string_lossy());
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        let mut w = Watched { dir: dir.clone(), shared: *shared, handle, event, overlapped: Box::new(unsafe { std::mem::zeroed() }), buffer: vec![0u32; 4096] };
        if !event.is_null() && w.read() {
            watched.push(w);
        } else {
            unsafe {
                CloseHandle(handle);
                if !event.is_null() {
                    CloseHandle(event);
                }
            }
        }
    }
    let mut events: Vec<HANDLE> = watched.iter().map(|w| w.event).collect();
    events.push(stop);
    loop {
        let index = unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, INFINITE) }.wrapping_sub(WAIT_OBJECT_0) as usize;
        if index >= watched.len() {
            break;
        }
        let w = &mut watched[index];
        let mut bytes = 0u32;
        let done = unsafe { GetOverlappedResult(w.handle, &*w.overlapped, &mut bytes, 0) } != 0;
        let mut found = Vec::new();
        if done && bytes > 0 {
            for name in notified_names(&w.buffer, bytes as usize) {
                let path = w.dir.join(name);
                if is_shortcut(&path) {
                    found.push((path, w.shared));
                }
            }
        }
        if !w.read() {
            break;
        }
        if !found.is_empty() {
            if let Ok(mut list) = FOUND.lock() {
                list.extend(found);
            }
            unsafe { PostMessageW(main, WM_APP_DESKTOP_FILE, 0, 0) };
        }
    }
    for w in watched {
        unsafe {
            CancelIoEx(w.handle, &*w.overlapped);
            let mut bytes = 0;
            GetOverlappedResult(w.handle, &*w.overlapped, &mut bytes, 1);
            CloseHandle(w.handle);
            CloseHandle(w.event);
        }
    }
}

/// The names of the files added (or renamed to) in a ReadDirectoryChangesW
/// buffer: entries of NextEntryOffset, Action, FileNameLength (u32 each), then
/// the name. Read field by field, never past `bytes`.
fn notified_names(buffer: &[u32], bytes: usize) -> Vec<String> {
    const HEADER: usize = 12;
    let mut names = Vec::new();
    let bytes = bytes.min(buffer.len() * 4);
    let base = buffer.as_ptr() as *const u8;
    let u32_at = |at: usize| unsafe { std::ptr::read_unaligned(base.add(at) as *const u32) };
    let mut offset = 0usize;
    while offset + HEADER <= bytes {
        let (next, action, len) = (u32_at(offset) as usize, u32_at(offset + 4), u32_at(offset + 8) as usize / 2);
        if offset + HEADER + len * 2 > bytes {
            break;
        }
        if action == FILE_ACTION_ADDED || action == FILE_ACTION_RENAMED_NEW_NAME {
            let units: Vec<u16> =
                (0..len).map(|k| unsafe { std::ptr::read_unaligned(base.add(offset + HEADER + 2 * k) as *const u16) }).collect();
            names.push(String::from_utf16_lossy(&units));
        }
        if next == 0 || next % 4 != 0 {
            break;
        }
        offset += next;
    }
    names
}

/// The watching thread found files: they are put away 2 seconds later.
pub fn on_found(main: HWND) {
    let found: Vec<(PathBuf, bool)> = FOUND.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default();
    let now = Instant::now();
    let mut any = false;
    WAITING.with(|w| {
        let mut w = w.borrow_mut();
        for (path, shared) in found {
            if during_drag_out(now) || was_put_back(&path) || w.iter().any(|x| x.path == path) {
                continue;
            }
            w.push(Waiting { path, shared, since: now });
            any = true;
        }
    });
    if any {
        unsafe { SetTimer(main, PUT_AWAY_TIMER, WAIT.as_millis() as u32, None) };
    }
}

/// The folder new shortcuts go into: the one of that name, made if needed.
fn target_folder() -> Option<u32> {
    let name = crate::store::with(|s| s.settings.new_shortcuts_folder.clone());
    if name.is_empty() {
        return None;
    }
    if let Some(id) = crate::store::with(|s| s.panel_named(&name)) {
        return Some(id);
    }
    Some(crate::new_folder_named(&name))
}

/// The timer: puts away the shortcuts that have waited long enough.
pub fn on_timer(main: HWND) {
    unsafe { KillTimer(main, PUT_AWAY_TIMER) };
    let ready: Vec<Waiting> = WAITING.with(|w| {
        let mut w = w.borrow_mut();
        let (ready, later): (Vec<Waiting>, Vec<Waiting>) = w.drain(..).partition(|x| x.since.elapsed() + Duration::from_millis(100) >= WAIT);
        *w = later;
        ready
    });
    if WAITING.with(|w| !w.borrow().is_empty()) {
        unsafe { SetTimer(main, PUT_AWAY_TIMER, 500, None) };
    }
    let ready: Vec<Waiting> = ready.into_iter().filter(|x| x.path.is_file() && !was_put_back(&x.path)).collect();
    if ready.is_empty() {
        return;
    }
    let Some(panel) = target_folder() else { return };
    let (dir, title) = crate::store::with(|s| (s.storage_of(panel), s.panel(panel).map(|p| p.title.clone()).unwrap_or_default()));
    let mut moved = Vec::new();
    let mut refused = Vec::new();
    let mut errors = Vec::new();
    for w in ready {
        match crate::dragdrop::move_file(&w.path, &dir) {
            Ok(dest) => moved.push(dest.to_string_lossy().to_string()),
            Err((_, code)) if w.shared && code == ERROR_ACCESS_DENIED => refused.push(w.path),
            Err((e, _)) => errors.push(e),
        }
    }
    let folder = crate::ui::short_name(&title, 40);
    if !moved.is_empty() {
        let names: Vec<String> = moved.iter().map(|p| crate::dragdrop::display_name(Path::new(p))).collect();
        // A link to a file of the storage folder is a file moved into it.
        crate::store::change(|s| s.add_files(panel, usize::MAX, &moved, true));
        let text = match names.as_slice() {
            [one] => format!("« {} » moved into « {folder} ». Remove it from the folder to put it back on the desktop.", crate::ui::short_name(one, 60)),
            many => format!("{} shortcuts moved into « {folder} ». Remove one from the folder to put it back on the desktop.", many.len()),
        };
        crate::notify(main, "New shortcut put away", &text, None);
    }
    if !errors.is_empty() {
        crate::notify(main, "A new shortcut was not moved", &errors.join("\n"), None);
    }
    // Last: a later notification would replace this question.
    if !refused.is_empty() {
        let text = match refused.as_slice() {
            [one] => format!(
                "« {} » is a shortcut for all users. Click here to move it into « {folder} ». Windows will ask for your permission.",
                crate::ui::short_name(&crate::dragdrop::display_name(one), 60)
            ),
            many => format!("{} shortcuts are for all users. Click here to move them into « {folder} ». Windows will ask for your permission.", many.len()),
        };
        REFUSED.with(|r| *r.borrow_mut() = refused.into_iter().map(|p| (p, panel)).collect());
        crate::notify(main, "Shortcut for all users", &text, Some(crate::Balloon::Shared));
    }
}

/// The notification about shortcuts for all users went away unclicked.
pub fn forget_refused() {
    REFUSED.with(|r| r.borrow_mut().clear());
}

/// The notification about shortcuts for all users was clicked (or they were
/// dropped on a folder): each is copied into its folder, then Windows' own
/// file operation sends the original to the Recycle Bin, asking for permission
/// with Windows' own prompt. A shortcut still there afterwards (refused, or
/// "No") stays where it was, and its copy is taken away again.
pub fn ask_for_shared(main: HWND) {
    // One question at a time: the others wait for their own click.
    if ASKING.get() {
        return;
    }
    let refused: Vec<(PathBuf, u32)> = REFUSED.with(|r| std::mem::take(&mut *r.borrow_mut()));
    let mut asking = Vec::new();
    for (shared, panel) in refused {
        if !shared.is_file() {
            continue;
        }
        let dir = crate::store::with(|s| s.storage_of(panel));
        if let Ok(copy) = crate::dragdrop::copy_into(&shared, &dir) {
            asking.push(Asking { shared, copy, panel });
        }
    }
    if asking.is_empty() {
        return;
    }
    let originals: Vec<PathBuf> = asking.iter().map(|a| a.shared.clone()).collect();
    ASKING.set(true);
    crate::shellmenu::recycle_with_windows(&originals, true);
    ASKING.set(false);
    let mut kept = Vec::new();
    for a in asking {
        if a.shared.exists() {
            // Our own copy goes; the original stays.
            let _ = std::fs::remove_file(&a.copy);
            kept.push(crate::dragdrop::display_name(&a.shared));
        } else {
            let copy = a.copy.to_string_lossy().to_string();
            crate::store::change(|s| s.add_files(a.panel, usize::MAX, &[copy], true));
        }
    }
    if !kept.is_empty() {
        let names: Vec<String> = kept.iter().map(|n| format!("« {} »", crate::ui::short_name(n, 60))).collect();
        let text = format!("{} stays on the desktop: Windows did not allow the move.", names.join(", "));
        crate::notify(main, "Shortcut for all users", &text, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_and_renamed_files_are_read_from_the_notifications() {
        fn entry(action: u32, name: &str, last: bool) -> Vec<u8> {
            let units: Vec<u16> = name.encode_utf16().collect();
            let mut body = Vec::new();
            body.extend(action.to_le_bytes());
            body.extend(((units.len() * 2) as u32).to_le_bytes());
            for u in units {
                body.extend(u.to_le_bytes());
            }
            while (body.len() + 4) % 4 != 0 {
                body.push(0);
            }
            let next = if last { 0 } else { (body.len() + 4) as u32 };
            let mut v = next.to_le_bytes().to_vec();
            v.extend(body);
            v
        }
        // "a.lnk" added, "gone.lnk" removed (2), "b.url" renamed to.
        let mut all = entry(FILE_ACTION_ADDED, "a.lnk", false);
        all.extend(entry(2, "gone.lnk", false));
        all.extend(entry(FILE_ACTION_RENAMED_NEW_NAME, "b.url", true));
        let mut buffer = vec![0u32; all.len().div_ceil(4)];
        unsafe { std::ptr::copy_nonoverlapping(all.as_ptr(), buffer.as_mut_ptr() as *mut u8, all.len()) };
        assert_eq!(notified_names(&buffer, all.len()), ["a.lnk", "b.url"]);
    }
}
