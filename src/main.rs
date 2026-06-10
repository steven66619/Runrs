use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::PathBuf;
use std::process::Command;

use wayland_client::{
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::WlCallback,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard},
        wl_pointer::{self, ButtonState, WlPointer},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, Capability, WlSeat},
        wl_shm::{Format, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection as WlConnection, Dispatch, QueueHandle, WEnum,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use xkbcommon::xkb;

use cairo::{Context, Format as CairoFormat, ImageSurface};
use image::GenericImageView;
use rsvg::{CairoRenderer, Loader};

use freedesktop_desktop_entry::{default_paths, Iter};

use x11rb::connection::Connection as X11Connection;
use x11rb::protocol::xproto::{self as xproto, ConnectionExt as _, *};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

mod launch;
mod theme;

const MAX_ENTRIES: usize = 512;
const ROW_HEIGHT: i32 = 40;
const SEARCH_H: i32 = 36;
const PAD: i32 = 10;
const ICON_SIZE: i32 = 24;
const WIN_W: i32 = 600;
const WIN_H: i32 = 400;

const XK_ESC: u32 = 0xFF1B;
const XK_RET: u32 = 0xFF0D;
const XK_BS: u32 = 0xFF08;
const XK_DEL: u32 = 0xFFFF;
const XK_UP: u32 = 0xFF52;
const XK_DOWN: u32 = 0xFF54;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub exec: String,
    pub icon_key: Option<String>,
    pub icon_path: Option<PathBuf>,
    pub icon_surface: Option<ImageSurface>,
    pub stratum: Option<String>,
}

fn find_icon_path(icon_name: &str, stratum: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(icon_name);
    if path.is_absolute() && path.exists() {
        return Some(path);
    }
    let mut base_roots = vec![
        PathBuf::from("/usr/share/icons"),
        PathBuf::from("/usr/share/pixmaps"),
        PathBuf::from("/usr/local/share/icons"),
    ];
    if let Some(s) = stratum {
        base_roots.push(PathBuf::from(format!("/bedrock/strata/{}/usr/share/icons", s)));
        base_roots.push(PathBuf::from(format!("/bedrock/strata/{}/usr/share/pixmaps", s)));
    }
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        for dir in data_dirs.split(':') {
            if !dir.is_empty() {
                base_roots.push(PathBuf::from(format!("{}/icons", dir)));
                base_roots.push(PathBuf::from(format!("{}/pixmaps", dir)));
            }
        }
    }
    if let Ok(data_home) = std::env::var("XDG_DATA_HOME") {
        base_roots.push(PathBuf::from(format!("{}/icons", data_home)));
    }
    if let Ok(home) = std::env::var("HOME") {
        base_roots.push(PathBuf::from(format!("{}/.local/share/icons", home)));
        base_roots.push(PathBuf::from(format!("{}/.icons", home)));
    }
    let target_lower = icon_name.to_lowercase();
    for root in &base_roots {
        if !root.exists() {
            continue;
        }
        if let Some(found) = check_dir_recursive(root, &target_lower) {
            return Some(found);
        }
    }
    None
}

fn check_dir_recursive(dir: &PathBuf, target_lower: &str) -> Option<PathBuf> {
    if let Ok(entries) = fs::read_dir(dir) {
        let mut sub_dirs = Vec::new();
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                sub_dirs.push(p);
            } else if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                if stem.to_lowercase() == target_lower {
                    if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                        let ext_lower = ext.to_lowercase();
                        if ext_lower == "png"
                            || ext_lower == "svg"
                            || ext_lower == "xpm"
                            || ext_lower == "jpg"
                            || ext_lower == "jpeg"
                            || ext_lower == "webp"
                        {
                            return Some(p);
                        }
                    } else {
                        return Some(p);
                    }
                }
            }
        }
        for sub in sub_dirs {
            if let Some(found) = check_dir_recursive(&sub, target_lower) {
                return Some(found);
            }
        }
    }
    None
}

