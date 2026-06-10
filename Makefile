PREFIX ?= /usr/local
BUILD_DIR ?= build

release:
	cmake -S . -B $(BUILD_DIR) -DCMAKE_BUILD_TYPE=Release
	cmake --build $(BUILD_DIR) -j$$(nproc)

debug:
	cmake -S . -B $(BUILD_DIR) -DCMAKE_BUILD_TYPE=Debug
	cmake --build $(BUILD_DIR) -j$$(nproc)

install: release
	install -Dm755 $(BUILD_DIR)/runrs $(DESTDIR)$(PREFIX)/bin/runrs

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/runrs

clean:
	rm -rf $(BUILD_DIR)

.PHONY: release debug install uninstall clean
