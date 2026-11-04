use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use toml_edit::{ArrayOfTables, DocumentMut, Table, value};

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
    /// Where files dropped into a panel are moved. Empty = Documents\deskfolds.
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

/// The config file, `%APPDATA%\deskfolds\config.toml`.
pub fn path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("deskfolds").join("config.toml")
    })
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

/// The `[[panel.item]]` tables of every panel, in order.
pub type PanelItems = Vec<Vec<Table>>;

/// Lets `edit` add, remove or reorder the items of any panel, then writes the
/// file back. Comments and formatting of the untouched parts are kept.
pub fn edit_items(path: &Path, edit: impl FnOnce(&mut PanelItems) -> Result<(), String>) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("Invalid config: {e}"))?;
    let panels = doc
        .get_mut("panel")
        .and_then(|item| item.as_array_of_tables_mut())
        .ok_or("No [[panel]] entries in the config file")?;

    let mut items: PanelItems = panels
        .iter()
        .map(|p| p.get("item").and_then(|i| i.as_array_of_tables()).map(|a| a.iter().cloned().collect()).unwrap_or_default())
        .collect();
    edit(&mut items)?;

    for (panel, tables) in panels.iter_mut().zip(items) {
        let mut array = ArrayOfTables::new();
        for table in tables {
            array.push(table);
        }
        panel.insert("item", toml_edit::Item::ArrayOfTables(array));
    }
    renumber_tables(&mut doc);
    fs::write(path, doc.to_string()).map_err(|e| format!("Cannot write {}: {e}", path.display()))
}

/// Table headers are printed in position order: renumber them so moved and new
/// items are written under the panel they belong to, in their new order.
fn renumber_tables(doc: &mut DocumentMut) {
    let mut position = 0;
    let mut next = || {
        position += 1;
        Some(position)
    };
    if let Some(settings) = doc.get_mut("settings").and_then(|i| i.as_table_mut()) {
        settings.set_position(next());
    }
    let Some(panels) = doc.get_mut("panel").and_then(|i| i.as_array_of_tables_mut()) else { return };
    for panel in panels.iter_mut() {
        panel.set_position(next());
        if let Some(items) = panel.get_mut("item").and_then(|i| i.as_array_of_tables_mut()) {
            for item in items.iter_mut() {
                item.set_position(next());
            }
        }
    }
}

pub fn new_item(name: &str, path: &str) -> Table {
    let mut table = Table::new();
    table["name"] = value(name);
    table["path"] = value(path);
    table
}

pub fn item_path(table: &Table) -> String {
    table.get("path").and_then(|p| p.as_str()).unwrap_or_default().to_string()
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
    fn items_are_added_moved_and_reordered_under_the_right_panel() {
        let path = std::env::temp_dir().join(format!("deskfolds-items-{}.toml", std::process::id()));
        fs::write(&path, DEFAULT_CONFIG).unwrap();
        edit_items(&path, |items| {
            items[0].insert(1, new_item("Steam", "C:\\Games\\Steam.lnk"));
            let docs = items[1].remove(0);
            items[0].push(docs);
            items[1].swap(0, 1);
            Ok(())
        })
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let config: Config = toml::from_str(&text).unwrap();
        let names = |p: usize| config.panels[p].items.iter().map(|i| i.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names(0), ["Notepad", "Steam", "Calculator", "Paint", "Explorer", "Documents"]);
        assert_eq!(names(1), ["Desktop", "Downloads", "Settings"]);
        assert_eq!(config.panels[0].items[1].path, "C:\\Games\\Steam.lnk");
        assert!(text.contains("# deskfolds configuration"));
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
