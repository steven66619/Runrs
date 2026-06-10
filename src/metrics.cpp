#include "metrics.h"
#include <algorithm>
#include <cctype>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

namespace runrs {

static bool match_interface(const std::string &line) {
  return line.find("wlan0") != std::string::npos ||
         line.find("eth0") != std::string::npos ||
         line.find("enp")  != std::string::npos ||
         line.find("wlp")  != std::string::npos;
}

static std::vector<uint64_t> parse_net_stats(const std::string &line) {
  auto pos = line.find(':');
  if (pos == std::string::npos) return {};
  std::string rest = line.substr(pos + 1);
  auto notspace = [](unsigned char c) { return !std::isspace(c); };
  auto start = std::find_if(rest.begin(), rest.end(), notspace);
  if (start == rest.end()) return {};
  rest = std::string(start, rest.end());

  std::vector<uint64_t> fields;
  std::istringstream ss(rest);
  uint64_t v;
  while (ss >> v)
    fields.push_back(v);
  return fields;
}

bool NetworkSpeedometer::read_sys_bytes(uint64_t &rx, uint64_t &tx) {
  std::ifstream f("/proc/net/dev");
  if (!f.is_open()) return false;

  std::string line;
  while (std::getline(f, line)) {
    if (!match_interface(line)) continue;
    auto fields = parse_net_stats(line);
    // Standard layout: 8 RX fields + 8 TX fields
    // RX bytes = fields[0], TX bytes = fields[8]
    if (fields.size() < 9) return false;
    rx = fields[0];
    tx = fields[8];
    return true;
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
