//! Windows' menus inside a folder.
//!
//! - The « New » menu: the list Explorer shows on the desktop (folder,
//!   shortcut, and the file types apps registered with a "ShellNew" key, such
//!   as Word or zip), made the way Explorer makes them, in the folder's storage
//!   folder. It is read from the registry when the menu opens. (Explorer's own
//!   menu object, CLSID_NewMenu, adds nothing outside an Explorer window.)
//! - A shortcut's Windows menu: its file's IContextMenu, as on the desktop,
//!   loaded only while the menu is open. Its submenus are filled when they
//!   open, through the messages of the menu's owner window, which forwards
//!   them here. Cut is left out; Delete and Rename are deskfolds' to do (see
//!   `panel::item_windows_menu`).
//!
//! windows-sys has no COM interfaces, so the vtables needed are declared here.

use std::cell::Cell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, S_OK, WPARAM};
use windows_sys::Win32::UI::Shell::Common::ITEMIDLIST;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, REG_BINARY, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ, RRF_NOEXPAND, RRF_RT_ANY, RegCloseKey,
    RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows_sys::Win32::UI::Shell::{
    FOLDERID_CommonTemplates, FOLDERID_Templates, ILCreateFromPathW, ILFree, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHBindToParent,
    SHCreateItemFromParsingName, SHELLEXECUTEINFOW, SHLoadIndirectString, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DeleteMenu, DestroyMenu, GetMenuItemCount, GetMenuItemID, HMENU, MF_BYPOSITION, PostMessageW, SW_SHOWNORMAL,
    SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL,
};
use windows_sys::core::{GUID, HRESULT};

use crate::util::wide;

const IID_ICONTEXT_MENU: GUID = GUID::from_u128(0x000214e4_0000_0000_c000_000000000046);
const IID_ICONTEXT_MENU2: GUID = GUID::from_u128(0x000214f4_0000_0000_c000_000000000046);
const IID_ICONTEXT_MENU3: GUID = GUID::from_u128(0xbcfce0a0_ec17_11d0_8d10_00a0c90f2719);
const IID_ISHELL_FOLDER: GUID = GUID::from_u128(0x000214e6_0000_0000_c000_000000000046);
const CLSID_FILE_OPERATION: GUID = GUID::from_u128(0x3ad05575_8857_4850_9277_11b85bdb8e09);
const IID_IFILE_OPERATION: GUID = GUID::from_u128(0x947aab5f_0a5c_4c13_b4d6_4bf7836fc9f8);
const IID_ISHELL_ITEM: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);
/// GCS_VERBW: a menu entry's verb, as UTF-16.
const GCS_VERBW: u32 = 4;
const CMF_NORMAL: u32 = 0;
/// With a Rename entry, as in Explorer.
const CMF_CANRENAME: u32 = 0x10;
/// The entries Explorer shows with Shift + right-click.
const CMF_EXTENDEDVERBS: u32 = 0x100;
const CMIC_MASK_UNICODE: u32 = 0x4000;
const CMIC_MASK_PTINVOKE: u32 = 0x2000_0000;

/// The command ids Windows' menus get: ours stay below.
pub const FIRST_ID: u32 = 0x1000;
pub const LAST_ID: u32 = 0x7fff;

#[repr(C)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct ContextMenuVtbl {
    base: UnknownVtbl,
    query_context_menu: unsafe extern "system" fn(*mut c_void, HMENU, u32, u32, u32, u32) -> HRESULT,
    invoke_command: unsafe extern "system" fn(*mut c_void, *const InvokeInfo) -> HRESULT,
    get_command_string: unsafe extern "system" fn(*mut c_void, usize, u32, *mut u32, *mut u8, u32) -> HRESULT,
}

/// IFileOperation, in vtable order.
#[repr(C)]
struct FileOperationVtbl {
    base: UnknownVtbl,
    advise: usize,
    unadvise: usize,
    set_operation_flags: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    set_progress_message: usize,
    set_progress_dialog: usize,
    set_properties: usize,
    set_owner_window: unsafe extern "system" fn(*mut c_void, HWND) -> HRESULT,
    apply_properties_to_item: usize,
    apply_properties_to_items: usize,
    rename_item: usize,
    rename_items: usize,
    move_item: usize,
    move_items: usize,
    copy_item: usize,
    copy_items: usize,
    delete_item: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> HRESULT,
    delete_items: usize,
    new_item: usize,
    perform_operations: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    get_any_operations_aborted: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
}

