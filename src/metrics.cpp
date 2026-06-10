#include "metrics.h"
#include <algorithm>
#include <cctype>
#include <fstream>
#include <sstream>
#include <string>

namespace runrs {

bool NetworkSpeedometer::read_sys_bytes(uint64_t &rx, uint64_t &tx) {
  std::ifstream f("/proc/net/dev");
  if (!f.is_open()) return false;

  std::string line;
  while (std::getline(f, line)) {
    if (line.find("wlan0") != std::string::npos ||
        line.find("eth0") != std::string::npos ||
        line.find("enp") != std::string::npos ||
        line.find("wlp") != std::string::npos) {
      size_t pos = line.find(':');
      if (pos == std::string::npos) continue;
      auto rest = line.substr(pos + 1);
      auto notspace = [](unsigned char c) { return !std::isspace(c); };
      auto start = std::find_if(rest.begin(), rest.end(), notspace);
      if (start == rest.end()) continue;
      rest = std::string(start, rest.end());

      std::istringstream ss(rest);
      std::string val;
      for (int i = 0; i < 9; ++i) {
        if (!(ss >> val)) break;
        if (i == 0) rx = std::stoull(val);
      }
      tx = std::stoull(val);
      return true;
    }
  }
  return false;
}

NetworkSpeedometer::NetworkSpeedometer()
  : last_check_(std::chrono::steady_clock::now()) {
  read_sys_bytes(last_rx_, last_tx_);
}

std::pair<double, double> NetworkSpeedometer::calculate_speeds() {
  auto now = std::chrono::steady_clock::now();
  auto elapsed = std::chrono::duration<double>(now - last_check_).count();
  if (elapsed <= 0.0) return {0.0, 0.0};

  uint64_t cur_rx{}, cur_tx{};
  read_sys_bytes(cur_rx, cur_tx);

  double rx_speed = (cur_rx - last_rx_) / 1024.0 / elapsed;
  double tx_speed = (cur_tx - last_tx_) / 1024.0 / elapsed;

  last_rx_ = cur_rx;
  last_tx_ = cur_tx;
  last_check_ = now;

  return {rx_speed, tx_speed};
}

std::pair<uint32_t, bool> get_power_status() {
  uint32_t capacity = 0;
  bool charging = false;

  std::ifstream cap("/sys/class/power_supply/BAT0/capacity");
  if (cap.is_open()) {
    std::string s;
    std::getline(cap, s);
    try { capacity = std::stoul(s); } catch (...) {}
  }

  std::ifstream stat("/sys/class/power_supply/BAT0/status");
  if (stat.is_open()) {
    std::string s;
    std::getline(stat, s);
    charging = (s.find("Charging") != std::string::npos);
  }

  return {capacity, charging};
}

} // namespace runrs
