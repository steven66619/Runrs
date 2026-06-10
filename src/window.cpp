#include "window.h"
#include "config.h"
#include "theme.h"
#include "launch.h"
#include "metrics.h"
#include "desktop.h"
#include "icons.h"

#include <cmath>
#include <cstring>
#include <iostream>
#include <sstream>
#include <algorithm>
#include <chrono>
#include <unistd.h>

#include <xcb/xcb.h>
#include <xcb/xcb_aux.h>
#include <xcb/xcb_icccm.h>
#include <xcb/xcb_keysyms.h>
#include <xcb/xproto.h>
#include <X11/keysym.h>
#include <cairo.h>
#include <cairo-xcb.h>
#include <pango/pangocairo.h>

namespace runrs {

static const char *APP_NAME = "runrs";

static uint64_t timestamp_ms() {
  return std::chrono::duration_cast<std::chrono::milliseconds>(
    std::chrono::steady_clock::now().time_since_epoch()
  ).count();
}

static void set_source_rgba(cairo_t *cr, const Rgba &c) {
  cairo_set_source_rgba(cr, c.r, c.g, c.b, c.a);
}

static void rounded_rect(cairo_t *cr, double x, double y, double w, double h, double r) {
  if (r > w / 2) r = w / 2;
  if (r > h / 2) r = h / 2;
  cairo_move_to(cr, x + r, y);
  cairo_arc(cr, x + w - r, y + r, r, -M_PI_2, 0);
  cairo_arc(cr, x + w - r, y + h - r, r, 0, M_PI_2);
  cairo_arc(cr, x + r, y + h - r, r, M_PI_2, M_PI);
  cairo_arc(cr, x + r, y + r, r, M_PI, 3 * M_PI_2);
  cairo_close_path(cr);
}

// ── Construction / Destruction ──────────────────────────────────────

LauncherWindow::LauncherWindow()
  : config_(std::make_unique<Config>(load_config()))
  , theme_(std::make_unique<Theme>(load_theme())) {
  apps_ = load_applications_cached();
  filtered_ = apps_;
  show_metrics_ = config_->show_metrics;
}

LauncherWindow::~LauncherWindow() {
  if (keysyms_) xcb_key_symbols_free(keysyms_);
  if (pango_ctx_) g_object_unref(pango_ctx_);
  if (back_cr_) cairo_destroy(back_cr_);
  if (backbuf_) cairo_surface_destroy(backbuf_);
  if (cr_) cairo_destroy(cr_);
  if (surface_) cairo_surface_destroy(surface_);
  if (conn_) xcb_disconnect(conn_);
}

// ── XCB helpers ──────────────────────────────────────────────────────

xcb_atom_t LauncherWindow::intern_atom(const std::string &name) {
  auto reply = xcb_intern_atom_reply(conn_,
    xcb_intern_atom(conn_, 0, name.size(), name.c_str()), nullptr);
  return reply ? reply->atom : XCB_ATOM_NONE;
}

void LauncherWindow::ewmh_set_cardinal(xcb_atom_t atom, uint32_t value) {
  xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, win_,
    atom, XCB_ATOM_CARDINAL, 32, 1, &value);
}

// ── Initialization ──────────────────────────────────────────────────

bool LauncherWindow::init() {
  int screen_num;
  conn_ = xcb_connect(nullptr, &screen_num);
  if (xcb_connection_has_error(conn_)) {
    std::cerr << "Failed to connect to X server" << std::endl;
    return false;
  }

  screen_ = xcb_aux_get_screen(conn_, screen_num);
  screen_width_ = screen_->width_in_pixels;
  screen_height_ = screen_->height_in_pixels;

  keysyms_ = xcb_key_symbols_alloc(conn_);

  setup_atoms();
  setup_window();
  setup_rendering();

  return true;
}

void LauncherWindow::setup_atoms() {
  wm_delete_window_ = intern_atom("WM_DELETE_WINDOW");
  wm_protocols_ = intern_atom("WM_PROTOCOLS");
  net_wm_name_ = intern_atom("_NET_WM_NAME");
  net_wm_window_type_ = intern_atom("_NET_WM_WINDOW_TYPE");
  net_wm_window_type_dialog_ = intern_atom("_NET_WM_WINDOW_TYPE_DIALOG");
  net_wm_state_ = intern_atom("_NET_WM_STATE");
  net_wm_state_above_ = intern_atom("_NET_WM_STATE_ABOVE");
  net_wm_state_sticky_ = intern_atom("_NET_WM_STATE_STICKY");
  net_wm_state_modal_ = intern_atom("_NET_WM_STATE_MODAL");
  net_wm_desktop_ = intern_atom("_NET_WM_DESKTOP");
  net_wm_pid_ = intern_atom("_NET_WM_PID");
  net_active_window_ = intern_atom("_NET_ACTIVE_WINDOW");
}

