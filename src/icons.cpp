#include "icons.h"
#include "config.h"
#include <algorithm>
#include <cctype>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <set>
#include <sstream>
#include <cairo.h>
#include <librsvg/rsvg.h>

namespace runrs {
namespace fs = std::filesystem;

static std::string to_lower(const std::string &s) {
  std::string r = s;
  for (auto &c : r) c = std::tolower(static_cast<unsigned char>(c));
  return r;
}

static std::vector<std::string> split(const std::string &s, char delim) {
  std::vector<std::string> parts;
  std::istringstream ss(s);
  std::string item;
  while (std::getline(ss, item, delim))
    if (!item.empty()) parts.push_back(item);
  return parts;
}

static std::string trim(const std::string &s) {
  size_t start = 0, end = s.size();
  while (start < end && std::isspace(static_cast<unsigned char>(s[start]))) ++start;
  while (end > start && std::isspace(static_cast<unsigned char>(s[end-1]))) --end;
  return s.substr(start, end - start);
}

// Parse an index.theme file
static IconTheme parse_index_theme(const fs::path &path) {
  IconTheme theme;
  theme.name = path.parent_path().filename().string();

  std::ifstream f(path);
  if (!f.is_open()) return theme;

  std::string line;
  std::string section;
  IconDir current_dir;

  while (std::getline(f, line)) {
    auto c = line.find('#');
    if (c != std::string::npos) line = line.substr(0, c);
    line = trim(line);
    if (line.empty()) continue;

    if (line.front() == '[') {
      // Finalize previous dir entry
      if (!current_dir.path.empty()) {
        theme.directories.push_back(current_dir.path);
        current_dir = IconDir{};
      }
      auto end = line.find(']');
      if (end != std::string::npos) section = trim(line.substr(1, end - 1));
      continue;
    }

    auto eq = line.find('=');
    if (eq == std::string::npos) continue;
    auto key = trim(line.substr(0, eq));
    auto val = trim(line.substr(eq + 1));

    if (section == "Icon Theme") {
      if (key == "Inherits") {
        auto names = split(val, ',');
        for (auto &n : names) theme.inherits.push_back(trim(n));
      } else if (key == "Directories") {
        auto names = split(val, ',');
        for (auto &n : names) {
          auto d = trim(n);
          if (!d.empty()) theme.directories.push_back(d);
        }
      }
    } else if (!section.empty()) {
      // Directory section — collect path for lookup
      current_dir.path = section;
    }
  }

  if (!current_dir.path.empty())
    theme.directories.push_back(current_dir.path);

  return theme;
}

std::vector<IconTheme> list_icon_themes() {
  std::vector<IconTheme> themes;
  auto dirs = data_dirs();

  for (auto &base : dirs) {
    auto icons_dir = fs::path(base) / "icons";
    if (!fs::is_directory(icons_dir)) continue;

    for (auto &entry : fs::directory_iterator(icons_dir)) {
      if (!entry.is_directory()) continue;
      auto theme_file = entry.path() / "index.theme";
      if (!fs::exists(theme_file)) continue;
      themes.push_back(parse_index_theme(theme_file));
    }
  }
  return themes;
}

// Find the best matching icon file for a given name in a theme directory
static std::string find_icon_in_dir(const fs::path &theme_dir, const std::string &dir_name,
                                     const std::string &name, int size) {
  auto dir = theme_dir / dir_name;
  if (!fs::is_directory(dir)) return {};

  // Try exact name with extensions
  static const char *exts[] = {".png", ".svg", ".xpm"};
  for (auto ext : exts) {
    auto p = dir / (name + ext);
    if (fs::exists(p)) return p.string();
  }

  // Try lowercased
  auto lower = to_lower(name);
  for (auto ext : exts) {
    auto p = dir / (lower + ext);
    if (fs::exists(p)) return p.string();
  }

  return {};
}

static std::string lookup_in_theme(const fs::path &theme_dir, const std::string &name,
                                    int size, std::set<std::string> &visited) {
  auto theme_path = theme_dir / "index.theme";
  if (!fs::exists(theme_path)) return {};
  auto theme = parse_index_theme(theme_path);

  // Try directories in the order listed in index.theme (usually size-ordered)
  for (auto &dir_name : theme.directories) {
    auto found = find_icon_in_dir(theme_dir, dir_name, name, size);
    if (!found.empty()) return found;
  }

  // Try inherits
  for (auto &inherit : theme.inherits) {
    if (visited.count(inherit)) continue;
    visited.insert(inherit);

    // Look in all data dirs for the inherited theme
    auto dirs = data_dirs();
    for (auto &base : dirs) {
      auto inherit_dir = fs::path(base) / "icons" / inherit;
      if (fs::is_directory(inherit_dir)) {
        auto found = lookup_in_theme(inherit_dir, name, size, visited);
        if (!found.empty()) return found;
      }
    }
  }

  return {};
}

std::string lookup_icon_in_theme(const std::string &name, const std::string &theme_name, int size) {
  auto dirs = data_dirs();
  for (auto &base : dirs) {
    auto theme_dir = fs::path(base) / "icons" / theme_name;
    if (!fs::is_directory(theme_dir)) continue;
    std::set<std::string> visited{theme_name};
    auto found = lookup_in_theme(theme_dir, name, size, visited);
    if (!found.empty()) return found;
  }
  return {};
}

std::string lookup_fallback_icon(const std::string &name, int size) {
  // Try hicolor theme
  auto found = lookup_icon_in_theme(name, "hicolor", size);
  if (!found.empty()) return found;

  // Check pixmaps
  auto dirs = data_dirs();
  for (auto &base : dirs) {
    auto pixmaps = fs::path(base) / "pixmaps";
    if (!fs::is_directory(pixmaps)) continue;

    static const char *exts[] = {".png", ".svg", ".xpm"};
    for (auto ext : exts) {
      auto p = pixmaps / (name + ext);
      if (fs::exists(p)) return p.string();
      auto p2 = pixmaps / (to_lower(name) + ext);
      if (fs::exists(p2)) return p2.string();
    }
  }

  return {};
}

cairo_surface_t *load_icon(const std::string &name, int size) {
  if (name.empty()) return nullptr;

  // If it's already a path, load directly
  if (name.find('/') != std::string::npos && fs::exists(name)) {
    auto ext = fs::path(name).extension().string();
    if (ext == ".svg") {
      // Load SVG with librsvg
      RsvgHandle *handle = rsvg_handle_new_from_file(name.c_str(), nullptr);
      if (!handle) return nullptr;
      RsvgRectangle viewport = {0, 0, (double)size, (double)size};
      cairo_surface_t *surf = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, size, size);
      cairo_t *cr = cairo_create(surf);
      gboolean ok = rsvg_handle_render_document(handle, cr, &viewport, nullptr);
      cairo_destroy(cr);
      g_object_unref(handle);
      if (ok) return surf;
      cairo_surface_destroy(surf);
      return nullptr;
    }
    // PNG via cairo
    return cairo_image_surface_create_from_png(name.c_str());
  }

