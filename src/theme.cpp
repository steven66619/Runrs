#include "theme.h"
#include "config.h"
#include "toml.h"
#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <sstream>

namespace runrs {
namespace fs = std::filesystem;

Rgba hex_to_rgba(const std::string &hex) {
  Rgba rgba{};
  auto h = hex;
  if (h.empty()) return rgba;
  if (h.front() == '#') h = h.substr(1);

  auto parse = [](const std::string &s, size_t start, size_t len) -> uint8_t {
    try { return static_cast<uint8_t>(std::stoi(s.substr(start, len), nullptr, 16)); }
    catch (...) { return 0; }
  };

  if (h.size() == 8) {
    rgba.r = parse(h, 0, 2) / 255.0;
    rgba.g = parse(h, 2, 2) / 255.0;
    rgba.b = parse(h, 4, 2) / 255.0;
    rgba.a = parse(h, 6, 2) / 255.0;
  } else if (h.size() == 6) {
    rgba.r = parse(h, 0, 2) / 255.0;
    rgba.g = parse(h, 2, 2) / 255.0;
    rgba.b = parse(h, 4, 2) / 255.0;
    rgba.a = 1.0;
  } else if (h.size() == 3) {
    rgba.r = parse(h, 0, 1) * 17 / 255.0;
    rgba.g = parse(h, 1, 1) * 17 / 255.0;
    rgba.b = parse(h, 2, 1) * 17 / 255.0;
    rgba.a = 1.0;
  }
  return rgba;
}

Theme Theme::default_theme() {
  Theme t;
  t.bg = hex_to_rgba("#0b081a");
  t.text = hex_to_rgba("#ffffff");
  t.accent = hex_to_rgba("#00e5ff");
  t.alt_bg = hex_to_rgba("#151030");
  t.hover_bg = hex_to_rgba("#201540");
  t.input_bg = hex_to_rgba("#0d0a20");
  t.border = hex_to_rgba("#2a2050");
  t.border_radius = 8;
  t.border_width = 1;
  return t;
}

std::string get_theme_file_name() {
  auto read_env_lower = [](const char *var) -> std::string {
    const char *v = std::getenv(var);
    if (!v) return {};
    std::string s(v);
    for (auto &c : s) c = std::tolower(static_cast<unsigned char>(c));
    return s;
  };

  std::string de = read_env_lower("XDG_CURRENT_DESKTOP");
  if (de.empty()) de = read_env_lower("DESKTOP_SESSION");

  static const char *wms[] = {
    "hyprland", "i3", "sway", "river", "dwl", "qtile",
    "bspwm", "awesome", "xfce", "kde", "gnome"
  };
  for (auto wm : wms) {
    if (de.find(wm) != std::string::npos)
      return std::string(wm) + ".conf";
  }

  const char *wl = std::getenv("WAYLAND_DISPLAY");
  if (wl && wl[0]) return "wayland.conf";
  return "x11.conf";
}

Theme load_theme() {
  auto base = config_dir();
  if (!fs::exists(base)) return Theme::default_theme();

  auto load_file = [](const fs::path &p) -> std::unique_ptr<toml::Table> {
    if (!fs::exists(p)) return nullptr;
    std::ifstream f(p);
    if (!f.is_open()) return nullptr;
    std::stringstream ss;
    ss << f.rdbuf();
    return std::make_unique<toml::Table>(toml::parse(ss.str()));
  };

  auto parse_theme = [](const toml::Table &t) -> std::unique_ptr<Theme> {
    auto th = std::make_unique<Theme>(Theme::default_theme());
    auto assign = [&](const std::string &key, Rgba &field) {
      auto v = t.get(key);
      if (v) field = hex_to_rgba(*v);
    };
    assign("bg_color", th->bg);
    assign("text_color", th->text);
    assign("accent_color", th->accent);
    assign("alt_bg_color", th->alt_bg);
    assign("hover_bg_color", th->hover_bg);
    assign("input_bg_color", th->input_bg);
    assign("border_color", th->border);

    auto v = t.get("border_radius");
    if (v) { try { th->border_radius = std::stoul(*v); } catch (...) {} }
    v = t.get("border_width");
    if (v) { try { th->border_width = std::stoul(*v); } catch (...) {} }
    return th;
  };

  // Try DE-specific file
  auto specific = fs::path(base) / get_theme_file_name();
  if (auto t = load_file(specific)) {
    if (auto th = parse_theme(*t)) return *th;
  }

  // Try generic config files
  for (auto &fname : {"config.toml", "theme.toml"}) {
    auto p = fs::path(base) / fname;
    if (auto t = load_file(p)) {
      if (auto th = parse_theme(*t)) return *th;
    }
  }

  return Theme::default_theme();
}

} // namespace runrs