void LauncherWindow::setup_window() {
  width_ = 520;
  height_ = 360;

  win_ = xcb_generate_id(conn_);
  uint32_t mask = XCB_CW_BACK_PIXEL | XCB_CW_EVENT_MASK;
  uint32_t values[2] = {
    screen_->black_pixel,
    XCB_EVENT_MASK_EXPOSURE |
    XCB_EVENT_MASK_KEY_PRESS |
    XCB_EVENT_MASK_KEY_RELEASE |
    XCB_EVENT_MASK_BUTTON_PRESS |
    XCB_EVENT_MASK_POINTER_MOTION |
    XCB_EVENT_MASK_STRUCTURE_NOTIFY |
    XCB_EVENT_MASK_FOCUS_CHANGE
  };
  xcb_create_window(conn_, XCB_COPY_FROM_PARENT, win_, screen_->root,
    0, 0, width_, height_, 0,
    XCB_WINDOW_CLASS_INPUT_OUTPUT, screen_->root_visual,
    mask, values);

  xcb_icccm_set_wm_protocols(conn_, win_, wm_protocols_, 1, &wm_delete_window_);
  xcb_icccm_set_wm_name(conn_, win_, XCB_ATOM_STRING, 8, 6, APP_NAME);
  ewmh_set_cardinal(net_wm_pid_, getpid());

  // Window type: dialog (not dock — docks steal input focus on many WMs)
  xcb_change_property(conn_, XCB_PROP_MODE_REPLACE, win_,
    net_wm_window_type_, XCB_ATOM_ATOM, 32, 1, &net_wm_window_type_dialog_);

  // Request above + sticky + modal
  auto send_state = [&](xcb_atom_t state, bool add) {
    xcb_client_message_event_t ev{};
    ev.response_type = XCB_CLIENT_MESSAGE;
    ev.window = win_;
    ev.type = net_wm_state_;
    ev.format = 32;
    ev.data.data32[0] = add ? 1 : 0;
    ev.data.data32[1] = state;
    ev.data.data32[3] = 1;
    xcb_send_event(conn_, 0, screen_->root,
      XCB_EVENT_MASK_SUBSTRUCTURE_REDIRECT | XCB_EVENT_MASK_SUBSTRUCTURE_NOTIFY,
      (const char *)&ev);
  };
  send_state(net_wm_state_above_, true);
  send_state(net_wm_state_sticky_, true);
  send_state(net_wm_state_modal_, true);

  uint32_t all_desktops = 0xFFFFFFFF;
  ewmh_set_cardinal(net_wm_desktop_, all_desktops);

  center_window();
  xcb_map_window(conn_, win_);
  xcb_flush(conn_);

  // Request input focus via _NET_ACTIVE_WINDOW
  xcb_client_message_event_t ev{};
  ev.response_type = XCB_CLIENT_MESSAGE;
  ev.window = screen_->root;
  ev.type = net_active_window_;
  ev.format = 32;
  ev.data.data32[0] = 2; // pager hint
  ev.data.data32[1] = 0; // timestamp (optional)
  ev.data.data32[2] = win_;
  xcb_send_event(conn_, 0, screen_->root,
    XCB_EVENT_MASK_SUBSTRUCTURE_REDIRECT | XCB_EVENT_MASK_SUBSTRUCTURE_NOTIFY,
    (const char *)&ev);
  xcb_flush(conn_);
}

void LauncherWindow::center_window() {
  int x = (screen_width_ - width_) / 2;
  int y = (screen_height_ - height_) / 3;
  uint32_t mask = XCB_CONFIG_WINDOW_X | XCB_CONFIG_WINDOW_Y |
                  XCB_CONFIG_WINDOW_WIDTH | XCB_CONFIG_WINDOW_HEIGHT;
  uint32_t values[] = {(uint32_t)std::max(0, x), (uint32_t)std::max(0, y),
                       (uint32_t)width_, (uint32_t)height_};
  xcb_configure_window(conn_, win_, mask, values);
}

