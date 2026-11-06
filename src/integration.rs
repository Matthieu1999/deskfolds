//! What deskfolds adds to Windows, each only when the user turns it on: a
//! shortcut in the Start menu (so Windows search finds the app), and
//! « New deskfolds folder » in the desktop's right-click menu. Both are the
//! user's own (their Start menu folder, HKEY_CURRENT_USER): no administrator
//! rights and no signature are needed.
//!
//! A copy of deskfolds running with another APPDATA (a test copy) keeps to
//! itself: its Start menu shortcut goes under that APPDATA, and its menu entry
//! has a name of its own, so it never changes what the usual copy added.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::S_OK;
use windows_sys::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::UI::Shell::FOLDERID_Programs;
use windows_sys::core::{GUID, HRESULT};

use crate::util::wide;

const CLSID_SHELL_LINK: GUID = GUID::from_u128(0x00021401_0000_0000_c000_000000000046);
const IID_ISHELL_LINK_W: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSIST_FILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
/// STGM_READ, for IPersistFile::Load.
const STGM_READ: u32 = 0;

/// The desktop's right-click menu entries of the current user.
const DESKTOP_MENU_KEY: &str = "Software\\Classes\\DesktopBackground\\Shell\\deskfolds.newfolder";
pub const DESKTOP_MENU_TEXT: &str = "New deskfolds folder";
/// The argument the desktop menu entry starts deskfolds with.
pub const NEW_FOLDER_ARG: &str = "--new-folder";
/// Followed by a folder: deskfolds uses it as APPDATA (a test copy's entries).
pub const APPDATA_ARG: &str = "--appdata";

#[repr(C)]
struct UnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// IShellLinkW, in vtable order. Only the methods used here are typed.
#[repr(C)]
struct ShellLinkVtbl {
    base: UnknownVtbl,
    get_path: unsafe extern "system" fn(*mut c_void, *mut u16, i32, *mut c_void, u32) -> HRESULT,
    get_id_list: usize,
    set_id_list: usize,
    get_description: usize,
    set_description: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_working_directory: usize,
    set_working_directory: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_arguments: usize,
    set_arguments: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
    get_hotkey: usize,
    set_hotkey: usize,
    get_show_cmd: usize,
    set_show_cmd: usize,
    get_icon_location: usize,
    set_icon_location: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
    set_relative_path: usize,
    resolve: usize,
    set_path: unsafe extern "system" fn(*mut c_void, *const u16) -> HRESULT,
}

/// IPersistFile, in vtable order.
#[repr(C)]
struct PersistFileVtbl {
    base: UnknownVtbl,
    get_class_id: usize,
    is_dirty: usize,
    load: unsafe extern "system" fn(*mut c_void, *const u16, u32) -> HRESULT,
    save: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> HRESULT,
    save_completed: usize,
    get_cur_file: usize,
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

fn new_shell_link() -> Option<Com> {
    let mut link = null_mut();
    let hr = unsafe { CoCreateInstance(&CLSID_SHELL_LINK, null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELL_LINK_W, &mut link) };
    (hr == S_OK && !link.is_null()).then_some(Com(link))
}

/// The path of deskfolds.exe, as it runs now.
pub fn exe_path() -> Option<String> {
    std::env::current_exe().ok().map(|p| p.to_string_lossy().to_string())
}

fn same_path(a: &str, b: &str) -> bool {
    let canon = |p: &str| std::fs::canonicalize(p).map(|c| c.to_string_lossy().to_lowercase()).unwrap_or_else(|_| p.to_lowercase());
    canon(a) == canon(b)
}

/// True when deskfolds uses the usual APPDATA. A copy started with another
/// one (a test copy) is not the usual copy.
pub fn usual_copy() -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    let ours = crate::config::app_dir();
    let our_root = ours.parent().map(canon);
    crate::dragdrop::roaming_app_data().is_none_or(|d| Some(canon(&d)) == our_root)
}

/// FNV-1a: the same value for the same text in every build of deskfolds.
fn stable_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// A short name that tells this copy from the usual one, from its config
/// folder; None for the usual copy.
pub fn copy_suffix() -> Option<String> {
    if usual_copy() {
        return None;
    }
    // The folder as Windows resolves it (links, short names), so every way of
    // writing it gives the same name.
    let dir = crate::config::app_dir();
    let root = dir.parent().map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.to_path_buf())).unwrap_or_default();
    let root = root.to_string_lossy();
    let root = root.strip_prefix(r"\\?\").unwrap_or(&root);
    Some(format!("{:016x}", stable_hash(&format!("{root}\\deskfolds").to_lowercase())))
}

