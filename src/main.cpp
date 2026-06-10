#include "window.h"
#include <iostream>

int main() {
  runrs::LauncherWindow app;
  if (!app.init()) {
    std::cerr << "Failed to initialize launcher" << std::endl;
    return 1;
  }
  app.run();
  return 0;
}