void LauncherWindow::setup_rendering() {
  xcb_visualtype_t *visual = xcb_aux_find_visual_by_attrs(
    screen_, -1, XCB_VISUAL_CLASS_TRUE_COLOR);
  if (!visual) visual = xcb_aux_find_visual_by_attrs(
    screen_, -1, XCB_VISUAL_CLASS_DIRECT_COLOR);
  if (!visual) {
    visual = xcb_aux_find_visual_by_id(screen_, screen_->root_visual);
  }

  // On-screen surface
  surface_ = cairo_xcb_surface_create(conn_, win_, visual, width_, height_);
  cr_ = cairo_create(surface_);

  // Backbuffer (off-screen image surface)
  backbuf_ = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width_, height_);
  back_cr_ = cairo_create(backbuf_);

  pango_ctx_ = pango_cairo_create_context(back_cr_);
}

// ── Main Loop ───────────────────────────────────────────────────────

void LauncherWindow::run() {
  last_frame_ = timestamp_ms();
  cursor_toggle_time_ = last_frame_;
  metrics_update_time_ = last_frame_;

  // Fallback: force input focus directly
  xcb_set_input_focus(conn_, XCB_INPUT_FOCUS_PARENT, win_, XCB_CURRENT_TIME);
  xcb_flush(conn_);

  while (running_) {
    auto ev = xcb_poll_for_event(conn_);
    if (ev) {
      do {
        uint8_t type = ev->response_type & ~0x80;
        if (type == XCB_CLIENT_MESSAGE) {
          auto *cm = (xcb_client_message_event_t *)ev;
          if (cm->data.data32[0] == wm_delete_window_)
            running_ = false;
        } else if (type == XCB_EXPOSE) {
          dirty_ = true;
        } else if (type == XCB_KEY_PRESS) {
          auto *kp = (xcb_key_press_event_t *)ev;
          handle_key_press(kp->detail, kp->state);
        } else if (type == XCB_BUTTON_PRESS) {
          auto *bp = (xcb_button_press_event_t *)ev;
          handle_button_press(bp->event_x, bp->event_y);
        } else if (type == XCB_CONFIGURE_NOTIFY) {
          auto *cn = (xcb_configure_notify_event_t *)ev;
          if ((int)cn->width != width_ || (int)cn->height != height_) {
            width_ = cn->width;
            height_ = cn->height;
            cairo_xcb_surface_set_size(surface_, width_, height_);
            // Recreate backbuffer at new size
            cairo_destroy(back_cr_);
            cairo_surface_destroy(backbuf_);
            backbuf_ = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width_, height_);
            back_cr_ = cairo_create(backbuf_);
            g_object_unref(pango_ctx_);
            pango_ctx_ = pango_cairo_create_context(back_cr_);
            dirty_ = true;
          }
        } else if (type == XCB_FOCUS_OUT) {
          // Only close on focus-out if we've had focus for >100ms
          // (avoids closing during initial map/focus handshake)
          // Only close if we've had the window open >100ms
          if (timestamp_ms() - last_frame_ > 100)
            running_ = false;
        }
        free(ev);
      } while ((ev = xcb_poll_for_event(conn_)));

      if (dirty_) {
        compose();
        flip();
        dirty_ = false;
      }
    } else {
      auto now = timestamp_ms();
      bool need_update = false;

      if (now - cursor_toggle_time_ > 500) {
        cursor_visible_ = !cursor_visible_;
        cursor_toggle_time_ = now;
        need_update = true;
      }

      if (show_metrics_ && now - metrics_update_time_ > 1000) {
        metrics_update_time_ = now;
        need_update = true;
      }

      if (need_update) {
        compose();
        flip();
      } else {
        xcb_flush(conn_);
        usleep(8000);
      }
    }
  }
}

// ── Input Handling ──────────────────────────────────────────────────

