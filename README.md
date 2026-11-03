# deskfolds

Tiny foldable shortcut panels for the Windows desktop.

Each panel is a title bar pinned to the desktop. Click it to unfold a grid of
shortcuts, click a shortcut to launch it, and the panel folds back. Panels are
locked in place: they don't move unless you unlock them.

- **Native and light**: Rust + raw Win32/GDI, no UI framework, no runtime.
  About 400 KB on disk, about 5 MB of private memory, and no CPU use while idle
  (it only wakes up on input).
- **Locked in place**: position changes are refused while locked, even ones
  requested by other programs.
- **Stays on the desktop**: always below normal windows, never takes focus,
  no taskbar button, and stays visible after *Show desktop* (Win+D).
- **Any shortcut**: programs, documents, folders, `.lnk` files and URIs
  (`https://…`, `ms-settings:…`), with their real Windows icons.
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
| Unfold or fold a panel | Left-click its title bar |
| Launch a shortcut | Left-click it |
| Open the menu | Right-click a panel, or click the tray icon |
| Move panels | Menu > **Unlock panels**, drag them by the title bar, then menu > **Lock panels** (positions are saved to the config) |
| Change panels and shortcuts | Menu > **Edit config**, save, then menu > **Reload config** |

## Configuration

The config file is `%APPDATA%\deskfolds\config.toml`. It is created from
[config.example.toml](config.example.toml) on first run, and that file
documents every option.

```toml
[settings]
opacity = 0.95
collapse_after_launch = true
collapse_on_leave = false
font_size = 13
icon_size = 32

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

Item fields: `name`, `path`, optional `args`, and optional `icon` (a file
whose icon is shown instead of the target's).

## How it works

Each panel is a borderless `WS_POPUP` tool window owned by the desktop window
(`Progman`). Being owned by the desktop is what keeps it visible after
*Show desktop*. The window handles `WM_WINDOWPOSCHANGING` to stay at the bottom
of the z-order and to pin its position while locked. It is `WS_EX_NOACTIVATE`,
so clicking it never steals focus. Drawing is plain double-buffered GDI, and
icons come from the shell image lists.

## Limitations

- Panels unfold downwards. A panel near the bottom of the screen unfolds
  partly off-screen.
- Positions are screen pixels. If you change your monitor layout, unlock the
  panels and move them again.

## License

[MIT](LICENSE)
