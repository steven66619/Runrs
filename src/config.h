#pragma once
#include <string>
#include <vector>

namespace runrs {

struct Config {
  std::string terminal = "xterm";
  std::string file_manager = "xdg-open";
  bool show_metrics = false;
  int max_results = 30;
};

Config load_config();
std::string config_dir();
std::string cache_dir();
std::vector<std::string> data_dirs();

} // namespace runrs
