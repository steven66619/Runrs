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
struct Entry {
    name: String,
    exec: String,
    icon_key: Option<String>,
    icon_path: Option<PathBuf>,
    icon_surface: Option<ImageSurface>,
    stratum: Option<String>,
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
        base_roots.push(PathBuf::from(format!(
            "/bedrock/strata/{}/usr/share/icons",
            s
        )));
        base_roots.push(PathBuf::from(format!(
            "/bedrock/strata/{}/usr/share/pixmaps",
            s
        )));
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
    use std::collections::HashMap;
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
    cr.arc(
        x + w - r,
        y + r,
        r,
        -90.0_f64.to_radians(),
        0.0_f64.to_radians(),
    );
    cr.arc(
        x + w - r,
        y + h - r,
        r,
        0.0_f64.to_radians(),
        90.0_f64.to_radians(),
    );
    cr.arc(
        x + r,
        y + h - r,
        r,
        90.0_f64.to_radians(),
        180.0_f64.to_radians(),
    );
    cr.arc(
        x + r,
        y + r,
        r,
        180.0_f64.to_radians(),
        270.0_f64.to_radians(),
    );
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
                        let clean_exec =
                            full_exec.split('%').next().unwrap_or("").trim().to_string();
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
        let mut brl_cache: HashMap<String, String> = HashMap::new();
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
                        let clean_exec =
                            full_exec.split('%').next().unwrap_or("").trim().to_string();
                        exec = Some(clean_exec);
                    } else if line.starts_with("Icon=") && icon_key.is_none() {
                        icon_key = Some(line.replacen("Icon=", "", 1).to_string());
                    }
                }
                if !no_display {
                    if let (Some(n), Some(e)) = (name, exec) {
                        if !e.is_empty() {
                            let bin_name = e.split_whitespace().next()
                                .and_then(|s| std::path::Path::new(s).file_name()
                                    .and_then(|f| f.to_str()))
                                .map(|s| s.to_string());
                            if let Some(ref binary) = bin_name {
                                let resolved = brl_cache.entry(binary.clone()).or_insert_with(|| {
                                    Command::new("brl-which")
                                        .arg(binary)
                                        .output()
                                        .ok()
                                        .filter(|o| o.status.success())
                                        .and_then(|o| {
                                            let s = String::from_utf8_lossy(&o.stdout)
                                                .trim().to_string();
                                            if s.is_empty() { None } else { Some(s) }
                                        })
                                        .unwrap_or_default()
                                });
                                if !resolved.is_empty() && resolved != &stratum_name {
                                    continue;
                                }
                            }
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
        self.hovered_idx = if !self.filtered.is_empty() {
            Some(0)
        } else {
            None
        };
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
            self.entries[entry_idx].icon_path = key.and_then(|k| find_icon_path(&k, stratum));
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
                            renderer
                                .render_document(
                                    &cr,
                                    &cairo::Rectangle::new(
                                        0.0,
                                        0.0,
                                        ICON_SIZE as f64,
                                        ICON_SIZE as f64,
                                    ),
                                )
                                .ok()?;
                            drop(cr);
                            Some(surface)
                        })
                    }),
                Err(_) => None,
            }
        } else {
            let src = match fs::File::open(&path)
                .ok()
                .and_then(|file| {
                    ImageSurface::create_from_png(&mut std::io::BufReader::new(file)).ok()
                })
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
                        let s = sx.min(sy);
                        let w = src.width() as f64 * s;
                        let h = src.height() as f64 * s;
                        let ox = (ICON_SIZE as f64 - w) / 2.0;
                        let oy = (ICON_SIZE as f64 - h) / 2.0;
                        cr.save().ok()?;
                        cr.translate(ox, oy);
                        cr.scale(s, s);
                        cr.set_source_surface(&src, 0.0, 0.0).ok()?;
                        cr.paint().ok()?;
                        cr.restore().ok()?;
                        drop(cr);
                        Some(dest)
                    })
                })
        };
        if let Some(surface) = loaded {
            self.entries[entry_idx].icon_surface = Some(surface);
        } else {
            self.entries[entry_idx].icon_path = None;
        }
    }

    fn draw(&mut self, cr: &Context, w: i32, h: i32) {
        let t = &self.theme;
        let br = t.border_radius as f64;
        let bw = t.border_width as f64;

        cr.set_operator(cairo::Operator::Clear);
        cr.paint().ok();
        cr.set_operator(cairo::Operator::Over);

        let (bg_r, bg_g, bg_b, bg_a) = t.bg_rgba();
        draw_rounded_rect(cr, 0.0, 0.0, w as f64, h as f64, br);
        cr.set_source_rgba(bg_r, bg_g, bg_b, bg_a);
        cr.fill().ok();

        let (ac_r, ac_g, ac_b, ac_a) = t.accent_rgba();
        cr.set_source_rgba(ac_r, ac_g, ac_b, ac_a * 0.25);
        cr.set_line_width(bw);
        draw_rounded_rect(cr, 0.0, 0.0, w as f64, h as f64, br);
        cr.stroke().ok();

        let sbx = PAD as f64;
        let sbw = (w - PAD * 2) as f64;
        let search_br = (br / 2.0).max(4.0);
        draw_rounded_rect(cr, sbx, 8.0, sbw, SEARCH_H as f64, search_br);
        cr.set_source_rgba(bg_r, bg_g, bg_b, (bg_a * 1.4).min(1.0));
        cr.fill().ok();
        cr.set_source_rgba(ac_r, ac_g, ac_b, ac_a * 0.4);
        cr.set_line_width(bw);
        draw_rounded_rect(cr, sbx, 8.0, sbw, SEARCH_H as f64, search_br);
        cr.stroke().ok();

        let disp = if self.search.is_empty() {
            "  Type to search...".to_string()
        } else {
            format!("  {}_", self.search)
        };

        let fd_s = pango::FontDescription::from_string("Sans 12");
        let pango_ctx = pangocairo::functions::create_context(cr);
        let lay_s = pango::Layout::new(&pango_ctx);
        lay_s.set_font_description(Some(&fd_s));
        lay_s.set_text(&disp);
        let (tx_r, tx_g, tx_b, tx_a) = t.text_rgba();
        let (ac_r, ac_g, ac_b, _) = t.accent_rgba();
        cr.move_to(sbx + 4.0, 8.0 + (SEARCH_H as f64 - 18.0) / 2.0);
        cr.set_source_rgba(ac_r, ac_g, ac_b, tx_a);
        pangocairo::functions::show_layout(cr, &lay_s);

        let start_y = 8.0 + (SEARCH_H as f64) + (PAD as f64);
        let max_visible = ((h as f64 - start_y) / ROW_HEIGHT as f64) as usize;

        for idx in 0..max_visible {
            let filtered_idx = idx + self.scroll_offset;
            if filtered_idx >= self.filtered.len() {
                break;
            }
            let entry_idx = self.filtered[filtered_idx];
            self.load_entry_icon(entry_idx);

            let entry = &self.entries[entry_idx];
            let row_y = start_y + (idx as f64 * ROW_HEIGHT as f64);

            if self.hovered_idx == Some(filtered_idx) {
                draw_rounded_rect(cr, sbx, row_y, sbw, ROW_HEIGHT as f64, 4.0);
                cr.set_source_rgba(ac_r, ac_g, ac_b, 0.15);
                cr.fill().ok();
            }

            let mut text_offset = 12.0;
            if let Some(icon) = entry.icon_surface.as_ref() {
                cr.save().ok();
                let icon_x = sbx + PAD as f64 + 6.0;
                let icon_y = row_y + ((ROW_HEIGHT - ICON_SIZE) / 2) as f64;
                cr.rectangle(icon_x, icon_y, ICON_SIZE as f64, ICON_SIZE as f64);
                cr.clip();
                cr.set_source_surface(icon, icon_x, icon_y).ok();
                cr.paint().ok();
                cr.restore().ok();
                text_offset += (ICON_SIZE + 10) as f64;
            }

            let text_layout = pango::Layout::new(&pango_ctx);
            text_layout.set_font_description(Some(&fd_s));
            text_layout.set_text(&entry.name);
            cr.move_to(
                sbx + PAD as f64 + text_offset,
                row_y + ((ROW_HEIGHT - 18) / 2) as f64,
            );
            cr.set_source_rgba(tx_r, tx_g, tx_b, tx_a);
            pangocairo::functions::show_layout(cr, &text_layout);

            if let Some(ref stratum) = entry.stratum {
                let tag = format!("  ({})", stratum);
                let (_ink, logical) = text_layout.pixel_extents();
                let extents = logical.width() as f64;
                let tag_layout = pango::Layout::new(&pango_ctx);
                let fd_tag = pango::FontDescription::from_string("Sans 10");
                tag_layout.set_font_description(Some(&fd_tag));
                tag_layout.set_text(&tag);
                cr.move_to(
                    sbx + PAD as f64 + text_offset + extents,
                    row_y + ((ROW_HEIGHT - 18) / 2) as f64 + 1.0,
                );
                cr.set_source_rgba(0.5, 0.5, 0.6, 0.8);
                pangocairo::functions::show_layout(cr, &tag_layout);
            }
        }
    }

    fn handle_keysym(&mut self, ks: u32) {
        match ks {
            XK_ESC => self.running = false,
            XK_RET => self.launch_selected(),
            XK_BS | XK_DEL => {
                self.search.pop();
                self.update_filter();
            }
            XK_UP => {
                if let Some(curr) = self.hovered_idx {
                    if curr > 0 {
                        self.hovered_idx = Some(curr - 1);
                        if curr - 1 < self.scroll_offset {
                            self.scroll_offset = curr - 1;
                        }
                    }
                }
            }
            XK_DOWN => {
                if let Some(curr) = self.hovered_idx {
                    if curr + 1 < self.filtered.len() {
                        self.hovered_idx = Some(curr + 1);
                        let max_visible = ((WIN_H as f64 - (8.0 + SEARCH_H as f64 + PAD as f64))
                            / ROW_HEIGHT as f64) as usize;
                        if curr + 1 >= self.scroll_offset + max_visible {
                            self.scroll_offset += 1;
                        }
                    }
                }
            }
            _ => {
                if let Some(c) = keysym_to_char(ks) {
                    self.search.push(c);
                    self.update_filter();
                }
            }
        }
    }

    fn handle_motion(&mut self, _x: f64, y: f64) {
        let start_y = 8.0 + (SEARCH_H as f64) + (PAD as f64);
        if y >= start_y {
            let click_idx = ((y - start_y) / ROW_HEIGHT as f64) as usize + self.scroll_offset;
            if click_idx < self.filtered.len() {
                self.hovered_idx = Some(click_idx);
            }
        }
    }
}

