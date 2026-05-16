#!/bin/sh
set -e

PREFIX="${PREFIX:-/usr/local}"
BINARY="$PREFIX/bin/launcher"

usage() {
    cat <<'EOF'
Usage: ./install.sh [OPTIONS]

Options:
  -u, --uninstall    Remove launcher and exit
  -p, --prefix DIR   Install to DIR/bin (default: /usr/local)
  -h, --help         Show this help

Environment:
  PREFIX             Same as --prefix (default: /usr/local)
EOF
    exit 0
}

uninstall() {
    echo "==> Removing $BINARY"
    rm -f "$BINARY"
    echo "==> Done"
    exit 0
}

for arg in "$@"; do
    case "$arg" in
        -h|--help) usage ;;
        -u|--uninstall) uninstall ;;
        --prefix=*) PREFIX="${arg#*=}" ;;
        -p) echo "use --prefix=PATH or PREFIX env var"; exit 1 ;;
    esac
done

echo "==> Checking dependencies"
for pkg in wayland-client cairo pangocairo; do
    if ! pkg-config --exists "$pkg" 2>/dev/null; then
        echo "ERROR: missing $pkg -- install build dependencies first"
        echo "  Arch:  sudo pacman -S base-devel wayland wayland-protocols cairo pango"
        echo "  Debian: sudo apt install build-essential pkg-config libwayland-dev libcairo2-dev libpango1.0-dev wayland-protocols"
        echo "  Fedora: sudo dnf install gcc pkgconfig wayland-devel wayland-protocols-devel cairo-devel pango-devel"
        exit 1
    fi
done
echo "    all found"

echo "==> Building"
make -s clean 2>/dev/null || true
make -s

echo "==> Installing to $BINARY"
install -Dm755 launcher "$BINARY"

echo ""
echo "==> Installation complete."
echo "    Bind 'launcher' to your preferred keybind (e.g. Super+Space in Hyprland)."
echo "    Run '$0 --uninstall' to remove."