/// What deskfolds adds to Windows starts it with these arguments: none for
/// the usual copy; a test copy gives its APPDATA, so it starts as itself.
pub fn launch_args() -> String {
    match (usual_copy(), crate::config::app_dir().parent()) {
        (false, Some(root)) => {
            // A folder ending in "\\" (a drive) would escape the closing quote.
            let mut root = root.display().to_string();
            if root.ends_with('\\') {
                root.push('\\');
            }
            format!("{APPDATA_ARG} \"{root}\"")
        }
        _ => String::new(),
    }
}

// ---- The Start menu ----

/// The user's own Start menu programs folder (a test copy uses one under its APPDATA).
fn programs_dir() -> Option<PathBuf> {
    if usual_copy() {
        crate::dragdrop::known_folder(&FOLDERID_Programs)
    } else {
        crate::config::app_dir().parent().map(|d| d.join("Microsoft\\Windows\\Start Menu\\Programs"))
    }
}

/// The Start menu shortcut deskfolds makes.
pub fn start_menu_link() -> Option<PathBuf> {
    programs_dir().map(|d| d.join("deskfolds.lnk"))
}

pub fn start_menu_enabled() -> bool {
    start_menu_link().is_some_and(|p| p.is_file())
}

/// What the Start menu shortcut opens, if it can be read.
fn link_target(link: &Path) -> Option<String> {
    let shell_link = new_shell_link()?;
    let file = shell_link.query(&IID_IPERSIST_FILE)?;
    let path = wide(&link.to_string_lossy());
    unsafe {
        if (file.vtbl::<PersistFileVtbl>().load)(file.0, path.as_ptr(), STGM_READ) != S_OK {
            return None;
        }
        let mut buf = vec![0u16; 32 * 1024];
        if (shell_link.vtbl::<ShellLinkVtbl>().get_path)(shell_link.0, buf.as_mut_ptr(), buf.len() as i32, null_mut(), 0) != S_OK {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// Writes a .lnk at `link` to `target` (a file or a folder), with its
/// arguments, description and icon ("file" or "file,index"; without one, the
/// target's).
fn write_link(link: &Path, target: &str, args: &str, description: &str, icon: Option<&str>) -> Result<(), String> {
    let shell_link = new_shell_link().ok_or("no shell link")?;
    let vt = shell_link.vtbl::<ShellLinkVtbl>();
    let target_w = wide(target);
    let dir = wide(&Path::new(target).parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_default());
    let description = wide(description);
    let icon = icon.map(|i| match i.rsplit_once(',') {
        Some((file, n)) if n.trim().parse::<i32>().is_ok() => (crate::util::resolve(file), n.trim().parse().unwrap_or(0)),
        _ => (crate::util::resolve(i), 0),
    });
    unsafe {
        if (vt.set_path)(shell_link.0, target_w.as_ptr()) != S_OK {
            return Err("path".into());
        }
        if !args.is_empty() {
            let args = wide(args);
            (vt.set_arguments)(shell_link.0, args.as_ptr());
        }
        (vt.set_working_directory)(shell_link.0, dir.as_ptr());
        (vt.set_description)(shell_link.0, description.as_ptr());
        if let Some((file, index)) = &icon {
            let file = wide(file);
            (vt.set_icon_location)(shell_link.0, file.as_ptr(), *index);
        }
        let file = shell_link.query(&IID_IPERSIST_FILE).ok_or("no file")?;
        let path = wide(&link.to_string_lossy());
        if (file.vtbl::<PersistFileVtbl>().save)(file.0, path.as_ptr(), 1) != S_OK {
            return Err("save".into());
        }
    }
    Ok(())
}

/// Makes the Start menu shortcut to deskfolds.exe, or replaces it.
fn write_start_menu(link: &Path, exe: &str) -> Result<(), String> {
    let fail = |what: &str| format!("The Start menu shortcut could not be made ({what}).");
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(&e.to_string()))?;
    }
    write_link(link, exe, &launch_args(), "Tiny foldable shortcut panels for the desktop", Some(exe)).map_err(|e| fail(&e))
}

/// A file name made from a shortcut's name: what Windows refuses becomes "_",
/// a device name (NUL, CON...) gets a "_", and it is at most 120 characters.
fn file_name_for(name: &str) -> String {
    let clean: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).take(120).collect();
    let mut clean = clean.trim().trim_end_matches(['.', ' ']).to_string();
    if crate::util::reserved_name(&clean) {
        clean.insert(clean.find('.').unwrap_or(clean.len()), '_');
    }
    if clean.is_empty() { "Shortcut".into() } else { clean }
}

