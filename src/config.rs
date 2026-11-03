use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub hover_open: bool,
    pub open_delay_ms: u32,
    pub close_delay_ms: u32,
    pub animation_ms: u32,
    pub collapse_after_launch: bool,
    pub font_size: i32,
    pub icon_size: i32,
    pub tint: String,
    pub tint_opacity: f32,
    pub text: String,
    pub hover: String,
    pub border: String,
    pub accent: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hover_open: true,
            open_delay_ms: 150,
            close_delay_ms: 400,
            animation_ms: 200,
            collapse_after_launch: true,
            font_size: 13,
            icon_size: 32,
            tint: "#FFFFFF".into(),
            tint_opacity: 0.10,
            text: "#F2F2F2".into(),
            hover: "#2E2E2E".into(),
            border: "#3A3A3A".into(),
            accent: "#4CC2FF".into(),
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct Panel {
    pub title: String,
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default = "default_width")]
    pub width: i32,
    #[serde(default = "default_columns")]
    pub columns: i32,
    /// Optional file whose icon is shown next to the name instead of a folder.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default, rename = "item")]
    pub items: Vec<Item>,
}

#[derive(Deserialize, Clone)]
pub struct Item {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub args: String,
    /// Optional file whose icon is shown instead of the target's icon.
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "panel")]
    pub panels: Vec<Panel>,
}

fn default_width() -> i32 {
    300
}

fn default_columns() -> i32 {
    4
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("deskfolds").join("config.toml")
}

/// Loads the config, writing the default one first if the file does not exist.
pub fn load(path: &Path) -> Result<Config, String> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        }
        fs::write(path, DEFAULT_CONFIG).map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    }
    let text = fs::read_to_string(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut config: Config = toml::from_str(&text).map_err(|e| format!("Invalid config {}:\n\n{e}", path.display()))?;

    let s = &mut config.settings;
    s.tint_opacity = s.tint_opacity.clamp(0.0, 1.0);
    s.open_delay_ms = s.open_delay_ms.clamp(1, 5000);
    s.close_delay_ms = s.close_delay_ms.clamp(1, 10000);
    s.animation_ms = s.animation_ms.min(2000);
    s.font_size = s.font_size.clamp(8, 40);
    s.icon_size = s.icon_size.clamp(16, 128);
    for p in &mut config.panels {
        p.columns = p.columns.clamp(1, 20);
        p.width = p.width.clamp(80, 4000);
    }
    Ok(config)
}

/// Writes panel positions back into the config file, keeping its comments and layout.
pub fn save_positions(path: &Path, positions: &[(i32, i32)]) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("Invalid config: {e}"))?;
    let panels = doc
        .get_mut("panel")
        .and_then(|item| item.as_array_of_tables_mut())
        .ok_or("No [[panel]] entries in the config file")?;
    if panels.len() != positions.len() {
        return Err("The config file changed since it was loaded. Reload it, then move the panels again.".into());
    }
    for (table, (x, y)) in panels.iter_mut().zip(positions) {
        table["x"] = toml_edit::value(*x as i64);
        table["y"] = toml_edit::value(*y as i64);
    }
    fs::write(path, doc.to_string()).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

/// Parses "#RRGGBB" into a GDI COLORREF (0x00BBGGRR).
pub fn parse_color(s: &str, fallback: u32) -> u32 {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return fallback;
    }
    match u32::from_str_radix(hex, 16) {
        Ok(rgb) => ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF),
        Err(_) => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(config.panels.len(), 2);
        assert_eq!(config.panels[0].items.len(), 4);
    }

    #[test]
    fn color_is_converted_to_bgr() {
        assert_eq!(parse_color("#112233", 0), 0x332211);
        assert_eq!(parse_color("bad", 7), 7);
    }

    #[test]
    fn positions_are_saved_without_losing_comments() {
        let path = std::env::temp_dir().join(format!("deskfolds-test-{}.toml", std::process::id()));
        fs::write(&path, DEFAULT_CONFIG).unwrap();
        save_positions(&path, &[(100, 200), (-50, 300)]).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let config: Config = toml::from_str(&text).unwrap();
        assert_eq!((config.panels[0].x, config.panels[0].y), (100, 200));
        assert_eq!((config.panels[1].x, config.panels[1].y), (-50, 300));
        assert!(text.contains("# deskfolds configuration"));
    }
}
