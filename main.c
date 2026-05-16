#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <wayland-client.h>
#include <poll.h>
#include <errno.h>
#include <ctype.h>
#include <dirent.h>
#include <signal.h>
#include "wlr-layer-shell-unstable-v1-client.h"
#include <cairo.h>
#include <pango/pangocairo.h>
#include <xkbcommon/xkbcommon.h>
#include <librsvg/rsvg.h>

#define MAX_ENTRIES 512
#define ICON_SIZE 28
#define ROW_HEIGHT 40
#define SEARCH_H 36
#define PAD 10

struct entry {
    char name[128];
    char exec[256];
    char icon_name[64];
    cairo_surface_t *icon;
};

struct state {
    struct wl_display *display;
    struct wl_compositor *compositor;
    struct wl_shm *shm;
    struct zwlr_layer_shell_v1 *layer_shell;
    struct wl_surface *surface;
    struct zwlr_layer_surface_v1 *layer_surface;
    struct wl_seat *seat;
    struct wl_pointer *pointer;
    struct wl_keyboard *keyboard;
    struct xkb_context *xkb_ctx;
    struct xkb_keymap *xkb_keymap;
    struct xkb_state *xkb_state;
    xkb_keycode_t kc_esc, kc_bksp, kc_ret, kc_up, kc_down;

    struct wl_buffer *buffer;
    cairo_surface_t *cairo_surface;
    cairo_t *cr;
    void *shm_data;
    int width, height;
    bool configured;

    struct entry entries[MAX_ENTRIES];
    int n_entries;
    int filtered[MAX_ENTRIES];
    int n_filtered;
    int scroll_offset;
    char search[64];
    int hovered_idx;
    int pointer_x, pointer_y;
    struct wl_surface *current_pointer_surface;
};

static cairo_surface_t *load_png(const char *base, const char *name)
{
    char path[512];
    snprintf(path, sizeof(path), "%s/%s.png", base, name);
    cairo_surface_t *img = cairo_image_surface_create_from_png(path);
    if (cairo_surface_status(img) == CAIRO_STATUS_SUCCESS)
        return img;
    cairo_surface_destroy(img);
    return NULL;
}

static cairo_surface_t *load_svg(const char *base, const char *name, int size)
{
    char path[512];
    snprintf(path, sizeof(path), "%s/%s.svg", base, name);
    GError *err = NULL;
    RsvgHandle *handle = rsvg_handle_new_from_file(path, &err);
    if (!handle) return NULL;

    cairo_surface_t *surface = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, size, size);
    cairo_t *cr = cairo_create(surface);

    RsvgRectangle viewport = {0, 0, (double)size, (double)size};
    gboolean ok = rsvg_handle_render_document(handle, cr, &viewport, &err);
    cairo_destroy(cr);
    g_object_unref(handle);

    if (!ok) {
        cairo_surface_destroy(surface);
        return NULL;
    }
    return surface;
}

static const char *icon_themes[] = {
    "hicolor", "Papirus", "Papirus-Dark", "Papirus-Light",
    "Adwaita", "gnome", "breeze", "breeze-dark",
    "Numix", "elementary-xfce", "Moka", "Faenza",
    "Humanity", "ubuntu-mono",
};

static cairo_surface_t *load_icon(const char *name)
{
    if (!name || !name[0]) return NULL;

    int sizes[] = {48, 64, 32, 128, 96, 72};

    for (size_t t = 0; t < sizeof(icon_themes)/sizeof(icon_themes[0]); t++) {
        for (size_t si = 0; si < sizeof(sizes)/sizeof(sizes[0]); si++) {
            char base[256];
            snprintf(base, sizeof(base),
                "/usr/share/icons/%s/%dx%d/apps", icon_themes[t], sizes[si], sizes[si]);
            cairo_surface_t *img = load_png(base, name);
            if (img) return img;
        }

        char base[256];
        snprintf(base, sizeof(base),
            "/usr/share/icons/%s/scalable/apps", icon_themes[t]);
        cairo_surface_t *img = load_svg(base, name, 48);
        if (img) return img;
    }

    cairo_surface_t *img = load_png("/usr/share/pixmaps", name);
    if (img) return img;

    return NULL;
}