// ---- Wayland backend ----

struct ShmPool {
    buffer: WlBuffer,
    _mmap: memmap2::MmapMut,
}

struct WaylandState {
    app: AppState,
    width: i32,
    height: i32,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    _configured: bool,
    shm_pool: Option<ShmPool>,
    _xkb_ctx: Option<xkb::Context>,
    xkb_state: Option<xkb::State>,
    cairo_surface: Option<ImageSurface>,
    needs_render: bool,
    first_configure_done: bool,
    frame_callback: Option<WlCallback>,
}

impl WaylandState {
    fn new() -> Self {
        WaylandState {
            app: AppState::new(),
            width: WIN_W,
            height: WIN_H,
            compositor: None,
            shm: None,
            layer_shell: None,
            seat: None,
            pointer: None,
            keyboard: None,
            surface: None,
            layer_surface: None,
            _configured: false,
            shm_pool: None,
            _xkb_ctx: None,
            xkb_state: None,
            cairo_surface: None,
            needs_render: false,
            first_configure_done: false,
            frame_callback: None,
        }
    }

    fn create_shm_pool(&mut self, qh: &QueueHandle<Self>) {
        let shm = match &self.shm {
            Some(s) => s,
            None => return,
        };
        let width = self.width;
        let height = self.height;
        let stride = match CairoFormat::ARgb32.stride_for_width(width as u32) {
            Ok(s) => s,
            Err(_) => return,
        };
        let size = (stride * height) as u64;

        let name = CString::new("launcher").unwrap();
        let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if raw_fd < 0 {
            return;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) } < 0 {
            return;
        }