#[repr(C)]
struct ContextMenu2Vtbl {
    base: ContextMenuVtbl,
    handle_menu_msg: unsafe extern "system" fn(*mut c_void, u32, WPARAM, LPARAM) -> HRESULT,
}

#[repr(C)]
struct ContextMenu3Vtbl {
    base: ContextMenu2Vtbl,
    handle_menu_msg2: unsafe extern "system" fn(*mut c_void, u32, WPARAM, LPARAM, *mut LRESULT) -> HRESULT,
}

#[repr(C)]
struct ShellFolderVtbl {
    base: UnknownVtbl,
    parse_display_name: usize,
    enum_objects: usize,
    bind_to_object: usize,
    bind_to_storage: usize,
    compare_ids: usize,
    create_view_object: usize,
    get_attributes_of: usize,
    get_ui_object_of:
        unsafe extern "system" fn(*mut c_void, HWND, u32, *const *const ITEMIDLIST, *const GUID, *mut u32, *mut *mut c_void) -> HRESULT,
}

/// CMINVOKECOMMANDINFOEX.
#[repr(C)]
struct InvokeInfo {
    size: u32,
    mask: u32,
    hwnd: HWND,
    verb: *const u8,
    parameters: *const u8,
    directory: *const u8,
    show: i32,
    hotkey: u32,
    icon: *mut c_void,
    title: *const u8,
    verb_w: *const u16,
    parameters_w: *const u16,
    directory_w: *const u16,
    title_w: *const u16,
    point: POINT,
}

/// A COM object pointer, released when dropped.
struct Com(*mut c_void);

impl Com {
    fn vtbl<T>(&self) -> &T {
        unsafe { &**(self.0 as *const *const T) }
    }

    fn query(&self, iid: &GUID) -> Option<Com> {
        let mut out = null_mut();
        let hr = unsafe { (self.vtbl::<UnknownVtbl>().query_interface)(self.0, iid, &mut out) };
        (hr == S_OK && !out.is_null()).then_some(Com(out))
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { (self.vtbl::<UnknownVtbl>().release)(self.0) };
        }
    }
}

/// An item ID list, freed when dropped.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ILFree(self.0) };
        }
    }
}

thread_local! {
    /// The Windows menu on screen: its IContextMenu3 (or 2), which one, and
    /// the popup menu it is in (ours: its own messages are not forwarded).
    static ACTIVE: Cell<(*mut c_void, u8, HMENU)> = const { Cell::new((null_mut(), 0, null_mut())) };
}

/// A Windows menu of a folder or a file, ready to go into a popup menu.
pub struct ShellMenu {
    menu: Com,
    /// IContextMenu3 or IContextMenu2, for the submenus (« Send to », « Open with »).
    handler: Option<(Com, u8)>,
}

/// Where Windows put its Delete and Rename entries in a shortcut's menu (all
/// of them, if a handler adds another): deskfolds does those itself.
#[derive(Default)]
pub struct ItemIds {
    pub delete: Vec<u32>,
    pub rename: Vec<u32>,
}

impl ShellMenu {
    fn new(menu: Com) -> Self {
        let handler = menu.query(&IID_ICONTEXT_MENU3).map(|m| (m, 3)).or_else(|| menu.query(&IID_ICONTEXT_MENU2).map(|m| (m, 2)));
        Self { menu, handler }
    }

    /// The entry opens, prints or shows the file, and makes no file next to it.
    pub fn opens_only(&self, id: u32) -> bool {
        const OPENING: [&str; 14] =
            ["open", "opennew", "opennewwindow", "opennewtab", "explore", "edit", "print", "printto", "runas", "runasuser", "openas", "properties", "play", "preview"];
        self.verb(id).is_some_and(|v| OPENING.contains(&v.as_str()))
    }