static double scale_icon(cairo_surface_t *icon, int target)
{
    if (!icon) return 1.0;
    double iw = cairo_image_surface_get_width(icon);
    double ih = cairo_image_surface_get_height(icon);
    if (iw == 0 || ih == 0) return 1.0;
    double scale = (double)target / (iw > ih ? iw : ih);
    return scale;
}

static void execute_command(const char *cmd)
{
    pid_t pid = fork();
    if (pid == 0) {
        execl("/bin/sh", "sh", "-c", cmd, NULL);
        _exit(1);
    }
}

static int create_shm_fd(size_t size)
{
    int fd = memfd_create("launcher", MFD_CLOEXEC);
    if (fd < 0) return -1;
    if (ftruncate(fd, (off_t)size) < 0) { close(fd); return -1; }
    return fd;
}

static void draw_rounded_rect(cairo_t *cr, double x, double y, double w, double h, double r)
{
    if (r > h / 2) r = h / 2;
    if (r > w / 2) r = w / 2;
    cairo_move_to(cr, x + r, y);
    cairo_line_to(cr, x + w - r, y);
    cairo_arc(cr, x + w - r, y + r, r, -M_PI_2, 0);
    cairo_line_to(cr, x + w, y + h - r);
    cairo_arc(cr, x + w - r, y + h - r, r, 0, M_PI_2);
    cairo_line_to(cr, x + r, y + h);
    cairo_arc(cr, x + r, y + h - r, r, M_PI_2, M_PI);
    cairo_line_to(cr, x, y + r);
    cairo_arc(cr, x + r, y + r, r, M_PI, 3 * M_PI_2);
    cairo_close_path(cr);
}

static void update_filter(struct state *st)
{
    st->n_filtered = 0;
    st->scroll_offset = 0;
    for (int i = 0; i < st->n_entries && st->n_filtered < MAX_ENTRIES; i++) {
        if (st->search[0] == '\0' ||
            strcasestr(st->entries[i].name, st->search)) {
            st->filtered[st->n_filtered++] = i;
        }
    }
}

static char *read_desktop_field(const char *content, const char *field)
{
    char *pat;
    if (asprintf(&pat, "\n%s=", field) < 0) return NULL;
    const char *val = strstr(content, pat + 1);
    if (!val) val = strstr(content, pat);
    if (!val) { free(pat); return NULL; }
    free(pat);
    val = strchr(val, '=');
    if (!val) return NULL;
    val++;
    const char *nl = strchr(val, '\n');
    size_t len = nl ? (size_t)(nl - val) : strlen(val);
    char *result = malloc(len + 1);
    if (result) {
        memcpy(result, val, len);
        result[len] = '\0';
    }
    return result;
}

static void populate_entries(struct state *st)
{
    st->n_entries = 0;
    DIR *dir = opendir("/usr/share/applications");
    if (!dir) return;

    struct dirent *de;
    while ((de = readdir(dir)) != NULL && st->n_entries < MAX_ENTRIES) {
        char path[512];
        if (de->d_type == DT_UNKNOWN) {
            struct stat st_buf;
            snprintf(path, sizeof(path), "/usr/share/applications/%s", de->d_name);
            if (stat(path, &st_buf) != 0 || !S_ISREG(st_buf.st_mode)) continue;
        } else if (de->d_type != DT_REG && de->d_type != DT_LNK) continue;
        char *dot = strrchr(de->d_name, '.');
        if (!dot || strcmp(dot, ".desktop") != 0) continue;

        snprintf(path, sizeof(path), "/usr/share/applications/%s", de->d_name);

        FILE *f = fopen(path, "r");
        if (!f) continue;
        fseek(f, 0, SEEK_END);
        long fsize = ftell(f);
        if (fsize <= 0) { fclose(f); continue; }
        rewind(f);

        char *buf = malloc(fsize + 2);
        if (!buf) { fclose(f); continue; }
        size_t nread = fread(buf, 1, fsize, f);
        buf[nread] = '\n';
        buf[nread + 1] = '\0';
        fclose(f);

        int hidden = 0;
        char *hidden_val = read_desktop_field(buf, "NoDisplay");
        if (hidden_val) {
            hidden = (strcasecmp(hidden_val, "true") == 0);
            free(hidden_val);
        }
        if (!hidden) {
            char *hv2 = read_desktop_field(buf, "Hidden");
            if (hv2) {
                hidden = (strcasecmp(hv2, "true") == 0);
                free(hv2);
            }
        }
        if (hidden) { free(buf); continue; }

        char *type = read_desktop_field(buf, "Type");
        if (!type || strcmp(type, "Application") != 0) {
            free(type); free(buf);
            continue;
        }
        free(type);

        char *name = read_desktop_field(buf, "Name");
        char *exec = read_desktop_field(buf, "Exec");
        char *icon = read_desktop_field(buf, "Icon");

        if (name && exec && name[0] && exec[0]) {
            int i = st->n_entries++;
            snprintf(st->entries[i].name, sizeof(st->entries[i].name), "%s", name);
            char exec_clean[256];
            char *dst = exec_clean;
            for (char *src = exec; *src; src++) {
                if (*src == '%' && *(src+1)) {
                    src++;
                    continue;
                }
                if (dst - exec_clean < (int)sizeof(exec_clean) - 1)
                    *dst++ = *src;
            }
            *dst = '\0';
            snprintf(st->entries[i].exec, sizeof(st->entries[i].exec), "%s", exec_clean);
            if (icon)
                snprintf(st->entries[i].icon_name, sizeof(st->entries[i].icon_name), "%s", icon);
            st->entries[i].icon = load_icon(st->entries[i].icon_name);
        }

        free(name);
        free(exec);
        free(icon);
        free(buf);
    }
    closedir(dir);
    update_filter(st);
}