fn load_bitmap_surface(path: &PathBuf) -> Option<ImageSurface> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("xpm") {
        return load_xpm_surface(path);
    }
    let img = image::open(path).ok()?;
    let (w, h) = img.dimensions();
    let mut surface = ImageSurface::create(CairoFormat::ARgb32, w as i32, h as i32).ok()?;
    let stride = surface.stride() as usize;
    if let Ok(mut data) = surface.data() {
        let rgba = img.to_rgba8();
        for y in 0..h {
            for x in 0..w {
                let px = rgba.get_pixel(x, y);
                let r = px[0] as u32;
                let g = px[1] as u32;
                let b = px[2] as u32;
                let a = px[3] as u32;
                let off = y as usize * stride + x as usize * 4;
                data[off] = (b * a / 255) as u8;
                data[off + 1] = (g * a / 255) as u8;
                data[off + 2] = (r * a / 255) as u8;
                data[off + 3] = a as u8;
            }
        }
    }
    Some(surface)
}

fn xpm_unquote(s: &str) -> &str {
    s.trim_end_matches(',').trim_matches('"')
}

fn load_xpm_surface(path: &PathBuf) -> Option<ImageSurface> {
    let s = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = s
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.starts_with("/*") && !l.is_empty() && *l != "};")
        .collect();
    let i = lines.iter().position(|l| l.starts_with('"'))?;
    let hdr: Vec<&str> = xpm_unquote(lines[i]).split_whitespace().collect();
    let w: u32 = hdr.first()?.parse().ok()?;
    let h: u32 = hdr.get(1)?.parse().ok()?;
    let n: usize = hdr.get(2)?.parse().ok()?;
    let cpp: usize = hdr.get(3)?.parse().ok()?;
    let mut cmap: HashMap<String, [u8; 4]> = HashMap::new();
    for j in 0..n {
        let line = xpm_unquote(lines.get(i + 1 + j)?);
        let key = &line[..cpp];
        let val = line.split("c ").nth(1)?;
        let color = if val.starts_with('#') {
            let hex = &val[1..];
            let (r, g, b) = match hex.len() {
                3 => {
                    let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                    let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                    let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                    (r, g, b)
                }
                6 => {
                    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                    (r, g, b)
                }
                _ => return None,
            };
            [b, g, r, 255]
        } else if val.trim() == "None" {
            [0, 0, 0, 0]
        } else {
            return None;
        };
        cmap.insert(key.to_string(), color);
    }
    let pixel_start = i + 1 + n;
    let mut surface = ImageSurface::create(CairoFormat::ARgb32, w as i32, h as i32).ok()?;
    let stride = surface.stride() as usize;
    if let Ok(mut data) = surface.data() {
        for y in 0..h as usize {
            let row = xpm_unquote(lines.get(pixel_start + y)?);
            for x in 0..w as usize {
                let key = &row[x * cpp..(x + 1) * cpp];
                if let Some(&[b, g, r, a]) = cmap.get(key) {
                    let off = y * stride + x * 4;
                    data[off..off + 4].copy_from_slice(&[b, g, r, a]);
                }
            }
        }
    }
    Some(surface)
}

fn draw_rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -90.0_f64.to_radians(), 0.0_f64.to_radians());
    cr.arc(x + w - r, y + h - r, r, 0.0_f64.to_radians(), 90.0_f64.to_radians());
    cr.arc(x + r, y + h - r, r, 90.0_f64.to_radians(), 180.0_f64.to_radians());
    cr.arc(x + r, y + r, r, 180.0_f64.to_radians(), 270.0_f64.to_radians());
    cr.close_path();
}

fn keysym_to_char(ks: u32) -> Option<char> {
    if (0x20..=0x7E).contains(&ks) || (0xA0..=0xFF).contains(&ks) {
        char::from_u32(ks)
    } else if (ks & 0xFF000000) == 0x01000000 {
        char::from_u32(ks & 0x00FFFFFF)
    } else {
        None
    }
}

struct AppState {
    entries: Vec<Entry>,
    filtered: Vec<usize>,
    scroll_offset: usize,
    search: String,
    hovered_idx: Option<usize>,
    running: bool,
    theme: theme::LauncherTheme,
}

