use std::ffi::{CStr, CString};
use std::fs;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::ptr;

use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_buffer::WlBuffer,
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard, KeymapFormat, KeyState},
        wl_pointer::{self, WlPointer, ButtonState},
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat, Capability},
        wl_shm::{WlShm, Format},
        wl_shm_pool::WlShmPool,
        wl_surface::{self, WlSurface},
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{ZwlrLayerShellV1, Layer},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1, Anchor, KeyboardInteractivity},
};

use xkbcommon::xkb;
use xkeysym::Keysym;

use cairo::{Context, Format as CairoFormat, ImageSurface};
use memmap2::MmapMut;

const MAX_ENTRIES: usize = 512;
const ICON_SIZE: i32 = 28;
const ROW_HEIGHT: i32 = 40;
const SEARCH_H: i32 = 36;
const PAD: i32 = 10;

mod rsvg_ffi {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_double, c_int};
    use std::path::Path;
    use std::ptr;

    #[repr(C)]
    struct RsvgRectangle {
        x: c_double,
        y: c_double,
        w: c_double,
        h: c_double,
    }

    #[link(name = "rsvg-2")]
    extern "C" {
        fn rsvg_handle_new_from_file(
            path: *const c_char,
            err: *mut *mut libc::c_void,
        ) -> *mut libc::c_void;
        fn rsvg_handle_render_document(
            handle: *mut libc::c_void,
            cr: *mut libc::c_void,
            viewport: *const RsvgRectangle,
            err: *mut *mut libc::c_void,
        ) -> c_int;
        fn g_object_unref(object: *mut libc::c_void);
    }

    pub fn load_svg(path: &Path, size: i32) -> Option<crate::ImageSurface> {
        let cpath = CString::new(path.to_str()?).ok()?;
        unsafe {
            let handle = rsvg_handle_new_from_file(cpath.as_ptr(), ptr::null_mut());
            if handle.is_null() {
                return None;
            }

            let surface =
                crate::ImageSurface::create(crate::CairoFormat::ARgb32, size, size).ok()?;
            let cr = crate::Context::new(&surface).ok()?;
            let raw_cr = cr.to_raw_none().cast::<libc::c_void>();

            let viewport = RsvgRectangle {
                x: 0.0,
                y: 0.0,
                w: size as f64,
                h: size as f64,
            };

            let ok = rsvg_handle_render_document(handle, raw_cr, &viewport, ptr::null_mut());
            g_object_unref(handle);
            drop(cr);
            if ok == 0 {
                return None;
            }
            Some(surface)
        }
    }
}

struct Entry {
    name: String,
    exec: String,
    icon: Option<ImageSurface>,
}

struct ShmPool {
    buffer: WlBuffer,
    _mmap: MmapMut,
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
    configured: bool,

    shm_pool: Option<ShmPool>,

    xkb_ctx: Option<xkb::Context>,
    xkb_state: Option<xkb::State>,

    entries: Vec<Entry>,
    filtered: Vec<usize>,
    scroll_offset: usize,
    search: String,
    hovered_idx: Option<usize>,

    cairo_surface: Option<ImageSurface>,
    cr: Option<Context>,
}

impl WaylandState {
    fn new() -> Self {
        WaylandState {
            running: true,
            width: 600,
            height: 400,
            compositor: None,
            shm: None,
            layer_shell: None,
            seat: None,
            pointer: None,
            keyboard: None,
            surface: None,
            layer_surface: None,
            configured: false,
            shm_pool: None,
            xkb_ctx: None,
            xkb_state: None,
            entries: Vec::new(),
            filtered: Vec::new(),
            scroll_offset: 0,
            search: String::new(),
            hovered_idx: None,
            cairo_surface: None,
            cr: None,
        }
    }

