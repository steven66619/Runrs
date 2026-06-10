#include "desktop.h"
#include "config.h"
#include <algorithm>
#include <cctype>
#include <chrono>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <vector>

namespace runrs {
namespace fs = std::filesystem;

static std::string trim(const std::string &s) {
  size_t start = 0, end = s.size();
  while (start < end && std::isspace(static_cast<unsigned char>(s[start]))) ++start;
  while (end > start && std::isspace(static_cast<unsigned char>(s[end-1]))) --end;
  return s.substr(start, end - start);
}

static std::string unescape_exec(const std::string &s) {
  std::string result;
  for (size_t i = 0; i < s.size(); ++i) {
    if (s[i] == '\\' && i + 1 < s.size()) {
      switch (s[++i]) {
        case 's': result += ' '; break;
        case 'n': result += '\n'; break;
        case 't': result += '\t'; break;
        case 'r': result += '\r'; break;
        case '\\': result += '\\'; break;
        default: result += s[i]; break;
      }
    } else {
      result += s[i];
    }
  }
  return result;
}

static std::string to_lower(const std::string &s) {
  std::string r = s;
  for (auto &c : r) c = std::tolower(static_cast<unsigned char>(c));
  return r;
}

// Extract bedrock stratum name from path, or empty string
static std::string detect_stratum(const fs::path &path) {
  auto p = path.lexically_normal().string();
  // /bedrock/strata/<stratum>/usr/share/applications/...
  auto marker = std::string("/bedrock/strata/");
  auto pos = p.find(marker);
  if (pos == std::string::npos) return {};
  auto start = pos + marker.size();
  auto end = p.find('/', start);
  if (end == std::string::npos) return {};
  return p.substr(start, end - start);
}

DesktopEntry parse_desktop_file(const fs::path &path) {
  DesktopEntry entry;
  entry.filepath = path.string();
  entry.stratum = detect_stratum(path);

  std::ifstream f(path);
  if (!f.is_open()) return entry;

  std::string line;
  bool in_desktop_entry = false;
  std::string lang;

  const char *locale_env = std::getenv("LANG");
  if (locale_env) {
    std::string locale(locale_env);
    auto dot = locale.find('.');
    if (dot != std::string::npos) locale = locale.substr(0, dot);
    lang = locale;
  }

  while (std::getline(f, line)) {
    auto c = line.find('#');
    if (c != std::string::npos) line = line.substr(0, c);
    line = trim(line);
    if (line.empty()) continue;

    if (line.front() == '[') {
      auto end = line.find(']');
      if (end != std::string::npos) {
        in_desktop_entry = (line.substr(1, end - 1) == "Desktop Entry");
      }
      continue;
    }

    if (!in_desktop_entry) continue;

    auto eq = line.find('=');
    if (eq == std::string::npos) continue;

    auto key = line.substr(0, eq);
    auto val = unescape_exec(line.substr(eq + 1));

    auto bracket = key.find('[');
    if (bracket != std::string::npos) {
      auto close = key.find(']', bracket);
      if (close != std::string::npos) {
        std::string key_lang = key.substr(bracket + 1, close - bracket - 1);
        // Skip non-matching locale variants entirely
        if (!lang.empty() && key_lang != lang &&
            key_lang != lang.substr(0, 2)) {
          continue;
        }
      }
    }

    if (key == "Name") entry.name = val;
    else if (key == "GenericName") entry.generic_name = val;
    else if (key == "Comment") entry.comment = val;
    else if (key == "Icon") entry.icon = val;
    else if (key == "Exec") entry.exec = val;
    else if (key == "Categories") entry.categories = val;
    else if (key == "Keywords") entry.keywords = val;
    else if (key == "Terminal") entry.terminal = val;
    else if (key == "NoDisplay") entry.no_display = (val == "true");
    else if (key == "Hidden") entry.hidden = (val == "true");
    else if (key == "StartupNotify") entry.startup_notify = (val == "true");
  }

  return entry;
}

std::string DesktopEntry::display_name() const {
  std::string n = name.empty() ? fs::path(filepath).stem().string() : name;
  if (!stratum.empty())
    n += " [" + stratum + "]";
  return n;
}

// ── Cache format ────────────────────────────────────────────────────
// Header: magic(4) ver(4) ndirs(4) [plen(4) path(plen) mtime(8)]*  nentries(4)
// Entry:  plen(4) filepath(plen) ... (all strings length-prefixed) flags(1)
static const uint32_t CACHE_MAGIC = 0x52554E52; // "RUNR"
static const uint32_t CACHE_VER   = 1;

struct DirInfo {
  std::string path;
  int64_t mtime;
};

static std::vector<DirInfo> scan_dirs() {
  std::vector<DirInfo> dirs;
  for (auto &base : data_dirs()) {
    auto apps = fs::path(base) / "applications";
    if (!fs::is_directory(apps)) continue;
    DirInfo di;
    di.path = apps.lexically_normal().string();
    auto ft = fs::last_write_time(apps);
    di.mtime = std::chrono::duration_cast<std::chrono::seconds>(
                 ft.time_since_epoch()).count();
    dirs.push_back(di);
  }
  // Also scan bedrock strata applications dirs
  fs::path bedrock("/bedrock/strata");
  if (fs::is_directory(bedrock)) {
    for (auto &stratum : fs::directory_iterator(bedrock)) {
      if (!stratum.is_directory()) continue;
      auto apps = stratum.path() / "usr/share/applications";
      if (!fs::is_directory(apps)) continue;
      DirInfo di;
      di.path = apps.lexically_normal().string();
      auto ft = fs::last_write_time(apps);
      di.mtime = std::chrono::duration_cast<std::chrono::seconds>(
                   ft.time_since_epoch()).count();
      dirs.push_back(di);
    }
  }
  return dirs;
}

static bool cache_valid(const std::vector<DirInfo> &dirs, const fs::path &cache_path) {
  std::ifstream f(cache_path, std::ios::binary);
  if (!f.is_open()) return false;

  auto read32 = [&]() -> uint32_t {
    uint32_t v{};
    f.read(reinterpret_cast<char*>(&v), 4);
    return v;
  };
  auto read64 = [&]() -> int64_t {
    int64_t v{};
    f.read(reinterpret_cast<char*>(&v), 8);
    return v;
  };

  if (read32() != CACHE_MAGIC || read32() != CACHE_VER) return false;

  uint32_t ndirs = read32();
  if (ndirs != (uint32_t)dirs.size()) return false;

  for (uint32_t i = 0; i < ndirs; ++i) {
    uint32_t plen = read32();
    std::string p((size_t)plen, '\0');
    f.read(&p[0], plen);
    int64_t mtime = read64();
    if (p != dirs[i].path || mtime != dirs[i].mtime) return false;
  }
  return true;
}

static void write_cache(const std::vector<DirInfo> &dirs,
                         const std::vector<DesktopEntry> &entries,
                         const fs::path &cache_path) {
  fs::create_directories(cache_path.parent_path());
  std::ofstream f(cache_path, std::ios::binary);
  if (!f.is_open()) return;

  auto write32 = [&](uint32_t v) { f.write((const char*)&v, 4); };
  auto write64 = [&](int64_t v) { f.write((const char*)&v, 8); };
  auto writestr = [&](const std::string &s) {
    uint32_t len = (uint32_t)s.size();
    write32(len);
    f.write(s.data(), len);
  };

  write32(CACHE_MAGIC);
  write32(CACHE_VER);
  write32((uint32_t)dirs.size());
  for (auto &d : dirs) {
    writestr(d.path);
    write64(d.mtime);
  }

  write32((uint32_t)entries.size());
  for (auto &e : entries) {
    writestr(e.filepath);
    writestr(e.name);
    writestr(e.generic_name);
    writestr(e.comment);
    writestr(e.icon);
    writestr(e.exec);
    writestr(e.categories);
    writestr(e.keywords);
    writestr(e.stratum);
    writestr(e.terminal);

    uint8_t flags = 0;
    if (e.no_display) flags |= 1;
    if (e.hidden)     flags |= 2;
    if (e.startup_notify) flags |= 4;
    write32(flags);
  }
}

static std::vector<DesktopEntry> read_cache(const std::vector<DirInfo> &dirs,
                                              const fs::path &cache_path) {
  std::ifstream f(cache_path, std::ios::binary);
  if (!f.is_open()) return {};

  auto read32 = [&]() -> uint32_t {
    uint32_t v{};
    f.read(reinterpret_cast<char*>(&v), 4);
    return v;
  };
  auto readstr = [&]() -> std::string {
    uint32_t len = read32();
    std::string s((size_t)len, '\0');
    f.read(&s[0], len);
    return s;
  };

  read32(); read32(); // magic + ver (already validated)
  uint32_t ndirs = read32();
  for (uint32_t i = 0; i < ndirs; ++i) {
    readstr(); read32(); // path plen
    uint32_t tmp[2];
    f.read((char*)tmp, 8); // mtime
  }

  uint32_t n = read32();
  std::vector<DesktopEntry> entries;
  entries.reserve(n);

  for (uint32_t i = 0; i < n; ++i) {
    DesktopEntry e;
    e.filepath      = readstr();
    e.name          = readstr();
    e.generic_name  = readstr();
    e.comment       = readstr();
    e.icon          = readstr();
    e.exec          = readstr();
    e.categories    = readstr();
    e.keywords      = readstr();
    e.stratum       = readstr();
    e.terminal      = readstr();
    uint8_t flags = (uint8_t)read32();
    e.no_display    = flags & 1;
    e.hidden        = flags & 2;
    e.startup_notify = flags & 4;
    entries.push_back(std::move(e));
  }
  return entries;
}

std::vector<DesktopEntry> load_applications_cached() {
  auto dirs = scan_dirs();
  auto cache_path = fs::path(cache_dir()) / "apps.cache";

  if (cache_valid(dirs, cache_path)) {
    auto cached = read_cache(dirs, cache_path);
    if (!cached.empty()) return cached;
  }

  // Cache miss — scan from scratch
  auto entries = load_applications();
  write_cache(dirs, entries, cache_path);
  return entries;
}

std::vector<DesktopEntry> load_applications() {
  std::vector<DesktopEntry> entries;
  auto dirs = data_dirs();

  for (auto &base : dirs) {
    auto apps_dir = fs::path(base) / "applications";
    if (!fs::is_directory(apps_dir)) continue;
    for (auto &entry : fs::recursive_directory_iterator(apps_dir)) {
      if (!entry.is_regular_file()) continue;
      if (entry.path().extension() != ".desktop") continue;
      auto app = parse_desktop_file(entry.path());
      if (app.hidden || app.no_display) continue;
      if (app.exec.empty()) continue;
      entries.push_back(std::move(app));
    }
  }

  // Also scan bedrock strata
  fs::path bedrock("/bedrock/strata");
  if (fs::is_directory(bedrock)) {
    for (auto &stratum : fs::directory_iterator(bedrock)) {
      if (!stratum.is_directory()) continue;
      auto apps_dir = stratum.path() / "usr/share/applications";
      if (!fs::is_directory(apps_dir)) continue;
      for (auto &entry : fs::recursive_directory_iterator(apps_dir)) {
        if (!entry.is_regular_file()) continue;
        if (entry.path().extension() != ".desktop") continue;
        auto app = parse_desktop_file(entry.path());
        if (app.hidden || app.no_display) continue;
        if (app.exec.empty()) continue;
        entries.push_back(std::move(app));
      }
    }
  }

  return entries;
}

std::vector<DesktopEntry> search_applications(const std::vector<DesktopEntry> &apps,
                                               const std::string &query) {
  if (query.empty()) return apps;

  std::vector<DesktopEntry> results;
  auto q = to_lower(trim(query));

  for (auto &app : apps) {
    auto name = to_lower(app.name);
    auto gn = to_lower(app.generic_name);
    auto exec = to_lower(app.exec);

    if (name.find(q) != std::string::npos ||
        gn.find(q) != std::string::npos ||
        exec.find(q) != std::string::npos ||
        to_lower(app.stratum).find(q) != std::string::npos) {
      results.push_back(app);
    }
  }

  return results;
}

} // namespace runrs
