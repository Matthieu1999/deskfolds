//! Windows drag and drop: an OLE drop target for the panels, dragging a
//! shortcut out with the shell's own drag loop, and the file moves behind it.
//! windows-sys has no COM interfaces, so the few vtables needed are declared here.

use std::cell::Cell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{DRAGDROP_S_DROP, E_NOINTERFACE, GetLastError, HWND, POINT, POINTL, S_OK};
use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_COPY_ALLOWED, MoveFileExW};
use windows_sys::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, FORMATETC, STGMEDIUM};
use windows_sys::Win32::System::Ole::{
    CF_HDROP, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows_sys::Win32::UI::Shell::{
    DragQueryFileW, FOLDERID_Desktop, FOLDERID_Documents, ILClone, ILCreateFromPathW, ILFindLastID, ILFree, ILRemoveLastID,
    SHCreateDataObject, SHDoDragDrop,
    SHGetKnownFolderPath,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GA_ROOT, GetAncestor, GetClassNameW, GetCursorPos, WindowFromPoint};
use windows_sys::core::{GUID, HRESULT};

use crate::util::wide;

const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_IDROPTARGET: GUID = GUID::from_u128(0x00000122_0000_0000_c000_000000000046);
const IID_IDATAOBJECT: GUID = GUID::from_u128(0x0000010e_0000_0000_c000_000000000046);
const CLSID_DRAGDROPHELPER: GUID = GUID::from_u128(0x4657278a_411b_11d2_839a_00c04fd918d0);
const IID_IDROPTARGETHELPER: GUID = GUID::from_u128(0x4657278b_411b_11d2_839a_00c04fd918d0);
const DVASPECT_CONTENT: u32 = 1;
const TYMED_HGLOBAL: u32 = 1;

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

#[repr(C)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

unsafe fn release(object: *mut c_void) {
    if !object.is_null() {
        unsafe { ((**(object as *const *const UnknownVtbl)).release)(object) };
    }
}

/// The start of IDataObject's vtable: only the methods used here.
#[repr(C)]
struct DataObjectVtbl {
    base: UnknownVtbl,
    get_data: unsafe extern "system" fn(*mut c_void, *const FORMATETC, *mut STGMEDIUM) -> HRESULT,
    get_data_here: usize,
    query_get_data: usize,
}

/// IDropTargetHelper draws the shell's drag image over our windows.
#[repr(C)]
struct DropTargetHelperVtbl {
    base: UnknownVtbl,
    drag_enter: unsafe extern "system" fn(*mut c_void, HWND, *mut c_void, *const POINT, u32) -> HRESULT,
    drag_leave: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    drag_over: unsafe extern "system" fn(*mut c_void, *const POINT, u32) -> HRESULT,
    drop: unsafe extern "system" fn(*mut c_void, *mut c_void, *const POINT, u32) -> HRESULT,
}

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP, ptd: null_mut(), dwAspect: DVASPECT_CONTENT, lindex: -1, tymed: TYMED_HGLOBAL }
}

/// Some data objects (the shell's own) answer QueryGetData wrongly for CF_HDROP,
/// so this asks for the files themselves.
fn has_files(data: *mut c_void) -> bool {
    !dropped_files(data).is_empty()
}

fn dropped_files(data: *mut c_void) -> Vec<String> {
    let format = hdrop_format();
    let mut medium: STGMEDIUM = unsafe { std::mem::zeroed() };
    let mut files = Vec::new();
    unsafe {
        if ((**(data as *const *const DataObjectVtbl)).get_data)(data, &format, &mut medium) != S_OK {
            return files;
        }
        let hdrop = medium.u.hGlobal;
        let count = DragQueryFileW(hdrop, u32::MAX, null_mut(), 0);
        for i in 0..count {
            let len = DragQueryFileW(hdrop, i, null_mut(), 0);
            let mut buf = vec![0u16; len as usize + 1];
            DragQueryFileW(hdrop, i, buf.as_mut_ptr(), buf.len() as u32);
            files.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        ReleaseStgMedium(&mut medium);
    }
    files
}

#[repr(C)]
struct DropTargetVtbl {
    base: UnknownVtbl,
    drag_enter: unsafe extern "system" fn(*mut DropTarget, *mut c_void, u32, POINTL, *mut u32) -> HRESULT,
    drag_over: unsafe extern "system" fn(*mut DropTarget, u32, POINTL, *mut u32) -> HRESULT,
    drag_leave: unsafe extern "system" fn(*mut DropTarget) -> HRESULT,
    drop: unsafe extern "system" fn(*mut DropTarget, *mut c_void, u32, POINTL, *mut u32) -> HRESULT,
}

/// What a window does with files dragged over it. Each function gets the
/// window, the mouse position in screen pixels, the key state and the effects
/// the drag source allows; it returns the effect it chose.
pub struct DropHandler {
    pub over: fn(HWND, POINT, u32, u32) -> u32,
    pub leave: fn(HWND),
    pub drop: fn(HWND, Vec<String>, POINT, u32, u32) -> u32,
}

#[repr(C)]
struct DropTarget {
    vtbl: *const DropTargetVtbl,
    refs: Cell<u32>,
    hwnd: HWND,
    handler: &'static DropHandler,
    accepts: Cell<bool>,
    helper: *mut c_void,
}

static DROP_TARGET_VTBL: DropTargetVtbl = DropTargetVtbl {
    base: UnknownVtbl { query_interface: dt_query_interface, add_ref: dt_add_ref, release: dt_release },
    drag_enter: dt_drag_enter,
    drag_over: dt_drag_over,
    drag_leave: dt_drag_leave,
    drop: dt_drop,
};

unsafe extern "system" fn dt_query_interface(this: *mut c_void, riid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        if same_guid(&*riid, &IID_IUNKNOWN) || same_guid(&*riid, &IID_IDROPTARGET) {
            dt_add_ref(this);
            *out = this;
            S_OK
        } else {
            *out = null_mut();
            E_NOINTERFACE
        }
    }
}