static void destroy_buffer(struct state *st)
{
    if (st->cr) cairo_destroy(st->cr);
    st->cr = NULL;
    if (st->cairo_surface) cairo_surface_destroy(st->cairo_surface);
    st->cairo_surface = NULL;
    if (st->shm_data) {
        int stride = cairo_format_stride_for_width(CAIRO_FORMAT_ARGB32, st->width);
        munmap(st->shm_data, stride * st->height);
    }
    st->shm_data = NULL;
    if (st->buffer) wl_buffer_destroy(st->buffer);
    st->buffer = NULL;
}

static int create_buffer(struct state *st)
{
    if (st->buffer) return 0;
    int stride = cairo_format_stride_for_width(CAIRO_FORMAT_ARGB32, st->width);
    int size = stride * st->height;
    int fd = create_shm_fd(size);
    if (fd < 0) return -1;

    struct wl_shm_pool *pool = wl_shm_create_pool(st->shm, fd, size);
    st->buffer = wl_shm_pool_create_buffer(pool, 0, st->width, st->height,
        stride, WL_SHM_FORMAT_ARGB8888);
    wl_shm_pool_destroy(pool);

    st->shm_data = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    if (st->shm_data == MAP_FAILED) return -1;

    st->cairo_surface = cairo_image_surface_create_for_data(
        st->shm_data, CAIRO_FORMAT_ARGB32, st->width, st->height, stride);
    st->cr = cairo_create(st->cairo_surface);
    return 0;
}

