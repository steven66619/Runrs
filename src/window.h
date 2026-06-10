#pragma once
#include <cstdint>
#include <memory>
#include <string>
#include <vector>

#include <xcb/xcb.h>
#include <xcb/xproto.h>
#include <xcb/xcb_keysyms.h>
#include <cairo.h>
#include <pango/pango.h>

typedef struct _PangoContext PangoContext;

namespace runrs {

struct Theme;
struct DesktopEntry;
struct Config;

class LauncherWindow {
public:
  LauncherWindow();
  ~LauncherWindow();

  bool init();
  void run();

private:
  xcb_connection_t *conn_ = nullptr;
  xcb_screen_t *screen_ = nullptr;
  xcb_window_t win_{};
  xcb_key_symbols_t *keysyms_ = nullptr;

  xcb_atom_t wm_delete_window_{};
  xcb_atom_t wm_protocols_{};
  xcb_atom_t net_wm_name_{};
  xcb_atom_t net_wm_window_type_{};
  xcb_atom_t net_wm_window_type_dialog_{};
  xcb_atom_t net_wm_state_{};
  xcb_atom_t net_wm_state_above_{};
  xcb_atom_t net_wm_state_sticky_{};
  xcb_atom_t net_wm_state_modal_{};
  xcb_atom_t net_wm_desktop_{};
  xcb_atom_t net_wm_pid_{};
  xcb_atom_t net_active_window_{};

  // Window surface (on-screen)
  cairo_surface_t *surface_ = nullptr;
  cairo_t *cr_ = nullptr;

  // Backbuffer (off-screen, eliminates flicker)
  cairo_surface_t *backbuf_ = nullptr;
  cairo_t *back_cr_ = nullptr;

  PangoContext *pango_ctx_ = nullptr;

  int width_ = 700;
  int height_ = 500;
  int screen_width_ = 0;
  int screen_height_ = 0;

  std::string input_;
  std::vector<DesktopEntry> apps_;
  std::vector<DesktopEntry> filtered_;
  std::unique_ptr<Theme> theme_;
  std::unique_ptr<Config> config_;
  int selection_ = 0;
  int scroll_offset_ = 0;
  bool running_ = true;
  bool dirty_ = true;
  bool show_metrics_ = false;

  uint64_t last_frame_ = 0;
  bool cursor_visible_ = true;
  uint64_t cursor_toggle_time_ = 0;
  uint64_t metrics_update_time_ = 0;

  void setup_window();
  void setup_rendering();
  void setup_atoms();
  void center_window();

  void handle_event();
  void handle_key_press(uint32_t keycode, uint16_t mods);
  void handle_button_press(int x, int y);

  void compose();
  void flip();
  void compose_input_field();
  void compose_results();
  void compose_metrics();
  void compose_entry(int index, int y, bool hovered);

  void launch_selected();
  void update_filter();

  xcb_atom_t intern_atom(const std::string &name);
  void ewmh_set_cardinal(xcb_atom_t atom, uint32_t value);

};

} // namespace runrs
