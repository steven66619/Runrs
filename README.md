# launcher

A lightweight Wayland application launcher with search and app icons.
Looks like wofi and dmenu had a baby.

## Dependencies

- wayland
- wayland-protocols
- cairo
- pango
- xkbcommon

### Arch

sudo pacman -S wayland wayland-protocols cairo pango libxkbcommon

### Debian/Ubuntu

sudo apt install libwayland-dev libcairo2-dev libpango1.0-dev \
  libxkbcommon-dev wayland-protocols

### Fedora

sudo dnf install wayland-devel wayland-protocols-devel cairo-devel \
  pango-devel libxkbcommon-devel

## Build & Install

make
sudo make install

## Usage

Run from a keybind (e.g. in Hyprland):

bind = SUPER, SPACE, exec, launcher

Type to filter, arrow keys to navigate, Enter to launch, Esc to close.