static void render(struct state *st)
{
    cairo_t *cr = st->cr;
    int w = st->width, h = st->height;

    cairo_set_operator(cr, CAIRO_OPERATOR_CLEAR);
    cairo_paint(cr);
    cairo_set_operator(cr, CAIRO_OPERATOR_OVER);

    draw_rounded_rect(cr, 0, 0, w, h, 8);
    cairo_set_source_rgba(cr, 0.04, 0.03, 0.10, 0.96);
    cairo_fill(cr);

    float acc[] = {0.0f, 0.90f, 1.0f, 1.0f};
    cairo_set_source_rgba(cr, acc[0], acc[1], acc[2], 0.25);
    cairo_set_line_width(cr, 1);
    draw_rounded_rect(cr, 0, 0, w, h, 8);
    cairo_stroke(cr);

    int sbx = PAD, sbw = w - PAD * 2;
    draw_rounded_rect(cr, sbx, 8, sbw, SEARCH_H, 6);
    cairo_set_source_rgba(cr, 0.08, 0.06, 0.18, 0.9);
    cairo_fill(cr);
    cairo_set_source_rgba(cr, acc[0], acc[1], acc[2], 0.4);
    cairo_set_line_width(cr, 1);
    draw_rounded_rect(cr, sbx, 8, sbw, SEARCH_H, 6);
    cairo_stroke(cr);

    char disp[128];
    if (st->search[0] == '\0')
        snprintf(disp, sizeof(disp), "  Type to search...");
    else
        snprintf(disp, sizeof(disp), "  %s_", st->search);

    PangoFontDescription *fd_s = pango_font_description_from_string("Sans 12");
    PangoLayout *lay_s = pango_cairo_create_layout(cr);
    pango_layout_set_font_description(lay_s, fd_s);
    pango_layout_set_text(lay_s, disp, -1);
    int stw, sth;
    pango_layout_get_pixel_size(lay_s, &stw, &sth);
    cairo_set_source_rgb(cr, 0.7, 0.75, 1.0);
    cairo_move_to(cr, sbx + 4, 8 + (SEARCH_H - sth) / 2);
    pango_cairo_show_layout(cr, lay_s);
    g_object_unref(lay_s);

    int n_results = st->n_filtered;
    int max_visible = (h - SEARCH_H - 8 - 4) / ROW_HEIGHT;
    if (n_results > max_visible) n_results = max_visible;
    int skip = st->scroll_offset;

    PangoFontDescription *fd_n = pango_font_description_from_string("Sans 12");
    PangoFontDescription *fd_sub = pango_font_description_from_string("Monospace 9");
    PangoLayout *lay_n = pango_cairo_create_layout(cr);
    pango_layout_set_font_description(lay_n, fd_n);
    PangoLayout *lay_sub = pango_cairo_create_layout(cr);
    pango_layout_set_font_description(lay_sub, fd_sub);

    float acc_dim[] = {acc[0]*0.5f, acc[1]*0.5f, acc[2]*0.5f, 0.3f};

    for (int j = 0; j < n_results; j++) {
        int idx = skip + j;
        if (idx >= st->n_filtered) break;
        int i = st->filtered[idx];
        int ry = SEARCH_H + 8 + 4 + j * ROW_HEIGHT;
        bool hovered = (i == st->hovered_idx);

        if (hovered) {
            cairo_set_source_rgba(cr, acc[0], acc[1], acc[2], 0.12);
            draw_rounded_rect(cr, 2, ry, w - 4, ROW_HEIGHT - 2, 6);
            cairo_fill(cr);
        }

        if (st->entries[i].icon) {
            double s = scale_icon(st->entries[i].icon, ICON_SIZE);
            int ih = cairo_image_surface_get_height(st->entries[i].icon);
            int dx = PAD + 2;
            int dy = ry + (ROW_HEIGHT - (int)(ih * s)) / 2;
            cairo_save(cr);
            cairo_translate(cr, dx, dy);
            cairo_scale(cr, s, s);
            cairo_set_source_surface(cr, st->entries[i].icon, 0, 0);
            cairo_paint(cr);
            cairo_restore(cr);
        } else {
            cairo_set_source_rgba(cr, acc_dim[0], acc_dim[1], acc_dim[2], 0.5);
            double cx = PAD + 2 + ICON_SIZE / 2.0;
            double cy = ry + ROW_HEIGHT / 2.0;
            cairo_arc(cr, cx, cy, ICON_SIZE / 2.0 - 2, 0, 2 * M_PI);
            cairo_fill(cr);
        }

        int tx = PAD + 2 + ICON_SIZE + 10;
        pango_layout_set_text(lay_n, st->entries[i].name, -1);
        int nw, nh;
        pango_layout_get_pixel_size(lay_n, &nw, &nh);
        cairo_set_source_rgb(cr, 0.9, 0.92, 1.0);
        cairo_move_to(cr, tx, ry + (ROW_HEIGHT - nh) / 2);
        pango_cairo_show_layout(cr, lay_n);

        if (st->entries[i].exec[0]) {
            int ex_w = w - tx - PAD;
            pango_layout_set_text(lay_sub, st->entries[i].exec, -1);
            pango_layout_set_width(lay_sub, ex_w * PANGO_SCALE);
            pango_layout_set_ellipsize(lay_sub, PANGO_ELLIPSIZE_MIDDLE);
            pango_layout_set_alignment(lay_sub, PANGO_ALIGN_RIGHT);
            int sw, sh;
            pango_layout_get_pixel_size(lay_sub, &sw, &sh);
            cairo_set_source_rgba(cr, 0.5, 0.55, 0.7, 0.6);
            cairo_move_to(cr, tx, ry + (ROW_HEIGHT - sh) / 2);
            pango_cairo_show_layout(cr, lay_sub);

            cairo_set_source_rgba(cr, acc[0], acc[1], acc[2], 0.1);
            cairo_set_line_width(cr, 0.5);
            cairo_move_to(cr, PAD, ry + ROW_HEIGHT - 1);
            cairo_line_to(cr, w - PAD, ry + ROW_HEIGHT - 1);
            cairo_stroke(cr);
        }
    }

    g_object_unref(lay_n);
    g_object_unref(lay_sub);
    pango_font_description_free(fd_n);
    pango_font_description_free(fd_sub);
    pango_font_description_free(fd_s);

    cairo_surface_flush(st->cairo_surface);
    wl_surface_attach(st->surface, st->buffer, 0, 0);
    wl_surface_damage_buffer(st->surface, 0, 0, w, h);
    wl_surface_commit(st->surface);
}