    /// The verb of one of its entries ("delete", "open"...), if it has one.
    fn verb(&self, id: u32) -> Option<String> {
        if !(FIRST_ID..=LAST_ID).contains(&id) {
            return None;
        }
        let mut buf = [0u16; 64];
        let get = self.menu.vtbl::<ContextMenuVtbl>().get_command_string;
        let ok = unsafe { get(self.menu.0, (id - FIRST_ID) as usize, GCS_VERBW, null_mut(), buf.as_mut_ptr() as *mut u8, buf.len() as u32) } == S_OK;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| String::from_utf16_lossy(&buf[..len]).to_lowercase())
    }

    /// The full Windows menu of the file or folder at `path`.
    pub fn of_file(owner: HWND, path: &Path) -> Option<Self> {
        let file = wide(&path.to_string_lossy());
        unsafe {
            let pidl = Pidl(ILCreateFromPathW(file.as_ptr()));
            if pidl.0.is_null() {
                return None;
            }
            let mut folder = null_mut();
            let mut child: *mut ITEMIDLIST = null_mut();
            if SHBindToParent(pidl.0, &IID_ISHELL_FOLDER, &mut folder, &mut child) != S_OK || folder.is_null() {
                return None;
            }
            let folder = Com(folder);
            let items = [child as *const ITEMIDLIST];
            let mut object = null_mut();
            let get = folder.vtbl::<ShellFolderVtbl>().get_ui_object_of;
            if get(folder.0, owner, 1, items.as_ptr(), &IID_ICONTEXT_MENU, null_mut(), &mut object) != S_OK || object.is_null() {
                return None;
            }
            Some(Self::new(Com(object)))
        }
    }

    /// Adds Windows' menu of a shortcut's file to `menu` at `index`, with ids
    /// from FIRST_ID on (with `extended`, the entries of Shift + right-click),
    /// without Cut: moving the file away would leave the folder a broken
    /// shortcut, and dragging it out does that properly. Returns where its
    /// Delete and Rename are.
    pub fn fill_for_item(&self, menu: HMENU, index: u32, extended: bool) -> ItemIds {
        let mut ids = ItemIds::default();
        let query = self.menu.vtbl::<ContextMenuVtbl>().query_context_menu;
        let flags = CMF_NORMAL | CMF_CANRENAME | if extended { CMF_EXTENDEDVERBS } else { 0 };
        unsafe {
            let count_before = GetMenuItemCount(menu).max(0);
            if query(self.menu.0, menu, index, FIRST_ID, LAST_ID, flags) < 0 {
                return ids;
            }
            // Windows' entries are the ones it added, at `index`.
            let added = (GetMenuItemCount(menu).max(0) - count_before).max(0);
            for pos in (index as i32..index as i32 + added).rev() {
                let id = GetMenuItemID(menu, pos);
                match self.verb(id).as_deref() {
                    Some("cut") => {
                        DeleteMenu(menu, pos as u32, MF_BYPOSITION);
                    }
                    Some("delete") => ids.delete.push(id),
                    Some("rename") => ids.rename.push(id),
                    _ => {}
                }
            }
            tidy_lines(menu);
        }
        ids
    }

    /// Runs the entry chosen in its popup menu, which must still be there.
    pub fn invoke(&self, owner: HWND, chosen: &Chosen, dir: Option<&Path>, at: POINT) -> bool {
        let id = chosen.id;
        let dir_w = dir.map(|d| wide(&d.to_string_lossy()));
        let info = InvokeInfo {
            size: std::mem::size_of::<InvokeInfo>() as u32,
            mask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
            hwnd: owner,
            // MAKEINTRESOURCE: the offset of the entry from FIRST_ID.
            verb: (id - FIRST_ID) as usize as *const u8,
            parameters: null(),
            directory: null(),
            show: SW_SHOWNORMAL,
            hotkey: 0,
            icon: null_mut(),
            title: null(),
            verb_w: (id - FIRST_ID) as usize as *const u16,
            parameters_w: null(),
            directory_w: dir_w.as_ref().map_or(null(), |d| d.as_ptr()),
            title_w: null(),
            point: at,
        };
        let invoke = self.menu.vtbl::<ContextMenuVtbl>().invoke_command;
        unsafe { invoke(self.menu.0, &info) >= 0 }
    }

    /// While its menu is open, the owner window forwards the menu messages here.
    fn activate(&self, top: HMENU) -> ActiveGuard {
        let previous = ACTIVE.get();
        if let Some((h, kind)) = &self.handler {
            ACTIVE.set((h.0, *kind, top));
        }
        ActiveGuard(previous)
    }
}

struct ActiveGuard((*mut c_void, u8, HMENU));

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.set(self.0);
    }
}

