cmake_minimum_required(VERSION 3.20)
project(runrs
  VERSION 1.0.3
  DESCRIPTION "Application launcher with system icons"
  LANGUAGES CXX
)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

find_package(PkgConfig REQUIRED)

pkg_check_modules(XCB REQUIRED
  xcb
  xcb-icccm
  xcb-keysyms
  xcb-aux
)

pkg_check_modules(CAIRO REQUIRED cairo cairo-xcb)
pkg_check_modules(PANGO REQUIRED pangocairo)
pkg_check_modules(RSVG REQUIRED librsvg-2.0)
pkg_check_modules(XKBCOMMON REQUIRED xkbcommon xkbcommon-x11)

add_executable(runrs
  src/main.cpp
  src/config.cpp
  src/theme.cpp
  src/launch.cpp
  src/metrics.cpp
  src/icons.cpp
  src/desktop.cpp
  src/window.cpp
)

target_include_directories(runrs PRIVATE
  ${XCB_INCLUDE_DIRS}
  ${CAIRO_INCLUDE_DIRS}
  ${PANGO_INCLUDE_DIRS}
  ${RSVG_INCLUDE_DIRS}
  ${XKBCOMMON_INCLUDE_DIRS}
  src
)

target_link_libraries(runrs PRIVATE
  ${XCB_LIBRARIES}
  ${CAIRO_LIBRARIES}
  ${PANGO_LIBRARIES}
  ${RSVG_LIBRARIES}
  ${XKBCOMMON_LIBRARIES}
)

target_compile_options(runrs PRIVATE
  ${XCB_CFLAGS_OTHER}
  ${CAIRO_CFLAGS_OTHER}
  ${PANGO_CFLAGS_OTHER}
  ${RSVG_CFLAGS_OTHER}
  ${XKBCOMMON_CFLAGS_OTHER}
)

install(TARGETS runrs DESTINATION bin)