static void destroy(struct state *st)
{
    destroy_buffer(st);
    if (st->layer_surface) {
        zwlr_layer_surface_v1_destroy(st->layer_surface);
        st->layer_surface = NULL;
    }
    if (st->surface) {
        wl_surface_destroy(st->surface);
        st->surface = NULL;
    }
    st->configured = false;
}

static bool running;

static void layer_surface_configure(void *data,
    struct zwlr_layer_surface_v1 *surface, uint32_t serial,
    uint32_t width, uint32_t height)
{
    struct state *st = data;
    zwlr_layer_surface_v1_ack_configure(surface, serial);
    if (width > 0 && height > 0) {
        st->width = width;
        st->height = height;
    }
    if (!st->buffer) {
        if (create_buffer(st) != 0) {
            fprintf(stderr, "failed to create buffer\n");
            running = false;
            return;
        }
        render(st);
    }
    st->configured = true;
}

static void layer_surface_closed(void *data,
    struct zwlr_layer_surface_v1 *surface)
{
    running = false;
}

static const struct zwlr_layer_surface_v1_listener layer_surface_listener = {
    .configure = layer_surface_configure,
    .closed = layer_surface_closed,
};

static void pointer_enter(void *data, struct wl_pointer *pointer,
    uint32_t serial, struct wl_surface *surface,
    wl_fixed_t sx, wl_fixed_t sy)
{
    struct state *st = data;
    st->current_pointer_surface = surface;
    st->pointer_x = wl_fixed_to_int(sx);
    st->pointer_y = wl_fixed_to_int(sy);
}

static void pointer_leave(void *data, struct wl_pointer *pointer,
    uint32_t serial, struct wl_surface *surface)
{
    struct state *st = data;
    st->current_pointer_surface = NULL;
    if (st->hovered_idx != -1) {
        st->hovered_idx = -1;
        render(st);
    }
}

static void pointer_motion(void *data, struct wl_pointer *pointer,
    uint32_t time, wl_fixed_t sx, wl_fixed_t sy)
{
    struct state *st = data;
    int x = wl_fixed_to_int(sx);
    int y = wl_fixed_to_int(sy);
    st->pointer_x = x;
    st->pointer_y = y;

    if (!st->current_pointer_surface || st->current_pointer_surface != st->surface)
        return;

    int old = st->hovered_idx;
    st->hovered_idx = -1;
    int skip = st->scroll_offset;
    int max_visible = (st->height - SEARCH_H - 8 - 4) / ROW_HEIGHT;
    for (int j = 0; j < max_visible; j++) {
        int idx = skip + j;
        if (idx >= st->n_filtered) break;
        int ry = SEARCH_H + 8 + 4 + j * ROW_HEIGHT;
        if (y >= ry && y < ry + ROW_HEIGHT) {
            st->hovered_idx = st->filtered[idx];
            break;
        }
    }
    if (old != st->hovered_idx)
        render(st);
}

static void pointer_button(void *data, struct wl_pointer *pointer,
    uint32_t serial, uint32_t time, uint32_t button, uint32_t state)
{
    struct state *st = data;
    if (state != 1 || button != 0x110) return;
    if (st->current_pointer_surface != st->surface) return;

    if (st->hovered_idx >= 0 && st->hovered_idx < st->n_entries) {
        execute_command(st->entries[st->hovered_idx].exec);
        running = false;
    }
}