/// Makes a shortcut on the desktop `dir` named after `name`, to `target` (a
/// file, a folder, or a URI such as https://, which gets a .url file), under a
/// free name, and tells the desktop watcher it is deskfolds'. Never replaces a
/// file. Returns where it is.
pub fn make_shortcut(dir: &Path, name: &str, target: &str, args: &str, icon: Option<&str>) -> Result<PathBuf, String> {
    let fail = |what: &str| format!("A shortcut to {target} could not be made on the desktop ({what}).");
    // A line break would add lines of its own to a .url file.
    if target.chars().chain(args.chars()).chain(icon.unwrap_or("").chars()).any(char::is_control) {
        return Err(fail("its target has a control character"));
    }
    let uri = crate::util::is_uri(target);
    let file = format!("{}.{}", file_name_for(name), if uri { "url" } else { "lnk" });
    // A test copy's own Desktop folder may not be there yet.
    std::fs::create_dir_all(dir).map_err(|e| fail(&e.to_string()))?;
    let path = crate::dragdrop::free_name(dir, std::ffi::OsStr::new(&file));
    crate::watch::note_put_back(&path);
    // The name is taken first, so a file that appeared meanwhile is never replaced.
    let mut reserved = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            crate::watch::forget_put_back(&path);
            return Err(fail(&e.to_string()));
        }
    };
    let written = if uri {
        use std::io::Write;
        let text = format!("[InternetShortcut]\r\nURL={target}\r\n");
        reserved.write_all(text.as_bytes()).map_err(|e| fail(&e.to_string()))
    } else {
        drop(reserved);
        write_link(&path, target, args, "", icon).map_err(|e| fail(&e))
    };
    if let Err(e) = written {
        // Only the file made just above.
        let _ = std::fs::remove_file(&path);
        crate::watch::forget_put_back(&path);
        return Err(e);
    }
    Ok(path)
}

/// Adds or removes the Start menu shortcut.
pub fn set_start_menu(on: bool) -> Result<(), String> {
    let link = start_menu_link().ok_or("The Start menu folder was not found.")?;
    if on {
        let exe = exe_path().ok_or("The path of deskfolds.exe is unknown.")?;
        write_start_menu(&link, &exe)
    } else {
        // Only the shortcut deskfolds made: a file of that name and nothing else.
        match std::fs::remove_file(&link) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("The Start menu shortcut could not be removed: {e}")),
            _ => Ok(()),
        }
    }
}

// ---- The desktop's right-click menu ----

fn desktop_menu_key() -> String {
    match copy_suffix() {
        Some(s) => format!("{DESKTOP_MENU_KEY}.{s}"),
        None => DESKTOP_MENU_KEY.to_string(),
    }
}

fn desktop_menu_command(exe: &str) -> String {
    let args = launch_args();
    if args.is_empty() { format!("\"{exe}\" {NEW_FOLDER_ARG}") } else { format!("\"{exe}\" {args} {NEW_FOLDER_ARG}") }
}

/// The program a command starts: its first quoted part.
fn command_exe(command: &str) -> Option<&str> {
    command.strip_prefix('"')?.split('"').next()
}

fn read_value(key: &str, name: Option<&str>) -> Option<String> {
    let key = wide(key);
    let name = name.map(wide);
    let name_ptr = name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let mut size: u32 = 0;
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name_ptr, RRF_RT_REG_SZ, null_mut(), null_mut(), &mut size) != 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name_ptr, RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut bytes) != 0 {
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

fn write_value(key: &str, name: Option<&str>, value: &str) -> bool {
    let key = wide(key);
    let name = name.map(wide);
    let data = wide(value);
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
            REG_SZ,
            data.as_ptr() as *const c_void,
            (data.len() * 2) as u32,
        ) == 0
    }
}