/// Called by the owner window of a menu: lets Windows' menu on screen fill
/// and draw its submenus. Some(result) when it handled the message.
pub fn handle_menu_message(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    let (handler, kind, top) = ACTIVE.get();
    // Only Windows' own submenus go to it: the top menu holds deskfolds'
    // entries too, and is filled by deskfolds.
    if handler.is_null() || (msg == WM_INITMENUPOPUP && wparam as HMENU == top) {
        return None;
    }
    unsafe {
        if kind == 3 {
            let vt = &**(handler as *const *const ContextMenu3Vtbl);
            let mut result: LRESULT = 0;
            if (vt.handle_menu_msg2)(handler, msg, wparam, lparam, &mut result) == S_OK {
                return Some(result);
            }
        } else {
            let vt = &**(handler as *const *const ContextMenu2Vtbl);
            if (vt.handle_menu_msg)(handler, msg, wparam, lparam) == S_OK {
                return Some(if msg == WM_INITMENUPOPUP { 0 } else { 1 });
            }
        }
    }
    None
}

/// No line at the top of a menu, at the bottom, or twice in a row.
unsafe fn tidy_lines(menu: HMENU) {
    unsafe {
        let mut previous_line = true;
        let mut pos = 0;
        while pos < GetMenuItemCount(menu).max(0) {
            let line = GetMenuItemID(menu, pos) == 0;
            // A line Windows would not take away is stepped over.
            if line && previous_line && DeleteMenu(menu, pos as u32, MF_BYPOSITION) != 0 {
                continue;
            }
            previous_line = line;
            pos += 1;
        }
        let last = GetMenuItemCount(menu) - 1;
        if last >= 0 && GetMenuItemID(menu, last) == 0 {
            DeleteMenu(menu, last as u32, MF_BYPOSITION);
        }
    }
}

/// The entry chosen in a popup menu: our id, the id of Windows' entry
/// (FIRST_ID and above), or 0. The menu lives as long as this, as in Explorer,
/// where it goes after the command ran: some of Windows' entries (Send to,
/// Open with) find what they do in it when they run.
pub struct Chosen {
    pub id: u32,
    menu: HMENU,
}

impl Drop for Chosen {
    fn drop(&mut self) {
        if !self.menu.is_null() {
            unsafe { DestroyMenu(self.menu) };
        }
    }
}

/// Shows a popup menu at `at`: `build` fills it (our entries with ids below
/// FIRST_ID, Windows' with `ShellMenu::fill_for_item`), and the messages of
/// `shell`'s submenus are forwarded to it. Keep what it returns until the
/// chosen entry has run.
pub fn track(owner: HWND, at: POINT, shell: Option<&ShellMenu>, build: impl FnOnce(HMENU)) -> Chosen {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return Chosen { id: 0, menu };
        }
        build(menu);
        let _active = shell.map(|s| s.activate(menu));
        // The owner must be in front, or the menu does not close when clicking elsewhere.
        SetForegroundWindow(owner);
        let choice = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, at.x, at.y, 0, owner, null());
        PostMessageW(owner, WM_NULL, 0, 0);
        Chosen { id: choice.max(0) as u32, menu }
    }
}

// ---- The « New » menu ----

/// How an entry of the « New » menu makes its file.
enum How {
    Folder,
    /// Windows' « Create Shortcut » window, for an empty shortcut made first.
    Shortcut,
    Empty,
    Data(Vec<u8>),
    /// A copy of a template file.
    Template(PathBuf),
}

/// One entry of the « New » menu.
pub struct NewKind {
    pub label: String,
    /// ".docx", or "" for a folder.
    ext: String,
    how: How,
}

/// What an entry of the « New » menu made: the new file, and Windows'
/// « Create Shortcut » window when it is still to fill the shortcut.
pub struct Made {
    pub path: PathBuf,
    pub wizard: Option<HANDLE>,
}

/// A registry value: (type, bytes).
fn reg_value(root: HKEY, key: &str, name: Option<&str>) -> Option<(u32, Vec<u8>)> {
    let key = wide(key);
    let name = name.map(wide);
    let name_ptr = name.as_ref().map_or(null(), |n| n.as_ptr());
    let (mut kind, mut size) = (0u32, 0u32);
    unsafe {
        if RegGetValueW(root, key.as_ptr(), name_ptr, RRF_RT_ANY | RRF_NOEXPAND, &mut kind, null_mut(), &mut size) != 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize + 2];
        let mut size = data.len() as u32;
        if RegGetValueW(root, key.as_ptr(), name_ptr, RRF_RT_ANY | RRF_NOEXPAND, &mut kind, data.as_mut_ptr() as *mut c_void, &mut size) != 0 {
            return None;
        }
        data.truncate(size as usize);
        Some((kind, data))
    }
}

fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

/// A text value, expanded and without its ending zero.
fn reg_text(root: HKEY, key: &str, name: Option<&str>) -> Option<String> {
    let (kind, data) = reg_value(root, key, name)?;
    let text = match kind {
        REG_SZ => utf16(&data),
        REG_EXPAND_SZ => crate::util::expand_env(&utf16(&data)),
        _ => return None,
    };
    let text = text.trim_end_matches('\0').to_string();
    Some(text)
}

fn key_exists(root: HKEY, key: &str) -> bool {
    let key = wide(key);
    let mut h: HKEY = null_mut();
    unsafe {
        let ok = RegOpenKeyExW(root, key.as_ptr(), 0, KEY_READ, &mut h) == 0;
        if ok {
            RegCloseKey(h);
        }
        ok
    }
}

/// "@shell32.dll,-1234" strings, loaded in the language of Windows.
fn indirect(text: &str) -> String {
    if !text.starts_with('@') {
        return text.to_string();
    }
    let source = wide(text);
    let mut buf = vec![0u16; 512];
    if unsafe { SHLoadIndirectString(source.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, null()) } != S_OK {
        return String::new();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf16_lossy(&buf[..len])
}

/// A plain file extension: a dot, then a few letters, digits or "_+-". The
/// registry is the user's to change: nothing else may reach a file name.
fn plain_ext(ext: &str) -> bool {
    ext.strip_prefix('.').is_some_and(|e| (1..=16).contains(&e.len()) && e.chars().all(|c| c.is_ascii_alphanumeric() || "_+-".contains(c)))
}

/// The types Explorer's « New » menu lists, as Explorer last found them, or
/// as found in the registry when Explorer has not listed them yet.
fn new_extensions() -> Vec<String> {
    const CACHE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Discardable\\PostSetup\\ShellNew";
    if let Some((REG_MULTI_SZ, data)) = reg_value(HKEY_CURRENT_USER, CACHE, Some("Classes")) {
        let list: Vec<String> = utf16(&data).split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
        if !list.is_empty() {
            return list;
        }
    }
    let mut list = Vec::new();
    let mut i = 0;
    loop {
        let mut name = vec![0u16; 256];
        let mut len = name.len() as u32;
        let found = unsafe { RegEnumKeyExW(HKEY_CLASSES_ROOT, i, name.as_mut_ptr(), &mut len, null(), null_mut(), null_mut(), null_mut()) };
        if found != 0 {
            break;
        }
        i += 1;
        let ext = String::from_utf16_lossy(&name[..len as usize]);
        if ext.starts_with('.') && shell_new_key(&ext).is_some() {
            list.push(ext);
        }
    }
    list
}

/// The "ShellNew" key of a file type: under the extension, or under its program.
fn shell_new_key(ext: &str) -> Option<String> {
    let direct = format!("{ext}\\ShellNew");
    if key_exists(HKEY_CLASSES_ROOT, &direct) {
        return Some(direct);
    }
    let program = reg_text(HKEY_CLASSES_ROOT, ext, None).filter(|p| !p.is_empty())?;
    let under = format!("{ext}\\{program}\\ShellNew");
    key_exists(HKEY_CLASSES_ROOT, &under).then_some(under)
}

/// A template named in a "FileName" value: a full path, or a file of the
/// template folders.
fn template(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    let dirs = [
        Some(PathBuf::from(crate::util::expand_env("%SystemRoot%\\ShellNew"))),
        crate::dragdrop::known_folder(&FOLDERID_Templates),
        crate::dragdrop::known_folder(&FOLDERID_CommonTemplates),
    ];
    dirs.into_iter().flatten().map(|d| d.join(name)).find(|p| p.is_file())
}

/// The entry for a file type, if it makes its file in a way deskfolds does too.
fn kind_of(ext: &str) -> Option<NewKind> {
    let key = shell_new_key(ext)?;
    // A type whose app makes it with its own handler (a library, for example):
    // an empty file would not be one.
    if reg_value(HKEY_CLASSES_ROOT, &key, Some("Handler")).is_some() {
        return None;
    }
    let how = if reg_value(HKEY_CLASSES_ROOT, &key, Some("NullFile")).is_some() {
        How::Empty
    } else if let Some((kind, data)) = reg_value(HKEY_CLASSES_ROOT, &key, Some("Data")) {
        match kind {
            REG_BINARY => How::Data(data),
            REG_SZ | REG_EXPAND_SZ => How::Data(utf16(&data).trim_end_matches('\0').as_bytes().to_vec()),
            _ => return None,
        }
    } else {
        // Made by a command or a program's own handler: not offered here.
        let name = reg_text(HKEY_CLASSES_ROOT, &key, Some("FileName"))?;
        // A template that cannot be found: Explorer makes an empty file instead.
        template(&name).map_or(How::Empty, How::Template)
    };
    let program = reg_text(HKEY_CLASSES_ROOT, ext, None).unwrap_or_default();
    let friendly = (!program.is_empty()).then(|| reg_text(HKEY_CLASSES_ROOT, &program, Some("FriendlyTypeName")).map(|t| indirect(&t))).flatten();
    let plain = (!program.is_empty()).then(|| reg_text(HKEY_CLASSES_ROOT, &program, None)).flatten();
    let label = friendly
        .into_iter()
        .chain(plain)
        .map(|t| t.trim().to_string())
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| format!("{} file", ext.trim_start_matches('.').to_uppercase()));
    Some(NewKind { label, ext: ext.to_string(), how })
}

