PREFIX ?= /usr/local

release:
	cargo build --release

install: release
	install -Dm755 target/release/launcher-wayland $(DESTDIR)$(PREFIX)/bin/launcher

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/launcher

.PHONY: release install uninstall
