PREFIX ?= /usr/local

release:
	cargo build --release

install: release
	install -Dm755 target/release/runrs $(DESTDIR)$(PREFIX)/bin/runrs

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/runrs

.PHONY: release install uninstall
