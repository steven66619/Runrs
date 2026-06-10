use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize, Debug, Clone)]
pub struct LauncherTheme {
    pub bg_color: String,
    pub text_color: String,
    pub accent_color: String,
    pub border_radius: u32,
    pub border_width: u32,
}

impl Default for LauncherTheme {
    fn default() -> Self {
        LauncherTheme {
            bg_color: "#0b081a".to_string(),
            text_color: "#ffffff".to_string(),
            accent_color: "#00e5ff".to_string(),
            border_radius: 8,
            border_width: 1,
        }
    }
}

fn hex_to_rgba(hex: &str) -> (f64, f64, f64, f64) {
    let hex = hex.trim_start_matches('#');
    let (r, g, b, a) = match hex.len() {
        8 => (
            u8::from_str_radix(&hex[0..2], 16).unwrap_or(0),
            u8::from_str_radix(&hex[2..4], 16).unwrap_or(0),
            u8::from_str_radix(&hex[4..6], 16).unwrap_or(0),
            u8::from_str_radix(&hex[6..8], 16).unwrap_or(255),
        ),
        6 => (
            u8::from_str_radix(&hex[0..2], 16).unwrap_or(0),
            u8::from_str_radix(&hex[2..4], 16).unwrap_or(0),
            u8::from_str_radix(&hex[4..6], 16).unwrap_or(0),
            255,
        ),
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).unwrap_or(0) * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).unwrap_or(0) * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).unwrap_or(0) * 17;
            (r, g, b, 255)
        }
        _ => (0, 0, 0, 255),
    };
    (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, a as f64 / 255.0)
}

impl LauncherTheme {
    pub fn bg_rgba(&self) -> (f64, f64, f64, f64) {
        hex_to_rgba(&self.bg_color)
    }

    pub fn text_rgba(&self) -> (f64, f64, f64, f64) {
        hex_to_rgba(&self.text_color)
    }

    pub fn accent_rgba(&self) -> (f64, f64, f64, f64) {
        hex_to_rgba(&self.accent_color)
    }
}

pub fn get_theme_file_name() -> String {
    let de = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_default()
        .to_lowercase();

    let wms = ["hyprland", "i3", "sway", "river", "dwl", "qtile", "bspwm", "awesome", "xfce", "kde", "gnome"];
    for wm in &wms {
        if de.contains(wm) {
            return format!("{}.conf", wm);
        }
    }

    if std::env::var("WAYLAND_DISPLAY").ok().map_or(false, |v| !v.is_empty()) {
        "wayland.conf".to_string()
    } else {
        "x11.conf".to_string()
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".config")
    }).join("Runrs")
}

fn try_load(path: &PathBuf) -> Option<LauncherTheme> {
    let content = fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

pub fn load_theme() -> LauncherTheme {
    let base = config_dir();
    if !base.exists() {
        return LauncherTheme::default();
    }

    let specific = base.join(get_theme_file_name());
    if specific.exists() {
        if let Some(theme) = try_load(&specific) {
            return theme;
        }
    }

    let fallbacks = [base.join("config.toml"), base.join("theme.toml")];
    for fb in &fallbacks {
        if fb.exists() {
            if let Some(theme) = try_load(fb) {
                return theme;
            }
        }
    }

    LauncherTheme::default()
}
