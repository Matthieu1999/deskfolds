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
- **DPI aware**: sharp on high-DPI and mixed-DPI multi-monitor setups.

## Install

Build it (requires [Rust](https://rustup.rs) and the MSVC build tools):

```
cargo build --release
```

Then run `target\release\deskfolds.exe`. A folder icon appears in the
notification area. To start it with Windows, use its tray menu:
**Start with Windows**.

## Use

| Action | How |
|---|---|
| Open a panel | Hover it (it closes when the mouse leaves) |
| Keep a panel open | Click its name (a blue dot shows it is pinned); click again to unpin |
| Launch a shortcut | Click it |
| Open the menu | Right-click a panel, or click the tray icon |
| Move panels | Menu > **Unlock panels**, drag them by the title bar, then menu > **Lock panels** (positions are saved to the config) |
| Change panels and shortcuts | Menu > **Edit config**, save, then menu > **Reload config** |

## Configuration

The config file is `%APPDATA%\deskfolds\config.toml`. It is created from
[config.example.toml](config.example.toml) on first run, and that file
documents every option.

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

Panel fields: `title`, `x`, `y`, `width`, `columns`, and optional `icon`
(shown next to the name instead of a folder).

Item fields: `name`, `path`, optional `args`, and optional `icon` (a file
whose icon is shown instead of the target's).

## How it works

Each panel is a borderless `WS_POPUP` tool window owned by the desktop window
(`Progman`). Being owned by the desktop is what keeps it visible after
*Show desktop*. The window handles `WM_WINDOWPOSCHANGING` to stay at the bottom
of the z-order and to pin its position while locked. It is `WS_EX_NOACTIVATE`,
so clicking it never steals focus. The glass is the DWM acrylic accent
(`SetWindowCompositionAttribute`). Drawing is double-buffered GDI into a 32-bit
buffer whose alpha is rebuilt so text and highlights show on the glass, and
icons come from the shell image lists.

## Limitations

- Panels unfold downwards. A panel near the bottom of the screen unfolds
  partly off-screen.
- Positions are screen pixels. If you change your monitor layout, unlock the
  panels and move them again.

## License

[MIT](LICENSE)
