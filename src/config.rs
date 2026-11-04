//! The config file format. Reading and writing it is done by `store`, which
//! owns the data while the app runs.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::layout::{Shape, Spec};

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
    /// How a closed folder shows its name: "centered", "left", "icon_above" or "fit".
    pub title_style: String,
    /// Lines a long folder name may take before it ends with "…".
    pub title_lines: i32,
    /// Shows the number of shortcuts next to the folder name.
    pub show_count: bool,
    /// Lines a shortcut name may take under its icon (1 or 2).
    pub item_lines: i32,
    /// A folder being moved snaps to the edges of the others and of the screen.
    pub snap: bool,
    /// Space kept between folders by snapping, pushing and arranging.
    pub gap: i32,
    /// An opening folder pushes the folders stacked right under or above it out of its way.
    pub push_below: bool,
    /// Folders with no room below open upwards, with the folders they touch.
    pub open_upwards: bool,
    /// While unlocked, every folder is open (otherwise they open on hover).
    pub unlocked_open_all: bool,
    /// Width of one column, for folders whose width follows their columns.
    pub cell_width: i32,
    /// Width given to new folders.
    pub new_folder_width: i32,
    /// The arrangement's shape: "row", "column" or "grid".
    pub arrange_shape: String,
    /// For a grid: the number of columns, or of rows with arrange_by = "rows".
    pub arrange_count: i32,
    /// For a grid: "columns" (filled left to right) or "rows" (top to bottom).
    pub arrange_by: String,
    /// Where it sits on the screen: "top_left", "top_center", ... "center", ... "bottom_right".
    pub arrange_anchor: String,
    /// Moved from its anchor by this much, in pixels.
    pub arrange_offset_x: i32,
    pub arrange_offset_y: i32,
    /// The screen it is on: a monitor's device name, or empty for the main screen.
    pub arrange_screen: String,
    /// New folders join the arrangement, if it has folders.
    pub new_folders_join: bool,
}

/// Anchor names, by [vertical][horizontal].
pub const ANCHORS: [[&str; 3]; 3] = [
    ["top_left", "top_center", "top_right"],
    ["middle_left", "center", "middle_right"],
    ["bottom_left", "bottom_center", "bottom_right"],
];

/// "Top left", "Center", "Bottom right"...
pub fn anchor_label(anchor: (usize, usize)) -> &'static str {
    const LABELS: [[&str; 3]; 3] = [
        ["Top left", "Top center", "Top right"],
        ["Middle left", "Center", "Middle right"],
        ["Bottom left", "Bottom center", "Bottom right"],
    ];
    LABELS[anchor.1.min(2)][anchor.0.min(2)]
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TitleStyle {
    /// As wide as the folder, icon and name centered; long names wrap.
    Centered,
    /// As wide as the folder, icon and name on the left; long names wrap.
    Left,
    /// As wide as the folder, the icon above the name.
    IconAbove,
    /// As wide as the name, on one line (the look before deskfolds 0.3).
    Fit,
}