        let mut mmap = match unsafe { memmap2::MmapMut::map_mut(&fd) } {
            Ok(m) => m,
            Err(_) => return,
        };
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, Format::Argb8888, qh, ());
        drop(pool);

        let ptr = mmap.as_mut_ptr();
        let len = mmap.len();
        let static_slice: &'static mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr, len) };

        let cairo_surface =
            ImageSurface::create_for_data(static_slice, CairoFormat::ARgb32, width, height, stride)
                .ok();

        self.cairo_surface = cairo_surface;
        self.shm_pool = Some(ShmPool {
            buffer,
            _mmap: mmap,
        });
    }

    fn render(&mut self, qh: &QueueHandle<Self>) {
        if !self.first_configure_done {
            return;
        }
        if self.frame_callback.is_some() {
            return;
        }

        let any_missing = self
            .app
            .entries
            .iter()
            .any(|e| e.icon_path.is_some() && e.icon_surface.is_none());

        let cairo_surface = match self.cairo_surface.as_ref() {
            Some(s) => s,
            _ => return,
        };
        let shm_pool = match self.shm_pool.as_ref() {
            Some(p) => p,
            _ => return,
        };
        let surface = match self.surface.as_ref() {
            Some(s) => s,
            _ => return,
        };
        let w = self.width;
        let h = self.height;

        let cr = match Context::new(cairo_surface) {
            Ok(c) => c,
            Err(_) => return,
        };
        self.app.draw(&cr, w, h);
        drop(cr);
        cairo_surface.flush();

        let callback = surface.frame(qh, ());
        self.frame_callback = Some(callback);

        if any_missing {
            self.needs_render = true;
        }

        surface.attach(Some(&shm_pool.buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
    }
}

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor =
                        Some(registry.bind::<WlCompositor, _, _>(name, version, qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind::<WlShm, _, _>(name, version, qh, ())),
                "zwlr_layer_shell_v1" => {
                    state.layer_shell =
                        Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, version, qh, ()))
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind::<WlSeat, _, _>(name, version, qh, ()))
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlCompositor, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wayland_client::protocol::wl_compositor::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlShm, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wayland_client::protocol::wl_shm::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlShmPool, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wayland_client::protocol::wl_shm_pool::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlBuffer, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wayland_client::protocol::wl_buffer::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlSurface, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wayland_client::protocol::wl_surface::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlCallback, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        _: wayland_client::protocol::wl_callback::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        state.frame_callback = None;
        if state.needs_render {
            state.needs_render = false;
            state.render(qh);
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ZwlrLayerShellV1,
        _: wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                surface.ack_configure(serial);
                state.width = if width > 0 { width as i32 } else { WIN_W };
                state.height = if height > 0 { height as i32 } else { WIN_H };
                state.create_shm_pool(qh);
                state.first_configure_done = true;
                state.render(qh);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.app.running = false;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if caps.contains(Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Motion { surface_y, .. } => {
                state.app.handle_motion(0.0, surface_y);
                state.render(qh);
            }
            wl_pointer::Event::Button {
                state: WEnum::Value(ButtonState::Pressed),
                ..
            } => {
                state.app.launch_selected();
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                if let Ok(Some(device_keymap)) = unsafe {
                    xkb::Keymap::new_from_fd(
                        &context,
                        fd,
                        size as usize,
                        xkb::KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    )
                } {
                    state.xkb_state = Some(xkb::State::new(&device_keymap));
                }
                state._xkb_ctx = Some(context);
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(KeyState::Pressed),
                ..
            } => {
                if let Some(xkb_state) = &state.xkb_state {
                    let sym = xkb_state.key_get_one_sym((key + 8).into());
                    let raw_ks = u32::from(sym);
                    state.app.handle_keysym(raw_ks);
                    state.render(qh);
                }
            }
            _ => {}
        }
    }
}

