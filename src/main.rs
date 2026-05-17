use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::process::Command;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard, KeymapFormat, KeyState},
        wl_pointer::{self, WlPointer, ButtonState},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat, Capability},
        wl_shm::{WlShm, Format},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
        wl_callback::WlCallback,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{ZwlrLayerShellV1, Layer},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1, Anchor, KeyboardInteractivity},
};

use xkbcommon::xkb;
use xkeysym::key;

use cairo::{Context, Format as CairoFormat, ImageSurface};
use rsvg::{CairoRenderer, Loader};

// Only pull system target paths from the freedesktop utility crate
use freedesktop_desktop_entry::{default_paths, Iter};

const MAX_ENTRIES: usize = 512;
const ROW_HEIGHT: i32 = 40;
const SEARCH_H: i32 = 36;
const PAD: i32 = 10;
const ICON_SIZE: i32 = 24;

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    exec: String,
    icon_key: Option<String>,
    icon_path: Option<PathBuf>,          
    icon_surface: Option<ImageSurface>,  
}

struct ShmPool {
    buffer: WlBuffer,
    _mmap: memmap2::MmapMut,
}

struct WaylandState {
    running: bool,
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
    entries: Vec<Entry>,                 
    filtered: Vec<usize>,
    scroll_offset: usize,
    search: String,
    hovered_idx: Option<usize>,
    cairo_surface: Option<ImageSurface>,
    needs_render: bool,
    first_configure_done: bool,
    frame_callback: Option<WlCallback>,
    found_paths: Arc<Mutex<Vec<(usize, PathBuf)>>>,
}

