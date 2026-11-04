# deskfolds

Tiny foldable shortcut panels for the Windows desktop.

Each panel is a small frosted-glass folder label on the desktop. Hover it and
it unrolls a glass card of shortcuts; move the mouse away and it folds back.
Click its name to pin it open. Panels are locked in place: they don't move
unless you unlock them.

- **Native and light**: one small program, with no framework or runtime to
  install. About 560 KB on disk, 5 to 10 MB of memory, and no CPU use while
  idle (it only wakes up when you use it).
- **Locked in place**: panels don't move while locked, and other programs
  cannot move them either.
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
- **Safe saves**: saved at once, atomically, with the last 5 versions kept
  as backups.
  A hand edit of the config file is never silently overwritten.
- **Never deletes a file**: files dragged into a folder are moved there, and
  no file is ever deleted or overwritten.
- **DPI aware**: panels and the settings window scale with each monitor's DPI.

## Requirements

- Windows 11, or Windows 10 version 1903 or later. deskfolds is tested on
  Windows 11.
- To build it: [Rust](https://rustup.rs) (1.88 or later) and Microsoft's C++
  build tools. Step 1 below installs both.

There is no ready-made download yet: you build deskfolds from its source code.
It takes a few minutes the first time.

## Install

The commands below are for PowerShell or Windows Terminal.

### 1. Install the build tools (once)

```powershell
winget install --id Microsoft.VisualStudio.BuildTools -e --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Rustlang.Rustup -e
winget install --id Git.Git -e
```

- The first command installs the Visual Studio C++ build tools that Rust
  needs on Windows. It is a large download and takes a while: accept the
  Windows permission prompt, and restart the PC if the installer asks.
- The second installs Rust. If it asks a question in the terminal, press
  Enter to accept the default.
- The third installs Git, used to download the code and its updates.

Skip any line for a tool you already have. Then close the terminal and open
a new one, so that the new commands are found, and check that Rust is ready:

```powershell
cargo --version
```

It should print a version number.

### 2. Get the code and build it

```powershell
cd $HOME
git clone https://github.com/Matthieu1999/deskfolds.git
cd deskfolds
cargo build --release
```

This creates `deskfolds\target\release\deskfolds.exe`.

If the build stops with an error saying that `link.exe` was not found, the C++
build tools are missing: run the first command of step 1 again, or open the
Visual Studio Installer and add **Desktop development with C++**.

### 3. Start it

```powershell
.\target\release\deskfolds.exe
```

You can also double-click `deskfolds.exe` in File Explorer. deskfolds has no
window and no taskbar button: you use it through its panels, its tray icon
and its settings window.

Two sample folders appear at the top left of the desktop, and a folder icon
appears in the notification area, next to the clock. If you don't see the
icon, it may be hidden under the **^** arrow; to keep it visible, turn it on
in Windows Settings > Personalization > Taskbar > Other system tray icons.

Only one copy of deskfolds runs at a time: starting it again does nothing.

### 4. Start it with Windows

Tray menu > **Start with Windows**, or Settings > Behavior > **Start with
Windows**.

Windows then starts deskfolds from the place it is in now. Keep the
`deskfolds` folder where it is: if you move or delete it, deskfolds no longer
starts with Windows. To use another place, quit deskfolds, copy
`deskfolds.exe` there, start that copy, and turn **Start with Windows** off
and on again from it.

### Update

1. Quit deskfolds: tray menu > **Quit**. Windows does not let the build
   replace a program that is running.
2. Get the new code and build it:

   ```powershell
   cd $HOME\deskfolds
   git pull
   cargo build --release
   ```

3. Start `target\release\deskfolds.exe` again. Your folders and settings are
   kept.

### Uninstall

1. Turn **Start with Windows** off, then quit deskfolds. (If you forgot and
   already deleted it, turn deskfolds off in Windows Settings > Apps >
   Startup.)
2. Get back the files you dragged into folders. They are in each folder's
   storage folder: by default `deskfolds\<folder name>` inside your Documents
   folder (which may be in OneDrive). Deleting a folder in Settings with
   **Move them back to the desktop** also brings them back.
3. Delete the `deskfolds` folder you built it in, and, if you no longer want
   your settings, `%APPDATA%\deskfolds`.
4. If you installed the build tools only for deskfolds, remove them with
   `winget uninstall` and the same ids as in step 1.

## Use

| Action | How |
|---|---|
| Open a panel | Hover it (it closes when the mouse leaves). With hover turned off in Settings, click its name to open or close it |
| Keep a panel open | Click its name (a blue dot shows it is pinned); click again to unpin |
| Launch a shortcut | Click it |
| Open the menu | Right-click a panel or the tray icon, or click the tray icon once |
| Open Settings | Double-click the tray icon, or menu > **Settings…** (from a panel's menu: **Edit '<name>'…**) |
| Create a folder | Menu > **New folder**, or Settings > Folders > **New folder** |
| Delete a folder | Settings > Folders > **Delete…**. Files you moved into it go back to the desktop, or stay in its storage folder if you choose so |
| Rename, resize, reorder folders | Settings > Folders: name, width, columns, position, ▲ ▼ |
| Add a shortcut | Drag a file, folder or shortcut onto a panel (a closed panel opens) or onto the shortcut list in Settings. It is **moved** into the folder's storage folder; hold **Ctrl** to keep it where it is and only add a link. Works even while locked. Settings > **Add…** adds links |
| Edit a shortcut | Settings > Folders: select it, then change its name, arguments or icon |
| Edit panels on the desktop | Menu > **Unlock panels (move and edit)**, or Settings > Folders > **Move on the desktop**: every panel opens with a blue border |
| Move a panel | While unlocked, drag it by its name |
| Resize a panel | While unlocked, drag its right edge |
| Reorder or move shortcuts | While unlocked, drag a shortcut within the panel or onto another panel |
| Take a shortcut out | While unlocked, drag it onto the desktop or an Explorer window. A file that was moved into the folder goes there. For a link, a shortcut to it is created there and the original stays where it is. Hold **Ctrl** to copy it and keep it in the panel |
| Rename or remove a shortcut | While unlocked, right-click it. **Remove from folder** puts a moved file back on the desktop; a link is just removed |
| Finish editing | Menu > **Lock panels**, or **Done** in Settings |
| Undo a mistake | Settings > About & advanced > **Previous versions** > **Restore…** |

deskfolds never deletes a file: it only moves files, and never overwrites one
(a name already taken becomes "name (2)").

## The settings window

Every change is saved as soon as you make it: when you press Enter or leave a
field, release a slider, or click a button. There is no Save button. **Tab**
moves between fields and **Enter** saves the field you are typing in. **Esc**
puts back the previous value of that field; when you are not typing, it
closes the window.

The folder selected in Settings stays open and outlined on the desktop, so you
can see what you are editing.

- **Folders**: the list of folders, with **New folder**, ▲ ▼ to reorder, and
  for the selected folder:
  - its name, icon, width, number of columns and position (in screen pixels);
  - **Move on the desktop** to unlock all folders and drag them, then **Done**;
  - its storage folder, with **Open** to show it in Explorer;
  - its shortcuts: **Add…**, **Remove**, ▲ ▼, **Open file location**, and
    for the selected shortcut its name, arguments and icon. You can also drop
    files on the list.
- **Appearance**: three presets (**Light glass**, **Dark glass**, **Solid**),
  the glass tint and its strength, the accent, text, hover and border colors,
  the text and icon sizes, and the open/close animation.
- **Behavior**: open folders on hover (when off, a folder opens and closes by
  clicking its name), the delays before opening and closing, close after
  launching a shortcut, lock folders in place, start with Windows, and the
  storage folder for new folders.
- **About & advanced**: the config file, with **Open in Notepad** and
  **Reload**; the previous versions, with **Restore…**; and **Reset…** for the
  appearance and behavior (folders and shortcuts are kept).

## Configuration

Everything can be set in the settings window. Behind it, the config file is
`%APPDATA%\deskfolds\config.toml`. It is created from
[config.example.toml](config.example.toml) on first run, and that file
documents every option.

deskfolds keeps the data in memory and is the only one writing the file. Each
save goes to a temporary file that then replaces the config, so a crash never
leaves it half written. Backups go to `%APPDATA%\deskfolds\backups\`: the
previous version is saved at most once a minute while you make changes, and
always before a restore, a reset, or a choice between the file and the
screen. The last 5 backups are kept. If you edit the file by hand,
deskfolds notices before its next save (or when you open the menu or
Settings) and asks whether to load the file or keep the folders on screen;
the other version is saved as a backup. An invalid file is never loaded:
the folders on screen stay as they are, and at startup deskfolds shows the
newest valid backup instead. A missing file is restored from the newest
valid backup.

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
the files dropped into the panel. deskfolds picks it when the panel is created
(or, in an older config without it, when it is first loaded) and writes it on
the next save. While the folder holds no files, it follows
the panel's name; once it holds files, it never changes, so renaming a panel
does not split its files.

Item fields: `name`, `path`, optional `args`, and optional `icon` (a file
whose icon is shown instead of the target's).

## How it works

Each panel is a borderless `WS_POPUP` tool window owned by the desktop window
(`Progman`). Being owned by the desktop is what keeps it visible after
*Show desktop*. The window handles `WM_WINDOWPOSCHANGING` to stay at the bottom
of the z-order and to pin its position while locked. It answers
`WM_MOUSEACTIVATE` with `MA_NOACTIVATE`, so clicking it never steals focus
(`WS_EX_NOACTIVATE` would stop other programs from dropping files on it).
The glass is the DWM acrylic accent (`SetWindowCompositionAttribute`).
Drawing is double-buffered GDI into a 32-bit buffer whose alpha is rebuilt so
text and highlights show on the glass, and icons come from the shell image
lists.

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
- So far tested only at 100 % display scaling.

## License

[MIT](LICENSE)