impl TitleStyle {
    pub const ALL: [(TitleStyle, &'static str); 4] =
        [(TitleStyle::Centered, "centered"), (TitleStyle::Left, "left"), (TitleStyle::IconAbove, "icon_above"), (TitleStyle::Fit, "fit")];

    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(s, _)| *s == self).map_or("centered", |(_, n)| n)
    }
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
            title_style: "centered".into(),
            title_lines: 2,
            show_count: false,
            item_lines: 1,
            snap: true,
            gap: 8,
            push_below: true,
            open_upwards: true,
            unlocked_open_all: false,
            cell_width: 72,
            new_folder_width: DEFAULT_WIDTH,
            arrange_shape: "grid".into(),
            arrange_count: 3,
            arrange_by: "columns".into(),
            arrange_anchor: "top_left".into(),
            arrange_offset_x: 0,
            arrange_offset_y: 0,
            arrange_screen: String::new(),
            new_folders_join: true,
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
        self.title_style = self.title_style().name().to_string();
        self.title_lines = self.title_lines.clamp(1, 3);
        self.item_lines = self.item_lines.clamp(1, 2);
        self.gap = self.gap.clamp(0, 40);
        self.cell_width = self.cell_width.clamp(40, 200);
        self.new_folder_width = self.new_folder_width.clamp(MIN_WIDTH, 4000);
        let spec = self.arrange_spec();
        self.arrange_shape = match spec.shape {
            Shape::Row => "row",
            Shape::Column => "column",
            Shape::Grid => "grid",
        }
        .into();
        self.arrange_count = self.arrange_count.clamp(1, 20);
        self.arrange_by = if spec.by_rows { "rows" } else { "columns" }.into();
        self.arrange_anchor = ANCHORS[spec.anchor.1][spec.anchor.0].into();
        self.arrange_offset_x = self.arrange_offset_x.clamp(-20000, 20000);
        self.arrange_offset_y = self.arrange_offset_y.clamp(-20000, 20000);
    }

    /// The arrangement's shape and place; unknown names mean the defaults.
    pub fn arrange_spec(&self) -> Spec {
        let shape = match self.arrange_shape.trim().to_ascii_lowercase().as_str() {
            "row" => Shape::Row,
            "column" => Shape::Column,
            _ => Shape::Grid,
        };
        let name = self.arrange_anchor.trim().to_ascii_lowercase();
        let anchor = (0..3)
            .flat_map(|v| (0..3).map(move |h| (h, v)))
            .find(|&(h, v)| ANCHORS[v][h] == name)
            .unwrap_or((0, 0));
        Spec {
            shape,
            count: self.arrange_count.clamp(1, 20) as usize,
            by_rows: self.arrange_by.trim().eq_ignore_ascii_case("rows"),
            anchor,
            offset: (self.arrange_offset_x, self.arrange_offset_y),
        }
    }

    /// The title style; an unknown name means the default one.
    pub fn title_style(&self) -> TitleStyle {
        let name = self.title_style.trim();
        TitleStyle::ALL.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map_or(TitleStyle::Centered, |(s, _)| *s)
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
    /// The width follows the columns: columns × the cell width of the settings.
    #[serde(default)]
    pub fit_columns: bool,
    /// In the arrangement: placed by it, and moved with it.
    #[serde(default)]
    pub arranged: bool,
    #[serde(default, rename = "item")]
    pub items: Vec<Item>,
}

impl Panel {
    pub fn clamp(&mut self) {
        self.columns = self.columns.clamp(1, 20);
        self.width = self.width.clamp(MIN_WIDTH, 4000);
    }

    /// The width it is shown at, in pixels at 100 % scaling.
    pub fn shown_width(&self, settings: &Settings) -> i32 {
        if self.fit_columns { self.columns * settings.cell_width + 2 * PANEL_PAD } else { self.width }
    }
}

/// Space between a panel's edge and its shortcuts, at 100 % scaling.
pub const PANEL_PAD: i32 = 6;

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

    #[test]
    fn layout_settings_are_read_and_kept_in_range() {
        let text = "[settings]\ntitle_style = \"Icon_Above\"\ntitle_lines = 9\ngap = -3\npush_below = false\n\
                    [[panel]]\ntitle = \"A\"\ncolumns = 3\nfit_columns = true";
        let config = parse(text).unwrap();
        let s = &config.settings;
        assert_eq!((s.title_style(), s.title_style.as_str(), s.title_lines, s.gap, s.push_below), (TitleStyle::IconAbove, "icon_above", 3, 0, false));
        assert_eq!(config.panels[0].shown_width(s), 3 * 72 + 12);
        let odd = parse("[settings]\ntitle_style = \"sideways\"").unwrap();
        assert_eq!(odd.settings.title_style(), TitleStyle::Centered);
    }

    #[test]
    fn arrangement_settings_are_read_and_kept_in_range() {
        let text = "[settings]\narrange_shape = \"Column\"\narrange_count = 50\narrange_by = \"rows\"\narrange_anchor = \"bottom_center\"\n\
                    arrange_offset_x = -12\n[[panel]]\ntitle = \"A\"\narranged = true\n[[panel]]\ntitle = \"B\"";
        let config = parse(text).unwrap();
        let spec = config.settings.arrange_spec();
        assert_eq!((spec.shape, spec.count, spec.by_rows, spec.anchor, spec.offset), (Shape::Column, 20, true, (1, 2), (-12, 0)));
        assert_eq!(config.settings.arrange_shape, "column");
        assert!(config.panels[0].arranged && !config.panels[1].arranged);
        let odd = parse("[settings]\narrange_anchor = \"nowhere\"").unwrap();
        assert_eq!((odd.settings.arrange_spec().anchor, odd.settings.arrange_anchor.as_str()), ((0, 0), "top_left"));
        assert_eq!(anchor_label((1, 1)), "Center");
    }
}
