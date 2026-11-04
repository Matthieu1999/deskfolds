//! The config file format. Reading and writing it is done by `store`, which
//! owns the data while the app runs.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

pub const DEFAULT_CONFIG: &str = include_str!("../config.example.toml");

#[derive(Deserialize, Clone, PartialEq)]
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
    /// Where new folders keep the files dropped into them. Empty = Documents\deskfolds.
    pub storage_dir: String,
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
            storage_dir: String::new(),
        }
    }
}

impl Settings {
    /// Brings every value into the range the app supports.
    pub fn clamp(&mut self) {
        self.tint_opacity = self.tint_opacity.clamp(0.0, 1.0);
        self.open_delay_ms = self.open_delay_ms.min(5000);
        self.close_delay_ms = self.close_delay_ms.clamp(1, 10000);
        self.animation_ms = self.animation_ms.min(2000);
        self.font_size = self.font_size.clamp(8, 40);
        self.icon_size = self.icon_size.clamp(16, 128);
    }
}

#[derive(Deserialize, Clone, PartialEq)]
pub struct Panel {
    /// Runtime id, never written to the file.
    #[serde(skip)]
    pub id: u32,
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
    /// Folder holding the files dropped into this panel. Chosen once and never
    /// renamed, so renaming the panel does not split its files.
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(default, rename = "item")]
    pub items: Vec<Item>,
}

impl Panel {
    pub fn clamp(&mut self) {
        self.columns = self.columns.clamp(1, 20);
        self.width = self.width.clamp(MIN_WIDTH, 4000);
    }
}

#[derive(Deserialize, Clone, PartialEq)]
pub struct Item {
    /// Runtime id, never written to the file.
    #[serde(skip)]
    pub id: u32,
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

pub const DEFAULT_WIDTH: i32 = 300;
pub const DEFAULT_COLUMNS: i32 = 4;
pub const MIN_WIDTH: i32 = 80;

fn default_width() -> i32 {
    DEFAULT_WIDTH
}

fn default_columns() -> i32 {
    DEFAULT_COLUMNS
}

/// The deskfolds folder in `%APPDATA%`.
pub fn app_dir() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("deskfolds")
    })
}

/// The config file, `%APPDATA%\deskfolds\config.toml`.
pub fn path() -> PathBuf {
    app_dir().join("config.toml")
}

/// Parses a config file's text and brings its values into range.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut config: Config = toml::from_str(text).map_err(|e| e.to_string())?;
    config.settings.clamp();
    for panel in &mut config.panels {
        panel.clamp();
    }
    Ok(config)
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

/// Formats a GDI COLORREF as "#RRGGBB".
pub fn format_color(color: u32) -> String {
    format!("#{:02X}{:02X}{:02X}", color & 0xFF, (color >> 8) & 0xFF, (color >> 16) & 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_parses() {
        let config = parse(DEFAULT_CONFIG).unwrap();
        assert_eq!(config.panels.len(), 2);
        assert_eq!(config.panels[0].items.len(), 4);
    }

    #[test]
    fn colors_round_trip_through_bgr() {
        assert_eq!(parse_color("#112233", 0), 0x332211);
        assert_eq!(parse_color("bad", 7), 7);
        assert_eq!(format_color(0x332211), "#112233");
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let config = parse("[settings]\nfont_size = 2\n[[panel]]\ntitle = \"A\"\ncolumns = 0\nwidth = 1").unwrap();
        assert_eq!(config.settings.font_size, 8);
        assert_eq!((config.panels[0].columns, config.panels[0].width), (1, MIN_WIDTH));
    }
}
