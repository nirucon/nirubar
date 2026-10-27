//! StatusNotifierItem, the D-Bus tray protocol.
//!
//! A small background thread owns a Tokio runtime and a `system_tray::Client`.
//! The bar only reads a thread-safe snapshot, so a slow or absent bus never
//! blocks drawing or input. XEmbed lives beside this in `tray.rs`; the two
//! protocols share the same visual strip.

use crate::config::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use system_tray::client::{ActivateRequest, Client};
use system_tray::data::BaseMap;
use system_tray::item::{IconPixmap, Status, StatusNotifierItem};
use system_tray::menu::{MenuItem, MenuType, TrayMenu};
use tokio::sync::{broadcast, mpsc, watch};

/// How wide one icon is drawn, in pixels.
pub const ICON_SIZE: u16 = 18;
/// Space after an icon, including the gap to the next one.
pub const GAP: u16 = 2;
/// Large enough for the bar, small enough that scaling stays readable.
const MAX_ICON: i32 = 64;

/// One ARGB32 pixmap in network byte order, copied out of the client map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

/// A snapshot of one tray item. It owns everything it needs, so the bar can
/// use it without holding the client's lock.
#[derive(Clone, Debug)]
pub struct Item {
    pub address: String,
    pub id: String,
    pub title: String,
    pub status: Status,
    pub icon: Option<Icon>,
    /// Freedesktop icon name, used when the item has no pixmap.
    pub icon_name: Option<String>,
    pub item_is_menu: bool,
    pub menu: Option<TrayMenu>,
    pub menu_path: Option<String>,
    /// Cheap identity of the icon bytes, used in the frame key.
    pub fingerprint: u64,
}

impl Item {
    /// The one-character badge used when an item carries no pixmap.
    pub fn initial(&self) -> char {
        self.title
            .chars()
            .find(|c| !c.is_whitespace())
            .or_else(|| self.id.chars().find(|c| !c.is_whitespace()))
            .map(|c| c.to_uppercase().next().unwrap_or(c))
            .unwrap_or('?')
    }

    /// What can change the icon strip, without the (possibly large) menu.
    pub fn frame_key(&self) -> String {
        format!(
            "{}:{}:{:?}:{}:{}:{}:{}",
            self.address,
            self.id,
            self.status,
            self.title,
            self.item_is_menu,
            self.fingerprint,
            self.menu_path.as_deref().unwrap_or("")
        )
    }
}

/// One selectable row of a flattened DBusMenu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuRow {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub depth: usize,
}

/// A running SNI host. Dropping it stops the background thread.
pub struct Host {
    snapshot: Arc<Mutex<Vec<Item>>>,
    commands: mpsc::UnboundedSender<Command>,
    shutdown: watch::Sender<bool>,
}

enum Command {
    Activate {
        address: String,
        secondary: bool,
        x: i32,
        y: i32,
    },
    Menu {
        address: String,
        menu_path: String,
        id: i32,
    },
}