    fn update_filter(&mut self) {
        self.filtered.clear();
        self.scroll_offset = 0;
        let search_lower = self.search.to_lowercase();
        for (i, entry) in self.entries.iter().enumerate() {
            if self.search.is_empty()
                || entry.name.to_lowercase().contains(&search_lower)
            {
                self.filtered.push(i);
                if self.filtered.len() >= MAX_ENTRIES {
                    break;
                }
            }
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

        let mmap = match unsafe { MmapMut::map_mut(&fd) } {
            Ok(m) => m,
            Err(_) => return,
        };
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, Format::Argb8888, qh, ());
        drop(pool);

        let cairo_surface = match ImageSurface::create(CairoFormat::ARgb32, width, height) {
            Ok(s) => s,
            Err(_) => return,
        };
        let cr = match Context::new(&cairo_surface) {
            Ok(c) => c,
            Err(_) => return,
        };

        self.cairo_surface = Some(cairo_surface);
        self.cr = Some(cr);
        self.shm_pool = Some(ShmPool {
            buffer,
            _mmap: mmap,
        });
    }

    fn render(&mut self) {
        let cr = match self.cr.as_mut() {
            Some(c) => c,
            _ => return,
        };
        let cairo_surface = match self.cairo_surface.as_mut() {
            Some(s) => s,
            _ => return,
        };
        let shm_pool = match self.shm_pool.as_mut() {
            Some(p) => p,
            _ => return,
        };
        let surface = match self.surface.as_ref() {
            Some(s) => s,
            _ => return,
        };
        let w = self.width;
        let h = self.height;

        cr.set_operator(cairo::Operator::Clear);
        cr.paint().ok();
        cr.set_operator(cairo::Operator::Over);

        draw_rounded_rect(cr, 0.0, 0.0, w as f64, h as f64, 8.0);
        cr.set_source_rgba(0.04, 0.03, 0.10, 0.96);
        cr.fill().ok();

        cr.set_source_rgba(0.0, 0.90, 1.0, 0.25);
        cr.set_line_width(1.0);
        draw_rounded_rect(cr, 0.0, 0.0, w as f64, h as f64, 8.0);
        cr.stroke().ok();

        let sbx = PAD as f64;
        let sbw = (w - PAD * 2) as f64;
        draw_rounded_rect(cr, sbx, 8.0, sbw, SEARCH_H as f64, 6.0);
        cr.set_source_rgba(0.08, 0.06, 0.18, 0.9);
        cr.fill().ok();
        cr.set_source_rgba(0.0, 0.90, 1.0, 0.4);
        cr.set_line_width(1.0);
        draw_rounded_rect(cr, sbx, 8.0, sbw, SEARCH_H as f64, 6.0);
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
        let (_stw, sth) = lay_s.pixel_size();
        cr.set_source_rgb(0.7, 0.75, 1.0);
        cr.move_to(sbx + 4.0, 8.0 + (SEARCH_H - sth) as f64 / 2.0);
        pangocairo::functions::show_layout(cr, &lay_s);
        drop(lay_s);

        let n_results_total = self.filtered.len();
        let max_visible = ((h - SEARCH_H - 8 - 4) / ROW_HEIGHT) as usize;
        let n_visible = n_results_total.min(max_visible);
        let skip = self.scroll_offset;

        let fd_n = pango::FontDescription::from_string("Sans 12");
        let fd_sub = pango::FontDescription::from_string("Monospace 9");
        let ctx_n = pangocairo::functions::create_context(cr);

        for j in 0..n_visible {
            let idx = skip + j;
            if idx >= self.filtered.len() {
                break;
            }
            let i = self.filtered[idx];
            let ry = (SEARCH_H + 8 + 4 + j as i32 * ROW_HEIGHT) as f64;
            let hovered = self.hovered_idx.map_or(false, |h| h == i);

            if hovered {
                cr.set_source_rgba(0.0, 0.90, 1.0, 0.12);
                draw_rounded_rect(cr, 2.0, ry, (w - 4) as f64, (ROW_HEIGHT - 2) as f64, 6.0);
                cr.fill().ok();
            }

            if let Some(ref icon) = self.entries[i].icon {
                let iw = icon.width() as f64;
                let ih = icon.height() as f64;
                let scale = if iw > ih {
                    ICON_SIZE as f64 / iw
                } else {
                    ICON_SIZE as f64 / ih
                };
                let dx = (PAD + 2) as f64;
                let dy = ry + (ROW_HEIGHT as f64 - ih * scale) / 2.0;
                let _ = cr.save();
                cr.translate(dx, dy);
                cr.scale(scale, scale);
                let _ = cr.set_source_surface(icon, 0.0, 0.0);
                cr.paint().ok();
                let _ = cr.restore();
            } else {
                cr.set_source_rgba(0.0, 0.45, 0.5, 0.5);
                let cx = (PAD + 2 + ICON_SIZE / 2) as f64;
                let cy = ry + ROW_HEIGHT as f64 / 2.0;
                cr.arc(
                    cx,
                    cy,
                    (ICON_SIZE / 2 - 2) as f64,
                    0.0,
                    2.0 * std::f64::consts::PI,
                );
                cr.fill().ok();
            }

            let tx = (PAD + 2 + ICON_SIZE + 10) as f64;
            let lay_n = pango::Layout::new(&ctx_n);
            lay_n.set_font_description(Some(&fd_n));
            lay_n.set_text(&self.entries[i].name);
            let (_, nh) = lay_n.pixel_size();
            cr.set_source_rgb(0.9, 0.92, 1.0);
            cr.move_to(tx, ry + (ROW_HEIGHT as f64 - nh as f64) / 2.0);
            pangocairo::functions::show_layout(cr, &lay_n);
            drop(lay_n);

            if !self.entries[i].exec.is_empty() {
                let ex_w = (w as f64 - tx - PAD as f64) as i32;
                let lay_sub = pango::Layout::new(&ctx_n);
                lay_sub.set_font_description(Some(&fd_sub));
                lay_sub.set_text(&self.entries[i].exec);
                lay_sub.set_width(ex_w * pango::SCALE);
                lay_sub.set_ellipsize(pango::EllipsizeMode::Middle);
                lay_sub.set_alignment(pango::Alignment::Right);
                let (_, sh) = lay_sub.pixel_size();
                cr.set_source_rgba(0.5, 0.55, 0.7, 0.6);
                cr.move_to(tx, ry + (ROW_HEIGHT as f64 - sh as f64) / 2.0);
                pangocairo::functions::show_layout(cr, &lay_sub);
                drop(lay_sub);

                cr.set_source_rgba(0.0, 0.90, 1.0, 0.1);
                cr.set_line_width(0.5);
                cr.move_to(PAD as f64, ry + ROW_HEIGHT as f64 - 1.0);
                cr.line_to((w - PAD) as f64, ry + ROW_HEIGHT as f64 - 1.0);
                cr.stroke().ok();
            }
        }

        cairo_surface.flush();
        if let Ok(data) = cairo_surface.data() {
            let mmap = &mut shm_pool._mmap;
            let len = data.len().min(mmap.len());
            mmap[..len].copy_from_slice(&data[..len]);
        }

        surface.attach(Some(&shm_pool.buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
    }

    fn execute_command(cmd: &str) {
        std::process::Command::new("sh")
            .args(["-c", cmd])
            .spawn()
            .ok();
    }
}

fn draw_rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(h / 2.0).min(w / 2.0);
    cr.move_to(x + r, y);
    cr.line_to(x + w - r, y);
    cr.arc(x + w - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.line_to(x + w, y + h - r);
    cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    cr.line_to(x + r, y + h);
    cr.arc(x + r, y + h - r, r, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
    cr.line_to(x, y + r);
    cr.arc(x + r, y + r, r, std::f64::consts::PI, 3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();
}

fn read_desktop_field(content: &str, field: &str) -> Option<String> {
    let pat = format!("\n{}=", field);
    let start = content.find(&pat)?;
    let val_start = start + pat.len();
    let val = content[val_start..]
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if val.is_empty() { None } else { Some(val) }
}

fn clean_exec(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' && chars.peek().is_some() {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

fn load_png(base: &str, name: &str) -> Option<ImageSurface> {
    let path = format!("{}/{}.png", base, name);
    let mut file = fs::File::open(&path).ok()?;
    ImageSurface::create_from_png(&mut file).ok()
}

fn load_svg(base: &str, name: &str, size: i32) -> Option<ImageSurface> {
    let path = format!("{}/{}.svg", base, name);
    rsvg_ffi::load_svg(Path::new(&path), size)
}

const ICON_THEMES: &[&str] = &[
    "hicolor",
    "Papirus",
    "Papirus-Dark",
    "Papirus-Light",
    "Adwaita",
    "gnome",
    "breeze",
    "breeze-dark",
    "Numix",
    "elementary-xfce",
    "Moka",
    "Faenza",
    "Humanity",
    "ubuntu-mono",
];

fn load_icon(name: &str) -> Option<ImageSurface> {
    if name.is_empty() {
        return None;
    }
    let sizes = [48, 64, 32, 128, 96, 72];
    for theme in ICON_THEMES {
        for size in &sizes {
            let base = format!("/usr/share/icons/{theme}/{size}x{size}/apps");
            if Path::new(&format!("{base}/{}.png", name)).exists() {
                if let Some(img) = load_png(&base, name) {
                    return Some(img);
                }
            }
        }
        let base = format!("/usr/share/icons/{theme}/scalable/apps");
        if let Some(img) = load_svg(&base, name, 48) {
            return Some(img);
        }
    }
    load_png("/usr/share/pixmaps", name)
}

fn populate_entries() -> Vec<Entry> {
    let mut entries = Vec::new();
    let dir = match fs::read_dir("/usr/share/applications") {
        Ok(d) => d,
        Err(_) => return entries,
    };

    for entry in dir {
        let entry = match entry {
            Ok(e) => e,
            _ => continue,
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().map_or(true, |e| e != "desktop") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => format!("\n{}", c),
            Err(_) => continue,
        };

        let hidden = read_desktop_field(&content, "NoDisplay")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
            || read_desktop_field(&content, "Hidden")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
        if hidden {
            continue;
        }

        let type_field = read_desktop_field(&content, "Type");
        if type_field.as_deref() != Some("Application") {
            continue;
        }

        let name = read_desktop_field(&content, "Name");
        let exec = read_desktop_field(&content, "Exec");
        let icon = read_desktop_field(&content, "Icon");

        if let (Some(name), Some(exec)) = (name, exec) {
            if name.is_empty() || exec.is_empty() {
                continue;
            }
            let icon_name = icon.unwrap_or_default();
            let icon_surface = load_icon(&icon_name);
            entries.push(Entry {
                name,
                exec: clean_exec(&exec),
                icon: icon_surface,
            });
            if entries.len() >= MAX_ENTRIES {
                break;
            }
        }
    }
    entries
}

impl Dispatch<WlRegistry, ()> for WaylandState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: <WlRegistry as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
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
                        Some(registry.bind::<WlCompositor, _, _>(name, version.min(4), qh, ()));
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell = Some(registry.bind::<ZwlrLayerShellV1, _, _>(
                        name,
                        version.min(4),
                        qh,
                        (),
                    ));
                }
                "wl_seat" => {
                    state.seat =
                        Some(registry.bind::<WlSeat, _, _>(name, version.min(7), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlCompositor, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlCompositor,
        _event: <WlCompositor as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShm, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShm,
        _event: <WlShm as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlShmPool,
        _event: <WlShmPool as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &WlBuffer,
        _event: <WlBuffer as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_surface::WlSurface,
        _event: <wl_surface::WlSurface as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerShellV1, ()> for WaylandState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwlrLayerShellV1,
        _event: <ZwlrLayerShellV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &ZwlrLayerSurfaceV1,
        event: <ZwlrLayerSurfaceV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial, width, height, ..
            } => {
                let ls = state.layer_surface.as_ref().unwrap().clone();
                ls.ack_configure(serial);
                if width > 0 && height > 0 {
                    state.width = width as i32;
                    state.height = height as i32;
                }
                if state.shm_pool.is_none() {
                    state.create_shm_pool(qh);
                    state.render();
                }
                state.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.running = false;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlSeat,
        event: <WlSeat as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities, .. } = event {
            match capabilities {
                wayland_client::WEnum::Value(caps) => {
                    if caps.contains(Capability::Pointer) && state.pointer.is_none() {
                        if let Some(ref seat) = state.seat {
                            state.pointer = Some(seat.get_pointer(qh, ()));
                        }
                    } else if !caps.contains(Capability::Pointer) {
                        state.pointer.take();
                    }

                    if caps.contains(Capability::Keyboard) && state.keyboard.is_none() {
                        if let Some(ref seat) = state.seat {
                            state.keyboard = Some(seat.get_keyboard(qh, ()));
                        }
                    } else if !caps.contains(Capability::Keyboard) {
                        state.keyboard.take();
                    }
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlKeyboard,
        event: <WlKeyboard as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size, .. } => {
                match format {
                    wayland_client::WEnum::Value(KeymapFormat::XkbV1) => {
                        let ptr = unsafe {
                            libc::mmap(
                                ptr::null_mut(),
                                size as usize,
                                libc::PROT_READ,
                                libc::MAP_PRIVATE,
                                fd.as_raw_fd(),
                                0,
                            )
                        };
                        if ptr == libc::MAP_FAILED {
                            return;
                        }
                        let map_str = unsafe {
                            CStr::from_ptr(ptr as *const libc::c_char)
                                .to_string_lossy()
                                .to_string()
                        };
                        unsafe { libc::munmap(ptr, size as usize) };

                        if let Some(ref ctx) = state.xkb_ctx {
                            let keymap = xkb::Keymap::new_from_string(
                                ctx,
                                map_str,
                                xkb::KEYMAP_FORMAT_TEXT_V1,
                                0,
                            );
                            if let Some(keymap) = keymap {
                                state.xkb_state = Some(xkb::State::new(&keymap));
                            }
                        }
                    }
                    _ => {}
                }
            }
            wl_keyboard::Event::Key {
                key, state: kstate, ..
            } => {
                let xkb_state = match &mut state.xkb_state {
                    Some(s) => s,
                    _ => return,
                };
                let keycode = xkb::Keycode::new(key);

                let down = matches!(kstate, wayland_client::WEnum::Value(KeyState::Pressed));
                xkb_state.update_key(
                    keycode,
                    if down {
                        xkb::KeyDirection::Down
                    } else {
                        xkb::KeyDirection::Up
                    },
                );

                if !down {
                    return;
                }

                let sym = xkb_state.key_get_one_sym(keycode);

                if sym == Keysym::new(0xFF1B) {
                    state.running = false;
                    return;
                }
                if sym == Keysym::new(0xFF0D) || sym == Keysym::new(0xFF8D) {
                    let idx = state.hovered_idx.or_else(|| state.filtered.first().copied());
                    if let Some(i) = idx {
                        Self::execute_command(&state.entries[i].exec);
                        state.running = false;
                    }
                    return;
                }
                if sym == Keysym::new(0xFF08) {
                    if !state.search.is_empty() {
                        state.search.pop();
                    }
                    state.update_filter();
                    state.render();
                    return;
                }
                if sym == Keysym::new(0xFF52) {
                    if state.filtered.is_empty() {
                        return;
                    }
                    let cur = state.hovered_idx.and_then(|h| {
                        state.filtered.iter().position(|&x| x == h)
                    });
                    match cur {
                        Some(c) if c > 0 => {
                            state.hovered_idx = Some(state.filtered[c - 1]);
                        }
                        _ if state.scroll_offset > 0 => {
                            state.scroll_offset -= 1;
                            state.hovered_idx = Some(state.filtered[state.scroll_offset]);
                        }
                        _ => {}
                    }
                    state.render();
                    return;
                }
                if sym == Keysym::new(0xFF54) {
                    if state.filtered.is_empty() {
                        return;
                    }
                    let cur = state.hovered_idx.and_then(|h| {
                        state.filtered.iter().position(|&x| x == h)
                    });
                    let max_visible = ((state.height - SEARCH_H - 8 - 4) / ROW_HEIGHT) as usize;
                    match cur {
                        Some(c) if c < state.filtered.len() - 1 => {
                            let new_idx = c + 1;
                            state.hovered_idx = Some(state.filtered[new_idx]);
                            if new_idx - state.scroll_offset >= max_visible {
                                state.scroll_offset = new_idx - max_visible + 1;
                            }
                        }
                        _ if state.scroll_offset + max_visible < state.filtered.len() => {
                            state.scroll_offset += 1;
                        }
                        _ => {}
                    }
                    state.render();
                    return;
                }

                let utf8 = xkb_state.key_get_utf8(keycode);
                if !utf8.is_empty() {
                    if let Some(c) = utf8.chars().next() {
                        if !c.is_control() {
                            state.search.push(c);
                            state.update_filter();
                            state.hovered_idx = state.filtered.first().copied();
                            state.render();
                        }
                    }
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(ref mut s) = state.xkb_state {
                    s.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _proxy: &WlPointer,
        event: <WlPointer as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { .. } => {
                state.hovered_idx = None;
            }
            wl_pointer::Event::Leave { .. } => {
                if state.hovered_idx.take().is_some() {
                    state.render();
                }
            }
            wl_pointer::Event::Motion {
                surface_y, ..
            } => {
                let old = state.hovered_idx;
                state.hovered_idx = None;
                let skip = state.scroll_offset;
                let max_visible = ((state.height - SEARCH_H - 8 - 4) / ROW_HEIGHT) as usize;
                let y = surface_y as i32;
                for j in 0..max_visible {
                    let idx = skip + j;
                    if idx >= state.filtered.len() {
                        break;
                    }
                    let ry = SEARCH_H + 8 + 4 + j as i32 * ROW_HEIGHT;
                    if y >= ry && y < ry + ROW_HEIGHT {
                        state.hovered_idx = Some(state.filtered[idx]);
                        break;
                    }
                }
                if old != state.hovered_idx {
                    state.render();
                }
            }
            wl_pointer::Event::Button {
                button,
                state: bstate,
                ..
            } => {
                if !matches!(bstate, wayland_client::WEnum::Value(ButtonState::Pressed))
                    || button != 0x110
                {
                    return;
                }
                if let Some(i) = state.hovered_idx {
                    Self::execute_command(&state.entries[i].exec);
                    state.running = false;
                }
            }
            wl_pointer::Event::Axis { value, .. } => {
                let delta = value as i32;
                if delta == 0 {
                    return;
                }
                let max_visible = ((state.height - SEARCH_H - 8 - 4) / ROW_HEIGHT) as usize;
                let max_scroll = state.filtered.len().saturating_sub(max_visible);
                let old = state.scroll_offset;
                let new = (state.scroll_offset as i32 - delta)
                    .max(0)
                    .min(max_scroll as i32) as usize;
                state.scroll_offset = new;
                if old != state.scroll_offset {
                    state.hovered_idx = None;
                    state.render();
                }
            }
            wl_pointer::Event::Frame => {}
            _ => {}
        }
    }
}

fn main() {
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };

    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to connect to wayland display: {:?}", e);
            std::process::exit(1);
        }
    };

    let mut event_queue = conn.new_event_queue();
    let qh = event_queue.handle();

    let mut state = WaylandState::new();

    let _registry = conn.display().get_registry(&qh, ());
    if event_queue.roundtrip(&mut state).is_err() {
        eprintln!("failed to get globals");
        std::process::exit(1);
    }

    if state.compositor.is_none() || state.shm.is_none() || state.layer_shell.is_none() {
        eprintln!("missing required wayland globals");
        std::process::exit(1);
    }

    state.xkb_ctx = Some(xkb::Context::new(xkb::CONTEXT_NO_FLAGS));

    state.entries = populate_entries();
    state.update_filter();

    let compositor = state.compositor.as_ref().unwrap();
    let wl_surface = compositor.create_surface(&qh, ());
    state.surface = Some(wl_surface);

    let layer_shell = state.layer_shell.as_ref().unwrap();
    let layer_surface = layer_shell.get_layer_surface(
        state.surface.as_ref().unwrap(),
        None,
        Layer::Overlay,
        "launcher".to_string(),
        &qh,
        (),
    );
    state.layer_surface = Some(layer_surface);

    let n_results = state.filtered.len();
    let max_visible = 10usize.min(n_results);
    let list_h = max_visible * ROW_HEIGHT as usize;
    let total_h = (SEARCH_H + 8 + 4 + list_h as i32 + 8).min(600).max(100);

    if let Some(ref ls) = state.layer_surface {
        ls.set_size(600, total_h as u32);
        ls.set_anchor(Anchor::Left | Anchor::Right | Anchor::Top | Anchor::Bottom);
        ls.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        ls.set_exclusive_zone(0);
    }

    if let Some(ref s) = state.surface {
        s.commit();
    }
    if event_queue.roundtrip(&mut state).is_err() {
        eprintln!("surface was not configured");
        std::process::exit(1);
    }

    if !state.configured {
        eprintln!("surface was not configured");
        std::process::exit(1);
    }

    if !state.filtered.is_empty() {
        state.hovered_idx = Some(state.filtered[0]);
    }
    state.render();

    while state.running {
        if event_queue.blocking_dispatch(&mut state).is_err() {
            break;
        }
    }
}
