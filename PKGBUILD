# Maintainer: steven66619 <ste@example.com>
pkgname=launcher
pkgver=1.0
pkgrel=1
pkgdesc="Wayland application launcher with search, icons, and keyboard navigation"
arch=('x86_64')
url="https://github.com/steven66619/launcher"
license=('MIT')
depends=('wayland' 'cairo' 'pango' 'librsvg' 'glib2' 'libxkbcommon')
makedepends=('wayland-protocols')
source=("launcher-1.0.tar.gz::file:///tmp/launcher-1.0.tar.gz")
sha256sums=('SKIP')

build() {
  cd "$srcdir/$pkgname-$pkgver"
  make PREFIX=/usr
}

package() {
  cd "$srcdir/$pkgname-$pkgver"
  make PREFIX=/usr DESTDIR="$pkgdir" install
}