void LauncherWindow::handle_key_press(uint32_t keycode, uint16_t mods) {
  auto *ks = xcb_key_symbols_alloc(conn_);
  xcb_keysym_t sym = xcb_key_symbols_get_keysym(ks, keycode, 0);
  if (!sym) sym = xcb_key_symbols_get_keysym(ks, keycode, 1);
  xcb_key_symbols_free(ks);

  bool ctrl = mods & XCB_MOD_MASK_CONTROL;

  switch (sym) {
    case XK_Return:
    case XK_KP_Enter:
      launch_selected();
      running_ = false;
      break;

    case XK_Escape:
      running_ = false;
      break;

    case XK_BackSpace:
      if (!input_.empty()) {
        if (ctrl) {
          auto pos = input_.find_last_not_of(" ");
          if (pos != std::string::npos) {
            auto word_start = input_.rfind(' ', pos);
            if (word_start == std::string::npos)
              input_.clear();
            else
              input_ = input_.substr(0, word_start);
          }
        } else {
          input_.pop_back();
        }
        update_filter();
      }
      break;

    case XK_Up:
    case XK_KP_Up:
      if (selection_ > 0) --selection_;
      break;

    case XK_Down:
    case XK_KP_Down:
      if (selection_ < (int)filtered_.size() - 1) ++selection_;
      break;

    case XK_Page_Up:
    case XK_KP_Page_Up: {
      int page = (height_ - 50) / 42;
      selection_ = std::max(0, selection_ - page);
      break;
    }
    case XK_Page_Down:
    case XK_KP_Page_Down: {
      int page = (height_ - 50) / 42;
      selection_ = std::min((int)filtered_.size() - 1, selection_ + page);
      break;
    }

    case XK_Home:
    case XK_KP_Home:
      selection_ = 0;
      break;

    case XK_End:
    case XK_KP_End:
      selection_ = (int)filtered_.size() - 1;
      break;

    case XK_Tab:
      if (!filtered_.empty()) {
        input_ = filtered_[0].display_name();
        update_filter();
      }
      break;

    default: {
      char buf[8] = {};
      int len = 0;

      if (sym >= 0x20 && sym <= 0x7E) {
        buf[0] = (char)sym;
        len = 1;
      } else if (sym >= 0x0100 && sym < 0x10000) {
        uint32_t uc = sym;
        if (uc >= 0x0800) {
          buf[0] = 0xE0 | ((uc >> 12) & 0x0F);
          buf[1] = 0x80 | ((uc >> 6) & 0x3F);
          buf[2] = 0x80 | (uc & 0x3F);
          len = 3;
        } else if (uc >= 0x0080) {
          buf[0] = 0xC0 | ((uc >> 6) & 0x1F);
          buf[1] = 0x80 | (uc & 0x3F);
          len = 2;
        }
      }

      if (len > 0) {
        input_ += std::string(buf, len);
        update_filter();
      }
      break;
    }
  }

  // Keep selection visible
  int ih = 42, sy = 46;
  int max_visible = (height_ - sy - 4) / ih;
  if (selection_ < scroll_offset_)
    scroll_offset_ = selection_;
  else if (selection_ >= scroll_offset_ + max_visible)
    scroll_offset_ = selection_ - max_visible + 1;

  dirty_ = true;
}

void LauncherWindow::handle_button_press(int x, int y) {
  int input_height = 44;
  int item_height = 42;
  int start_y = input_height + 4;

  if (y < input_height) return;

  int slot = (y - start_y) / item_height;
  int index = scroll_offset_ + slot;
  if (slot >= 0 && index >= 0 && index < (int)filtered_.size()) {
    selection_ = index;
    launch_selected();
    running_ = false;
  }
}

void LauncherWindow::launch_selected() {
  if (filtered_.empty()) return;
  auto &entry = filtered_[std::min(selection_, (int)filtered_.size() - 1)];

  std::string cleaned;
  for (size_t i = 0; i < entry.exec.size(); ++i) {
    if (entry.exec[i] == '%' && i + 1 < entry.exec.size()) {
      switch (entry.exec[i + 1]) {
        case 'f': case 'F': case 'u': case 'U':
        case 'd': case 'D': case 'n': case 'N':
        case 'i': case 'c': case 'k': case 'm':
          i++;
          continue;
        case '%':
          cleaned += '%';
          i++;
          continue;
      }
    }
    cleaned += entry.exec[i];
  }

  launch_background(cleaned);
}

void LauncherWindow::update_filter() {
  filtered_ = search_applications(apps_, input_);
  selection_ = 0;
  scroll_offset_ = 0;
  if ((int)filtered_.size() > config_->max_results)
    filtered_.resize(config_->max_results);
}

