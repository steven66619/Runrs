#include "config.h"
#include "toml.h"
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <sstream>

namespace runrs {
namespace fs = std::filesystem;

std::string config_dir() {
  const char *xdg = std::getenv("XDG_CONFIG_HOME");
  if (xdg && xdg[0]) {
    return (fs::path(xdg) / "Runrs").string();
  }
  const char *home = std::getenv("HOME");
  if (home) {
    return (fs::path(home) / ".config" / "Runrs").string();
  }
  return "./.config/Runrs";
}

std::vector<std::string> data_dirs() {
  std::vector<std::string> dirs;
  const char *xdg = std::getenv("XDG_DATA_HOME");
  if (xdg && xdg[0]) {
    dirs.push_back(xdg);
  } else {
    const char *home = std::getenv("HOME");
    if (home) dirs.push_back(std::string(home) + "/.local/share");
  }
  const char *xdg_dirs = std::getenv("XDG_DATA_DIRS");
  if (xdg_dirs && xdg_dirs[0]) {
    std::istringstream ss(xdg_dirs);
    std::string d;
    while (std::getline(ss, d, ':')) {
      if (!d.empty()) dirs.push_back(d);
    }
  } else {
    dirs.push_back("/usr/local/share");
    dirs.push_back("/usr/share");
  }
  return dirs;
}

std::string cache_dir() {
  const char *xdg = std::getenv("XDG_CACHE_HOME");
  if (xdg && xdg[0]) {
    return (fs::path(xdg) / "runrs").string();
  }
  const char *home = std::getenv("HOME");
  if (home) {
    return (fs::path(home) / ".cache" / "runrs").string();
  }
  return "/tmp/runrs-cache";
}

Config load_config() {
  Config cfg;
  auto path = fs::path(config_dir()) / "config.toml";
  if (!fs::exists(path)) return cfg;

  std::ifstream f(path);
  if (!f.is_open()) return cfg;
  std::stringstream ss;
  ss << f.rdbuf();
  auto tbl = toml::parse(ss.str());

  if (auto v = tbl.get("terminal")) cfg.terminal = *v;
  if (auto v = tbl.get("file_manager")) cfg.file_manager = *v;
  if (auto v = tbl.get("show_metrics")) cfg.show_metrics = (*v == "true");
  if (auto v = tbl.get("max_results")) {
    try { cfg.max_results = std::stoi(*v); } catch (...) {}
  }
  return cfg;
}

} // namespace runrs