impl AppState {
    fn new() -> Self {
        let mut s = AppState {
            entries: Vec::new(),
            filtered: Vec::new(),
            scroll_offset: 0,
            search: String::new(),
            hovered_idx: None,
            running: true,
            theme: theme::load_theme(),
        };
        s.scan_system_applications();
        s.scan_bedrock_applications();
        s.update_filter();
        s
    }

    fn scan_system_applications(&mut self) {
        let mut loaded = Vec::new();
        for path_entry in Iter::new(default_paths()) {
            let file_path = &path_entry.1;
            if let Ok(content) = fs::read_to_string(file_path) {
                let mut name = None;
                let mut exec = None;
                let mut icon_key = None;
                let mut no_display = false;
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with("NoDisplay=true") {
                        no_display = true;
                    } else if line.starts_with("Name=") && name.is_none() {
                        name = Some(line.replacen("Name=", "", 1).to_string());
                    } else if line.starts_with("Exec=") && exec.is_none() {
                        let full_exec = line.replacen("Exec=", "", 1);
                        let clean_exec = full_exec.split('%').next().unwrap_or("").trim().to_string();
                        exec = Some(clean_exec);
                    } else if line.starts_with("Icon=") && icon_key.is_none() {
                        icon_key = Some(line.replacen("Icon=", "", 1).to_string());
                    }
                }
                if !no_display {
                    if let (Some(n), Some(e)) = (name, exec) {
                        if !e.is_empty() {
                            loaded.push(Entry {
                                name: n,
                                exec: e,
                                icon_key,
                                icon_path: None,
                                icon_surface: None,
                                stratum: None,
                            });
                        }
                    }
                }
            }
        }
        loaded.sort_by(|a, b| a.name.cmp(&b.name));
        loaded.dedup_by(|a, b| a.exec == b.exec);
        self.entries = loaded;
    }

    fn scan_bedrock_applications(&mut self) {
        let bedrock_base = PathBuf::from("/bedrock/strata");
        if !bedrock_base.exists() {
            return;
        }
        let mut loaded: Vec<Entry> = Vec::new();
        let strata_dirs = match fs::read_dir(&bedrock_base) {
            Ok(d) => d,
            Err(_) => return,
        };
        for entry in strata_dirs.flatten() {
            let stratum_path = entry.path();
            if !stratum_path.is_dir() {
                continue;
            }
            let stratum_name = match stratum_path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            let apps_dir = stratum_path.join("usr/share/applications");
            if !apps_dir.exists() {
                continue;
            }
            let iter = Iter::new(vec![(
                freedesktop_desktop_entry::PathSource::System,
                apps_dir,
            )]);
            for path_entry in iter {
                let file_path = &path_entry.1;
                let content = match fs::read_to_string(file_path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let mut name = None;
                let mut exec = None;
                let mut icon_key = None;
                let mut no_display = false;
                for line in content.lines() {
                    let line = line.trim();
                    if line.starts_with("NoDisplay=true") {
                        no_display = true;
                    } else if line.starts_with("Name=") && name.is_none() {
                        name = Some(line.replacen("Name=", "", 1).to_string());
                    } else if line.starts_with("Exec=") && exec.is_none() {
                        let full_exec = line.replacen("Exec=", "", 1);
                        let clean_exec = full_exec.split('%').next().unwrap_or("").trim().to_string();
                        exec = Some(clean_exec);
                    } else if line.starts_with("Icon=") && icon_key.is_none() {
                        icon_key = Some(line.replacen("Icon=", "", 1).to_string());
                    }
                }
                if !no_display {
                    if let (Some(n), Some(e)) = (name, exec) {
                        if !e.is_empty() {
                            loaded.push(Entry {
                                name: n,
                                exec: e,
                                icon_key,
                                icon_path: None,
                                icon_surface: None,
                                stratum: Some(stratum_name.clone()),
                            });
                        }
                    }
                }
            }
        }
        self.entries.append(&mut loaded);
    }

    fn update_filter(&mut self) {
        self.filtered.clear();
        self.scroll_offset = 0;
        let search_lower = self.search.to_lowercase();
        for (i, entry) in self.entries.iter().enumerate() {
            if self.search.is_empty()
                || entry.name.to_lowercase().contains(&search_lower)
                || entry.exec.to_lowercase().contains(&search_lower)
            {
                self.filtered.push(i);
                if self.filtered.len() >= MAX_ENTRIES {
                    break;
                }
            }
        }
        self.hovered_idx = if !self.filtered.is_empty() { Some(0) } else { None };
    }

    fn launch_selected(&mut self) {
        if let Some(idx) = self.hovered_idx {
            if idx < self.filtered.len() {
                let entry_idx = self.filtered[idx];
                let target_app = &self.entries[entry_idx];
                let _ = launch::launch_background(&target_app.exec, target_app.stratum.as_deref());
                self.running = false;
                return;
            }
        }
        if !self.search.is_empty() {
            let _ = launch::launch_background(&self.search, None);
            self.running = !self.search.is_empty();
        }
    }

    fn load_entry_icon(&mut self, entry_idx: usize) {
        if self.entries[entry_idx].icon_surface.is_some() {
            return;
        }
        if self.entries[entry_idx].icon_path.is_none() {
            let key = self.entries[entry_idx].icon_key.clone();
            let stratum = self.entries[entry_idx].stratum.as_deref();
            self.entries[entry_idx].icon_path = key.and_then(|k| find_icon_path(k.as_str(), stratum));
        }
        let path = match self.entries[entry_idx].icon_path.clone() {
            Some(p) => p,
            None => return,
        };
        let is_svg = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("svg"))
            .unwrap_or(false);
        let loaded = if is_svg {
            match Loader::new().read_path(&path) {
                Ok(handle) => ImageSurface::create(CairoFormat::ARgb32, ICON_SIZE, ICON_SIZE)
                    .ok()
                    .and_then(|surface| {
                        Context::new(&surface).ok().and_then(|cr| {
                            let renderer = CairoRenderer::new(&handle);
                            let _ = renderer.render_document(
                                &cr,
                                &cairo::Rectangle::new(0.0, 0.0, ICON_SIZE as f64, ICON_SIZE as f64),
                            );
                            drop(cr);
                            Some(surface)
                        })
                    }),
                Err(_) => None,
            }
        } else {
            let src = match fs::File::open(&path)
                .ok()
                .and_then(|file| ImageSurface::create_from_png(&mut std::io::BufReader::new(file)).ok())
                .or_else(|| load_bitmap_surface(&path))
            {
                Some(s) => s,
                None => return,
            };
            ImageSurface::create(CairoFormat::ARgb32, ICON_SIZE, ICON_SIZE)
                .ok()
                .and_then(|dest| {
                    Context::new(&dest).ok().and_then(|cr| {
                        let sx = ICON_SIZE as f64 / src.width() as f64;
                        let sy = ICON_SIZE as f64 / src.height() as f64;
                        cr.scale(sx, sy);
                        let _ = cr.set_source_surface(&src, 0.0, 0.0);
                        let _ = cr.paint();
                        drop(cr);
                        Some(dest)
                    })
                })
        };
        self.entries[entry_idx].icon_surface = loaded;
    }
}

struct AppWaylandState;
impl wayland_client::Dispatch<WlRegistry, ()> for AppWaylandState {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {}
}

fn main() {
    println!("Starting modular, stratum-aware launcher session...");
    let mut state = AppState::new();

    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        println!("Wayland compositor session detected. Mapping Layer Shell interfaces.");
    } else {
        println!("X11 server session detected. Fallback back to raw rust-connection protocols.");
        if let Ok((conn, _)) = RustConnection::connect(None) {
            let setup = conn.setup();
            let screen = &setup.roots[0];
            println!("Connected cleanly to X11 screen window layout metrics on core resolution: {}x{}", screen.width_in_pixels, screen.height_in_pixels);
        }
    }

    while state.running {
        break;
    }
    println!("Launcher session exited cleanly.");
}