static void pointer_axis(void *data, struct wl_pointer *pointer,
    uint32_t time, uint32_t axis, wl_fixed_t value)
{
    struct state *st = data;
    if (axis != 0) return;
    int delta = wl_fixed_to_int(value);
    if (delta == 0) return;

    int max_visible = (st->height - SEARCH_H - 8 - 4) / ROW_HEIGHT;
    int max_scroll = st->n_filtered - max_visible;
    if (max_scroll < 0) max_scroll = 0;

    int old = st->scroll_offset;
    st->scroll_offset -= delta;
    if (st->scroll_offset < 0) st->scroll_offset = 0;
    if (st->scroll_offset > max_scroll) st->scroll_offset = max_scroll;
    if (old != st->scroll_offset) {
        st->hovered_idx = -1;
        render(st);
    }
}

static void pointer_frame(void *data, struct wl_pointer *pointer) {}

static void keyboard_keymap(void *data, struct wl_keyboard *kb,
    uint32_t format, int fd, uint32_t size)
{
    struct state *st = data;
    if (format != WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1) { close(fd); return; }
    char *map = mmap(NULL, size, PROT_READ, MAP_PRIVATE, fd, 0);
    if (map == MAP_FAILED) { close(fd); return; }
    st->xkb_keymap = xkb_keymap_new_from_string(st->xkb_ctx, map,
        XKB_KEYMAP_FORMAT_TEXT_V1, 0);
    munmap(map, size);
    close(fd);
    if (!st->xkb_keymap) return;
    st->xkb_state = xkb_state_new(st->xkb_keymap);
    st->kc_esc  = xkb_keymap_key_by_name(st->xkb_keymap, "ESC");
    st->kc_bksp = xkb_keymap_key_by_name(st->xkb_keymap, "BKSP");
    st->kc_ret  = xkb_keymap_key_by_name(st->xkb_keymap, "RTRN");
    st->kc_up   = xkb_keymap_key_by_name(st->xkb_keymap, "UP");
    st->kc_down = xkb_keymap_key_by_name(st->xkb_keymap, "DOWN");
}

static void keyboard_enter(void *data, struct wl_keyboard *kb,
    uint32_t serial, struct wl_surface *surface, struct wl_array *keys) {}

static void keyboard_leave(void *data, struct wl_keyboard *kb,
    uint32_t serial, struct wl_surface *surface) {}

static void keyboard_key(void *data, struct wl_keyboard *kb,
    uint32_t serial, uint32_t time, uint32_t key, uint32_t state)
{
    struct state *st = data;
    if (!st->xkb_state) return;
    xkb_state_update_key(st->xkb_state, key, state ? XKB_KEY_DOWN : XKB_KEY_UP);
    if (state != 1) return;

    if (st->kc_esc != XKB_KEYCODE_INVALID && key == st->kc_esc) {
        running = false;
        return;
    }
    if (st->kc_ret != XKB_KEYCODE_INVALID && key == st->kc_ret) {
        int idx = (st->hovered_idx >= 0) ? st->hovered_idx :
                   (st->n_filtered > 0 ? st->filtered[0] : -1);
        if (idx >= 0) {
            execute_command(st->entries[idx].exec);
            running = false;
        }
        return;
    }
    if (st->kc_bksp != XKB_KEYCODE_INVALID && key == st->kc_bksp) {
        int len = strlen(st->search);
        if (len > 0) st->search[len - 1] = '\0';
        update_filter(st);
        render(st);
        return;
    }
    if (st->kc_up != XKB_KEYCODE_INVALID && key == st->kc_up) {
        if (st->n_filtered == 0) return;
        int cur = -1;
        for (int j = 0; j < st->n_filtered; j++) {
            if (st->filtered[j] == st->hovered_idx) { cur = j; break; }
        }
        if (cur > 0) {
            st->hovered_idx = st->filtered[cur - 1];
        } else if (cur <= 0 && st->scroll_offset > 0) {
            st->scroll_offset--;
            st->hovered_idx = st->filtered[st->scroll_offset];
        }
        render(st);
        return;
    }
    if (st->kc_down != XKB_KEYCODE_INVALID && key == st->kc_down) {
        if (st->n_filtered == 0) return;
        int cur = -1;
        for (int j = 0; j < st->n_filtered; j++) {
            if (st->filtered[j] == st->hovered_idx) { cur = j; break; }
        }
        int max_visible = (st->height - SEARCH_H - 8 - 4) / ROW_HEIGHT;
        if (cur < st->n_filtered - 1) {
            int new_idx = cur + 1;
            st->hovered_idx = st->filtered[new_idx];
            if (new_idx - st->scroll_offset >= max_visible)
                st->scroll_offset = new_idx - max_visible + 1;
        } else if (st->scroll_offset + max_visible < st->n_filtered) {
            st->scroll_offset++;
        }
        render(st);
        return;
    }

    char buf[8];
    int n = xkb_state_key_get_utf8(st->xkb_state, key, buf, sizeof(buf));
    if (n > 0) {
        gunichar uc = g_utf8_get_char_validated(buf, n);
        if (uc > 0 && g_unichar_isprint(uc)) {
            int len = strlen(st->search);
            if (len + n < (int)sizeof(st->search) - 1) {
                memcpy(st->search + len, buf, n);
                st->search[len + n] = '\0';
            }
            update_filter(st);
            st->hovered_idx = st->n_filtered > 0 ? st->filtered[0] : -1;
            render(st);
        }
    }
}