/// The entries of the « New » menu: Folder and Shortcut, then the file types
/// in alphabetical order.
pub fn new_kinds() -> Vec<NewKind> {
    let mut kinds = vec![
        NewKind { label: "Folder".into(), ext: String::new(), how: How::Folder },
        NewKind { label: "Shortcut".into(), ext: ".lnk".into(), how: How::Shortcut },
    ];
    let mut types: Vec<NewKind> = new_extensions()
        .into_iter()
        .filter(|e| plain_ext(e) && !e.eq_ignore_ascii_case(".lnk"))
        .filter_map(|e| kind_of(&e))
        .collect();
    types.sort_by_key(|k| k.label.to_lowercase());
    types.dedup_by(|a, b| a.ext.eq_ignore_ascii_case(&b.ext));
    kinds.extend(types);
    kinds
}

/// "New Microsoft Word Document.docx": a name that is safe as a file name.
fn new_name(kind: &NewKind) -> String {
    let label: String = kind.label.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    match kind.how {
        How::Folder => "New folder".into(),
        How::Shortcut => "New shortcut.lnk".into(),
        _ => format!("New {}{}", label.trim(), kind.ext),
    }
}

/// Makes the entry's new file in `dir`, under a free name. A shortcut is made
/// empty, and Windows' « Create Shortcut » window fills it.
pub fn make_new(dir: &Path, kind: &NewKind) -> Result<Made, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let path = crate::dragdrop::free_name(dir, std::ffi::OsStr::new(&new_name(kind)));
    // Only ever a file of that folder.
    if path.parent() != Some(dir) {
        return Err(format!("Cannot make a file named {}.", path.display()));
    }
    let fail = |e: std::io::Error| format!("Cannot make {}: {e}", path.display());
    let create = |bytes: &[u8]| -> Result<(), String> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(fail)?;
        f.write_all(bytes).map_err(fail)
    };
    match &kind.how {
        How::Folder => std::fs::create_dir(&path).map_err(fail)?,
        How::Empty => create(&[])?,
        How::Data(bytes) => create(bytes)?,
        How::Template(from) => {
            let (src, dst) = (wide(&from.to_string_lossy()), wide(&path.to_string_lossy()));
            if unsafe { windows_sys::Win32::Storage::FileSystem::CopyFileW(src.as_ptr(), dst.as_ptr(), 1) } == 0 {
                create(&[])?;
            }
        }
        How::Shortcut => {
            create(&[])?;
            let wizard = create_shortcut_window(&path);
            if wizard.is_none() {
                let _ = std::fs::remove_file(&path);
                return Err("Windows' Create Shortcut window could not be opened.".into());
            }
            return Ok(Made { path, wizard });
        }
    }
    Ok(Made { path, wizard: None })
}

/// Opens Windows' « Create Shortcut » window for the empty shortcut `link`.
fn create_shortcut_window(link: &Path) -> Option<HANDLE> {
    let mut system = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), system.len() as u32) } as usize;
    if n == 0 || n >= system.len() {
        return None;
    }
    let exe = wide(&format!("{}\\rundll32.exe", String::from_utf16_lossy(&system[..n])));
    let params = wide(&format!("appwiz.cpl,NewLinkHere {}", link.display()));
    let dir = link.parent().map(|d| wide(&d.to_string_lossy()));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
        info.lpFile = exe.as_ptr();
        info.lpParameters = params.as_ptr();
        info.lpDirectory = dir.as_ref().map_or(null(), |d| d.as_ptr());
        info.nShow = SW_SHOWNORMAL;
        (ShellExecuteExW(&mut info) != 0 && !info.hProcess.is_null()).then_some(info.hProcess)
    }
}

