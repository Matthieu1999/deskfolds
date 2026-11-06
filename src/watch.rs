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
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_FILE_NAME, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows_sys::Win32::UI::Shell::FOLDERID_PublicDesktop;
use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, PostMessageW, SetTimer, WM_APP};

use crate::util::wide;

/// Posted to the main window: new files appeared on a desktop.
pub const WM_APP_DESKTOP_FILE: u32 = WM_APP + 30;
/// Posted to the main window: ask Windows about the shortcuts for all users
/// dropped on a folder, once the drop is over.
pub const WM_APP_ASK_SHARED: u32 = WM_APP + 31;
/// How long an empty new shortcut (one Windows' Create Shortcut window is
/// still to fill) is waited for.
const EMPTY_WAIT: Duration = Duration::from_secs(10 * 60);
/// The main window's timer that puts the new shortcuts away.
pub const PUT_AWAY_TIMER: usize = 30;
/// How long after a shortcut appears it is moved.
const WAIT: Duration = Duration::from_secs(2);

/// A file the watching thread found: its path, its old path when it was
/// renamed, and whether it is on the shared desktop.
type Found = (PathBuf, Option<PathBuf>, bool);

/// New files found by the watching thread, for the main thread.
static FOUND: Mutex<Vec<Found>> = Mutex::new(Vec::new());
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
    /// Shortcuts of the shared desktop Windows refused to move, waiting for a
    /// click on their notification (or the tray menu): (file, folder).
    static REFUSED: RefCell<Vec<(PathBuf, u32)>> = const { RefCell::new(Vec::new()) };
    /// Shortcuts of the shared desktop dropped on a folder: asked about at once.
    static DROPPED: RefCell<Vec<(PathBuf, u32)>> = const { RefCell::new(Vec::new()) };
    /// Windows is asking for permission right now.
    static ASKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The copies made in folders while Windows is asked: not new files.
    static COPIES: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
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

/// A link elsewhere (a junction, a symbolic link), or missing.
fn reparse_or_missing(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(path).map_or(true, |m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// The shortcut on the shared desktop (of this copy, see `desktops`) that
/// `path` names, as a plain file directly in that folder, by its own long
/// name. However `path` was given (another way to the same folder, a short
/// name), only this path is ever handed to Windows.
fn shared_shortcut(path: &Path) -> Option<PathBuf> {
    let dir = desktops().1?;
    if reparse_or_missing(&dir) {
        return None;
    }
    let canon = std::fs::canonicalize(path).ok()?;
    if canon.parent()? != std::fs::canonicalize(&dir).ok()?.as_path() {
        return None;
    }
    let real = dir.join(canon.file_name()?);
    (is_shortcut(&real) && real.is_file() && !reparse_or_missing(&real)).then_some(real)
}

/// The file is a shortcut on the shared desktop.
pub fn on_shared_desktop(path: &Path) -> bool {
    shared_shortcut(path).is_some()
}

/// A copy made in a folder while Windows is asked about its original.
pub fn is_pending_copy(path: &Path) -> bool {
    COPIES.with(|c| c.borrow().iter().any(|p| p == path))
}

/// Adds shortcuts of the shared desktop to the ones Windows will be asked about.
fn add_refused(more: Vec<(PathBuf, u32)>) {
    REFUSED.with(|r| {
        let mut r = r.borrow_mut();
        for (path, panel) in more {
            if !r.iter().any(|(p, _)| *p == path) {
                r.push((path, panel));
            }
        }
    });
}

/// How many shortcuts for all users wait for their question (tray menu).
pub fn refused_waiting() -> usize {
    REFUSED.with(|r| r.borrow().len())
}

/// The shared desktop is a folder only Windows or an administrator owns. A
/// folder of the user (a test copy's) never gets Windows' permission prompt:
/// someone could change it while the prompt is open.
fn owned_by_windows(dir: &Path) -> bool {
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};
    const OWNERS: [&str; 3] = ["S-1-5-18", "S-1-5-32-544", "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"];
    let path = wide(&dir.to_string_lossy());
    unsafe {
        let (mut owner, mut descriptor): (PSID, PSECURITY_DESCRIPTOR) = (null_mut(), null_mut());
        if GetNamedSecurityInfoW(path.as_ptr(), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, &mut owner, null_mut(), null_mut(), null_mut(), &mut descriptor) != 0 {
            return false;
        }
        let mut text: *mut u16 = null_mut();
        let sid = if !owner.is_null() && ConvertSidToStringSidW(owner, &mut text) != 0 && !text.is_null() {
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text as HLOCAL);
            Some(sid)
        } else {
            None
        };
        LocalFree(descriptor as HLOCAL);
        sid.is_some_and(|s| OWNERS.contains(&s.as_str()))
    }
}

