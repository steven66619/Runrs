PREFIX ?= /usr/local
CFLAGS ?= -O2 -pipe

WAYLAND_SCANNER := $(shell pkg-config --variable=wayland_scanner wayland-scanner)
PROTOCOLS_DIR := /usr/share/wayland-protocols

CFLAGS += $(shell pkg-config --cflags wayland-client cairo pangocairo xkbcommon) \
	-std=c99 -Wall -Wextra -D_POSIX_C_SOURCE=200809L \
	-Wno-unused-parameter
LDLIBS = $(shell pkg-config --libs wayland-client cairo pangocairo xkbcommon) -lm

WLROOT := wlr-layer-shell-unstable-v1
WLHEADER := $(WLROOT)-client.h
WLCODE := $(WLROOT)-client.c

XDG   := xdg-shell
XDGXML := /usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml
XDGHDR := $(XDG)-client.h
XDGCOD := $(XDG)-client.c

OBJS := main.o $(WLCODE:.c=.o) $(XDGCOD:.c=.o)

launcher: $(OBJS)
	$(CC) $(CFLAGS) -o $@ $(OBJS) $(LDLIBS)

$(WLHEADER): $(WLROOT).xml
	$(WAYLAND_SCANNER) client-header < $< > $@

$(WLCODE): $(WLROOT).xml
	$(WAYLAND_SCANNER) private-code < $< > $@

$(XDGHDR): $(XDGXML)
	$(WAYLAND_SCANNER) client-header < $< > $@

$(XDGCOD): $(XDGXML)
	$(WAYLAND_SCANNER) private-code < $< > $@

main.o: main.c $(WLHEADER)

%.o: %.c
	$(CC) $(CFLAGS) -c -o $@ $<

clean:
	rm -f launcher *.o $(WLHEADER) $(WLCODE) $(XDGHDR) $(XDGCOD)

install: launcher
	install -Dm755 launcher $(DESTDIR)$(PREFIX)/bin/launcher

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/launcher

.PHONY: clean install uninstall