impl Host {
    /// Start the host, or return `None` when there is no usable session bus.
    ///
    /// The wait is bounded: a missing or wedged bus must not delay the bar.
    pub fn new() -> Result<Option<Self>> {
        let snapshot = Arc::new(Mutex::new(Vec::new()));
        let (ready_tx, ready_rx) = std_mpsc::channel();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (shutdown, shutdown_rx) = watch::channel(false);
        let thread_snapshot = snapshot.clone();
        thread::Builder::new()
            .name("nirubar-sni".into())
            .spawn(move || run(thread_snapshot, command_rx, shutdown_rx, ready_tx))?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Some(Self {
                snapshot,
                commands,
                shutdown,
            })),
            Ok(Err(error)) => {
                eprintln!("nirubar: sni: {error}");
                let _ = shutdown.send(true);
                Ok(None)
            }
            Err(_) => {
                eprintln!("nirubar: sni: timed out connecting to the session bus");
                let _ = shutdown.send(true);
                Ok(None)
            }
        }
    }

    /// Current items, left to right. The order is stable across refreshes.
    pub fn snapshot(&self) -> Vec<Item> {
        match self.snapshot.lock() {
            Ok(items) => items.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// A cheap identity for the frame key. This does not clone the menus.
    pub fn frame_key(&self) -> String {
        let items = match self.snapshot.lock() {
            Ok(items) => items,
            Err(poisoned) => poisoned.into_inner(),
        };
        items
            .iter()
            .map(Item::frame_key)
            .collect::<Vec<_>>()
            .join("|")
    }

    /// Ask the background task to activate item `index`.
    pub fn activate(&self, index: usize, secondary: bool, x: i16, y: i16) {
        let address = self.snapshot().get(index).map(|item| item.address.clone());
        let Some(address) = address else {
            return;
        };
        let _ = self.commands.send(Command::Activate {
            address,
            secondary,
            x: i32::from(x),
            y: i32::from(y),
        });
    }

    /// Ask for one DBusMenu entry to be invoked.
    pub fn menu_action(&self, address: &str, menu_path: &str, id: i32) {
        if address.is_empty() || menu_path.is_empty() {
            return;
        }
        let _ = self.commands.send(Command::Menu {
            address: address.to_string(),
            menu_path: menu_path.to_string(),
            id,
        });
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

fn run(
    snapshot: Arc<Mutex<Vec<Item>>>,
    command_rx: mpsc::UnboundedReceiver<Command>,
    mut shutdown_rx: watch::Receiver<bool>,
    ready_tx: std_mpsc::Sender<std::result::Result<(), String>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = ready_tx.send(Err(format!("cannot start Tokio runtime: {error}")));
            return;
        }
    };

    runtime.block_on(async move {
        let client = match Client::new().await {
            Ok(client) => Arc::new(client),
            Err(error) => {
                let _ = ready_tx.send(Err(format!("session bus unavailable: {error}")));
                return;
            }
        };
        let mut events = client.subscribe();
        refresh(&client, &snapshot);
        if ready_tx.send(Ok(())).is_err() {
            return;
        }

        let command_client = client.clone();
        tokio::spawn(async move {
            handle_commands(command_client, command_rx).await;
        });

        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_err() || *shutdown_rx.borrow() {
                        break;
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(_) => refresh(&client, &snapshot),
                        Err(broadcast::error::RecvError::Lagged(missed)) => {
                            eprintln!("nirubar: sni: missed {missed} events");
                            refresh(&client, &snapshot);
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

async fn handle_commands(client: Arc<Client>, mut commands: mpsc::UnboundedReceiver<Command>) {
    while let Some(command) = commands.recv().await {
        let result = match command {
            Command::Activate {
                address,
                secondary,
                x,
                y,
            } => {
                let request = if secondary {
                    ActivateRequest::Secondary { address, x, y }
                } else {
                    ActivateRequest::Default { address, x, y }
                };
                client.activate(request).await
            }
            Command::Menu {
                address,
                menu_path,
                id,
            } => {
                client
                    .activate(ActivateRequest::MenuItem {
                        address,
                        menu_path,
                        submenu_id: id,
                    })
                    .await
            }
        };
        if let Err(error) = result {
            eprintln!("nirubar: sni action: {error}");
        }
    }
}

fn refresh(client: &Client, snapshot: &Arc<Mutex<Vec<Item>>>) {
    let items = {
        let map = client.items();
        let mut items = match map.lock() {
            Ok(map) => items_from_map(&map),
            Err(poisoned) => items_from_map(&poisoned.into_inner()),
        };
        resolve_icons(&mut items);
        items
    };
    match snapshot.lock() {
        Ok(mut current) => *current = items,
        Err(poisoned) => *poisoned.into_inner() = items,
    }
}

fn resolve_icons(items: &mut [Item]) {
    for item in items {
        if item.icon.is_some() {
            continue;
        }
        let Some(name) = item.icon_name.as_deref() else {
            continue;
        };
        item.icon = icon_for_name(name);
        item.fingerprint = fingerprint(item.icon.as_ref());
    }
}

/// Look up a PNG icon by its Freedesktop name. Most SNI clients send only an
/// icon name; showing a badge for every one of them would make a real tray
/// look broken. SVG-only themes intentionally fall back to the badge because
/// decoding them would add a large renderer dependency.
fn icon_for_name(name: &str) -> Option<Icon> {
    if name.is_empty() || name.len() > 200 || name.contains('/') || name.contains('\0') {
        return None;
    }
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Icon>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock() {
        if let Some(cached) = cache.get(name) {
            return cached.clone();
        }
    }
    let found = find_icon(name).and_then(|path| load_png(&path));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(name.to_string(), found.clone());
    }
    found
}

fn find_icon(name: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = &home {
        roots.push(home.join(".local/share/icons"));
        roots.push(home.join(".icons"));
        roots.push(home.join(".local/share/pixmaps"));
        roots.push(home.join(".local/share/flatpak/exports/share/icons"));
    }
    roots.push(PathBuf::from("/usr/local/share/icons"));
    roots.push(PathBuf::from("/usr/share/icons"));
    roots.push(PathBuf::from("/var/lib/flatpak/exports/share/icons"));
    roots.push(PathBuf::from("/usr/local/share/pixmaps"));
    roots.push(PathBuf::from("/usr/share/pixmaps"));

    let names = [
        name,
        name.trim_end_matches("-symbolic"),
        name.trim_end_matches("-panel"),
    ];
    let sizes = [
        "22x22", "24x24", "16x16", "32x32", "48x48", "64x64", "128x128", "256x256", "scalable",
    ];
    let categories = ["apps", "status", "panel", "devices", "places"];
    for root in roots {
        for candidate in &names {
            let direct = root.join(format!("{candidate}.png"));
            if direct.is_file() {
                return Some(direct);
            }
        }
        let Ok(themes) = std::fs::read_dir(&root) else {
            continue;
        };
        for theme in themes.flatten().take(32) {
            let theme = theme.path();
            for size in &sizes {
                for category in &categories {
                    for candidate in &names {
                        let path = theme
                            .join(size)
                            .join(category)
                            .join(format!("{candidate}.png"));
                        if path.is_file() {
                            return Some(path);
                        }
                    }
                }
            }
        }
    }
    None
}

fn load_png(path: &Path) -> Option<Icon> {
    let file = std::fs::File::open(path).ok()?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    let bytes = &buffer[..info.buffer_size()];
    let (width, height) = (info.width as i32, info.height as i32);
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut pixels = Vec::with_capacity(bytes.len().min((width as usize) * (height as usize) * 4));
    match info.color_type {
        png::ColorType::Rgba => {
            for pixel in bytes.chunks_exact(4) {
                pixels.extend_from_slice(&[pixel[3], pixel[0], pixel[1], pixel[2]]);
            }
        }
        png::ColorType::Rgb => {
            for pixel in bytes.chunks_exact(3) {
                pixels.extend_from_slice(&[255, pixel[0], pixel[1], pixel[2]]);
            }
        }
        png::ColorType::Grayscale => {
            for value in bytes {
                pixels.extend_from_slice(&[255, *value, *value, *value]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in bytes.chunks_exact(2) {
                pixels.extend_from_slice(&[pixel[1], pixel[0], pixel[0], pixel[0]]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    (pixels.len() == (width as usize) * (height as usize) * 4).then_some(Icon {
        width,
        height,
        pixels,
    })
}

fn items_from_map(map: &BaseMap) -> Vec<Item> {
    let mut items: Vec<Item> = map
        .iter()
        .map(|(address, (item, menu))| item_from(address, item, menu.as_ref()))
        .collect();
    items.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.address.cmp(&b.address)));
    items
}

fn item_from(address: &str, item: &StatusNotifierItem, menu: Option<&TrayMenu>) -> Item {
    let icon = pick_icon(item.icon_pixmap.as_deref());
    Item {
        address: address.to_string(),
        id: item.id.clone(),
        title: item.title.clone().unwrap_or_default(),
        status: item.status,
        fingerprint: fingerprint(icon.as_ref()),
        icon,
        icon_name: item.icon_name.clone(),
        item_is_menu: item.item_is_menu,
        menu: menu.cloned(),
        menu_path: item.menu.clone(),
    }
}

/// Prefer the largest useful pixmap, but fall back to the smallest oversized
/// one rather than showing a badge when an application only ships big icons.
fn pick_icon(pixmaps: Option<&[IconPixmap]>) -> Option<Icon> {
    let mut best: Option<&IconPixmap> = None;
    let mut oversized: Option<&IconPixmap> = None;
    for pixmap in pixmaps.unwrap_or_default() {
        if pixmap.width <= 0 || pixmap.height <= 0 {
            continue;
        }
        let Some(expected) = (pixmap.width as usize)
            .checked_mul(pixmap.height as usize)
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            continue;
        };
        if pixmap.pixels.len() < expected {
            continue;
        }
        let dimension = pixmap.width.max(pixmap.height);
        let area = i64::from(pixmap.width) * i64::from(pixmap.height);
        if dimension <= MAX_ICON {
            let better = best.map_or(true, |current| {
                let current_dimension = current.width.max(current.height);
                dimension > current_dimension
                    || (dimension == current_dimension
                        && area > i64::from(current.width) * i64::from(current.height))
            });
            if better {
                best = Some(pixmap);
            }
        } else {
            let smaller = oversized.map_or(true, |current| {
                dimension < current.width.max(current.height)
                    || (dimension == current.width.max(current.height)
                        && area < i64::from(current.width) * i64::from(current.height))
            });
            if smaller {
                oversized = Some(pixmap);
            }
        }
    }
    let chosen = best.or(oversized)?;
    let expected = (chosen.width as usize)
        .saturating_mul(chosen.height as usize)
        .saturating_mul(4)
        .min(chosen.pixels.len());
    Some(Icon {
        width: chosen.width,
        height: chosen.height,
        pixels: chosen.pixels[..expected].to_vec(),
    })
}

fn fingerprint(icon: Option<&Icon>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut byte = |value: u8| {
        hash ^= u64::from(value);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    match icon {
        Some(icon) => {
            for value in icon.width.to_le_bytes() {
                byte(value);
            }
            for value in icon.height.to_le_bytes() {
                byte(value);
            }
            for &value in &icon.pixels {
                byte(value);
            }
        }
        None => {
            for value in 0u64.to_le_bytes() {
                byte(value);
            }
        }
    }
    hash
}

/// Flatten a DBusMenu into rows the popup can draw and select.
///
/// Invisible entries disappear with their children. Separators stay as blank
/// disabled rows so spacing matches the application's menu. Submenus are
/// shown inline, indented by two spaces per level.
pub fn menu_rows(menu: &TrayMenu) -> Vec<MenuRow> {
    fn walk(items: &[MenuItem], depth: usize, rows: &mut Vec<MenuRow>) {
        for item in items {
            if !item.visible {
                continue;
            }
            let separator = item.menu_type == MenuType::Separator;
            let label = if separator {
                String::new()
            } else {
                let text = item.label.as_deref().unwrap_or_default();
                format!("{}{}", "  ".repeat(depth), text)
            };
            rows.push(MenuRow {
                id: item.id,
                label,
                enabled: item.enabled && !separator,
                separator,
                depth,
            });
            if !item.submenu.is_empty() {
                walk(&item.submenu, depth + 1, rows);
            }
        }
    }

    let mut rows = Vec::new();
    walk(&menu.submenus, 0, &mut rows);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use system_tray::item::Category;

    fn pixmap(width: i32, height: i32) -> IconPixmap {
        IconPixmap {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
        }
    }

    fn item(
        title: Option<&str>,
        icon: Option<Vec<IconPixmap>>,
        menu: Option<&str>,
    ) -> StatusNotifierItem {
        StatusNotifierItem {
            id: "app".into(),
            category: Category::ApplicationStatus,
            title: title.map(str::to_string),
            status: Status::Active,
            window_id: 0,
            icon_theme_path: None,
            icon_name: None,
            icon_pixmap: icon,
            overlay_icon_name: None,
            overlay_icon_pixmap: None,
            attention_icon_name: None,
            attention_icon_pixmap: None,
            attention_movie_name: None,
            tool_tip: None,
            item_is_menu: false,
            menu: menu.map(str::to_string),
        }
    }

    fn menu_item(
        id: i32,
        label: &str,
        visible: bool,
        enabled: bool,
        separator: bool,
        submenu: Vec<MenuItem>,
    ) -> MenuItem {
        MenuItem {
            id,
            menu_type: if separator {
                MenuType::Separator
            } else {
                MenuType::Standard
            },
            label: Some(label.to_string()),
            enabled,
            visible,
            submenu,
            ..MenuItem::default()
        }
    }

    #[test]
    fn conversion_keeps_identity_and_menu_path() {
        let source = item(Some("Volume"), Some(vec![pixmap(24, 24)]), Some("/MenuBar"));
        let converted = item_from(":1.42/StatusNotifierItem", &source, None);
        assert_eq!(converted.address, ":1.42/StatusNotifierItem");
        assert_eq!(converted.id, "app");
        assert_eq!(converted.title, "Volume");
        assert_eq!(converted.status, Status::Active);
        assert_eq!(converted.menu_path.as_deref(), Some("/MenuBar"));
        assert_eq!(converted.icon.as_ref().map(|icon| icon.width), Some(24));
    }

    #[test]
    fn icon_selection_prefers_the_largest_useful_pixmap() {
        let pixmaps = vec![
            pixmap(16, 16),
            pixmap(128, 128),
            pixmap(64, 64),
            pixmap(32, 32),
        ];
        let chosen = pick_icon(Some(&pixmaps)).expect("a usable icon");
        assert_eq!((chosen.width, chosen.height), (64, 64));
    }

    #[test]
    fn icon_selection_ignores_short_pixel_buffers() {
        let mut broken = pixmap(64, 64);
        broken.pixels.truncate(8);
        let chosen = pick_icon(Some(&[broken, pixmap(20, 20)])).expect("a usable icon");
        assert_eq!((chosen.width, chosen.height), (20, 20));
    }

    #[test]
    fn icon_selection_falls_back_to_the_smallest_oversized_pixmap() {
        let pixmaps = vec![pixmap(256, 256), pixmap(96, 96), pixmap(512, 512)];
        let chosen = pick_icon(Some(&pixmaps)).expect("a fallback icon");
        assert_eq!((chosen.width, chosen.height), (96, 96));
    }

    #[test]
    fn rgba_png_is_converted_to_argb_pixels() {
        let path = std::env::temp_dir().join(format!("nirubar-sni-{}.png", std::process::id()));
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut encoder = png::Encoder::new(file, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[10, 20, 30, 40]).unwrap();
        }
        let icon = load_png(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((icon.width, icon.height), (1, 1));
        assert_eq!(icon.pixels, vec![40, 10, 20, 30]);
    }

    #[test]
    fn menu_flattening_skips_invisible_and_indents_children() {
        let child = menu_item(2, "Child", true, true, false, Vec::new());
        let menu = TrayMenu {
            id: 0,
            submenus: vec![
                menu_item(1, "Open", true, true, false, vec![child]),
                menu_item(3, "Hidden", false, true, false, Vec::new()),
                menu_item(4, "", true, true, true, Vec::new()),
                menu_item(5, "Disabled", true, false, false, Vec::new()),
            ],
        };
        let rows = menu_rows(&menu);
        let ids: Vec<i32> = rows.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec![1, 2, 4, 5]);
        assert_eq!(rows[1].label, "  Child");
        assert_eq!(rows[1].depth, 1);
        assert!(rows[2].separator);
        assert!(!rows[2].enabled);
        assert!(!rows[3].label.is_empty());
        assert!(!rows[3].enabled);
    }

    #[test]
    fn a_missing_title_uses_the_id_initial() {
        let mut converted = item_from(":1.7", &item(None, None, None), None);
        assert_eq!(converted.initial(), 'A');
        converted.id = "  volume".into();
        assert_eq!(converted.initial(), 'V');
    }
}
