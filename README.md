# deskfolds

Tiny foldable shortcut panels for the Windows desktop.

Each panel is a small frosted-glass folder label on the desktop, as wide as
the folder. Hover it and it unrolls a glass card of shortcuts; move the mouse
away and it folds back. Click its name to pin it open. Panels are locked in
place: they don't move unless you unlock them.

- **Native and light**: one small program, with no framework or runtime to
  install: [download](#option-a-download-it) one file and run it. About 750 KB
  on disk, about 5 MB of memory, and no CPU use while idle (it only wakes up
  when you use it). See [Size and memory](#size-and-memory).
- **Locked in place**: panels don't move while locked, and other programs
  cannot move them either.
- **Stays on the desktop**: always below normal windows, never takes focus,
  no taskbar button, and stays visible after *Show desktop* (Win+D).
- **Easy to arrange**: folders snap next to each other while you move them.
  An **arrangement** keeps them in a row, a column or a grid of as many
  columns or rows as you want, in a corner, on an edge or in the center of a
  screen; drag its frame to move them all at once. An opening folder pushes
  the folders stacked under or above it out of its way instead of covering
  them. Folders that touch open the same way, upwards near the bottom of the
  screen, and are never pushed off it.
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
- Nothing else to use the ready-made download.
- To build it yourself: [Rust](https://rustup.rs) (1.88 or later) and
  Microsoft's C++ build tools. Step 1 of [Option B](#option-b-build-it-from-its-source-code)
  installs them, along with Git.

## Install

There are two ways, and both give the same program:

- **Option A: download it.** The easiest: one file, nothing to install.
- **Option B: build it from its source code.** It takes a few minutes the
  first time.

### Option A: download it

1. Open the [latest release](https://github.com/Matthieu1999/deskfolds/releases/latest)
   and download **deskfolds.exe**.
2. Put it in a folder of its own that you keep, for example
   `%LOCALAPPDATA%\Programs\deskfolds`. To open that place, paste it in the
   address bar of File Explorer (create the `deskfolds` folder if it is not
   there). **Start with Windows** remembers where the file is, so choose its
   place before you turn it on.
3. Double-click **deskfolds.exe**. The first time, Windows may show
   **Windows protected your PC**: deskfolds is not signed with a paid
   certificate, so Windows does not know it yet. Click **More info**, then
   **Run anyway**. Windows usually does not ask again for that file.

Then see [First start](#first-start).

- If **Smart App Control** is on (Windows Security > App & browser control),
  Windows may block deskfolds with no **Run anyway** button: it does not let
  unsigned programs it does not know run. deskfolds cannot run on that PC
  while it is on.
- An antivirus may also be wary of a new, unsigned program at first.

To check that the download is not damaged (optional), run this in PowerShell
in the folder of the file, with the SHA-256 shown on the release page between
the quotes. It prints `True` when they are the same:

```powershell
(Get-FileHash .\deskfolds.exe).Hash -eq "PASTE-THE-SHA-256-HERE"
```

deskfolds.exe needs nothing else: no .NET, no Visual C++ Redistributable. It
is built from this repository by GitHub, on GitHub's own machines, each time a
version is released (see [Making a release](#making-a-release)).

### Option B: build it from its source code

The commands below are for PowerShell or Windows Terminal.

#### 1. Install the build tools (once)

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

#### 2. Get the code and build it

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

#### 3. Start it

```powershell
.\target\release\deskfolds.exe
```

You can also double-click `deskfolds.exe` in File Explorer.

### First start

deskfolds has no window and no taskbar button: you use it through its
panels, its tray icon and its settings window.

Two sample folders appear at the top left of the desktop, and a folder icon
appears in the notification area, next to the clock. If you don't see the
icon, it may be hidden under the **^** arrow; to keep it visible, turn it on
in Windows Settings > Personalization > Taskbar > Other system tray icons.

Only one copy of deskfolds runs at a time: starting it again does nothing.

### Start it with Windows

Tray menu > **Start with Windows**, or Settings > Behavior > **Start with
Windows**.

Windows then starts deskfolds from the place it is in now. Keep
`deskfolds.exe` (or the `deskfolds` folder you built it in) where it is: if
you move or delete it, deskfolds no longer starts with Windows. To use another
place, quit deskfolds, move `deskfolds.exe` there, start it from there, and
turn **Start with Windows** off and on again.

### Update

deskfolds does not update itself. Settings > **About & advanced** shows the
version you have, and the [releases page](https://github.com/Matthieu1999/deskfolds/releases)
lists the new ones. Your folders, shortcuts and settings are kept when you
update: they are in `%APPDATA%\deskfolds`, not in the program.

#### If you downloaded it (option A)

1. Quit deskfolds: tray menu > **Quit**. Windows does not let you replace a
   program that is running.
2. Download the new **deskfolds.exe** from the
   [latest release](https://github.com/Matthieu1999/deskfolds/releases/latest).
3. Replace the old file with it: same folder, same name `deskfolds.exe`. Move
   the new file into that folder and choose **Replace the file in the
   destination**, or delete the old `deskfolds.exe` first. With the same
   place and name, **Start with Windows** keeps working.
4. Double-click the new `deskfolds.exe`. Windows may ask once more to
   **Run anyway**, since it is a new file.

#### If you built it (option B)

1. Quit deskfolds: tray menu > **Quit**. Windows does not let the build
   replace a program that is running.
2. Get the new code and build it:

   ```powershell
   cd $HOME\deskfolds
   git pull
   cargo build --release
   ```

3. Start `target\release\deskfolds.exe` again. If you run a copy of
   `deskfolds.exe` from another folder, copy the new one over it first.
   Your folders and settings are kept.

### Uninstall

1. Turn **Start with Windows** off, then quit deskfolds. (If you forgot and
   already deleted it, turn deskfolds off in Windows Settings > Apps >
   Startup.)
2. Get back the files you dragged into folders. They are in each folder's
   storage folder: by default `deskfolds\<folder name>` inside your Documents
   folder (which may be in OneDrive). Deleting a folder in Settings with
   **Move them back to the desktop** also brings them back.
3. Delete `deskfolds.exe` (or the `deskfolds` folder you built it in), and,
   if you no longer want your settings, `%APPDATA%\deskfolds`.
4. If you installed the build tools only for deskfolds, remove them with
   `winget uninstall` and the same ids as in step 1 of option B.

## Use

| Action | How |
|---|---|
| Open a panel | Hover it (it closes when the mouse leaves). With hover turned off in Settings, click its name to open or close it |
| Keep a panel open | Click its name (a blue dot shows it is pinned); click again to unpin |
| Launch a shortcut | Click it |
| Open the menu | Right-click a panel or the tray icon, or click the tray icon once |
| Open Settings | Double-click the tray icon, or menu > **Settings…** (from a panel's menu: **Edit '<name>'…**) |
| Create a folder | Menu > **New folder**, or Settings > Folders > **New folder** |
| Delete a folder | Right-click it > **Delete '…'…**, or Settings > Folders > **Delete…**. Both ask first. Files you moved into it go back to the desktop, or stay in its storage folder if you choose so |
| Rename, resize, reorder folders | Settings > Folders: name, width, columns, position, ▲ ▼. Type a number in the box next to a slider, or use its arrows or the Up/Down keys (Shift: ten steps) |
| Arrange folders | Settings > Folders > **Arrange all…** (row, column or grid), or Settings > Layout > **Arrange all folders**. The folders form an **arrangement**: they stay in its shape and place, and are placed again when one is added, removed, renamed or resized |
| Move the whole arrangement | While unlocked, drag its frame (the dashed outline with a blue tab). It snaps to the corners, edges and center of the screen; hold **Alt** to place it freely, press **Esc** to put it back. Or pick its place, offset and screen in Settings > Layout |
| Change a folder's place in the arrangement | While unlocked, drag the folder inside the frame: a dashed slot shows where it goes and the others make room. The Folders list follows |
| Take a folder out of the arrangement, or add one | While unlocked, drag it out of the frame (it stays where you drop it) or into it; hold **Alt** to drop it over the frame and keep it out. Or tick **In the arrangement** in Settings > Folders, or use **Choose…** in Settings > Layout |
| Add a shortcut | Drag a file, folder or shortcut onto a panel (a closed panel opens) or onto the shortcut list in Settings. It is **moved** into the folder's storage folder; hold **Ctrl** to keep it where it is and only add a link. Works even while locked. Settings > **Add…** adds links |
| Edit a shortcut | Settings > Folders: select it, then change its name, arguments or icon |
| Edit panels on the desktop | Menu > **Unlock panels (move and edit)**, or Settings > Folders > **Move on the desktop**: every panel gets a blue border and opens when you hover it (Settings > Layout can open them all instead) |
| Move a panel | While unlocked, drag it by its name. It snaps to the edges of the other folders and of the screen, keeping a small gap; the folder it snaps to lights up. Hold **Alt** to place it freely, press **Esc** to put it back |
| Resize a panel | While unlocked, drag its right edge (not for a folder whose width follows its columns) |
| Reorder or move shortcuts | While unlocked, drag a shortcut within the panel or onto another panel |
| Take a shortcut out | While unlocked, drag it onto the desktop or an Explorer window. A file that was moved into the folder goes there. For a link, a shortcut to it is created there and the original stays where it is. Hold **Ctrl** to copy it and keep it in the panel |
| Rename or remove a shortcut | While unlocked, right-click it. **Remove from folder** puts a moved file back on the desktop; a link is just removed |
| Finish editing | Menu > **Lock panels**, or **Done** in Settings |
| Undo a mistake | Settings > About & advanced > **Previous versions** > **Restore…** |

deskfolds never deletes a file: it only moves files, and never overwrites one
(a name already taken becomes "name (2)").

## The settings window

Every change is saved as soon as you make it: when you press Enter or leave a
field, release a slider, click an arrow, or click a button. There is no Save
button. **Tab** moves between fields and **Enter** saves the field you are
typing in. **Esc** puts back the previous value of that field; when you are
not typing, it closes the window. Each slider has a number box next to it:
type a value, or use its arrows or the Up/Down keys (Shift: ten steps). A
typed value can go past the slider's range, up to what the config file
accepts.

The folder selected in Settings stays open and outlined on the desktop, so you
can see what you are editing.

- **Folders**: the list of folders, with **New folder**, ▲ ▼ to reorder (also
  the order of the arrangement), **Arrange all…**, and for the selected folder:
  - its name, icon, width, number of columns and position (in screen pixels;
    grayed out when the arrangement places it);
  - **Width follows the columns**: the width becomes columns × the cell width
    (Layout page), so folders with the same number of columns line up;
  - **In the arrangement**: placed by the arrangement, and moved with it;
  - **Move on the desktop** to unlock all folders and drag them, then **Done**;
  - its storage folder, with **Open** to show it in Explorer;
  - its shortcuts: **Add…**, **Remove**, ▲ ▼, **Open file location**, and
    for the selected shortcut its name, arguments and icon. You can also drop
    files on the list.
- **Appearance**: three presets (**Light glass**, **Dark glass**, **Solid**),
  the glass tint and its strength, the accent, text, hover and border colors,
  the text and icon sizes, and the open/close animation. **Folder titles**:
  centered, on the left, with the icon above the name, or **Fit the name**
  (a label only as wide as the name, on one line); how many lines a long name
  may take (then it ends with "…"); the number of shortcuts next to the
  name; and shortcut names on one or two lines.
- **Behavior**: open folders on hover (when off, a folder opens and closes by
  clicking its name), the delays before opening and closing, close after
  launching a shortcut, lock folders in place, start with Windows, and the
  storage folder for new folders.
- **Layout**: snap when moving, the gap kept between folders, what happens
  when a folder opens over the ones under or above it (**Push the folders
  next to it** or **Cover them**), the open direction (**Automatic**: folders
  that touch open the same way, upwards when there is no room below them; or
  **Always down**), whether folders stay closed or all
  open while unlocked, the cell width and the width of new folders.
  **Arrangement**: its shape (**Row**, **Column**, or **Grid** of N
  **columns**, filled left to right, or N **rows**, filled top to bottom),
  its place on the screen (one of 9 anchors, an offset in pixels and the
  screen), the folders in it (**Choose…**), whether new folders join it, and
  **Arrange all folders** (a backup is made first, so **Restore…** can undo
  it) or **Free all folders**. Each column is as wide as its widest folder,
  and an incomplete last row follows the anchor (on the left, centered or on
  the right).
- **About & advanced**: the config file, with **Open in Notepad** and
  **Reload**; the previous versions, with **Restore…**; and **Reset…** for the
  appearance, behavior and layout (folders and shortcuts are kept).

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

Panel fields: `title`, `x`, `y`, `width`, `columns`, optional `fit_columns`
(`true`: the width is `columns` × `cell_width`, plus 12 px), optional
`arranged` (`true`: in the arrangement; its `x` and `y` are then written by
deskfolds each time it places the arrangement), optional `icon`
(shown next to the name instead of a folder), and `storage`, the folder that holds
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

Placing folders (wrapping names, finding the folders stacked under an open
one, snapping, the slots and anchor of the arrangement, finding a free spot)
is plain geometry in `src/layout.rs`, with unit tests. Snapping happens in
`WM_MOVING`, while Windows moves the folder, so it costs nothing at rest.

The arrangement (`src/arrange.rs`) places its folders again after every
change and saves their positions like any other move. Its frame
(`src/frame.rs`) is a layered window that exists only while folders are
unlocked; it is kept just under the folders, so they still get the mouse.
Dragging it moves the folders with it, and on release its anchor and offset
are saved.

The data lives in one store (`src/store.rs`) that every change goes through:
panels, drag and drop and the settings window all call it, and it tells the
app which panel to redraw. Panels and shortcuts get ids in memory only, so a
change always reaches the right one and the config file stays clean.

## Limitations

- Folders open downwards, or upwards when there is no room below. Folders
  that touch (an arrangement, or folders snapped together) open the same way:
  downwards when the tallest of them fits below with every folder it pushes,
  otherwise upwards. In the **Fit the name** title style, an open folder is
  wider than its label and can go past the right edge of the screen.
- Only the folders stacked right under or above an open one (within the gap)
  are pushed; folders further away are covered while it is open. When pushing
  would put a folder past the edge of the screen, the open folder covers them
  instead. While the mouse is on a folder that was pushed away, the folder
  that pushed it stays open, so the folder under the mouse never moves.
- Positions are screen pixels. When a monitor is unplugged, its panels show
  on the nearest one; they return when it comes back. An arrangement on a
  screen that is unplugged goes to the main screen, and back when it returns.
- There is one arrangement. Folders that are not in it are placed by hand.
- The settings window is always dark. Windows' own dialogs (color picker,
  file picker, confirmations) follow their own theme.
- So far tested only at 100 % display scaling.
- deskfolds.exe is not signed with a paid code-signing certificate, so
  Windows SmartScreen asks before running a new version (**More info** >
  **Run anyway**), and Smart App Control, when it is on, blocks it.

## Size and memory

Measured on Windows 11 at 100 % scaling with 12 folders and 82
shortcuts. "Memory" is what Task Manager shows (the private working set); the
working set also counts the Windows libraries shared with other programs.

| Situation | Memory | Working set |
|---|---|---|
| At rest, folders closed | 4.4 MB | 28 MB |
| Settings window open, any page | 4.6 MB | 32 MB |
| Unlocked, with the arrangement's frame | 5.0 MB | 33 MB |
| After closing the settings window | 5.0 MB | 33 MB |

`deskfolds.exe` is about 750 KB, with the C runtime inside it. At rest it uses no CPU (0 ms over 10
seconds): it only runs when you move the mouse over a folder, open the menu
or the settings window, or when Windows tells it the screens changed.

## Making a release

For the maintainer. Releases are built and published by GitHub Actions
(`.github/workflows/release.yml`), never from a personal PC:

1. Set the new version in `Cargo.toml` (for example `version = "0.5.0"`), in
   a pull request, and merge it.
2. Tag that version on `main` and push the tag:

   ```powershell
   git switch main
   git pull
   git tag v0.5.0
   git push origin v0.5.0
   ```

3. GitHub tests and builds `deskfolds.exe` and publishes the release with it
   and its SHA-256. It stops without publishing if the tag does not match the
   version in `Cargo.toml`, if the tag is not on `main`, or if the exe needs
   the Visual C++ runtime or contains a path of the build machine. Follow it
   in the **Actions** tab.

Every pull request is built and checked the same way, without publishing.

## License

[MIT](LICENSE)