// Deep structural folder walker optimized for CachyOS and Arch theme directories
fn find_icon_path(icon_name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(icon_name);
    if path.is_absolute() && path.exists() {
        return Some(path);
    }

    let mut base_roots = vec![
        PathBuf::from("/usr/share/icons"),
        PathBuf::from("/usr/share/pixmaps"),
        PathBuf::from("/usr/local/share/icons"),
    ];

    if let Ok(home) = std::env::var("HOME") {
        base_roots.push(PathBuf::from(format!("{}/.local/share/icons", home)));
        base_roots.push(PathBuf::from(format!("{}/.icons", home)));
    }

    let target_lower = icon_name.to_lowercase();

    for root in base_roots {
        if !root.exists() { continue; }
        if let Some(found) = check_dir_recursive(&root, &target_lower) {
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
                        // Accepts modern bitmaps as well as structural vector images natively
                        if ext_lower == "png" || ext_lower == "svg" {
                            return Some(p);
                        }
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

fn draw_rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -90.0_f64.to_radians(), 0.0_f64.to_radians());
    cr.arc(x + w - r, y + h - r, r, 0.0_f64.to_radians(), 90.0_f64.to_radians());
    cr.arc(x + r, y + h - r, r, 90.0_f64.to_radians(), 180.0_f64.to_radians());
    cr.arc(x + r, y + r, r, 180.0_f64.to_radians(), 270.0_f64.to_radians());
    cr.close_path();
}

impl WaylandState {
    fn new() -> Self {
        let mut state = WaylandState {
            running: true, width: 600, height: 400,
            compositor: None, shm: None, layer_shell: None, seat: None,
            pointer: None, keyboard: None, surface: None, layer_surface: None,
            _configured: false, shm_pool: None, _xkb_ctx: None, xkb_state: None,
            entries: Vec::new(), filtered: Vec::new(), scroll_offset: 0,
            search: String::new(), hovered_idx: None, cairo_surface: None,
            needs_render: false, first_configure_done: false,
            frame_callback: None,
            found_paths: Arc::new(Mutex::new(Vec::new())),
        };
        
        state.scan_system_applications();
        state.update_filter();
        state
    }

    fn scan_system_applications(&mut self) {
        let mut loaded = Vec::new();

        // FIXED: Universal hand-rolled line crawler completely avoids crate API parsing blocks
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
                        // Clean trailing utility variables like %U, %f from shortcuts
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
                            });
                        }
                    }
                }
            }
        }
        
        loaded.sort_by(|a, b| a.name.cmp(&b.name));
        loaded.dedup_by(|a, b| a.exec == b.exec);
        self.entries = loaded;

        let keys_to_load: Vec<(usize, String)> = self.entries.iter().enumerate()
            .filter_map(|(i, entry)| entry.icon_key.clone().map(|key| (i, key)))
            .collect();

        let found_paths_worker = Arc::clone(&self.found_paths);

        thread::spawn(move || {
            for (idx, key) in keys_to_load {
                if let Some(img_path) = find_icon_path(&key) {
                    if let Ok(mut guard) = found_paths_worker.lock() {
                        guard.push((idx, img_path));
                    }
                }
            }
        });
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
                if self.filtered.len() >= MAX_ENTRIES { break; }
            }
        }
        self.hovered_idx = if !self.filtered.is_empty() { Some(0) } else { None };
    }

    fn launch_selected(&mut self) {
        if let Some(idx) = self.hovered_idx {
            if idx < self.filtered.len() {
                let entry_idx = self.filtered[idx];
                let target_app = &self.entries[entry_idx];

                let clean_command = target_app.exec
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .replace('"', "");

                if !clean_command.is_empty() {
                    let _ = Command::new(clean_command).spawn(); 
                }
                self.running = false; 
            }
        }
    }

    fn create_shm_pool(&mut self, qh: &QueueHandle<Self>) {
        let shm = match &self.shm { Some(s) => s, None => return };
        let width = self.width;
        let height = self.height;
        let stride = match CairoFormat::ARgb32.stride_for_width(width as u32) { Ok(s) => s, Err(_) => return };
        let size = (stride * height) as u64;

        let name = CString::new("launcher").unwrap();
        let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if raw_fd < 0 { return; }
        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) } < 0 { return; }

        let mut mmap = match unsafe { memmap2::MmapMut::map_mut(&fd) } { Ok(m) => m, Err(_) => return };
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, Format::Argb8888, qh, ());
        drop(pool);

        let ptr = mmap.as_mut_ptr();
        let len = mmap.len();
        let static_slice: &'static mut [u8] = unsafe { std::slice::from_raw_parts_mut(ptr, len) };

        let cairo_surface = ImageSurface::create_for_data(
            static_slice,
            CairoFormat::ARgb32,
            width,
            height,
            stride,
        ).ok();

        self.cairo_surface = cairo_surface;
        self.shm_pool = Some(ShmPool { buffer, _mmap: mmap });
    }

    fn render(&mut self, qh: &QueueHandle<Self>) {
        if !self.first_configure_done { return; }
        if self.frame_callback.is_some() { return; }

        if let Ok(mut guard) = self.found_paths.lock() {
            while let Some((idx, path)) = guard.pop() {
                if idx < self.entries.len() {
                    self.entries[idx].icon_path = Some(path);
                }
            }
        }

        let any_missing = self.entries.iter().any(|e|
            e.icon_key.is_some() && e.icon_path.is_none() && e.icon_surface.is_none()
        );

        let cairo_surface = match self.cairo_surface.as_ref() { Some(s) => s, _ => return };
        let shm_pool = match self.shm_pool.as_ref() { Some(p) => p, _ => return };
        let surface = match self.surface.as_ref() { Some(s) => s, _ => return };
        let w = self.width;
        let h = self.height;

        let cr = match Context::new(cairo_surface) { Ok(c) => c, Err(_) => return };

        cr.set_operator(cairo::Operator::Clear);
        cr.paint().ok();
        cr.set_operator(cairo::Operator::Over);

        draw_rounded_rect(&cr, 0.0, 0.0, w as f64, h as f64, 8.0);
        cr.set_source_rgba(0.04, 0.03, 0.10, 0.96);
        cr.fill().ok();

        cr.set_source_rgba(0.0, 0.90, 1.0, 0.25);
        cr.set_line_width(1.0);
        draw_rounded_rect(&cr, 0.0, 0.0, w as f64, h as f64, 8.0);
        cr.stroke().ok();

        let sbx = PAD as f64;
        let sbw = (w - PAD * 2) as f64;
        draw_rounded_rect(&cr, sbx, 8.0, sbw, SEARCH_H as f64, 6.0);
        cr.set_source_rgba(0.08, 0.06, 0.18, 0.9);
        cr.fill().ok();
        cr.set_source_rgba(0.0, 0.90, 1.0, 0.4);
        cr.set_line_width(1.0);
        draw_rounded_rect(&cr, sbx, 8.0, sbw, SEARCH_H as f64, 6.0);
        cr.stroke().ok();

        let disp = if self.search.is_empty() { "  Type to search...".to_string() } else { format!("  {}_", self.search) };

        let fd_s = pango::FontDescription::from_string("Sans 12");
        let pango_ctx = pangocairo::functions::create_context(&cr);
        let lay_s = pango::Layout::new(&pango_ctx);
        lay_s.set_font_description(Some(&fd_s));
        lay_s.set_text(&disp);
        cr.move_to(sbx + 4.0, 8.0 + (SEARCH_H as f64 - 18.0) / 2.0);
        cr.set_source_rgba(0.0, 0.9, 1.0, 1.0);
        pangocairo::functions::show_layout(&cr, &lay_s);

        let start_y = 8.0 + (SEARCH_H as f64) + (PAD as f64);
        let max_visible = (((h as f64) - start_y) / ROW_HEIGHT as f64) as usize;

        for idx in 0..max_visible {
            let filtered_idx = idx + self.scroll_offset;
            if filtered_idx >= self.filtered.len() { break; }
            let entry_idx = self.filtered[filtered_idx];
            
            if self.entries[entry_idx].icon_surface.is_none() {
                if let Some(ref path) = self.entries[entry_idx].icon_path.clone() {
                    let is_svg = path.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.eq_ignore_ascii_case("svg"))
                        .unwrap_or(false);
                    if is_svg {
                        if let Ok(handle) = Loader::new().read_path(path) {
                            if let Ok(surface) = ImageSurface::create(CairoFormat::ARgb32, ICON_SIZE, ICON_SIZE) {
                                if let Ok(cr) = Context::new(&surface) {
                                    let renderer = CairoRenderer::new(&handle);
                                    let _ = renderer.render_document(
                                        &cr,
                                        &cairo::Rectangle::new(0.0, 0.0, ICON_SIZE as f64, ICON_SIZE as f64),
                                    );
                                    self.entries[entry_idx].icon_surface = Some(surface);
                                }
                            }
                        }
                    } else if let Ok(file) = fs::File::open(path) {
                        if let Ok(surface) = ImageSurface::create_from_png(&mut std::io::BufReader::new(file)) {
                            self.entries[entry_idx].icon_surface = Some(surface);
                        }
                    }
                }
            }

            let entry = &self.entries[entry_idx];
            let row_y = start_y + (idx as f64 * ROW_HEIGHT as f64);

            if self.hovered_idx == Some(filtered_idx) {
                draw_rounded_rect(&cr, sbx, row_y, sbw, ROW_HEIGHT as f64, 4.0);
                cr.set_source_rgba(0.0, 0.9, 1.0, 0.15);
                cr.fill().ok();
            }

            let mut text_offset = 12.0;
            if let Some(ref icon) = entry.icon_surface {
                cr.save().ok();
                let icon_x = sbx + PAD as f64 + 6.0;
                let icon_y = row_y + ((ROW_HEIGHT - ICON_SIZE) / 2) as f64;
                
                cr.set_source_surface(icon, icon_x, icon_y).ok();
                cr.rectangle(icon_x, icon_y, ICON_SIZE as f64, ICON_SIZE as f64);
                cr.fill().ok();
                cr.restore().ok();
                
                text_offset += (ICON_SIZE + 10) as f64;
            }

            let text_layout = pango::Layout::new(&pango_ctx);
            text_layout.set_font_description(Some(&fd_s));
            text_layout.set_text(&entry.name);
            cr.move_to(sbx + PAD as f64 + text_offset, row_y + ((ROW_HEIGHT - 18) / 2) as f64);
            cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
            pangocairo::functions::show_layout(&cr, &text_layout);
        }

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
    fn event(state: &mut Self, registry: &WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind::<WlCompositor, _, _>(name, version, qh, ())),
                "wl_shm" => state.shm = Some(registry.bind::<WlShm, _, _>(name, version, qh, ())),
                "zwlr_layer_shell_v1" => state.layer_shell = Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, version, qh, ())),
                "wl_seat" => state.seat = Some(registry.bind::<WlSeat, _, _>(name, version, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<WlCompositor, ()> for WaylandState { fn event(_: &mut Self, _: &WlCompositor, _: wayland_client::protocol::wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<WlShm, ()> for WaylandState { fn event(_: &mut Self, _: &WlShm, _: wayland_client::protocol::wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<WlShmPool, ()> for WaylandState { fn event(_: &mut Self, _: &WlShmPool, _: wayland_client::protocol::wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<WlBuffer, ()> for WaylandState { fn event(_: &mut Self, _: &WlBuffer, _: wayland_client::protocol::wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<WlSurface, ()> for WaylandState { fn event(_: &mut Self, _: &WlSurface, _: wayland_client::protocol::wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<WlCallback, ()> for WaylandState {
    fn event(state: &mut Self, _: &WlCallback, _: wayland_client::protocol::wl_callback::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        state.frame_callback = None;
        let has_pending = state.found_paths.lock().map(|g| !g.is_empty()).unwrap_or(false);
        if state.needs_render || has_pending {
            state.needs_render = false;
            state.render(qh);
        }
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for WaylandState { fn event(_: &mut Self, _: &ZwlrLayerShellV1, _: wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {} }
impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(state: &mut Self, surface: &ZwlrLayerSurfaceV1, event: wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                surface.ack_configure(serial);
                state.width = if width > 0 { width as i32 } else { 600 };
                state.height = if height > 0 { height as i32 } else { 400 };
                state.create_shm_pool(qh);
                state.first_configure_done = true;
                state.render(qh);
            },
            zwlr_layer_surface_v1::Event::Closed => { state.running = false; },
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
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
    fn event(state: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Motion { surface_y, .. } => {
                let start_y = 8.0 + (SEARCH_H as f64) + (PAD as f64);
                if surface_y >= start_y {
                    let click_idx = ((surface_y - start_y) / ROW_HEIGHT as f64) as usize + state.scroll_offset;
                    if click_idx < state.filtered.len() {
                        state.hovered_idx = Some(click_idx);
                        state.render(qh);
                    }
                }
            },
            wl_pointer::Event::Button { state: WEnum::Value(ButtonState::Pressed), .. } => {
                state.launch_selected();
            },
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for WaylandState {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Keymap { format: WEnum::Value(KeymapFormat::XkbV1), fd, size } => {
                let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                if let Ok(Some(device_keymap)) = unsafe {
                    xkb::Keymap::new_from_fd(
                        &context, fd, size as usize,
                        xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS
                    )
                } {
                    state.xkb_state = Some(xkb::State::new(&device_keymap));
                }
                state._xkb_ctx = Some(context);
            },
            wl_keyboard::Event::Key { key, state: WEnum::Value(KeyState::Pressed), .. } => {
                if let Some(xkb_state) = &state.xkb_state {
                    let sym = xkb_state.key_get_one_sym((key + 8).into());
                    
                    if sym == xkeysym::Keysym::from(key::Escape) {
                        state.running = false;
                    } else if sym == xkeysym::Keysym::from(key::Return) {
                        state.launch_selected();
                    } else if sym == xkeysym::Keysym::from(key::BackSpace) {
                        state.search.pop();
                        state.update_filter();
                        state.render(qh);
                    } else if sym == xkeysym::Keysym::from(key::Up) {
                        if let Some(curr) = state.hovered_idx {
                            if curr > 0 {
                                state.hovered_idx = Some(curr - 1);
                                if curr - 1 < state.scroll_offset { state.scroll_offset = curr - 1; }
                                state.render(qh);
                            }
                        }
                    } else if sym == xkeysym::Keysym::from(key::Down) {
                        if let Some(curr) = state.hovered_idx {
                            if curr + 1 < state.filtered.len() {
                                state.hovered_idx = Some(curr + 1);
                                let max_visible = ((state.height as f64 - (8.0 + SEARCH_H as f64 + PAD as f64)) / ROW_HEIGHT as f64) as usize;
                                if curr + 1 >= state.scroll_offset + max_visible { state.scroll_offset += 1; }
                                state.render(qh);
                            }
                        }
                    } else {
                        let txt = xkb_state.key_get_utf8((key + 8).into());
                        if !txt.is_empty() && txt.chars().all(|c| !c.is_control()) {
                            state.search.push_str(&txt);
                            state.update_filter();
                            state.render(qh);
                        }
                    }
                }
            },
            _ => {}
        }
    }
}

fn main() {
    let conn = Connection::connect_to_env().expect("Failed to link into Wayland backend");
    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = WaylandState::new();
    let _registry = conn.display().get_registry(&qh, ());

    event_queue.roundtrip(&mut state).unwrap();
    event_queue.roundtrip(&mut state).unwrap();

    let compositor = state.compositor.as_ref().expect("wl_compositor missing");
    let layer_shell = state.layer_shell.as_ref().expect("zwlr_layer_shell_v1 missing");

    let wl_surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(&wl_surface, None, Layer::Overlay, "launcher".to_string(), &qh, ());

    layer_surface.set_size(600, 400);
    layer_surface.set_anchor(Anchor::all());
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);

    wl_surface.commit();

    state.surface = Some(wl_surface);
    state.layer_surface = Some(layer_surface);

    while state.running {
        event_queue.blocking_dispatch(&mut state).unwrap();
    }
}