/// The notification that asks to click it, for the shortcuts waiting.
fn notify_refused(main: HWND) {
    let (count, first, panel) = REFUSED.with(|r| {
        let r = r.borrow();
        (r.len(), r.first().map(|(p, _)| crate::dragdrop::display_name(p)), r.first().map(|(_, id)| *id))
    });
    let (Some(first), Some(panel)) = (first, panel) else { return };
    let title = crate::store::with(|s| s.panel(panel).map(|p| p.title.clone()).unwrap_or_default());
    let folder = crate::ui::short_name(&title, 40);
    let text = if count == 1 {
        format!(
            "« {} » is a shortcut for all users. Click here to move it into « {folder} ». Windows will ask for your permission.",
            crate::ui::short_name(&first, 60)
        )
    } else {
        format!("{count} shortcuts are for all users. Click here to move them. Windows will ask for your permission.")
    };
    crate::notify(main, "Shortcut for all users", &text, Some(crate::Balloon::Shared));
}

/// Shortcuts of the shared desktop dropped on a folder: moved like the
/// others when Windows allows it; otherwise Windows is asked at once, as when
/// the notification is clicked. Returns the errors. Only shortcuts are given.
pub fn move_shared_now(main: HWND, panel: u32, files: &[String]) -> Vec<String> {
    let dir = crate::store::with(|s| s.storage_of(panel));
    let (mut moved, mut refused, mut errors) = (Vec::new(), Vec::new(), Vec::new());
    for file in files.iter().filter_map(|f| shared_shortcut(Path::new(f))) {
        match crate::dragdrop::move_file(&file, &dir) {
            Ok(dest) => moved.push(dest.to_string_lossy().to_string()),
            Err((_, code)) if code == ERROR_ACCESS_DENIED => refused.push((file, panel)),
            Err((e, _)) => errors.push(e),
        }
    }
    if !moved.is_empty() {
        crate::store::change(|s| s.add_files(panel, usize::MAX, &moved, true));
    }
    if !refused.is_empty() {
        // Only these, the ones dropped: not the ones waiting for their notification.
        DROPPED.with(|d| d.borrow_mut().extend(refused));
        // Once the drop is over: Windows' prompt must not hold up the drag.
        unsafe { PostMessageW(main, WM_APP_ASK_SHARED, 0, 0) };
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
    /// The old name of a rename whose new name is still to come.
    renamed_from: Option<String>,
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
        let mut w = Watched {
            dir: dir.clone(),
            shared: *shared,
            renamed_from: None,
            handle,
            event,
            overlapped: Box::new(unsafe { std::mem::zeroed() }),
            buffer: vec![0u32; 4096],
        };
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
            for (name, old) in notified_names(&w.buffer, bytes as usize, &mut w.renamed_from) {
                let path = w.dir.join(name);
                if is_shortcut(&path) {
                    found.push((path, old.map(|o| w.dir.join(o)), w.shared));
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

/// The files added, or renamed to, in a ReadDirectoryChangesW buffer: their
/// name, and their old name when renamed. Entries are NextEntryOffset, Action,
/// FileNameLength (u32 each), then the name; read field by field, never past
/// `bytes`. `renamed_from` carries a rename's old name over to the next buffer.
fn notified_names(buffer: &[u32], bytes: usize, renamed_from: &mut Option<String>) -> Vec<(String, Option<String>)> {
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
        let name = || {
            let units: Vec<u16> =
                (0..len).map(|k| unsafe { std::ptr::read_unaligned(base.add(offset + HEADER + 2 * k) as *const u16) }).collect();
            String::from_utf16_lossy(&units)
        };
        match action {
            FILE_ACTION_RENAMED_OLD_NAME => *renamed_from = Some(name()),
            FILE_ACTION_RENAMED_NEW_NAME => names.push((name(), renamed_from.take())),
            FILE_ACTION_ADDED => {
                *renamed_from = None;
                names.push((name(), None));
            }
            _ => *renamed_from = None,
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
    let found: Vec<Found> = FOUND.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default();
    let now = Instant::now();
    let mut any = false;
    WAITING.with(|w| {
        let mut w = w.borrow_mut();
        for (path, old, shared) in found {
            if let Some(old) = old {
                // A new shortcut renamed while it waits (Create Shortcut names
                // it last): it keeps waiting under its new name.
                if let Some(waiting) = w.iter_mut().find(|x| x.path == old) {
                    waiting.path = path;
                    continue;
                }
                // A shortcut the user renamed on the desktop is not a new one.
                if is_shortcut(&old) {
                    continue;
                }
            }
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
    // An empty shortcut is still being made (Windows' Create Shortcut window):
    // it waits until it is filled in.
    let empty = |p: &Path| std::fs::metadata(p).is_ok_and(|m| m.len() == 0);
    let (unfinished, ready): (Vec<Waiting>, Vec<Waiting>) = ready.into_iter().partition(|x| empty(&x.path));
    let unfinished: Vec<Waiting> = unfinished.into_iter().filter(|x| x.since.elapsed() < EMPTY_WAIT).collect();
    if !unfinished.is_empty() {
        WAITING.with(|w| w.borrow_mut().extend(unfinished));
        unsafe { SetTimer(main, PUT_AWAY_TIMER, 1000, None) };
    }
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
    // Last: a later notification would hide this question.
    let refused: Vec<(PathBuf, u32)> = refused.into_iter().filter_map(|p| shared_shortcut(&p)).map(|p| (p, panel)).collect();
    if !refused.is_empty() {
        add_refused(refused);
        notify_refused(main);
    }
}

/// The notification about shortcuts for all users was clicked (or they were
/// dropped on a folder): each is copied into its folder, then Windows' own
/// file operation sends the original to the Recycle Bin, asking for permission
/// with Windows' own prompt. A shortcut still there afterwards (refused, or
/// "No") stays where it was, and its copy is taken away again.
pub fn ask_for_shared(main: HWND) {
    if !ASKING.get() {
        let refused: Vec<(PathBuf, u32)> = REFUSED.with(|r| std::mem::take(&mut *r.borrow_mut()));
        ask_windows(main, refused);
    }
}

/// Shortcuts for all users dropped on a folder: Windows is asked about these
/// only, once the drop is over.
pub fn ask_dropped(main: HWND) {
    if !ASKING.get() {
        let dropped: Vec<(PathBuf, u32)> = DROPPED.with(|d| std::mem::take(&mut *d.borrow_mut()));
        ask_windows(main, dropped);
    }
}

fn ask_windows(main: HWND, list: Vec<(PathBuf, u32)>) {
    let waiting_before = refused_waiting();
    let mut asking = Vec::new();
    let mut kept = Vec::new();
    for (shared, panel) in list {
        // Checked again: the file and its folder, as they are now.
        let Some(shared) = shared_shortcut(&shared) else { continue };
        if !shared.parent().is_some_and(owned_by_windows) {
            kept.push(crate::dragdrop::display_name(&shared));
            continue;
        }
        let Some(dir) = crate::store::with(|s| s.panel(panel).is_some().then(|| s.storage_of(panel))) else { continue };
        if let Ok(copy) = crate::dragdrop::copy_into(&shared, &dir) {
            asking.push(Asking { shared, copy, panel });
        }
    }
    if !asking.is_empty() {
        let originals: Vec<PathBuf> = asking.iter().map(|a| a.shared.clone()).collect();
        ASKING.set(true);
        COPIES.with(|c| c.borrow_mut().extend(asking.iter().map(|a| a.copy.clone())));
        // Not owned by the hidden main window: Windows' dialog gets a taskbar button.
        crate::shellmenu::recycle_with_windows(null_mut(), &originals, true);
        ASKING.set(false);
        let mut lost = Vec::new();
        for a in asking {
            if a.shared.exists() {
                // Our own copy goes; the original stays.
                let _ = std::fs::remove_file(&a.copy);
                kept.push(crate::dragdrop::display_name(&a.shared));
            } else if crate::store::with(|s| s.panel(a.panel).is_some()) {
                let copy = a.copy.to_string_lossy().to_string();
                crate::store::change(|s| s.add_files(a.panel, usize::MAX, &[copy], true));
            } else {
                // Its folder was deleted meanwhile: the shortcut goes on the desktop.
                let back = crate::dragdrop::desktop_dir().map(|d| crate::dragdrop::move_into(&a.copy, &d));
                if !matches!(back, Some(Ok(_))) {
                    lost.push(a.copy.display().to_string());
                }
            }
        }
        COPIES.with(|c| c.borrow_mut().clear());
        if !lost.is_empty() {
            let text = format!("Its folder was deleted. The shortcut is kept here: {}", lost.join(", "));
            crate::notify(main, "Shortcut for all users", &text, None);
        }
    }
    if !kept.is_empty() {
        let names: Vec<String> = kept.iter().map(|n| format!("« {} »", crate::ui::short_name(n, 60))).collect();
        let text = format!("{} stays on the desktop: Windows did not allow the move.", names.join(", "));
        crate::notify(main, "Shortcut for all users", &text, None);
    }
    // Shortcuts that came while Windows was asking get their own question;
    // shortcuts dropped meanwhile are asked about now.
    if refused_waiting() > waiting_before {
        notify_refused(main);
    }
    if DROPPED.with(|d| !d.borrow().is_empty()) {
        unsafe { PostMessageW(main, WM_APP_ASK_SHARED, 0, 0) };
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
        // "a.lnk" added, "gone.lnk" removed (2), "x.tmp" renamed to "b.url",
        // and "old.lnk" renamed to "new.lnk" (a shortcut renamed: not new).
        let mut all = entry(FILE_ACTION_ADDED, "a.lnk", false);
        all.extend(entry(2, "gone.lnk", false));
        all.extend(entry(FILE_ACTION_RENAMED_OLD_NAME, "x.tmp", false));
        all.extend(entry(FILE_ACTION_RENAMED_NEW_NAME, "b.url", false));
        all.extend(entry(FILE_ACTION_RENAMED_OLD_NAME, "old.lnk", false));
        all.extend(entry(FILE_ACTION_RENAMED_NEW_NAME, "new.lnk", true));
        let mut buffer = vec![0u32; all.len().div_ceil(4)];
        unsafe { std::ptr::copy_nonoverlapping(all.as_ptr(), buffer.as_mut_ptr() as *mut u8, all.len()) };
        let mut from = None;
        let names = notified_names(&buffer, all.len(), &mut from);
        let owned = |a: &str, b: Option<&str>| (a.to_string(), b.map(String::from));
        assert_eq!(names, [owned("a.lnk", None), owned("b.url", Some("x.tmp")), owned("new.lnk", Some("old.lnk"))]);
        // A rename's old name at the end of one buffer, its new name in the next.
        let old = entry(FILE_ACTION_RENAMED_OLD_NAME, "New shortcut.lnk", true);
        let mut first = vec![0u32; old.len().div_ceil(4)];
        unsafe { std::ptr::copy_nonoverlapping(old.as_ptr(), first.as_mut_ptr() as *mut u8, old.len()) };
        assert!(notified_names(&first, old.len(), &mut from).is_empty());
        let new = entry(FILE_ACTION_RENAMED_NEW_NAME, "Paint.lnk", true);
        let mut second = vec![0u32; new.len().div_ceil(4)];
        unsafe { std::ptr::copy_nonoverlapping(new.as_ptr(), second.as_mut_ptr() as *mut u8, new.len()) };
        assert_eq!(notified_names(&second, new.len(), &mut from), [owned("Paint.lnk", Some("New shortcut.lnk"))]);
    }

    #[test]
    fn only_folders_windows_owns_get_its_permission_prompt() {
        assert!(owned_by_windows(Path::new(&crate::util::expand_env("%SystemRoot%"))));
        let mine = std::env::temp_dir().join(format!("deskfolds-owner-{}", std::process::id()));
        std::fs::create_dir_all(&mine).unwrap();
        assert!(!owned_by_windows(&mine));
        std::fs::remove_dir_all(&mine).unwrap();
    }
}