pub fn desktop_menu_enabled() -> bool {
    read_value(&format!("{}\\command", desktop_menu_key()), None).is_some()
}

fn write_desktop_menu(exe: &str) -> bool {
    let key = desktop_menu_key();
    write_value(&key, None, DESKTOP_MENU_TEXT)
        && write_value(&key, Some("MUIVerb"), DESKTOP_MENU_TEXT)
        && write_value(&key, Some("Icon"), &format!("\"{exe}\",0"))
        && write_value(&format!("{key}\\command"), None, &desktop_menu_command(exe))
}

/// Adds or removes « New deskfolds folder » in the desktop's right-click menu.
pub fn set_desktop_menu(on: bool) -> Result<(), String> {
    if on {
        let exe = exe_path().ok_or("The path of deskfolds.exe is unknown.")?;
        if !write_desktop_menu(&exe) {
            let _ = set_desktop_menu(false);
            return Err("The desktop menu entry could not be added.".into());
        }
        Ok(())
    } else {
        let key = wide(&desktop_menu_key());
        match unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) } {
            // ERROR_FILE_NOT_FOUND: it was not there.
            0 | 2 => Ok(()),
            code => Err(format!("The desktop menu entry could not be removed (Windows error {code}).")),
        }
    }
}

/// At start: what deskfolds added to Windows follows deskfolds.exe when it
/// was moved. Only then: while the deskfolds it opens is still there, another
/// copy (an old download, for example) never takes it over.
pub fn refresh() {
    let Some(exe) = exe_path() else { return };
    let gone = |target: &str| !Path::new(target).is_file();
    if let Some(link) = start_menu_link()
        && link.is_file()
        && link_target(&link).is_some_and(|t| gone(&t) && !same_path(&t, &exe))
    {
        let _ = write_start_menu(&link, &exe);
    }
    let command = read_value(&format!("{}\\command", desktop_menu_key()), None);
    if command.as_deref().and_then(command_exe).is_some_and(gone) {
        write_desktop_menu(&exe);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_menu_starts_this_exe_with_the_new_folder_argument() {
        // The tests run as the usual copy: no APPDATA is given.
        assert_eq!(desktop_menu_command("C:\\Apps\\deskfolds.exe"), "\"C:\\Apps\\deskfolds.exe\" --new-folder");
        assert_eq!(command_exe("\"C:\\A b\\deskfolds.exe\" --appdata \"C:\\x\" --new-folder"), Some("C:\\A b\\deskfolds.exe"));
        assert_eq!(command_exe("deskfolds.exe"), None);
    }

    #[test]
    fn shortcuts_on_the_desktop_get_a_free_file_name() {
        assert_eq!(file_name_for("Projects: 2026/1"), "Projects_ 2026_1");
        assert_eq!(file_name_for(" .. "), "Shortcut");
        assert_eq!((file_name_for("NUL"), file_name_for("com1.old"), file_name_for("Console")), ("NUL_".into(), "com1_.old".into(), "Console".into()));
        assert_eq!(file_name_for(&"x".repeat(300)).len(), 120);
        unsafe { windows_sys::Win32::System::Com::CoInitializeEx(std::ptr::null(), windows_sys::Win32::System::Com::COINIT_APARTMENTTHREADED as u32) };
        let dir = std::env::temp_dir().join(format!("deskfolds-shortcut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = crate::util::expand_env("%SystemRoot%\\System32\\notepad.exe");
        let first = make_shortcut(&dir, "Notes", &target, "", None).unwrap();
        let second = make_shortcut(&dir, "Notes", &target, "", None).unwrap();
        assert_eq!((first.file_name().unwrap(), second.file_name().unwrap()), (std::ffi::OsStr::new("Notes.lnk"), std::ffi::OsStr::new("Notes (2).lnk")));
        assert!(same_path(&link_target(&first).unwrap(), &target));
        let site = make_shortcut(&dir, "Site", "https://example.com", "", None).unwrap();
        assert!(std::fs::read_to_string(&site).unwrap().contains("URL=https://example.com"));
        // A line break in the target is refused, and nothing is left.
        assert!(make_shortcut(&dir, "Bad", "https://x\r\nIconFile=y", "", None).is_err());
        assert!(!dir.join("Bad.url").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copy_names_are_the_same_in_every_build() {
        assert_eq!(stable_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(stable_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }
}