fn run_wayland() {
    let conn = WlConnection::connect_to_env().expect("Failed to connect to Wayland");
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = WaylandState::new();
    let _registry = conn.display().get_registry(&qh, ());

    event_queue.roundtrip(&mut state).unwrap();
    event_queue.roundtrip(&mut state).unwrap();

    let compositor = state
        .compositor
        .as_ref()
        .expect("wl_compositor not available");
    let layer_shell = state
        .layer_shell
        .as_ref()
        .expect("zwlr_layer_shell_v1 not available");

    let wl_surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &wl_surface,
        None,
        Layer::Overlay,
        "launcher".to_string(),
        &qh,
        (),
    );

    layer_surface.set_size(WIN_W as u32, WIN_H as u32);
    layer_surface.set_anchor(Anchor::all());
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);

    wl_surface.commit();

    state.surface = Some(wl_surface);
    state.layer_surface = Some(layer_surface);

    while state.app.running {
        event_queue.blocking_dispatch(&mut state).unwrap();
    }
}

// ---- X11 backend ----

struct X11Backend {
    app: AppState,
    conn: RustConnection,
    window: u32,
    gc: u32,
    cairo_surface: ImageSurface,
    min_keycode: Keycode,
    keymap: Vec<Vec<Keysym>>,
    keysyms_per_keycode: usize,
}

