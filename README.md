# runrs

A lightweight X11 application launcher with search, system icons, and
Bedrock Linux cross-stratum support. Built with C++17, XCB, Cairo, and Pango.

---

## Features

- **Fast search** — real-time filtering across desktop entry names, exec fields,
  and generic names as you type
- **System icons** — full XDG Icon Theme Spec resolution (SVG via librsvg, PNG)
- **Cross-distro launching** — searches Bedrock strata for apps; displays
  `[stratum]` tags and runs commands in the correct environment
- **Scrollable results** — keyboard navigation with arrows, page up/down,
  home/end; scroll arrows indicate off-screen entries
- **Dark theme** — TOML-configurable colors with DE-aware auto-detection
- **Network & battery metrics** — optional real-time speedometer and power
  status in the corner
- **Background detachment** — launched processes are fully detached

---

## Dependencies

| Package         | Debian / Ubuntu              |
|-----------------|------------------------------|
| xcb             | `libxcb-dev`                 |
| xcb-icccm       | `libxcb-icccm4-dev`          |
| xcb-keysyms     | `libxcb-keysyms1-dev`        |
| xcb-aux         | `libxcb-util-dev`            |
| cairo           | `libcairo2-dev`              |
| pango           | `libpango1.0-dev`            |
| librsvg         | `librsvg2-dev`               |
| xkbcommon       | `libxkbcommon-dev`           |

### Quick install

```bash
# Debian / Ubuntu
sudo apt install libxcb-dev libxcb-icccm4-dev libxcb-keysyms1-dev \
  libxcb-util-dev libcairo2-dev libpango1.0-dev librsvg2-dev libxkbcommon-dev
```

---

## Build & Install

```bash
make release
sudo make install
```

The binary is placed at `/usr/local/bin/runrs`.

---

## Usage

Bind to a key in your window manager:

```conf
# Hyprland
bind = SUPER, SPACE, exec, runrs

# i3 / sway
bindsym Mod4+space exec runrs
```

- **Type** to filter applications
- **Arrow keys**, **PgUp/PgDn**, **Home/End** to navigate
- **Enter** to launch, **Esc** to dismiss
- **Tab** to autocomplete from the first match
- **Ctrl+Backspace** to delete the last word

---

## Bedrock Linux

runrs automatically detects Bedrock Linux strata and tags each app with its
stratum name: `Firefox [fedora]`, `Alacritty [arch]`.

---

## Configuration

Create `~/.config/Runrs/config.toml`:

```toml
terminal = "foot"
max_results = 20
show_metrics = true
```

Theme colors can be customized in `~/.config/Runrs/config.toml`:

```toml
bg_color = "#0b081a"
text_color = "#ffffff"
accent_color = "#00e5ff"
```

DE-specific files (`hyprland.conf`, `sway.conf`, etc.) override the base config
when the matching desktop environment is detected.
