#pragma once
#include <string>
#include <vector>
#include <cairo.h>

namespace runrs {

struct IconDir {
  std::string path;
  int size = 48;
  int min_size = 48;
  int max_size = 48;
  std::string type = "Scalable";
  std::string context = "Applications";
};

struct IconTheme {
  std::string name;
  std::vector<std::string> directories;
  std::vector<std::string> inherits;
};

cairo_surface_t *load_icon(const std::string &name, int size = 48);
std::string lookup_icon_in_theme(const std::string &name, const std::string &theme, int size);
std::string lookup_fallback_icon(const std::string &name, int size);
std::vector<IconTheme> list_icon_themes();

} // namespace runrs