impl X11Backend {
    fn new() -> Result<Self, String> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|e| format!("X11 connect: {}", e))?;
        let screen = &conn.setup().roots[screen_num];
        let screen_w = screen.width_in_pixels;
        let screen_h = screen.height_in_pixels;
        let x = (screen_w as i16 - WIN_W as i16) / 2;
        let y = (screen_h as i16 - WIN_H as i16) / 2;

        let setup = conn.setup();
        let min_kc = setup.min_keycode;
        let max_kc = setup.max_keycode;
        let kc_count = max_kc - min_kc + 1;
        let km_reply = conn
            .get_keyboard_mapping(min_kc, kc_count)
            .map_err(|e| format!("get_keyboard_mapping: {}", e))?
            .reply()
            .map_err(|e| format!("get_keyboard_mapping reply: {}", e))?;
        let kspk = km_reply.keysyms_per_keycode as usize;
        let keymap: Vec<Vec<Keysym>> = km_reply.keysyms.chunks(kspk).map(|c| c.to_vec()).collect();

        // Find a 24-bit depth visual for pixel compatibility
        let (use_depth, use_visual) = screen
            .allowed_depths
            .iter()
            .find(|d| d.depth == 24)
            .and_then(|d| d.visuals.first())
            .map(|v| (24u8, v.visual_id))
            .unwrap_or((screen.root_depth, screen.root_visual));

        let win = conn
            .generate_id()
            .map_err(|e| format!("generate_id: {}", e))?;
        let cmap = conn
            .generate_id()
            .map_err(|e| format!("generate_id cmap: {}", e))?;
        conn.create_colormap(ColormapAlloc::NONE, cmap, screen.root, use_visual)
            .map_err(|e| format!("create_colormap: {}", e))?
            .check()
            .map_err(|e| format!("create_colormap check: {}", e))?;
        conn.create_window(
            use_depth,
            win,
            screen.root,
            x.max(0),
            y.max(0),
            WIN_W as u16,
            WIN_H as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            use_visual,
            &CreateWindowAux::new()
                .override_redirect(1u32)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::KEY_PRESS
                        | EventMask::BUTTON_PRESS
                        | EventMask::POINTER_MOTION,
                )
                .background_pixel(0)
                .colormap(cmap),
        )
        .map_err(|e| format!("create_window: {}", e))?;

        let gc = conn
            .generate_id()
            .map_err(|e| format!("generate_id gc: {}", e))?;
        conn.create_gc(gc, win, &CreateGCAux::new().graphics_exposures(0))
            .map_err(|e| format!("create_gc: {}", e))?;

        let surface = ImageSurface::create(CairoFormat::Rgb24, WIN_W, WIN_H)
            .map_err(|e| format!("cairo surface: {}", e))?;

        Ok(X11Backend {
            app: AppState::new(),
            conn,
            window: win,
            gc,
            cairo_surface: surface,
            min_keycode: min_kc,
            keymap,
            keysyms_per_keycode: kspk,
        })
    }

    fn lookup_keysym(&self, keycode: Keycode, col: u8) -> u32 {
        let idx = (keycode.saturating_sub(self.min_keycode)) as usize;
        if idx < self.keymap.len() {
            let col = col as usize;
            if col < self.keysyms_per_keycode {
                if let Some(&ks) = self.keymap[idx].get(col) {
                    return ks;
                }
            }
        }
        0
    }

    fn run(&mut self) {
        self.conn.map_window(self.window).ok();
        self.conn.flush().ok();
        // Render immediately so content is visible even before first Expose
        self.render();

        if let Err(e) =
            self.conn
                .set_input_focus(InputFocus::PARENT, self.window, x11rb::CURRENT_TIME)
        {
            eprintln!("set_input_focus error: {e}");
        }
        for attempt in 0..50 {
            self.conn.flush().ok();
            let grabbed = self.conn
                .grab_keyboard(false, self.window, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
                .ok()
                .and_then(|c| c.reply().ok())
                .map_or(false, |r| r.status == GrabStatus::SUCCESS);
            if grabbed {
                break;
            }
            if attempt == 49 {
                eprintln!("grab_keyboard: could not grab after 50 attempts");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        while self.app.running {
            if let Ok(event) = self.conn.wait_for_event() {
                self.handle_event(event);
            } else {
                break;
            }
        }

        self.conn.ungrab_keyboard(x11rb::CURRENT_TIME).ok();
        self.conn.destroy_window(self.window).ok();
        self.conn.flush().ok();
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Expose(e) => {
                if e.count == 0 {
                    self.render();
                }
            }
            Event::KeyPress(e) => {
                let shift = (e.state & KeyButMask::SHIFT) != KeyButMask::default();
                let col = if shift { 1 } else { 0 };
                let ks = self.lookup_keysym(e.detail, col);
                self.app.handle_keysym(ks);
                self.render();
            }
            Event::ButtonPress(e) => {
                if e.detail == 1 {
                    self.app.launch_selected();
                }
            }
            Event::MotionNotify(e) => {
                self.app.handle_motion(e.event_x as f64, e.event_y as f64);
                self.render();
            }
            _ => {}
        }
    }

    fn render(&mut self) {
        let cr = match Context::new(&self.cairo_surface) {
            Ok(c) => c,
            Err(_) => return,
        };
        self.app.draw(&cr, WIN_W, WIN_H);
        drop(cr);
        self.cairo_surface.flush();

        let setup = self.conn.setup();
        let pix_fmt = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == 24)
            .expect("no pixmap format for depth 24");
        let bpp = pix_fmt.bits_per_pixel as usize / 8;
        let sp = pix_fmt.scanline_pad as usize;
        let x11_stride =
            ((WIN_W as usize * pix_fmt.bits_per_pixel as usize + sp - 1) / sp) * (sp / 8);
        let cairo_stride = CairoFormat::Rgb24
            .stride_for_width(WIN_W as u32)
            .unwrap_or(WIN_W * 4) as usize;

        let data = match self.cairo_surface.data() {
            Ok(d) => d,
            Err(_) => return,
        };

        // Repack from Cairo stride to X11 stride with correct bytes-per-pixel
        let mut packed = vec![0u8; x11_stride * WIN_H as usize];
        for row in 0..WIN_H as usize {
            let src_off = row * cairo_stride;
            let dst_off = row * x11_stride;
            for col in 0..WIN_W as usize {
                let si = src_off + col * 4;
                let di = dst_off + col * bpp;
                let copy_len = bpp.min(3);
                packed[di..di + copy_len].copy_from_slice(&data[si..si + copy_len]);
            }
        }

        let _ = xproto::put_image(
            &self.conn,
            ImageFormat::Z_PIXMAP,
            self.window,
            self.gc,
            WIN_W as u16,
            WIN_H as u16,
            0,
            0,
            0,
            24,
            &packed,
        );
        self.conn.flush().ok();
    }
}

fn run_x11() {
    let mut backend = match X11Backend::new() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to start X11 backend: {}", e);
            return;
        }
    };
    backend.run();
}

// ---- main ----

fn main() {
    let wayland_env = std::env::var("WAYLAND_DISPLAY").ok();
    let is_wayland = wayland_env.as_ref().map_or(false, |v| !v.is_empty());

    if is_wayland {
        run_wayland();
    } else {
        run_x11();
    }
}
