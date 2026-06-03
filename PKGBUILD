# Maintainer: Ste <steven4x4@gmail.com>
pkgname=runrs-git
pkgver=1.0.3
pkgrel=1
pkgdesc="Ultra-fast standard-compliant application menu overlay for Wayland and X11"
arch=('x86_64')
url="https://github.com/steven66619/Runrs"
license=('MIT')
depends=('cairo' 'pango' 'libxkbcommon' 'wayland')
makedepends=('cargo' 'git')
provides=('runrs')
conflicts=('runrs')

source=("remote_repo::git+https://github.com/steven66619/Runrs.git#branch=master")
md5sums=('SKIP')

validpgpkeys=('908BA367E7E797F3F72B673710F1B1E3F275953D')

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
  install -Dm755 "target/release/runrs" "$pkgdir/usr/bin/runrs"
}

