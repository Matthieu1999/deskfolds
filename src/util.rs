use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{MAX_PATH, RECT};
use windows_sys::Win32::Graphics::Gdi::HFONT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE, VK_RETURN};
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_NORMAL, SearchPathW};
use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::UI::Controls::{EM_SETSEL, ILD_TRANSPARENT, ImageList_GetIcon};
use windows_sys::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE,
    SHIL_JUMBO, SHIL_LARGE, SHIL_SMALL, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use windows_sys::core::GUID;

const IID_IIMAGELIST: GUID = GUID::from_u128(0x46eb5926_582e_4017_9fdf_e8998daa0950);
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "deskfolds";

pub fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

pub fn error_box(text: &str) {
    let text = wide(text);
    let title = wide("deskfolds");
    unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
}

/// Shows a one-line text box over `rect` (screen pixels). Enter confirms,
/// Esc or clicking elsewhere cancels.
pub fn prompt_text(rect: RECT, initial: &str, font: HFONT) -> Option<String> {
    let class = wide("EDIT");
    let text = wide(initial);
    unsafe {
        let edit = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            text.as_ptr(),
            WS_POPUP | WS_BORDER | WS_VISIBLE | ES_AUTOHSCROLL as u32,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            null_mut(),
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if edit.is_null() {
            return None;
        }
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        SetForegroundWindow(edit);
        SetFocus(edit);
        SendMessageW(edit, EM_SETSEL, 0, -1);

        let mut confirmed = false;
        let mut msg: MSG = std::mem::zeroed();
        loop {
            if GetMessageW(&mut msg, null_mut(), 0, 0) <= 0 {
                PostQuitMessage(0);
                break;
            }
            if msg.hwnd == edit && msg.message == WM_KEYDOWN {
                if msg.wParam == VK_RETURN as usize {
                    confirmed = true;
                    break;
                }
                if msg.wParam == VK_ESCAPE as usize {
                    break;
                }
            }
            let click = matches!(msg.message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_NCLBUTTONDOWN);
            if click && msg.hwnd != edit {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if GetForegroundWindow() != edit {
                break;
            }
        }

        let len = GetWindowTextLengthW(edit);
        let mut buf = vec![0u16; len as usize + 1];
        GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32);
        DestroyWindow(edit);
        let value = String::from_utf16_lossy(&buf[..len as usize]).trim().to_string();
        (confirmed && !value.is_empty()).then_some(value)
    }
}

pub fn expand_env(s: &str) -> String {
    let src = wide(s);
    let mut buf = vec![0u16; 1024];
    let n = unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize > buf.len() {
        return s.to_string();
    }
    String::from_utf16_lossy(&buf[..n as usize - 1])
}

/// True for "https://...", "ms-settings:" and the like, but not for "C:\...".
fn is_uri(s: &str) -> bool {
    match s.find(':') {
        Some(i) if i > 1 => s[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)),
        _ => false,
    }
}

/// Expands environment variables and finds bare program names in PATH.
pub fn resolve(path: &str) -> String {
    let expanded = expand_env(path.trim());
    if is_uri(&expanded) || Path::new(&expanded).is_absolute() {
        return expanded;
    }
    // A bare name ("notepad.exe") is looked up in the Windows folders and PATH
    // only, never in the current folder or next to deskfolds, where a file of
    // the same name could have been put.
    let search = wide(&safe_search_path());
    let name = wide(&expanded);
    let mut buf = vec![0u16; MAX_PATH as usize * 4];
    let n = unsafe { SearchPathW(search.as_ptr(), name.as_ptr(), null(), buf.len() as u32, buf.as_mut_ptr(), null_mut()) };
    if n > 0 && (n as usize) < buf.len() {
        return String::from_utf16_lossy(&buf[..n as usize]);
    }
    expanded
}

/// The folders where a bare program name is looked for: System32, Windows, then
/// the absolute folders of PATH.
fn safe_search_path() -> String {
    let mut dirs = vec![expand_env("%SystemRoot%\\System32"), expand_env("%SystemRoot%")];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path).filter(|p| p.is_absolute()).map(|p| p.to_string_lossy().to_string()));
    }
    dirs.join(";")
}

/// Returns the shell icon of a file, folder or URI, or null if none is found.
pub fn load_icon(path: &str, size: i32) -> HICON {
    let target = resolve(path);
    let mut info: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let info_size = std::mem::size_of::<SHFILEINFOW>() as u32;
    let found = if is_uri(&target) {
        0
    } else {
        let name = wide(&target);
        unsafe { SHGetFileInfoW(name.as_ptr(), 0, &mut info, info_size, SHGFI_SYSICONINDEX) }
    };
    if found == 0 {
        // Missing file or URI: fall back to the generic icon for its type.
        let ext = if is_uri(&target) {
            ".url".to_string()
        } else {
            Path::new(&target).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default()
        };
        let name = wide(&ext);
        let flags = SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES;
        if unsafe { SHGetFileInfoW(name.as_ptr(), FILE_ATTRIBUTE_NORMAL, &mut info, info_size, flags) } == 0 {
            return null_mut();
        }
    }
    let list = match size {
        ..=16 => SHIL_SMALL,
        17..=32 => SHIL_LARGE,
        33..=48 => SHIL_EXTRALARGE,
        _ => SHIL_JUMBO,
    };
    let mut image_list: *mut c_void = null_mut();
    if unsafe { SHGetImageList(list as i32, &IID_IIMAGELIST, &mut image_list) } < 0 {
        return null_mut();
    }
    // The IImageList pointer is documented as usable as an HIMAGELIST.
    unsafe { ImageList_GetIcon(image_list as _, info.iIcon, ILD_TRANSPARENT) }
}

pub fn launch(path: &str, args: &str) {
    let target = resolve(path);
    let dir = if is_uri(&target) {
        None
    } else {
        Path::new(&target).parent().map(|p| wide(&p.to_string_lossy()))
    };
    let file = wide(&target);
    let args = (!args.is_empty()).then(|| wide(args));
    let verb = wide("open");
    unsafe {
        AllowSetForegroundWindow(ASFW_ANY);
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            args.as_ref().map_or(null(), |a| a.as_ptr()),
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            SW_SHOWNORMAL,
        );
    }
}

pub fn open_in_notepad(path: &Path) {
    launch("notepad.exe", &format!("\"{}\"", path.display()));
}

pub fn autostart_enabled() -> bool {
    let key = wide(RUN_KEY);
    let value = wide(RUN_VALUE);
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), null_mut())
            == 0
    }
}

pub fn set_autostart(enabled: bool) {
    let key = wide(RUN_KEY);
    let value = wide(RUN_VALUE);
    unsafe {
        if enabled {
            let Ok(exe) = std::env::current_exe() else { return };
            let data = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr() as *const c_void,
                (data.len() * 2) as u32,
            );
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_detection() {
        assert!(is_uri("ms-settings:"));
        assert!(is_uri("https://example.com"));
        assert!(!is_uri("C:\\Windows\\notepad.exe"));
        assert!(!is_uri("notepad.exe"));
    }

    #[test]
    fn bare_names_are_found_in_path() {
        assert!(resolve("notepad.exe").to_lowercase().ends_with("\\notepad.exe"));
        assert!(Path::new(&resolve("notepad.exe")).exists());
    }
}