unsafe extern "system" fn dt_add_ref(this: *mut c_void) -> u32 {
    let target = unsafe { &*(this as *const DropTarget) };
    target.refs.set(target.refs.get() + 1);
    target.refs.get()
}

unsafe extern "system" fn dt_release(this: *mut c_void) -> u32 {
    let target = unsafe { &*(this as *const DropTarget) };
    let refs = target.refs.get() - 1;
    target.refs.set(refs);
    if refs == 0 {
        unsafe {
            release(target.helper);
            drop(Box::from_raw(this as *mut DropTarget));
        }
    }
    refs
}

fn helper_vtbl(helper: *mut c_void) -> Option<&'static DropTargetHelperVtbl> {
    (!helper.is_null()).then(|| unsafe { &**(helper as *const *const DropTargetHelperVtbl) })
}

unsafe extern "system" fn dt_drag_enter(this: *mut DropTarget, data: *mut c_void, keys: u32, pt: POINTL, effect: *mut u32) -> HRESULT {
    unsafe {
        let target = &*this;
        target.accepts.set(has_files(data));
        let point = POINT { x: pt.x, y: pt.y };
        *effect = if target.accepts.get() { (target.handler.over)(target.hwnd, point, keys, *effect) } else { 0 };
        if let Some(h) = helper_vtbl(target.helper) {
            (h.drag_enter)(target.helper, target.hwnd, data, &point, *effect);
        }
    }
    S_OK
}

unsafe extern "system" fn dt_drag_over(this: *mut DropTarget, keys: u32, pt: POINTL, effect: *mut u32) -> HRESULT {
    unsafe {
        let target = &*this;
        let point = POINT { x: pt.x, y: pt.y };
        *effect = if target.accepts.get() { (target.handler.over)(target.hwnd, point, keys, *effect) } else { 0 };
        if let Some(h) = helper_vtbl(target.helper) {
            (h.drag_over)(target.helper, &point, *effect);
        }
    }
    S_OK
}

unsafe extern "system" fn dt_drag_leave(this: *mut DropTarget) -> HRESULT {
    unsafe {
        let target = &*this;
        (target.handler.leave)(target.hwnd);
        if let Some(h) = helper_vtbl(target.helper) {
            (h.drag_leave)(target.helper);
        }
    }
    S_OK
}

unsafe extern "system" fn dt_drop(this: *mut DropTarget, data: *mut c_void, keys: u32, pt: POINTL, effect: *mut u32) -> HRESULT {
    unsafe {
        let target = &*this;
        let point = POINT { x: pt.x, y: pt.y };
        if let Some(h) = helper_vtbl(target.helper) {
            (h.drop)(target.helper, data, &point, *effect);
        }
        *effect = if target.accepts.get() {
            let allowed = *effect;
            (target.handler.drop)(target.hwnd, dropped_files(data), point, keys, allowed)
        } else {
            0
        };
    }
    S_OK
}

/// Makes a window accept files dragged from Explorer, the desktop or a panel.
pub fn register(hwnd: HWND, handler: &'static DropHandler) {
    let mut helper: *mut c_void = null_mut();
    unsafe {
        CoCreateInstance(&CLSID_DRAGDROPHELPER, null_mut(), CLSCTX_INPROC_SERVER, &IID_IDROPTARGETHELPER, &mut helper);
        let target = Box::into_raw(Box::new(DropTarget {
            vtbl: &DROP_TARGET_VTBL,
            refs: Cell::new(1),
            hwnd,
            handler,
            accepts: Cell::new(false),
            helper,
        }));
        // OLE keeps its own reference while registered.
        RegisterDragDrop(hwnd, target as *mut c_void);
        dt_release(target as *mut c_void);
    }
}

pub fn revoke(hwnd: HWND) {
    unsafe { RevokeDragDrop(hwnd) };
}

