#pragma once
#include <cstdint>
#include <chrono>
#include <utility>

namespace runrs {

class NetworkSpeedometer {
  uint64_t last_rx_ = 0;
  uint64_t last_tx_ = 0;
  std::chrono::steady_clock::time_point last_check_;

  static bool read_sys_bytes(uint64_t &rx, uint64_t &tx);

public:
  NetworkSpeedometer();

  std::pair<double, double> calculate_speeds();
};

std::pair<uint32_t, bool> get_power_status();

} // namespace runrs
