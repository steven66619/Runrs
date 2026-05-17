# Maintainer: Ste <steven4x4@gmail.com>
pkgname=launcher-wayland-git
pkgver=1.0.3
pkgrel=1
pkgdesc="Ultra-fast standard-compliant application menu overlay for Wayland and Hyprland"
arch=('x86_64')
url="https://github.com/steven66619/launcher"
license=('MIT')
depends=('cairo' 'pango' 'libxkbcommon' 'wayland')
makedepends=('cargo' 'git')
provides=('launcher')
conflicts=('launcher')

source=()
md5sums=()

validpgpkeys=('908BA367E7E797F3F72B673710F1B1E3F275953D')

prepare() {
  mkdir -p "$srcdir/remote_repo"
  cp "$startdir/Cargo.toml" "$srcdir/remote_repo/"
  cp "$startdir/Cargo.lock" "$srcdir/remote_repo/"
  cp -r "$startdir/src" "$srcdir/remote_repo/"
  cp -r "$startdir/.git" "$srcdir/remote_repo/"
}

pkgver() {
  cd "$srcdir/remote_repo"
  printf "1.0.3.r%s.%s" "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)"
}

build() {
  cd "$srcdir/remote_repo"
  cargo build --release --locked
}

package() {
  cd "$srcdir/remote_repo"
  install -Dm755 "target/release/launcher-wayland" "$pkgdir/usr/bin/launcher"
}

