#pragma once
#include <string>
#include <vector>

namespace runrs {

struct DesktopEntry {
  std::string filepath;
  std::string name;
  std::string generic_name;
  std::string comment;
  std::string icon;
  std::string exec;
  std::string categories;
  std::string keywords;
  std::string stratum; // bedrock stratum, empty if not in a stratum
  bool no_display = false;
  bool hidden = false;
  bool startup_notify = true;
  std::string terminal = "false";

  std::string display_name() const;
};

std::vector<DesktopEntry> load_applications();
std::vector<DesktopEntry> load_applications_cached();
std::vector<DesktopEntry> search_applications(const std::vector<DesktopEntry> &apps,
                                               const std::string &query);

} // namespace runrs
