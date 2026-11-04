//! Small helpers for native Windows UI: dark mode, the standard dialogs
//! (task dialog, file, color and folder pickers) and popup menus.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, MAX_PATH, POINT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows_sys::Win32::UI::Controls::Dialogs::{
    CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW, GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER,
    OFN_FILEMUSTEXIST, OFN_NODEREFERENCELINKS, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Controls::{
    SetWindowTheme, TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0, TD_INFORMATION_ICON, TD_WARNING_ICON,
    TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TDF_POSITION_RELATIVE_TO_WINDOW, TDF_USE_COMMAND_LINKS,
    TaskDialogIndirect,
};
use windows_sys::Win32::UI::Shell::{BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, IDCANCEL, IsWindowVisible, MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, AppendMenuW, WM_NULL,
};

use crate::util::wide;

/// Undocumented uxtheme exports, stable since Windows 10 1903, that let a
/// Win32 app use the dark theme. Missing on older systems: nothing happens.
fn uxtheme(ordinal: u16) -> Option<unsafe extern "system" fn() -> isize> {
    let name = wide("uxtheme.dll");
    unsafe {
        let module = LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if module.is_null() {
            return None;
        }
        GetProcAddress(module, ordinal as usize as *const u8)
    }
}

/// Dark popup menus and dark common controls for the whole app.
pub fn enable_dark_mode() {
    const SET_PREFERRED_APP_MODE: u16 = 135;
    const FLUSH_MENU_THEMES: u16 = 136;
    const FORCE_DARK: i32 = 2;
    unsafe {
        if let Some(f) = uxtheme(SET_PREFERRED_APP_MODE) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(FORCE_DARK);
        }
        if let Some(f) = uxtheme(FLUSH_MENU_THEMES) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

fn allow_dark(hwnd: HWND) {
    const ALLOW_DARK_MODE_FOR_WINDOW: u16 = 133;
    unsafe {
        if let Some(f) = uxtheme(ALLOW_DARK_MODE_FOR_WINDOW) {
            let allow: unsafe extern "system" fn(HWND, i32) -> i32 = std::mem::transmute(f);
            allow(hwnd, 1);
        }
    }
}

/// Dark title bar for a top-level window.
pub fn dark_window(hwnd: HWND) {
    allow_dark(hwnd);
    let on: i32 = 1;
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE as _, &on as *const i32 as *const c_void, 4);
    }
}

/// Dark theme for a child control. Edits use the file dialog's dark theme,
/// everything else Explorer's.
pub fn dark_control(hwnd: HWND, edit: bool) {
    allow_dark(hwnd);
    let theme = wide(if edit { "DarkMode_CFD" } else { "DarkMode_Explorer" });
    unsafe { SetWindowTheme(hwnd, theme.as_ptr(), null()) };
}

pub struct Ask<'a> {
    pub instruction: &'a str,
    pub content: &'a str,
    /// Custom buttons: (id, text). With `links`, a line after "\n" is the note under it.
    pub buttons: &'a [(i32, &'a str)],
    pub default_button: i32,
    pub radios: &'a [(i32, &'a str)],
    pub default_radio: i32,
    pub links: bool,
    /// Adds a Cancel button and lets Esc or the close button cancel.
    pub cancel: bool,
    pub warning: bool,
    pub footer: Option<&'a str>,
}

impl Default for Ask<'_> {
    fn default() -> Self {
        Self {
            instruction: "",
            content: "",
            buttons: &[],
            default_button: 0,
            radios: &[],
            default_radio: 0,
            links: false,
            cancel: true,
            warning: false,
            footer: None,
        }
    }
}

/// Shows a task dialog. Returns (button id, radio id); IDCANCEL if cancelled.
pub fn ask(owner: HWND, a: &Ask) -> (i32, i32) {
    let title = wide("deskfolds");
    let instruction = wide(a.instruction);
    let content = wide(a.content);
    let footer = a.footer.map(wide);
    let button_texts: Vec<Vec<u16>> = a.buttons.iter().map(|(_, t)| wide(t)).collect();
    let radio_texts: Vec<Vec<u16>> = a.radios.iter().map(|(_, t)| wide(t)).collect();
    let buttons: Vec<TASKDIALOG_BUTTON> = a
        .buttons
        .iter()
        .zip(&button_texts)
        .map(|((id, _), text)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: text.as_ptr() })
        .collect();
    let radios: Vec<TASKDIALOG_BUTTON> = a
        .radios
        .iter()
        .zip(&radio_texts)
        .map(|((id, _), text)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: text.as_ptr() })
        .collect();

    let mut flags = 0;
    // Centered on its owner, unless the owner is the hidden main window.
    if !owner.is_null() && unsafe { IsWindowVisible(owner) } != 0 {
        flags |= TDF_POSITION_RELATIVE_TO_WINDOW;
    }
    if a.cancel {
        flags |= TDF_ALLOW_DIALOG_CANCELLATION;
    }
    if a.links {
        flags |= TDF_USE_COMMAND_LINKS;
    }
    let mut cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        dwFlags: flags,
        dwCommonButtons: if a.cancel { TDCBF_CANCEL_BUTTON } else { 0 },
        pszWindowTitle: title.as_ptr(),
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: if a.warning { TD_WARNING_ICON } else { TD_INFORMATION_ICON } },
        pszMainInstruction: instruction.as_ptr(),
        pszContent: content.as_ptr(),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: a.default_button,
        cRadioButtons: radios.len() as u32,
        pRadioButtons: radios.as_ptr(),
        nDefaultRadioButton: a.default_radio,
        ..Default::default()
    };
    if let Some(f) = &footer {
        cfg.pszFooter = f.as_ptr();
    }
    let (mut button, mut radio) = (0, 0);
    if unsafe { TaskDialogIndirect(&cfg, &mut button, &mut radio, null_mut()) } < 0 {
        return (IDCANCEL, 0);
    }
    (button, radio)
}

