#pragma once
#include <string>
#include <unordered_map>
#include <sstream>
#include <cctype>

namespace toml {

class Table {
  std::unordered_map<std::string, std::string> vals_;
public:
  void set(const std::string &k, const std::string &v) { vals_[k] = v; }
  const std::string *get(const std::string &k) const {
    auto it = vals_.find(k);
    return it != vals_.end() ? &it->second : nullptr;
  }
};

inline std::string trim(const std::string &s) {
  size_t start = 0, end = s.size();
  while (start < end && (s[start] == ' ' || s[start] == '\t')) ++start;
  while (end > start && (s[end-1] == ' ' || s[end-1] == '\t')) --end;
  return s.substr(start, end - start);
}

inline std::string unquote(const std::string &s) {
  if (s.size() >= 2 && s.front() == '"' && s.back() == '"')
    return s.substr(1, s.size() - 2);
  if (s.size() >= 2 && s.front() == '\'' && s.back() == '\'')
    return s.substr(1, s.size() - 2);
  return s;
}

inline Table parse(const std::string &content) {
  Table tbl;
  std::istringstream ss(content);
  std::string line;
  std::string section;

  while (std::getline(ss, line)) {
    auto c = line.find('#');
    if (c != std::string::npos) line = line.substr(0, c);
    line = trim(line);
    if (line.empty()) continue;

    if (line.front() == '[') {
      auto end = line.find(']');
      if (end != std::string::npos)
        section = trim(line.substr(1, end - 1));
      continue;
    }

    auto eq = line.find('=');
    if (eq == std::string::npos) continue;

    auto key = trim(line.substr(0, eq));
    auto val = trim(line.substr(eq + 1));

    if (!section.empty()) key = section + "." + key;

    // Strip quotes from value
    if (val.size() >= 2 && val.front() == '"' && val.back() == '"') {
      // Handle escape sequences minimally
      std::string unescaped;
      for (size_t i = 1; i < val.size() - 1; ++i) {
        if (val[i] == '\\' && i + 1 < val.size() - 1) {
          switch (val[++i]) {
            case 'n': unescaped += '\n'; break;
            case 't': unescaped += '\t'; break;
            case '"': unescaped += '"'; break;
            case '\\': unescaped += '\\'; break;
            default: unescaped += val[i]; break;
          }
        } else {
          unescaped += val[i];
        }
      }
      val = unescaped;
    }

    tbl.set(key, val);
  }
  return tbl;
}

} // namespace toml
