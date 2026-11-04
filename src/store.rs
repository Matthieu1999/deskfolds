//! The data of the running app: settings, panels and their shortcuts.
//!
//! The store is the only writer of `config.toml`. Every change goes through it:
//! it updates the data in memory, tells the app what to redraw, and saves the
//! file atomically, keeping its comments and the last few versions as backups.
//! Before saving it checks whether the file was edited by hand in the meantime,
//! and lets the user choose which version to keep instead of overwriting either.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Instant;

use toml_edit::{ArrayOfTables, DocumentMut, Item as TomlItem, Table, Value};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::config::{self, DEFAULT_COLUMNS, DEFAULT_WIDTH, Item, Panel, Settings};
use crate::dragdrop::{self, display_name, is_stored, move_into, panel_dir, storage_root};
use crate::util::{error_box, expand_env, wide};

/// Posted to the main window when something changed and must be redrawn.
pub const WM_APP_REFRESH: u32 = WM_APP + 10;
/// Posted to the main window when a save found the file edited by hand.
pub const WM_APP_CONFLICT: u32 = WM_APP + 11;

const BACKUPS_KEPT: usize = 5;
/// Saves in a burst (dragging a slider, moving several folders) share one backup.
const BACKUP_INTERVAL_SECS: u64 = 60;

/// What changed, so only that is redrawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Change {
    Panel(u32),
    Added(u32),
    Removed(u32),
    /// Settings shared by every panel (look, behavior).
    Settings,
    /// Everything was replaced (reload, restore).
    All,
}

pub struct Store {
    path: PathBuf,
    pub settings: Settings,
    pub panels: Vec<Panel>,
    /// The parsed file, kept so comments and layout survive saves.
    doc: DocumentMut,
    panel_tables: HashMap<u32, Table>,
    item_tables: HashMap<u32, Table>,
    next_id: u32,
    /// Hash of the file as deskfolds last read or wrote it. None when the file on
    /// disk is not one deskfolds loaded (it was invalid), so a save asks first.
    file_hash: Option<u64>,
    changes: Vec<Change>,
    notify: HWND,
    refresh_posted: bool,
    /// A save is waiting for the user to choose between the file and the screen.
    conflict: bool,
    last_backup: Option<Instant>,
}

thread_local! {
    static STORE: RefCell<Option<Store>> = const { RefCell::new(None) };
}

pub fn init(store: Store) {
    STORE.with(|s| *s.borrow_mut() = Some(store));
}

/// Runs `f` on the store. Never call a window function inside `f` (it could
/// send a message that needs the store again): copy what you need, then do it.
pub fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    STORE.with(|s| f(s.borrow_mut().as_mut().expect("store not initialised")))
}