static void keyboard_modifiers(void *data, struct wl_keyboard *kb,
    uint32_t serial, uint32_t mods_depressed, uint32_t mods_latched,
    uint32_t mods_locked, uint32_t group)
{
    struct state *st = data;
    if (st->xkb_state)
        xkb_state_update_mask(st->xkb_state, mods_depressed, mods_latched, mods_locked, 0, 0, group);
}

static void keyboard_repeat_info(void *data, struct wl_keyboard *kb,
    int32_t rate, int32_t delay) {}

static const struct wl_keyboard_listener keyboard_listener = {
    .keymap = keyboard_keymap,
    .enter = keyboard_enter,
    .leave = keyboard_leave,
    .key = keyboard_key,
    .modifiers = keyboard_modifiers,
    .repeat_info = keyboard_repeat_info,
};

static const struct wl_pointer_listener pointer_listener = {
    .enter = pointer_enter,
    .leave = pointer_leave,
    .motion = pointer_motion,
    .button = pointer_button,
    .axis = pointer_axis,
    .frame = pointer_frame,
};

static void seat_capabilities(void *data, struct wl_seat *seat,
    uint32_t capabilities)
{
    struct state *st = data;
    if ((capabilities & WL_SEAT_CAPABILITY_POINTER) && !st->pointer) {
        st->pointer = wl_seat_get_pointer(seat);
        wl_pointer_add_listener(st->pointer, &pointer_listener, st);
    } else if (!(capabilities & WL_SEAT_CAPABILITY_POINTER) && st->pointer) {
        wl_pointer_destroy(st->pointer);
        st->pointer = NULL;
    }
    if ((capabilities & WL_SEAT_CAPABILITY_KEYBOARD) && !st->keyboard) {
        st->keyboard = wl_seat_get_keyboard(seat);
        wl_keyboard_add_listener(st->keyboard, &keyboard_listener, st);
    } else if (!(capabilities & WL_SEAT_CAPABILITY_KEYBOARD) && st->keyboard) {
        wl_keyboard_destroy(st->keyboard);
        st->keyboard = NULL;
    }
}

static void seat_name(void *data, struct wl_seat *seat, const char *name) {}

static const struct wl_seat_listener seat_listener = {
    .capabilities = seat_capabilities,
    .name = seat_name,
};

static void registry_global(void *data, struct wl_registry *registry,
    uint32_t name, const char *interface, uint32_t version)
{
    struct state *st = data;
    if (strcmp(interface, wl_compositor_interface.name) == 0)
        st->compositor = wl_registry_bind(registry, name,
            &wl_compositor_interface, 4);
    else if (strcmp(interface, wl_shm_interface.name) == 0)
        st->shm = wl_registry_bind(registry, name,
            &wl_shm_interface, 1);
    else if (strcmp(interface, zwlr_layer_shell_v1_interface.name) == 0)
        st->layer_shell = wl_registry_bind(registry, name,
            &zwlr_layer_shell_v1_interface, 4);
    else if (strcmp(interface, wl_seat_interface.name) == 0) {
        st->seat = wl_registry_bind(registry, name,
            &wl_seat_interface, 7);
        wl_seat_add_listener(st->seat, &seat_listener, st);
    }
}