// ── Composition (backbuffer) ────────────────────────────────────────

void LauncherWindow::compose() {
  // Paint background
  set_source_rgba(back_cr_, theme_->bg);
  cairo_paint(back_cr_);

  // Border
  set_source_rgba(back_cr_, theme_->border);
  cairo_set_line_width(back_cr_, theme_->border_width);
  rounded_rect(back_cr_, 0.5, 0.5, width_ - 1, height_ - 1, theme_->border_radius);
  cairo_stroke(back_cr_);

  compose_input_field();
  compose_results();
  if (show_metrics_) compose_metrics();
}

void LauncherWindow::flip() {
  // Blit backbuffer to window in one shot
  cairo_set_source_surface(cr_, backbuf_, 0, 0);
  cairo_paint(cr_);
  cairo_surface_flush(surface_);
}

void LauncherWindow::compose_input_field() {
  int fx = 10, fy = 6, fw = width_ - 20, fh = 34;

  set_source_rgba(back_cr_, theme_->input_bg);
  rounded_rect(back_cr_, fx, fy, fw, fh, theme_->border_radius);
  cairo_fill(back_cr_);

  // Accent line
  set_source_rgba(back_cr_, theme_->accent);
  cairo_rectangle(back_cr_, fx + 2, fy + fh - 2, fw - 4, 2);
  cairo_fill(back_cr_);

  // Text layout
  auto layout = pango_cairo_create_layout(back_cr_);
  pango_layout_set_text(layout, input_.c_str(), input_.size());
  auto fd = pango_font_description_from_string("Sans 14");
  pango_layout_set_font_description(layout, fd);
  pango_font_description_free(fd);

  set_source_rgba(back_cr_, theme_->text);
  cairo_move_to(back_cr_, fx + 8, fy + (fh - 20) / 2);
  pango_cairo_show_layout(back_cr_, layout);

  // Cursor
  if (cursor_visible_) {
    PangoRectangle extents;
    pango_layout_get_cursor_pos(layout, input_.size(), &extents, nullptr);
    int cx = fx + 8 + extents.x / PANGO_SCALE;
    int cy = fy + (fh - 20) / 2 + extents.y / PANGO_SCALE;
    set_source_rgba(back_cr_, theme_->accent);
    cairo_rectangle(back_cr_, cx, cy, 2, extents.height / PANGO_SCALE);
    cairo_fill(back_cr_);
  }
  g_object_unref(layout);
}

void LauncherWindow::compose_results() {
  int ih = 42, sy = 46;
  int max_visible = (height_ - sy - 4) / ih;

  // Clamp scroll
  int n = (int)filtered_.size();
  if (scroll_offset_ > n - max_visible)
    scroll_offset_ = std::max(0, n - max_visible);

  int vis = std::min(n - scroll_offset_, max_visible);
  int end_y = sy + vis * ih;

  for (int i = 0; i < vis; ++i) {
    int idx = scroll_offset_ + i;
    compose_entry(idx, sy + i * ih, idx == selection_);
  }

  // Scroll arrows
  auto draw_arrow = [&](int cx, int cy, bool up) {
    set_source_rgba(back_cr_, theme_->accent);
    cairo_set_line_width(back_cr_, 2);
    if (up) {
      cairo_move_to(back_cr_, cx - 4, cy + 3);
      cairo_line_to(back_cr_, cx, cy - 2);
      cairo_line_to(back_cr_, cx + 4, cy + 3);
    } else {
      cairo_move_to(back_cr_, cx - 4, cy - 3);
      cairo_line_to(back_cr_, cx, cy + 2);
      cairo_line_to(back_cr_, cx + 4, cy - 3);
    }
    cairo_stroke(back_cr_);
  };
  if (scroll_offset_ > 0)
    draw_arrow(width_ - 14, sy + 3, true);
  if (scroll_offset_ + vis < n)
    draw_arrow(width_ - 14, end_y - 3, false);
}

