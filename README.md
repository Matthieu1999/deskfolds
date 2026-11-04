# deskfolds

Tiny foldable shortcut panels for the Windows desktop.

Each panel is a small frosted-glass folder label on the desktop. Hover it and
it unrolls a glass card of shortcuts; move the mouse away and it folds back.
Click its name to pin it open. Panels are locked in place: they don't move
unless you unlock them.

- **Native and light**: Rust + raw Win32/GDI, no UI framework, no runtime.
  About 400 KB on disk, about 5 MB of private memory, and no CPU use while idle
  (it only wakes up on input).
- **Locked in place**: position changes are refused while locked, even ones
  requested by other programs.
- **Stays on the desktop**: always below normal windows, never takes focus,
  no taskbar button, and stays visible after *Show desktop* (Win+D).
- **Any shortcut**: programs, documents, folders, `.lnk` files and URIs
  (`https://…`, `ms-settings:…`), with their real Windows icons.
- **Glass look**: the blurred acrylic background used by the Start menu,
  with a configurable tint and a short open/close animation (CPU is only used
  during the 200 ms animation).
- **Settings window**: create, delete, resize and arrange folders, edit their
  shortcuts, and change the look and behavior, without touching a config
  file. Native Windows controls in dark mode, built only while it is open.
- **Safe saves**: saved at once, atomically, with the last 5 versions kept.
  A hand edit of the config file is never silently overwritten.
- **DPI aware**: sharp on high-DPI and mixed-DPI multi-monitor setups.

## Install

Build it (requires [Rust](https://rustup.rs) and the MSVC build tools):

```
cargo build --release
```

Then run `target\release\deskfolds.exe`. A folder icon appears in the
notification area. To start it with Windows, use its tray menu:
**Start with Windows**.

Right-click the tray icon (or click it once) for the menu; double-click it to
open **Settings**.

## Use

| Action | How |
|---|---|
| Open a panel | Hover it (it closes when the mouse leaves) |
| Keep a panel open | Click its name (a blue dot shows it is pinned); click again to unpin |
| Launch a shortcut | Click it |
| Open the menu | Right-click a panel or the tray icon, or click the tray icon once |
| Open Settings | Double-click the tray icon, or menu > **Settings…** (from a panel: **Edit '…'**) |
| Create a folder | Menu > **New folder**, or Settings > Folders > **New folder** |
| Delete a folder | Settings > Folders > **Delete…**. Files you moved into it go back to the desktop, or stay in its storage folder if you choose so |
| Rename, resize, reorder folders | Settings > Folders: name, width, columns, position, ▲ ▼ |
| Add a shortcut | Drag a file, folder or shortcut onto a panel (a closed panel opens) or onto the shortcut list in Settings. It is **moved** into the folder's storage folder; hold **Ctrl** to keep it where it is and only add a link. Works even while locked. Settings > **Add…** adds links |
| Edit a shortcut | Settings > Folders: select it, then change its name, arguments or icon |
| Edit panels on the desktop | Menu > **Unlock panels**: every panel opens with a blue border |
| Move a panel | While unlocked, drag it by its name |
| Resize a panel | While unlocked, drag its right edge |
| Reorder or move shortcuts | While unlocked, drag a shortcut within the panel or onto another panel |
| Take a shortcut out | While unlocked, drag it onto the desktop or an Explorer window: the file goes there |
| Rename or remove a shortcut | While unlocked, right-click it. **Remove from folder** puts a moved file back on the desktop; a link is just removed |
| Finish editing | Menu > **Lock panels** |
| Undo a mistake | Settings > About & advanced > **Previous versions** > **Restore…** |

deskfolds never deletes a file: it only moves files, and never overwrites one
(a name already taken becomes "name (2)").

## Configuration

Everything can be set in the settings window. Behind it, the config file is
`%APPDATA%\deskfolds\config.toml`. It is created from
[config.example.toml](config.example.toml) on first run, and that file
documents every option.

deskfolds keeps the data in memory and is the only one writing the file. Each
save goes to a temporary file that then replaces the config, so a crash never
leaves it half written, and the previous version is kept in
`%APPDATA%\deskfolds\backups\` (the last 5). If you edit the file by hand,
deskfolds notices before its next save (or when you open the menu or
Settings) and asks whether to load the file or keep the folders on screen;
the other version is saved as a backup. An invalid file never makes the
folders disappear, and a missing file is restored from the latest backup.

```toml
[settings]
hover_open = true
open_delay_ms = 150
close_delay_ms = 400
animation_ms = 200
tint = "#FFFFFF"
tint_opacity = 0.10

[[panel]]
title = "Apps"
x = 40
y = 40
width = 300
columns = 4

[[panel.item]]
name = "Notepad"
path = "notepad.exe"

[[panel.item]]
name = "Settings"
path = "ms-settings:"
icon = "%SystemRoot%\\ImmersiveControlPanel\\SystemSettings.exe"
```

Panel fields: `title`, `x`, `y`, `width`, `columns`, optional `icon` (shown
next to the name instead of a folder), and `storage`, the folder that holds
the files dropped into the panel. It is chosen once when the panel is created
and never renamed, so renaming a panel does not split its files.

Item fields: `name`, `path`, optional `args`, and optional `icon` (a file
whose icon is shown instead of the target's).

## How it works

Each panel is a borderless `WS_POPUP` tool window owned by the desktop window
(`Progman`). Being owned by the desktop is what keeps it visible after
*Show desktop*. The window handles `WM_WINDOWPOSCHANGING` to stay at the bottom
of the z-order and to pin its position while locked. It answers
`WM_MOUSEACTIVATE` with `MA_NOACTIVATE`, so clicking it never steals focus
(`WS_EX_NOACTIVATE` would stop other programs from dropping files on it). The glass is the DWM acrylic accent
(`SetWindowCompositionAttribute`). Drawing is double-buffered GDI into a 32-bit
buffer whose alpha is rebuilt so text and highlights show on the glass, and
icons come from the shell image lists.

The data lives in one store (`src/store.rs`) that every change goes through:
panels, drag and drop and the settings window all call it, and it tells the
app which panel to redraw. Panels and shortcuts get ids in memory only, so a
change always reaches the right one and the config file stays clean.

## Limitations

- Panels unfold downwards. A panel near the bottom of the screen unfolds
  partly off-screen.
- Positions are screen pixels. When a monitor is unplugged, its panels show
  on the nearest one; they return when it comes back.
- The settings window is always dark. Windows' own dialogs (color picker,
  file picker, confirmations) follow their own theme.

## License

[MIT](LICENSE)