/// Runs the shell's drag loop for one file. Returns the effect if it was dropped somewhere.
pub fn drag_file_out(hwnd: HWND, path: &str, allowed: u32) -> Option<u32> {
    let file = wide(path);
    unsafe {
        let pidl = ILCreateFromPathW(file.as_ptr());
        if pidl.is_null() {
            return None;
        }
        let mut data: *mut c_void = null_mut();
        // The shell wants the parent folder plus a child ID, not one absolute ID,
        // or the data object has no file list (CF_HDROP).
        let parent = ILClone(pidl);
        ILRemoveLastID(parent);
        let items = [ILFindLastID(pidl) as *const _];
        let created = SHCreateDataObject(parent, 1, items.as_ptr(), null_mut(), &IID_IDATAOBJECT, &mut data);
        let mut result = None;
        if created == S_OK && !data.is_null() {
            let mut effect = 0;
            if SHDoDragDrop(hwnd, data, null_mut(), allowed, &mut effect) == DRAGDROP_S_DROP {
                result = Some(effect);
            }
            release(data);
        }
        ILFree(parent);
        ILFree(pidl);
        result
    }
}

/// True if the mouse is over the desktop's icon view.
pub fn cursor_over_desktop() -> bool {
    let class_of = |hwnd: HWND| {
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    };
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let hwnd = WindowFromPoint(pt);
        let root = class_of(GetAncestor(hwnd, GA_ROOT));
        class_of(hwnd) == "SysListView32" && (root == "Progman" || root == "WorkerW")
    }
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let mut raw = null_mut();
        if SHGetKnownFolderPath(id, 0, null_mut(), &mut raw) != S_OK {
            return None;
        }
        let len = (0..).take_while(|&i| *raw.add(i) != 0).count();
        let path = String::from_utf16_lossy(std::slice::from_raw_parts(raw, len));
        CoTaskMemFree(raw as *const c_void);
        Some(PathBuf::from(path))
    }
}

pub fn desktop_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop)
}

/// Root folder for files moved into panels: `storage_dir`, or Documents\deskfolds.
pub fn storage_root(storage_dir: &str) -> PathBuf {
    if !storage_dir.trim().is_empty() {
        return PathBuf::from(crate::util::expand_env(storage_dir.trim()));
    }
    known_folder(&FOLDERID_Documents).unwrap_or_else(|| PathBuf::from(".")).join("deskfolds")
}

/// The folder holding one panel's files, named after the panel.
pub fn panel_dir(root: &Path, title: &str) -> PathBuf {
    let name: String = title.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let name = name.trim().trim_end_matches('.');
    root.join(if name.is_empty() { "Folder" } else { name })
}

/// True if the file lives in the storage folder, i.e. it was moved into a panel.
pub fn is_stored(path: &str, root: &Path) -> bool {
    let root = root.to_string_lossy().to_lowercase();
    let path = crate::util::expand_env(path).to_lowercase();
    path.starts_with(&format!("{}\\", root.trim_end_matches('\\')))
}

/// Moves a file or folder into `dir`, renaming it "name (2)" if the name is taken.
/// Never overwrites anything.
pub fn move_into(src: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let file_name = src.file_name().ok_or_else(|| format!("Invalid path {}", src.display()))?;
    let mut dest = dir.join(file_name);
    let stem = Path::new(file_name).file_stem().unwrap_or(file_name).to_string_lossy().to_string();
    let ext = Path::new(file_name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut n = 2;
    while dest.exists() {
        dest = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    let (from, to) = (wide(&src.to_string_lossy()), wide(&dest.to_string_lossy()));
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_COPY_ALLOWED) } == 0 {
        let code = unsafe { GetLastError() };
        return Err(format!("Cannot move {} (Windows error {code}).", src.display()));
    }
    Ok(dest)
}

/// The name shown for a dropped file: no ".lnk"/".url" extension, folders keep their name.
pub fn display_name(path: &Path) -> String {
    let name = if path.is_dir() { path.file_name() } else { path.file_stem() };
    name.map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_files_are_detected_case_insensitively() {
        let root = Path::new("C:\\Users\\me\\Documents\\deskfolds");
        assert!(is_stored("c:\\users\\me\\documents\\deskfolds\\Apps\\Steam.lnk", root));
        assert!(!is_stored("C:\\Users\\me\\Desktop\\Steam.lnk", root));
        assert!(!is_stored("C:\\Users\\me\\Documents\\deskfolds-other\\a.lnk", root));
    }

    #[test]
    fn panel_folder_names_are_safe() {
        let root = Path::new("C:\\root");
        assert_eq!(panel_dir(root, "Games: 2/3"), Path::new("C:\\root\\Games_ 2_3"));
        assert_eq!(panel_dir(root, " ... "), Path::new("C:\\root\\Folder"));
    }

    #[test]
    fn moving_never_overwrites() {
        let base = std::env::temp_dir().join(format!("deskfolds-move-{}", std::process::id()));
        let (src_dir, dest_dir) = (base.join("src"), base.join("dest"));
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        std::fs::write(dest_dir.join("a.txt"), "existing").unwrap();
        std::fs::write(src_dir.join("a.txt"), "moved").unwrap();
        let dest = move_into(&src_dir.join("a.txt"), &dest_dir).unwrap();
        assert_eq!(dest, dest_dir.join("a (2).txt"));
        assert_eq!(std::fs::read_to_string(dest_dir.join("a.txt")).unwrap(), "existing");
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "moved");
        assert!(!src_dir.join("a.txt").exists());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