static void registry_global_remove(void *data,
    struct wl_registry *registry, uint32_t name) {}

static const struct wl_registry_listener registry_listener = {
    .global = registry_global,
    .global_remove = registry_global_remove,
};

int main(int argc, char *argv[])
{
    struct state st = {0};
    st.hovered_idx = -1;
    st.width = 600;
    st.height = 400;

    st.display = wl_display_connect(NULL);
    if (!st.display) {
        fprintf(stderr, "failed to connect to wayland display\n");
        return 1;
    }

    struct wl_registry *registry = wl_display_get_registry(st.display);
    wl_registry_add_listener(registry, &registry_listener, &st);
    wl_display_roundtrip(st.display);
    wl_registry_destroy(registry);

    if (!st.compositor || !st.shm || !st.layer_shell) {
        fprintf(stderr, "missing required wayland globals\n");
        return 1;
    }

    st.xkb_ctx = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    if (!st.xkb_ctx)
        fprintf(stderr, "warning: failed to create xkb context (keyboard input disabled)\n");

    populate_entries(&st);

    signal(SIGCHLD, SIG_IGN);

    st.surface = wl_compositor_create_surface(st.compositor);
    if (!st.surface) return 1;

    st.layer_surface = zwlr_layer_shell_v1_get_layer_surface(
        st.layer_shell, st.surface, NULL,
        ZWLR_LAYER_SHELL_V1_LAYER_OVERLAY, "launcher");

    zwlr_layer_surface_v1_add_listener(st.layer_surface,
        &layer_surface_listener, &st);

    int n_results = st.n_filtered;
    int max_visible = 10;
    if (n_results > max_visible) n_results = max_visible;
    int list_h = n_results * ROW_HEIGHT;
    int total_h = SEARCH_H + 8 + 4 + list_h + 8;
    if (total_h > 600) total_h = 600;
    if (total_h < 100) total_h = 100;

    zwlr_layer_surface_v1_set_size(st.layer_surface, 600, total_h);
    zwlr_layer_surface_v1_set_anchor(st.layer_surface,
        ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT | ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT |
        ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP | ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM);
    zwlr_layer_surface_v1_set_keyboard_interactivity(st.layer_surface,
        ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_EXCLUSIVE);
    zwlr_layer_surface_v1_set_exclusive_zone(st.layer_surface, 0);

    wl_surface_commit(st.surface);
    wl_display_roundtrip(st.display);

    if (!st.configured) {
        fprintf(stderr, "surface was not configured\n");
        return 1;
    }

    if (st.n_filtered > 0)
        st.hovered_idx = st.filtered[0];

    running = true;
    while (running) {
        struct pollfd fd = {
            .fd = wl_display_get_fd(st.display),
            .events = POLLIN,
        };
        while (wl_display_prepare_read(st.display) != 0)
            wl_display_dispatch_pending(st.display);
        wl_display_flush(st.display);

        if (poll(&fd, 1, -1) < 0) {
            if (errno == EINTR) {
                wl_display_cancel_read(st.display);
                continue;
            }
            break;
        }

        if (fd.revents & POLLIN)
            wl_display_read_events(st.display);
        else
            wl_display_cancel_read(st.display);

        wl_display_dispatch_pending(st.display);
    }

    if (st.keyboard) wl_keyboard_destroy(st.keyboard);
    if (st.pointer) wl_pointer_destroy(st.pointer);
    if (st.seat) wl_seat_destroy(st.seat);
    destroy(&st);
    if (st.layer_shell) zwlr_layer_shell_v1_destroy(st.layer_shell);
    if (st.compositor) wl_compositor_destroy(st.compositor);
    if (st.shm) wl_shm_destroy(st.shm);
    for (int i = 0; i < st.n_entries; i++)
        if (st.entries[i].icon)
            cairo_surface_destroy(st.entries[i].icon);
    wl_display_disconnect(st.display);
    return 0;
}