void LauncherWindow::compose_entry(int index, int y, bool hovered) {
  auto &entry = filtered_[index];
  int ix = 8, iw = width_ - 16, ih = 40;

  set_source_rgba(back_cr_, hovered ? theme_->hover_bg : theme_->alt_bg);
  rounded_rect(back_cr_, ix, y, iw, ih, 5);
  cairo_fill(back_cr_);

  // Icon
  int is = 28;
  int iix = ix + 8, iiy = y + (ih - is) / 2;
  cairo_surface_t *icon = load_icon(entry.icon, is);

  if (icon) {
    cairo_save(back_cr_);
    rounded_rect(back_cr_, iix, iiy, is, is, 4);
    cairo_clip(back_cr_);
    cairo_set_source_surface(back_cr_, icon, iix, iiy);
    cairo_paint(back_cr_);
    cairo_restore(back_cr_);
    cairo_surface_destroy(icon);
  } else {
    set_source_rgba(back_cr_, theme_->accent);
    rounded_rect(back_cr_, iix, iiy, is, is, 4);
    cairo_fill(back_cr_);
    set_source_rgba(back_cr_, theme_->bg);
    cairo_rectangle(back_cr_, iix + 6, iiy + 7, is - 12, is - 14);
    cairo_fill(back_cr_);
    set_source_rgba(back_cr_, theme_->accent);
    cairo_set_line_width(back_cr_, 1.5);
    cairo_rectangle(back_cr_, iix + 6, iiy + 7, is - 12, is - 14);
    cairo_stroke(back_cr_);
  }

  // Name
  auto layout = pango_cairo_create_layout(back_cr_);
  pango_layout_set_text(layout, entry.display_name().c_str(), entry.display_name().size());
  auto fd = pango_font_description_from_string("Sans Bold 12");
  pango_layout_set_font_description(layout, fd);
  pango_font_description_free(fd);

  set_source_rgba(back_cr_, hovered ? theme_->accent : theme_->text);
  cairo_move_to(back_cr_, iix + is + 10, y + 5);
  pango_cairo_show_layout(back_cr_, layout);

  // Subtext
  auto sub = entry.generic_name.empty() ? entry.comment : entry.generic_name;
  if (!sub.empty() && (int)sub.size() < 60) {
    pango_layout_set_text(layout, sub.c_str(), sub.size());
    auto fd2 = pango_font_description_from_string("Sans 10");
    pango_layout_set_font_description(layout, fd2);
    pango_font_description_free(fd2);
    Rgba muted = theme_->text;
    muted.a = 0.55;
    set_source_rgba(back_cr_, muted);
    cairo_move_to(back_cr_, iix + is + 10, y + 22);
    pango_cairo_show_layout(back_cr_, layout);
  }
  g_object_unref(layout);
}

void LauncherWindow::compose_metrics() {
  static NetworkSpeedometer speedo;
  static auto last_update = timestamp_ms();
  static double rx = 0, tx = 0;
  static uint32_t cap = 0;
  static bool charging = false;

  auto now = timestamp_ms();
  if (now - last_update > 2000) {
    auto s = speedo.calculate_speeds();
    rx = s.first;
    tx = s.second;
    auto p = get_power_status();
    cap = p.first;
    charging = p.second;
    last_update = now;
  }

  int mx = width_ - 165, my = height_ - 48, mw = 155, mh = 42;

  set_source_rgba(back_cr_, theme_->alt_bg);
  rounded_rect(back_cr_, mx, my, mw, mh, 5);
  cairo_fill(back_cr_);

  auto layout = pango_cairo_create_layout(back_cr_);
  auto fd = pango_font_description_from_string("Sans Mono 9");
  pango_layout_set_font_description(layout, fd);
  pango_font_description_free(fd);

  char netbuf[64];
  snprintf(netbuf, sizeof(netbuf), "\xe2\x86\x93 %.1f \xe2\x86\x91 %.1f KB/s", rx, tx);
  pango_layout_set_text(layout, netbuf, -1);
  set_source_rgba(back_cr_, theme_->accent);
  cairo_move_to(back_cr_, mx + 6, my + 4);
  pango_cairo_show_layout(back_cr_, layout);

  char battbuf[24];
  snprintf(battbuf, sizeof(battbuf), "%u%%%s", cap, charging ? " +" : "");
  pango_layout_set_text(layout, battbuf, -1);
  Rgba bc = theme_->text;
  if (cap < 20) bc = {1.0, 0.2, 0.2, 1.0};
  else if (charging) bc = {0.2, 1.0, 0.2, 1.0};
  set_source_rgba(back_cr_, bc);
  cairo_move_to(back_cr_, mx + 6, my + 22);
  pango_cairo_show_layout(back_cr_, layout);

  g_object_unref(layout);
}

} // namespace runrs