/// Runs a change on the store, then saves it. A failed save is reported once
/// the store is released.
pub fn change<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let (result, saved) = with(|s| {
        let result = f(s);
        (result, s.save())
    });
    if let Err(e) = saved {
        error_box(&format!("Your change is on screen but was not saved.

{e}"));
    }
    result
}

/// A copy of the settings and panels, to read without holding the store.
pub fn snapshot() -> (Settings, Vec<Panel>) {
    with(|s| (s.settings.clone(), s.panels.clone()))
}

fn hash_text(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn file_hash(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok().map(|t| hash_text(&t))
}

/// Writes through a temporary file, so a crash never leaves a half-written config.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text).map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
    let (from, to) = (wide(&tmp.to_string_lossy()), wide(&path.to_string_lossy()));
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(format!("Cannot replace {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    Ok(())
}

/// The local time as "20261104-143210", for backup file names.
fn timestamp() -> String {
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// "config-20261104-143210.toml" -> "2026-11-04 14:32:10".
fn backup_label(name: &str) -> String {
    let s = name.trim_start_matches("config-").trim_end_matches(".toml");
    let d: Vec<char> = s.chars().collect();
    if d.len() < 15 {
        return s.to_string();
    }
    let p = |a: usize, b: usize| d[a..b].iter().collect::<String>();
    format!("{}-{}-{} {}:{}:{}", p(0, 4), p(4, 6), p(6, 8), p(9, 11), p(11, 13), p(13, 15))
}

pub struct Backup {
    pub path: PathBuf,
    pub label: String,
}

// ---- Writing values back into the file, keeping its formatting ----

enum V<'a> {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(&'a str),
}

impl V<'_> {
    fn same_as(&self, old: &Value) -> bool {
        match self {
            V::Bool(b) => old.as_bool() == Some(*b),
            V::Int(i) => old.as_integer() == Some(*i),
            V::Float(f) => old.as_float().or(old.as_integer().map(|i| i as f64)).is_some_and(|o| (o - f).abs() < 1e-6),
            V::Str(s) => old.as_str() == Some(*s),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            V::Bool(b) => Value::from(*b),
            V::Int(i) => Value::from(*i),
            V::Float(f) => Value::from(*f),
            V::Str(s) => Value::from(*s),
        }
    }
}

/// Sets `key`, keeping the comment next to an existing value. A key the file
/// does not have yet is only added if its value differs from the default.
fn put(table: &mut Table, key: &str, v: V, is_default: bool) {
    match table.get_mut(key).and_then(|i| i.as_value_mut()) {
        Some(old) => {
            if !v.same_as(old) {
                let decor = old.decor().clone();
                *old = v.to_value();
                *old.decor_mut() = decor;
            }
        }
        None if !is_default => {
            table.insert(key, TomlItem::Value(v.to_value()));
        }
        None => {}
    }
}

fn put_opt(table: &mut Table, key: &str, v: Option<&str>) {
    match v {
        Some(s) if !s.is_empty() => put(table, key, V::Str(s), false),
        _ => {
            table.remove(key);
        }
    }
}

fn sync_settings(t: &mut Table, s: &Settings) {
    let d = Settings::default();
    put(t, "hover_open", V::Bool(s.hover_open), s.hover_open == d.hover_open);
    put(t, "open_delay_ms", V::Int(s.open_delay_ms as i64), s.open_delay_ms == d.open_delay_ms);
    put(t, "close_delay_ms", V::Int(s.close_delay_ms as i64), s.close_delay_ms == d.close_delay_ms);
    put(t, "animation_ms", V::Int(s.animation_ms as i64), s.animation_ms == d.animation_ms);
    put(t, "collapse_after_launch", V::Bool(s.collapse_after_launch), s.collapse_after_launch == d.collapse_after_launch);
    put(t, "font_size", V::Int(s.font_size as i64), s.font_size == d.font_size);
    put(t, "icon_size", V::Int(s.icon_size as i64), s.icon_size == d.icon_size);
    let opacity = (s.tint_opacity as f64 * 100.0).round() / 100.0;
    put(t, "tint_opacity", V::Float(opacity), (s.tint_opacity - d.tint_opacity).abs() < 0.001);
    for (key, value, default) in [
        ("tint", &s.tint, &d.tint),
        ("text", &s.text, &d.text),
        ("hover", &s.hover, &d.hover),
        ("border", &s.border, &d.border),
        ("accent", &s.accent, &d.accent),
        ("storage_dir", &s.storage_dir, &d.storage_dir),
    ] {
        put(t, key, V::Str(value), value.eq_ignore_ascii_case(default));
    }
    put(t, "title_style", V::Str(&s.title_style), s.title_style == d.title_style);
    put(t, "title_lines", V::Int(s.title_lines as i64), s.title_lines == d.title_lines);
    put(t, "show_count", V::Bool(s.show_count), s.show_count == d.show_count);
    put(t, "show_folder_icon", V::Bool(s.show_folder_icon), s.show_folder_icon == d.show_folder_icon);
    put(t, "item_lines", V::Int(s.item_lines as i64), s.item_lines == d.item_lines);
    put(t, "snap", V::Bool(s.snap), s.snap == d.snap);
    put(t, "gap", V::Int(s.gap as i64), s.gap == d.gap);
    put(t, "push_below", V::Bool(s.push_below), s.push_below == d.push_below);
    put(t, "open_upwards", V::Bool(s.open_upwards), s.open_upwards == d.open_upwards);
    put(t, "unlocked_open_all", V::Bool(s.unlocked_open_all), s.unlocked_open_all == d.unlocked_open_all);
    put(t, "cell_width", V::Int(s.cell_width as i64), s.cell_width == d.cell_width);
    put(t, "new_folder_width", V::Int(s.new_folder_width as i64), s.new_folder_width == d.new_folder_width);
    for (key, value, default) in [
        ("arrange_shape", &s.arrange_shape, &d.arrange_shape),
        ("arrange_by", &s.arrange_by, &d.arrange_by),
        ("arrange_anchor", &s.arrange_anchor, &d.arrange_anchor),
        ("arrange_screen", &s.arrange_screen, &d.arrange_screen),
    ] {
        put(t, key, V::Str(value), value == default);
    }
    put(t, "arrange_count", V::Int(s.arrange_count as i64), s.arrange_count == d.arrange_count);
    put(t, "arrange_offset_x", V::Int(s.arrange_offset_x as i64), s.arrange_offset_x == d.arrange_offset_x);
    put(t, "arrange_offset_y", V::Int(s.arrange_offset_y as i64), s.arrange_offset_y == d.arrange_offset_y);
    put(t, "new_folders_join", V::Bool(s.new_folders_join), s.new_folders_join == d.new_folders_join);
}

fn sync_panel(t: &mut Table, p: &Panel) {
    put(t, "title", V::Str(&p.title), false);
    put(t, "x", V::Int(p.x as i64), false);
    put(t, "y", V::Int(p.y as i64), false);
    put(t, "width", V::Int(p.width as i64), p.width == DEFAULT_WIDTH);
    put(t, "columns", V::Int(p.columns as i64), p.columns == DEFAULT_COLUMNS);
    put(t, "fit_columns", V::Bool(p.fit_columns), !p.fit_columns);
    put(t, "arranged", V::Bool(p.arranged), !p.arranged);
    put_opt(t, "icon", p.icon.as_deref());
    put_opt(t, "storage", p.storage.as_deref());
}

fn sync_item(t: &mut Table, it: &Item) {
    put(t, "name", V::Str(&it.name), false);
    put(t, "path", V::Str(&it.path), false);
    put_opt(t, "args", Some(it.args.as_str()));
    put_opt(t, "icon", it.icon.as_deref());
}

/// Table headers are printed in position order: renumber them so new and moved
/// panels and items are written in their new order, under the right panel.
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

impl Store {
    fn empty(path: PathBuf, notify: HWND) -> Self {
        Self {
            path,
            settings: Settings::default(),
            panels: Vec::new(),
            doc: DocumentMut::new(),
            panel_tables: HashMap::new(),
            item_tables: HashMap::new(),
            next_id: 1,
            file_hash: None,
            changes: Vec::new(),
            notify,
            refresh_posted: false,
            conflict: false,
            last_backup: None,
        }
    }

    /// Loads the config at startup. Never fails: the returned notices say what
    /// went wrong and what was done instead.
    pub fn open(path: PathBuf, notify: HWND) -> (Self, Vec<String>) {
        let mut store = Self::empty(path, notify);
        let mut notices = Vec::new();
        match fs::read_to_string(&store.path) {
            Ok(text) => match store.load_text(&text) {
                Ok(()) => store.file_hash = Some(hash_text(&text)),
                Err(e) => {
                    notices.push(format!("config.toml has an error, so it was not loaded: {e}"));
                    // Show the latest backup, but leave the file as it is: the
                    // first save will ask before replacing it.
                    if let Some(label) = store.load_latest_backup() {
                        notices.push(format!("Showing the backup from {label} instead. Your file was not changed."));
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Never silently replace a lost config with the sample folders.
                match store.load_latest_backup() {
                    Some(label) => notices.push(format!("config.toml was missing: restored the backup from {label}.")),
                    None => {
                        let _ = store.load_text(config::DEFAULT_CONFIG);
                    }
                }
                if let Err(e) = store.write(true) {
                    notices.push(e);
                }
            }
            Err(e) => {
                notices.push(format!("Cannot read config.toml: {e}"));
                // It is there but unreadable (not UTF-8, locked): the first save
                // must ask before replacing it, like after a hand edit.
                store.file_hash = Some(u64::MAX);
            }
        }
        store.changes.clear();
        (store, notices)
    }

    /// Loads the newest backup that is a valid config, and returns its label.
    /// A backup of a broken hand edit is skipped.
    fn load_latest_backup(&mut self) -> Option<String> {
        self.backups().into_iter().find_map(|b| {
            let text = fs::read_to_string(&b.path).ok()?;
            self.load_text(&text).ok().map(|_| b.label)
        })
    }

    /// Replaces the data with a config file's text.
    fn load_text(&mut self, text: &str) -> Result<(), String> {
        let config = config::parse(text)?;
        let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
        let table_list = |doc: &DocumentMut| -> Vec<Table> {
            doc.get("panel").and_then(|i| i.as_array_of_tables()).map(|a| a.iter().cloned().collect()).unwrap_or_default()
        };
        let tables = table_list(&doc);

        self.panel_tables.clear();
        self.item_tables.clear();
        self.settings = config.settings;
        self.panels = config.panels;
        let root = storage_root(&self.settings.storage_dir);
        for (i, panel) in self.panels.iter_mut().enumerate() {
            panel.id = self.next_id;
            self.next_id += 1;
            // Folders from before per-folder storage keep using their old folder.
            if panel.storage.is_none() {
                panel.storage = Some(panel_dir(&root, &panel.title).to_string_lossy().to_string());
            }
            let table = tables.get(i).cloned().unwrap_or_default();
            let item_tables: Vec<Table> = table
                .get("item")
                .and_then(|i| i.as_array_of_tables())
                .map(|a| a.iter().cloned().collect())
                .unwrap_or_default();
            for (k, item) in panel.items.iter_mut().enumerate() {
                item.id = self.next_id;
                self.next_id += 1;
                self.item_tables.insert(item.id, item_tables.get(k).cloned().unwrap_or_default());
            }
            self.panel_tables.insert(panel.id, table);
        }
        self.doc = doc;
        self.push(Change::All);
        Ok(())
    }

    fn push(&mut self, change: Change) {
        if !self.changes.contains(&change) {
            self.changes.push(change);
        }
        if !self.refresh_posted && !self.notify.is_null() {
            self.refresh_posted = true;
            unsafe { PostMessageW(self.notify, WM_APP_REFRESH, 0, 0) };
        }
    }

    pub fn take_changes(&mut self) -> Vec<Change> {
        self.refresh_posted = false;
        std::mem::take(&mut self.changes)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    // ---- Saving ----

    /// True if the file on disk is not the one deskfolds last read or wrote.
    pub fn changed_outside(&self) -> bool {
        file_hash(&self.path) != self.file_hash
    }

    /// Saves the data. If the file was edited by hand since deskfolds last saw
    /// it, nothing is written and the app asks the user which version to keep.
    pub fn save(&mut self) -> Result<(), String> {
        if self.conflict {
            return Ok(());
        }
        if self.changed_outside() {
            self.conflict = true;
            unsafe { PostMessageW(self.notify, WM_APP_CONFLICT, 0, 0) };
            return Ok(());
        }
        self.write(false)
    }

    pub fn has_conflict(&self) -> bool {
        self.conflict
    }

    /// The user chose the folders on screen: replace the file (it is backed up first).
    pub fn keep_screen(&mut self) -> Result<(), String> {
        self.conflict = false;
        self.write(true)
    }

    /// The user chose the file: load it. If it is invalid, the folders stay as they are.
    /// What was on screen is saved as a backup first, so nothing is lost.
    pub fn load_file(&mut self) -> Result<(), String> {
        self.conflict = false;
        let text = fs::read_to_string(&self.path).map_err(|e| format!("Cannot read {}: {e}", self.path.display()))?;
        self.sync_doc();
        let screen = self.doc.to_string();
        if hash_text(&screen) != hash_text(&text) {
            self.write_backup(screen.as_bytes());
        }
        self.load_text(&text).map_err(|e| format!("config.toml has an error, so the folders were kept as they are.

{e}"))?;
        self.file_hash = Some(hash_text(&text));
        Ok(())
    }

    fn sync_doc(&mut self) {
        let mut settings = self.doc.get("settings").and_then(|i| i.as_table()).cloned().unwrap_or_default();
        sync_settings(&mut settings, &self.settings);
        if settings.is_empty() && self.doc.get("settings").is_none() {
            // Nothing to write: leave the file without a [settings] table.
        } else {
            self.doc.insert("settings", TomlItem::Table(settings));
        }

        let mut panels = ArrayOfTables::new();
        for p in &self.panels {
            let mut table = self.panel_tables.get(&p.id).cloned().unwrap_or_default();
            sync_panel(&mut table, p);
            let mut items = ArrayOfTables::new();
            for it in &p.items {
                let mut item_table = self.item_tables.get(&it.id).cloned().unwrap_or_default();
                sync_item(&mut item_table, it);
                self.item_tables.insert(it.id, item_table.clone());
                items.push(item_table);
            }
            if items.is_empty() {
                table.remove("item");
            } else {
                table.insert("item", TomlItem::ArrayOfTables(items));
            }
            self.panel_tables.insert(p.id, table.clone());
            panels.push(table);
        }
        if panels.is_empty() {
            self.doc.remove("panel");
        } else {
            self.doc.insert("panel", TomlItem::ArrayOfTables(panels));
        }
        renumber_tables(&mut self.doc);
    }

    fn write(&mut self, force_backup: bool) -> Result<(), String> {
        self.sync_doc();
        let text = self.doc.to_string();
        let hash = hash_text(&text);
        // Compared with the file on disk, not with the last save: after a hand
        // edit, keeping the screen must write even if the screen did not change.
        if file_hash(&self.path) == Some(hash) {
            self.file_hash = Some(hash);
            return Ok(());
        }
        self.backup_current(force_backup);
        write_atomic(&self.path, &text)?;
        self.file_hash = Some(hash);
        Ok(())
    }

    // ---- Backups ----

    pub fn backup_dir(&self) -> PathBuf {
        self.path.parent().map(|d| d.join("backups")).unwrap_or_else(|| PathBuf::from("backups"))
    }

    /// Backs up the current file now, before a change the user may want to undo.
    pub fn backup_now(&mut self) {
        self.backup_current(true);
    }

    /// Copies the current file to the backups folder, keeping the last few.
    fn backup_current(&mut self, force: bool) {
        if !force && self.last_backup.is_some_and(|t| t.elapsed().as_secs() < BACKUP_INTERVAL_SECS) {
            return;
        }
        if let Ok(text) = fs::read(&self.path) {
            self.write_backup(&text);
        }
    }

    fn write_backup(&mut self, text: &[u8]) {
        let dir = self.backup_dir();
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let stamp = timestamp();
        let mut target = dir.join(format!("config-{stamp}.toml"));
        let mut n = 2;
        while target.exists() {
            target = dir.join(format!("config-{stamp}-{n}.toml"));
            n += 1;
        }
        if fs::write(&target, text).is_ok() {
            self.last_backup = Some(Instant::now());
        }
        // Old backups beyond the last few are deskfolds' own copies: remove them.
        for old in self.backups().into_iter().skip(BACKUPS_KEPT) {
            let _ = fs::remove_file(old.path);
        }
    }

    /// Backups, newest first.
    pub fn backups(&self) -> Vec<Backup> {
        let mut list: Vec<Backup> = fs::read_dir(self.backup_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        (name.starts_with("config-") && name.ends_with(".toml"))
                            .then(|| Backup { path: e.path(), label: backup_label(&name) })
                    })
                    .collect()
            })
            .unwrap_or_default();
        list.sort_by(|a, b| b.path.file_name().cmp(&a.path.file_name()));
        list
    }

    /// Replaces the data with a backup and saves it (the current file is backed up first).
    pub fn restore(&mut self, backup: &Path) -> Result<(), String> {
        let text = fs::read_to_string(backup).map_err(|e| format!("Cannot read {}: {e}", backup.display()))?;
        self.load_text(&text).map_err(|e| format!("This backup has an error: {e}"))?;
        self.conflict = false;
        self.write(true)
    }

    // ---- Panels ----

    fn new_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id - 1
    }

    pub fn panel(&self, id: u32) -> Option<&Panel> {
        self.panels.iter().find(|p| p.id == id)
    }

    fn panel_index(&self, id: u32) -> Option<usize> {
        self.panels.iter().position(|p| p.id == id)
    }

    /// The folder holding this panel's files.
    pub fn storage_of(&self, panel_id: u32) -> PathBuf {
        let root = storage_root(&self.settings.storage_dir);
        match self.panel(panel_id) {
            Some(Panel { storage: Some(s), .. }) => PathBuf::from(expand_env(s)),
            Some(p) => panel_dir(&root, &p.title),
            None => root,
        }
    }

    /// True if this shortcut's file was moved into its panel's storage folder.
    pub fn is_stored(&self, panel_id: u32, item: &Item) -> bool {
        is_stored(&item.path, &self.storage_of(panel_id))
    }

    /// A storage folder no panel uses and that does not exist yet.
    fn new_storage_dir(&self, title: &str) -> String {
        let root = storage_root(&self.settings.storage_dir);
        let base = panel_dir(&root, title);
        let used = |dir: &Path| {
            dir.exists()
                || self.panels.iter().any(|p| p.storage.as_deref().is_some_and(|s| Path::new(&expand_env(s)) == dir))
        };
        let mut dir = base.clone();
        let mut n = 2;
        while used(&dir) {
            dir = PathBuf::from(format!("{} ({n})", base.display()));
            n += 1;
        }
        dir.to_string_lossy().to_string()
    }

    /// A title no other panel uses: "New folder", "New folder (2)", ...
    pub fn unique_title(&self, base: &str) -> String {
        let mut title = base.to_string();
        let mut n = 2;
        while self.panels.iter().any(|p| p.title.eq_ignore_ascii_case(&title)) {
            title = format!("{base} ({n})");
            n += 1;
        }
        title
    }

    pub fn add_panel(&mut self, title: &str, x: i32, y: i32, width: i32) -> u32 {
        let id = self.new_id();
        let storage = self.new_storage_dir(title);
        self.panels.push(Panel {
            id,
            title: title.to_string(),
            x,
            y,
            width: width.clamp(config::MIN_WIDTH, 4000),
            columns: DEFAULT_COLUMNS,
            icon: None,
            storage: Some(storage),
            fit_columns: false,
            arranged: false,
            items: Vec::new(),
        });
        self.push(Change::Added(id));
        id
    }

    /// Deletes a panel. With `files_to_desktop`, files moved into it go back to
    /// the desktop; otherwise they stay in its storage folder. Nothing is ever
    /// deleted. Returns the files that could not be moved (they stay in storage).
    pub fn delete_panel(&mut self, id: u32, files_to_desktop: bool) -> Vec<String> {
        let Some(index) = self.panel_index(id) else { return Vec::new() };
        let mut errors = Vec::new();
        if files_to_desktop {
            let dir = self.storage_of(id);
            match dragdrop::desktop_dir() {
                Some(desktop) => {
                    for item in &self.panels[index].items {
                        let path = expand_env(&item.path);
                        if is_stored(&path, &dir)
                            && Path::new(&path).exists()
                            && let Err(e) = move_into(Path::new(&path), &desktop)
                        {
                            errors.push(e);
                        }
                    }
                }
                None => errors.push("The desktop folder was not found, so the files stayed in the storage folder.".into()),
            }
        }
        let panel = self.panels.remove(index);
        self.panel_tables.remove(&id);
        for item in &panel.items {
            self.item_tables.remove(&item.id);
        }
        self.push(Change::Removed(id));
        errors
    }

    /// Moves a panel up (-1) or down (+1) in the list.
    pub fn move_panel(&mut self, id: u32, delta: i32) {
        let Some(i) = self.panel_index(id) else { return };
        let j = i as i32 + delta;
        if j < 0 || j >= self.panels.len() as i32 {
            return;
        }
        self.panels.swap(i, j as usize);
        self.push(Change::Panel(id));
    }

    pub fn update_panel(&mut self, id: u32, f: impl FnOnce(&mut Panel)) {
        let Some(i) = self.panel_index(id) else { return };
        let before = self.panels[i].clone();
        f(&mut self.panels[i]);
        self.panels[i].clamp();
        if self.panels[i].title != before.title {
            // Nothing was dropped in it yet: its storage folder can follow the new
            // name. Once it holds files, it never changes.
            let unused = self.panels[i].storage.as_deref().is_some_and(|s| !Path::new(&expand_env(s)).exists());
            if unused {
                self.panels[i].storage = None;
                let title = self.panels[i].title.clone();
                self.panels[i].storage = Some(self.new_storage_dir(&title));
            }
        }
        if self.panels[i] != before {
            self.push(Change::Panel(id));
        }
    }

    /// Puts panels in the arrangement, or frees them.
    pub fn set_arranged(&mut self, ids: &[u32], on: bool) {
        for &id in ids {
            self.update_panel(id, |p| p.arranged = on);
        }
    }

    /// Moves a panel to `index` in the list (the order of the arrangement).
    pub fn move_panel_to(&mut self, id: u32, index: usize) {
        let Some(i) = self.panel_index(id) else { return };
        let panel = self.panels.remove(i);
        let at = index.min(self.panels.len());
        self.panels.insert(at, panel);
        if at != i {
            self.push(Change::Panel(id));
        }
    }

    /// Moves several panels at once (Arrange): one change, saved once.
    pub fn set_positions(&mut self, positions: &[(u32, i32, i32)]) {
        for &(id, x, y) in positions {
            self.update_panel(id, |p| {
                p.x = x;
                p.y = y;
            });
        }
    }

    // ---- Shortcuts ----

    /// (panel index, item index) of a shortcut.
    fn find_item(&self, item_id: u32) -> Option<(usize, usize)> {
        self.panels
            .iter()
            .enumerate()
            .find_map(|(p, panel)| panel.items.iter().position(|it| it.id == item_id).map(|i| (p, i)))
    }

    pub fn item(&self, item_id: u32) -> Option<(u32, &Item)> {
        self.find_item(item_id).map(|(p, i)| (self.panels[p].id, &self.panels[p].items[i]))
    }

    /// Adds files to a panel at `index`. Files are moved into its storage folder
    /// unless `link` is set; a file that cannot be moved is added as a link.
    /// Returns the files that could not be moved.
    pub fn add_files(&mut self, panel_id: u32, index: usize, files: &[String], link: bool) -> Vec<String> {
        let Some(p) = self.panel_index(panel_id) else { return Vec::new() };
        let dir = self.storage_of(panel_id);
        let mut errors = Vec::new();
        let start = index.min(self.panels[p].items.len());
        for (at, file) in (start..).zip(files) {
            let src = Path::new(file);
            let name = display_name(src);
            let path = if link {
                file.clone()
            } else {
                match move_into(src, &dir) {
                    Ok(dest) => dest.to_string_lossy().to_string(),
                    Err(e) => {
                        errors.push(e);
                        file.clone()
                    }
                }
            };
            let id = self.new_id();
            self.panels[p].items.insert(at, Item { id, name, path, args: String::new(), icon: None });
        }
        self.push(Change::Panel(panel_id));
        errors
    }

    /// Removes a shortcut. With `file_to_desktop`, a file that was moved into the
    /// panel goes back to the desktop first; if that fails, nothing is removed.
    pub fn remove_item(&mut self, item_id: u32, file_to_desktop: bool) -> Result<(), String> {
        let Some((p, i)) = self.find_item(item_id) else { return Ok(()) };
        let panel_id = self.panels[p].id;
        if file_to_desktop && self.is_stored(panel_id, &self.panels[p].items[i]) {
            let path = expand_env(&self.panels[p].items[i].path);
            if Path::new(&path).exists() {
                let desktop = dragdrop::desktop_dir().ok_or("The desktop folder was not found.")?;
                move_into(Path::new(&path), &desktop)?;
            }
        }
        self.panels[p].items.remove(i);
        self.item_tables.remove(&item_id);
        self.push(Change::Panel(panel_id));
        Ok(())
    }

    /// Moves a shortcut to `index` in a panel (the same one or another). A file
    /// kept by the old panel moves to the new panel's storage folder.
    pub fn move_item(&mut self, item_id: u32, to_panel: u32, index: usize) -> Result<(), String> {
        let Some((p, i)) = self.find_item(item_id) else { return Ok(()) };
        let Some(to) = self.panel_index(to_panel) else { return Ok(()) };
        let from_panel = self.panels[p].id;
        let mut error = Ok(());
        let mut item = self.panels[p].items.remove(i);
        if from_panel != to_panel && self.is_stored(from_panel, &item) {
            match move_into(Path::new(&expand_env(&item.path)), &self.storage_of(to_panel)) {
                Ok(dest) => item.path = dest.to_string_lossy().to_string(),
                Err(e) => error = Err(e),
            }
        }
        let mut at = index;
        if p == to && i < at {
            at -= 1;
        }
        let at = at.min(self.panels[to].items.len());
        self.panels[to].items.insert(at, item);
        self.push(Change::Panel(from_panel));
        self.push(Change::Panel(to_panel));
        error
    }

    /// Moves a shortcut up (-1) or down (+1) within its panel.
    pub fn shift_item(&mut self, item_id: u32, delta: i32) {
        let Some((p, i)) = self.find_item(item_id) else { return };
        let j = i as i32 + delta;
        if j < 0 || j >= self.panels[p].items.len() as i32 {
            return;
        }
        self.panels[p].items.swap(i, j as usize);
        let id = self.panels[p].id;
        self.push(Change::Panel(id));
    }

    pub fn update_item(&mut self, item_id: u32, f: impl FnOnce(&mut Item)) {
        let Some((p, i)) = self.find_item(item_id) else { return };
        let before = self.panels[p].items[i].clone();
        f(&mut self.panels[p].items[i]);
        if self.panels[p].items[i] != before {
            let id = self.panels[p].id;
            self.push(Change::Panel(id));
        }
    }

    // ---- Settings ----

    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        f(&mut self.settings);
        self.settings.clamp();
        if self.settings != before {
            self.push(Change::Settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_from(text: &str) -> Store {
        let mut store = Store::empty(PathBuf::from("unused.toml"), std::ptr::null_mut());
        store.load_text(text).unwrap();
        store
    }

    fn saved_text(store: &mut Store) -> String {
        store.sync_doc();
        store.doc.to_string()
    }

    #[test]
    fn unchanged_data_writes_the_file_back_as_it_was_apart_from_storage() {
        let mut store = store_from(config::DEFAULT_CONFIG);
        for p in &mut store.panels {
            p.storage = None;
        }
        // Line endings depend on how Git checked the example out.
        assert_eq!(saved_text(&mut store).replace("\r\n", "\n"), config::DEFAULT_CONFIG.replace("\r\n", "\n"));
    }

    #[test]
    fn edits_keep_comments_and_land_under_the_right_panel() {
        let mut store = store_from(config::DEFAULT_CONFIG);
        let (apps, folders) = (store.panels[0].id, store.panels[1].id);
        let docs = store.panels[1].items[0].id;
        store.update_panel(apps, |p| p.title = "Tools".into());
        store.move_item(docs, apps, 1).unwrap();
        store.update_settings(|s| s.font_size = 15);
        // A storage folder that does not exist, so a real "Games" folder in
        // Documents\deskfolds does not turn the new one into "Games (2)".
        let root = std::env::temp_dir().join(format!("deskfolds-test-{}-none", std::process::id()));
        store.update_settings(|s| s.storage_dir = root.to_string_lossy().to_string());
        let new = store.add_panel("Games", 10, 20, 300);
        store.delete_panel(folders, false);

        let text = saved_text(&mut store);
        assert!(text.contains("# deskfolds configuration"));
        assert!(text.contains("hover_open = true             # open on hover"));
        let config = config::parse(&text).unwrap();
        assert_eq!(config.settings.font_size, 15);
        let titles: Vec<_> = config.panels.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["Tools", "Games"]);
        let names: Vec<_> = config.panels[0].items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Notepad", "Documents", "Calculator", "Paint", "Explorer"]);
        assert!(store.panel(new).unwrap().storage.as_deref().unwrap().ends_with("Games"));
    }

    #[test]
    fn new_panels_never_share_a_storage_folder() {
        let mut store = store_from("");
        let a = store.add_panel("Games", 0, 0, 300);
        let b = store.add_panel("Games", 0, 0, 300);
        assert_ne!(store.storage_of(a), store.storage_of(b));
        assert_eq!(store.unique_title("Games"), "Games (2)");
    }

    #[test]
    fn a_hand_edit_is_never_overwritten_without_asking() {
        let dir = std::env::temp_dir().join(format!("deskfolds-conflict-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        fs::write(&path, config::DEFAULT_CONFIG).unwrap();
        let (mut store, notices) = Store::open(path.clone(), std::ptr::null_mut());
        assert!(notices.is_empty());
        let apps = store.panels[0].id;
        store.update_panel(apps, |p| p.x = 5);
        store.save().unwrap();
        let edited = fs::read_to_string(&path).unwrap().replace("title = \"Folders\"", "title = \"Edited\"");
        fs::write(&path, &edited).unwrap();

        // A save after the hand edit writes nothing and waits for the user.
        store.update_panel(apps, |p| p.y = 7);
        store.save().unwrap();
        assert!(store.has_conflict());
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);

        // Keeping the screen writes it, and the hand edit goes to the backups.
        store.keep_screen().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("title = \"Folders\"") && text.contains("y = 7"));
        assert!(store.backups().iter().any(|b| fs::read_to_string(&b.path).unwrap() == edited));

        // Another hand edit while the screen is unchanged since that save: keeping
        // the screen must still write it.
        fs::write(&path, &edited).unwrap();
        assert!(store.changed_outside());
        store.keep_screen().unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("y = 7"));
        assert!(!store.changed_outside());

        // Loading the file instead keeps the screen as a backup.
        fs::write(&path, &edited).unwrap();
        store.load_file().unwrap();
        assert_eq!(store.panels[1].title, "Edited");
        assert!(store.backups().iter().any(|b| fs::read_to_string(&b.path).unwrap().contains("y = 7")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_lost_config_comes_back_from_the_newest_valid_backup() {
        let dir = std::env::temp_dir().join(format!("deskfolds-restore-{}", std::process::id()));
        let backups = dir.join("backups");
        fs::create_dir_all(&backups).unwrap();
        let older = config::DEFAULT_CONFIG.replace("title = \"Apps\"", "title = \"Mine\"");
        fs::write(backups.join("config-20261101-100000.toml"), &older).unwrap();
        fs::write(backups.join("config-20261102-100000.toml"), "title = \"broken").unwrap();
        let path = dir.join("config.toml");

        let (store, notices) = Store::open(path.clone(), std::ptr::null_mut());
        assert_eq!(store.panels[0].title, "Mine");
        assert!(notices[0].contains("2026-11-01 10:00:00"), "{notices:?}");
        assert!(fs::read_to_string(&path).unwrap().contains("title = \"Mine\""));

        // A broken file: the folders come from the same backup, the file is kept.
        fs::write(&path, "title = \"broken").unwrap();
        let (store, notices) = Store::open(path.clone(), std::ptr::null_mut());
        assert_eq!(store.panels[0].title, "Mine");
        assert!(store.changed_outside());
        assert_eq!(notices.len(), 2, "{notices:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "title = \"broken");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unused_storage_folder_follows_the_new_name() {
        let base = std::env::temp_dir().join(format!("deskfolds-rename-{}", std::process::id()));
        let mut store = store_from(&format!("[settings]
storage_dir = '{}'", base.display()));
        let id = store.add_panel("New folder", 0, 0, 300);
        store.update_panel(id, |p| p.title = "Tools".into());
        assert_eq!(store.storage_of(id), base.join("Tools"));
        // Once it holds files, renaming keeps the folder.
        std::fs::create_dir_all(base.join("Tools")).unwrap();
        store.update_panel(id, |p| p.title = "Games".into());
        assert_eq!(store.storage_of(id), base.join("Tools"));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn layout_settings_are_written_only_once_changed() {
        let mut store = store_from("[settings]\nfont_size = 13\n\n[[panel]]\ntitle = \"Apps\"\n\n[[panel]]\ntitle = \"Folders\"\n");
        let (apps, folders) = (store.panels[0].id, store.panels[1].id);
        store.update_settings(|s| {
            s.push_below = false;
            s.title_style = "left".into();
            s.gap = 12;
        });
        store.update_panel(apps, |p| p.fit_columns = true);
        store.set_positions(&[(apps, 5, 6), (folders, 300, 6)]);
        let text = saved_text(&mut store);
        assert!(!text.contains("snap =") && !text.contains("title_lines ="), "{text}");
        let config = config::parse(&text).unwrap();
        let s = &config.settings;
        assert_eq!((s.push_below, s.title_style.as_str(), s.gap), (false, "left", 12));
        assert!(config.panels[0].fit_columns && !config.panels[1].fit_columns);
        assert_eq!((config.panels[1].x, config.panels[1].y), (300, 6));
    }

    #[test]
    fn folder_icons_turned_off_are_saved_and_read_back() {
        let mut store = store_from("[settings]
font_size = 13

[[panel]]
title = \"Apps\"
icon = \"shell32.dll,3\"
");
        assert!(store.settings.show_folder_icon);
        assert!(!saved_text(&mut store).contains("show_folder_icon"));
        store.update_settings(|s| s.show_folder_icon = false);
        let text = saved_text(&mut store);
        let config = config::parse(&text).unwrap();
        assert!(!config.settings.show_folder_icon, "{text}");
        // The icon chosen for the folder is kept for when icons are on again.
        assert_eq!(config.panels[0].icon.as_deref(), Some("shell32.dll,3"));
    }

    #[test]
    fn the_arrangement_is_saved_with_its_folders() {
        let mut store = store_from("[[panel]]\ntitle = \"A\"\n\n[[panel]]\ntitle = \"B\"\n\n[[panel]]\ntitle = \"C\"\n");
        let ids: Vec<u32> = store.panels.iter().map(|p| p.id).collect();
        store.set_arranged(&ids[..2], true);
        store.move_panel_to(ids[2], 0);
        store.update_settings(|s| {
            s.arrange_anchor = "bottom_right".into();
            s.arrange_offset_x = -20;
        });
        let text = saved_text(&mut store);
        assert!(!text.contains("arrange_shape") && !text.contains("new_folders_join"), "{text}");
        let config = config::parse(&text).unwrap();
        let titles: Vec<_> = config.panels.iter().map(|p| (p.title.as_str(), p.arranged)).collect();
        assert_eq!(titles, [("C", false), ("A", true), ("B", true)]);
        assert_eq!(config.settings.arrange_spec().anchor, (2, 2));
        assert_eq!(config.settings.arrange_offset_x, -20);
    }

    #[test]
    fn backup_names_become_readable_labels() {
        assert_eq!(backup_label("config-20261104-143210.toml"), "2026-11-04 14:32:10");
    }
}