  // Try XDG icon themes
  std::string path;

  // Determine icon theme: GTK settings > XDG_CURSOR_THEME > hicolor
  std::string theme_name = "hicolor";
  auto try_gtk_theme = [&](const fs::path &ini) {
    if (!fs::exists(ini)) return;
    std::ifstream f(ini);
    std::string l;
    while (std::getline(f, l)) {
      if (l.find("gtk-icon-theme-name") != std::string::npos) {
        auto eq = l.find('=');
        if (eq != std::string::npos) {
          auto v = l.substr(eq + 1);
          v = trim(v);
          if (!v.empty()) theme_name = v;
        }
      }
    }
  };
  const char *home = std::getenv("HOME");
  if (home) {
    try_gtk_theme(fs::path(home) / ".config/gtk-3.0/settings.ini");
    try_gtk_theme(fs::path(home) / ".config/gtk-4.0/settings.ini");
  }
  const char *cursor = std::getenv("XCURSOR_THEME");
  if (cursor && cursor[0] && theme_name == "hicolor")
    theme_name = cursor;

  path = lookup_icon_in_theme(name, theme_name, size);
  if (path.empty()) path = lookup_icon_in_theme(name, "hicolor", size);
  if (path.empty()) path = lookup_fallback_icon(name, size);

  if (path.empty()) return nullptr;

  auto ext = fs::path(path).extension().string();
  if (ext == ".svg") {
    RsvgHandle *handle = rsvg_handle_new_from_file(path.c_str(), nullptr);
    if (!handle) return nullptr;
    RsvgRectangle viewport = {0, 0, (double)size, (double)size};
    cairo_surface_t *surf = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, size, size);
    cairo_t *cr = cairo_create(surf);
    gboolean ok = rsvg_handle_render_document(handle, cr, &viewport, nullptr);
    cairo_destroy(cr);
    g_object_unref(handle);
    if (ok) return surf;
    cairo_surface_destroy(surf);
    return nullptr;
  }

  return cairo_image_surface_create_from_png(path.c_str());
}

} // namespace runrs