const FOF_SILENT: u32 = 0x4;
const FOF_NOCONFIRMATION: u32 = 0x10;
const FOF_ALLOWUNDO: u32 = 0x40;
/// No error dialogs, and so no prompt for administrator rights either.
const FOF_NOERRORUI: u32 = 0x400;
/// Windows asks first when it would delete for good rather than recycle.
const FOF_WANTNUKEWARNING: u32 = 0x4000;
const FOFX_SHOWELEVATIONPROMPT: u32 = 0x0004_0000;
const FOFX_RECYCLEONDELETE: u32 = 0x0008_0000;

/// Sends files to the Recycle Bin (`recycle`; otherwise deletes them) with
/// Windows' own file operation, as Explorer does. When the user may not
/// change them, Windows asks for permission with its own prompt, and does it
/// with its own rights: no code of deskfolds runs as administrator. Check
/// afterwards which files are gone.
pub fn recycle_with_windows(owner: HWND, files: &[PathBuf], recycle: bool) {
    let undo = if recycle { FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE } else { 0 };
    // Windows' own dialogs stay: they name the file it asks permission for.
    file_operation(owner, files, FOF_SILENT | FOF_NOCONFIRMATION | FOFX_SHOWELEVATIONPROMPT | undo);
}

/// Delete from a shortcut's menu: its file goes to the Recycle Bin, with no
/// question. When Windows cannot recycle it (too big for the Recycle Bin, a
/// USB stick, a network drive), it asks before deleting it for good. It never
/// asks for administrator rights: a file of the user's own storage folder does
/// not need them, and while such a prompt is open the path could be pointed
/// elsewhere. The file is still there if the user said No or Windows could not.
pub fn delete_to_recycle_bin(files: &[PathBuf]) -> Outcome {
    // No owner: the hidden main window would put Windows' question behind others.
    file_operation(null_mut(), files, FOF_SILENT | FOF_NOERRORUI | FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING)
}

/// What Windows' file operation did.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Outcome {
    Done,
    /// The user said No to one of Windows' questions.
    Cancelled,
    /// Not done, and not because of the user (Windows may stop silently).
    Failed,
}

/// PerformOperations when the user said No (COPYENGINE_E_USER_CANCELLED),
/// or a cancelled operation in general.
const USER_CANCELLED: [HRESULT; 2] = [0x8027_0000_u32 as HRESULT, 0x8007_04C7_u32 as HRESULT];

fn file_operation(owner: HWND, files: &[PathBuf], flags: u32) -> Outcome {
    unsafe {
        let mut object = null_mut();
        if CoCreateInstance(&CLSID_FILE_OPERATION, null_mut(), CLSCTX_INPROC_SERVER, &IID_IFILE_OPERATION, &mut object) != S_OK || object.is_null() {
            return Outcome::Failed;
        }
        let op = Com(object);
        let vt = op.vtbl::<FileOperationVtbl>();
        (vt.set_operation_flags)(op.0, flags);
        if !owner.is_null() {
            (vt.set_owner_window)(op.0, owner);
        }
        let mut any = false;
        for file in files {
            let path = wide(&file.to_string_lossy());
            let mut item = null_mut();
            if SHCreateItemFromParsingName(path.as_ptr(), null_mut(), &IID_ISHELL_ITEM, &mut item) == S_OK && !item.is_null() {
                let item = Com(item);
                any |= (vt.delete_item)(op.0, item.0, null_mut()) == S_OK;
            }
        }
        if !any {
            return Outcome::Failed;
        }
        let hr = (vt.perform_operations)(op.0);
        // Windows may stop an operation without a word (no error dialogs): it
        // then says it succeeded, but that something was aborted.
        let mut aborted = 0;
        (vt.get_any_operations_aborted)(op.0, &mut aborted);
        if USER_CANCELLED.contains(&hr) {
            Outcome::Cancelled
        } else if hr >= 0 && aborted == 0 {
            Outcome::Done
        } else {
            Outcome::Failed
        }
    }
}

/// The names in a folder, to find what one of Windows' commands made there.
pub fn listing(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map(|entries| entries.flatten().map(|e| e.path()).collect()).unwrap_or_default()
}