/// Asks to confirm an action: true if the user clicked `action`.
pub fn confirm(owner: HWND, instruction: &str, content: &str, action: &str) -> bool {
    const ACTION: i32 = 1000;
    let (button, _) = ask(
        owner,
        &Ask { instruction, content, buttons: &[(ACTION, action)], default_button: ACTION, warning: true, ..Default::default() },
    );
    button == ACTION
}

/// The Windows "Open" dialog. `filter` is pairs of (label, patterns).
/// Shortcuts (.lnk) are returned as themselves, not as their target.
pub fn open_files(owner: HWND, title: &str, filter: &[(&str, &str)], multiple: bool) -> Vec<String> {
    let filter: String = filter.iter().map(|(label, pattern)| format!("{label}\0{pattern}\0")).collect();
    let filter = wide(&filter);
    let title = wide(title);
    let mut buf = vec![0u16; 32 * 1024];
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_NODEREFERENCELINKS | if multiple { OFN_ALLOWMULTISELECT } else { 0 };
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return Vec::new();
    }
    split_selection(&buf)
}

/// One file: "C:\dir\file.txt\0\0". Several: "C:\dir\0a.txt\0b.txt\0\0".
fn split_selection(buf: &[u16]) -> Vec<String> {
    let parts: Vec<String> = buf.split(|&c| c == 0).take_while(|s| !s.is_empty()).map(String::from_utf16_lossy).collect();
    match parts.as_slice() {
        [] => Vec::new(),
        [one] => vec![one.clone()],
        [dir, files @ ..] => files.iter().map(|f| format!("{}\\{f}", dir.trim_end_matches('\\'))).collect(),
    }
}

thread_local! {
    static CUSTOM_COLORS: Cell<[u32; 16]> = const { Cell::new([0x00FFFFFF; 16]) };
}

/// The Windows color picker. Colors are GDI COLORREFs (0x00BBGGRR).
pub fn choose_color(owner: HWND, initial: u32) -> Option<u32> {
    let mut custom = CUSTOM_COLORS.get();
    let mut cc: CHOOSECOLORW = unsafe { std::mem::zeroed() };
    cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
    cc.hwndOwner = owner;
    cc.rgbResult = initial;
    cc.lpCustColors = custom.as_mut_ptr();
    cc.Flags = CC_RGBINIT | CC_FULLOPEN;
    let chosen = unsafe { ChooseColorW(&mut cc) } != 0;
    CUSTOM_COLORS.set(custom);
    chosen.then_some(cc.rgbResult)
}

/// The Windows folder picker.
pub fn choose_folder(owner: HWND, title: &str) -> Option<String> {
    let title = wide(title);
    let mut name = vec![0u16; MAX_PATH as usize];
    let info = BROWSEINFOW {
        hwndOwner: owner,
        pidlRoot: null_mut(),
        pszDisplayName: name.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_NEWDIALOGSTYLE | BIF_RETURNONLYFSDIRS,
        lpfn: None,
        lParam: 0,
        iImage: 0,
    };
    unsafe {
        let pidl = SHBrowseForFolderW(&info);
        if pidl.is_null() {
            return None;
        }
        let mut path = vec![0u16; 32 * 1024];
        let ok = SHGetPathFromIDListW(pidl, path.as_mut_ptr()) != 0;
        CoTaskMemFree(pidl as *const c_void);
        let len = path.iter().position(|&c| c == 0).unwrap_or(0);
        (ok && len > 0).then(|| String::from_utf16_lossy(&path[..len]))
    }
}

/// A menu entry: (command id, text, flags). Id 0 with MF_SEPARATOR is a line.
pub type MenuEntry<'a> = (usize, &'a str, u32);

pub const SEPARATOR: MenuEntry<'static> = (0, "", MF_SEPARATOR);

/// Shows a popup menu at `at` (screen pixels) and returns the chosen id, or 0.
pub fn popup_menu(owner: HWND, entries: &[MenuEntry], at: POINT) -> usize {
    unsafe {
        let menu = CreatePopupMenu();
        for (id, text, flags) in entries {
            let text = wide(text);
            AppendMenuW(menu, MF_STRING | flags, *id, text.as_ptr());
        }
        // The owner must be in front, or the menu does not close when clicking elsewhere.
        SetForegroundWindow(owner);
        let choice = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, at.x, at.y, 0, owner, null());
        PostMessageW(owner, WM_NULL, 0, 0);
        DestroyMenu(menu);
        choice.max(0) as usize
    }
}

/// Menu text shows "&" as an underline: double it to show it as is.
pub fn menu_text(s: &str) -> String {
    s.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0, 0]).collect()
    }

    #[test]
    fn file_selections_are_split() {
        assert_eq!(split_selection(&buffer("C:\\a\\b.txt")), ["C:\\a\\b.txt"]);
        assert_eq!(split_selection(&buffer("C:\\a\0b.txt\0c.lnk")), ["C:\\a\\b.txt", "C:\\a\\c.lnk"]);
        assert_eq!(split_selection(&buffer("C:\\\0b.txt\0c.lnk")), ["C:\\b.txt", "C:\\c.lnk"]);
        assert!(split_selection(&[0, 0]).is_empty());
    }

    #[test]
    fn ampersands_show_in_menus() {
        assert_eq!(menu_text("Tools & games"), "Tools && games");
    }
}
