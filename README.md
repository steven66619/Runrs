# launcher

A lightweight Wayland/X11 application launcher with search, app icons, and
Bedrock Linux cross-stratum support. Looks like wofi and dmenu had a baby.

## Dependencies

- wayland (or X11 fallback)
- wayland-protocols
- cairo
- pango
- xkbcommon
- librsvg

### Arch

sudo pacman -S wayland wayland-protocols cairo pango libxkbcommon librsvg

### Debian/Ubuntu

sudo apt install libwayland-dev libcairo2-dev libpango1.0-dev \
  libxkbcommon-dev wayland-protocols librsvg2-dev

### Fedora

sudo dnf install wayland-devel wayland-protocols-devel cairo-devel \
  pango-devel libxkbcommon-devel librsvg2-devel

## Build & Install

make
sudo make install

## Usage

Run from a keybind (e.g. in Hyprland):

bind = SUPER, SPACE, exec, launcher

Type to filter, arrow keys to navigate, Enter to launch, Esc to close.

### Bedrock Linux cross-stratum launching

Prefix a command with a distribution shortcut to launch it inside a specific
Bedrock stratum:

- arch:firefox  →  strat arch firefox
- deb:apt update  →  strat debian apt update
- fed:dnf upgrade  →  strat fedora dnf upgrade

Typing a raw command in the search bar and pressing Enter will also execute it
(via sh -c), even if no desktop entry matches.