/// What is in `after` and was not in `before`, oldest first.
pub fn made(before: &[PathBuf], after: &[PathBuf]) -> Vec<PathBuf> {
    let mut new: Vec<PathBuf> = after.iter().filter(|p| !before.contains(p)).cloned().collect();
    new.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.created()).ok());
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows' real menu for a file: no Cut, and its Delete and Rename found.
    #[test]
    fn a_shortcuts_menu_has_no_cut_and_finds_delete_and_rename() {
        unsafe { windows_sys::Win32::System::Com::CoInitializeEx(null(), windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32) };
        let dir = std::env::temp_dir().join(format!("deskfolds-menu-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "x").unwrap();
        let shell = ShellMenu::of_file(null_mut(), &file).expect("Windows' menu");
        let verbs = |extended: bool| unsafe {
            let menu = CreatePopupMenu();
            let ids = shell.fill_for_item(menu, 0, extended);
            let verbs: Vec<String> = (0..GetMenuItemCount(menu)).filter_map(|pos| shell.verb(GetMenuItemID(menu, pos))).collect();
            DestroyMenu(menu);
            (ids, verbs)
        };
        for extended in [false, true] {
            let (ids, verbs) = verbs(extended);
            assert!(!verbs.iter().any(|v| v == "cut"), "{verbs:?}");
            assert!(verbs.iter().any(|v| v == "properties"), "{verbs:?}");
            assert!(!ids.delete.is_empty() && !ids.rename.is_empty(), "{verbs:?}");
            for (ids, verb) in [(&ids.delete, "delete"), (&ids.rename, "rename")] {
                assert!(ids.iter().all(|&id| shell.verb(id).as_deref() == Some(verb)));
            }
        }
        drop(shell);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn new_files_get_explorer_like_names() {
        let word = NewKind { label: "Microsoft Word Document".into(), ext: ".docx".into(), how: How::Empty };
        assert_eq!(new_name(&word), "New Microsoft Word Document.docx");
        let odd = NewKind { label: "A/B: test".into(), ext: ".x".into(), how: How::Empty };
        assert_eq!(new_name(&odd), "New A_B_ test.x");
        assert_eq!(new_name(&NewKind { label: "Folder".into(), ext: String::new(), how: How::Folder }), "New folder");
    }

    #[test]
    fn the_new_menu_starts_with_folder_and_shortcut() {
        let kinds = new_kinds();
        assert_eq!((kinds[0].label.as_str(), kinds[1].label.as_str()), ("Folder", "Shortcut"));
        let base = std::env::temp_dir().join(format!("deskfolds-new-{}", std::process::id()));
        let made = make_new(&base, &kinds[0]).unwrap();
        assert!(made.path.is_dir() && made.wizard.is_none());
        let again = make_new(&base, &kinds[0]).unwrap();
        assert_eq!(again.path, base.join("New folder (2)"));
        let zip = NewKind { label: "Zip".into(), ext: ".zip".into(), how: How::Data(vec![0x50, 0x4b, 5, 6]) };
        let made = make_new(&base, &zip).unwrap();
        assert_eq!(std::fs::read(&made.path).unwrap(), [0x50, 0x4b, 5, 6]);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn only_plain_extensions_name_new_files() {
        assert!(plain_ext(".docx") && plain_ext(".library-ms") && plain_ext(".c++"));
        for bad in ["docx", ".", ".x\\..\\..\\foo", ".a b", ".a:b", ".abcdefghijklmnopq"] {
            assert!(!plain_ext(bad), "{bad}");
        }
    }

    #[test]
    fn windows_file_operation_deletes_a_file() {
        unsafe { windows_sys::Win32::System::Com::CoInitializeEx(null(), windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32) };
        let dir = std::env::temp_dir().join(format!("deskfolds-fileop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.lnk");
        std::fs::write(&file, b"x").unwrap();
        recycle_with_windows(null_mut(), std::slice::from_ref(&file), false);
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn what_a_command_made_is_found() {
        let before = vec![PathBuf::from("C:\\x\\a.txt")];
        let after = vec![PathBuf::from("C:\\x\\a.txt"), PathBuf::from("C:\\x\\New Text Document.txt")];
        assert_eq!(made(&before, &after), [PathBuf::from("C:\\x\\New Text Document.txt")]);
        assert!(made(&after, &after).is_empty());
    }
}
