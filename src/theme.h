#pragma once
#include <cstdint>
#include <string>

namespace runrs {

struct Rgba {
  double r{}, g{}, b{}, a{1.0};
};

struct Theme {
  Rgba bg;
  Rgba text;
  Rgba accent;
  Rgba alt_bg;
  Rgba hover_bg;
  Rgba input_bg;
  Rgba border;
  uint32_t border_radius = 8;
  uint32_t border_width = 1;

  static Theme default_theme();
};

Theme load_theme();
std::string get_theme_file_name();

Rgba hex_to_rgba(const std::string &hex);

} // namespace runrs
