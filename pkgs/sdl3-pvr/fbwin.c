// SPDX-License-Identifier: GPL-2.0-or-later
// The fb* "native window" calls SDL's Vivante video backend expects from a
// Vivante libEGL, implemented for the PowerVR stock userspace: its NULL
// WSEGL takes EGL_DEFAULT_DISPLAY and a NULL native window (it refuses the
// `struct fbdev_window *` SDL2's Mali backend passed) and presents
// full-screen through the DC driver (brick_dc_drm). The panel size comes
// from the DRM fbdev emulation.
#include <fcntl.h>
#include <linux/fb.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <unistd.h>

static void panel_size(int *w, int *h)
{
	struct fb_var_screeninfo v;
	int fd = open("/dev/fb0", O_RDONLY | O_CLOEXEC);
	*w = 1024, *h = 768;
	if (fd >= 0) {
		if (ioctl(fd, FBIOGET_VSCREENINFO, &v) == 0 && v.xres && v.yres)
			*w = v.xres, *h = v.yres;
		close(fd);
	}
}

void *fbGetDisplay(void *context) { (void)context; return NULL; } // EGL_DEFAULT_DISPLAY
void *fbGetDisplayByIndex(int index) { (void)index; return NULL; }
void fbDestroyDisplay(void *display) { (void)display; }

void fbGetDisplayGeometry(void *display, int *w, int *h)
{
	(void)display;
	panel_size(w, h);
}

void fbGetDisplayInfo(void *display, int *w, int *h, unsigned long *physical, int *stride, int *bpp)
{
	(void)display;
	panel_size(w, h);
	if (physical) *physical = 0;
	if (stride) *stride = *w * 4;
	if (bpp) *bpp = 32;
}

// The DC driver scans out full-panel buffers, so every window is the panel,
// and its native handle is NULL (SDL is patched to accept that).
void *fbCreateWindow(void *display, int x, int y, int w, int h)
{
	(void)display, (void)x, (void)y, (void)w, (void)h;
	return NULL;
}

void fbGetWindowGeometry(void *window, int *x, int *y, int *w, int *h)
{
	int pw, ph;
	(void)window;
	panel_size(&pw, &ph);
	if (x) *x = 0;
	if (y) *y = 0;
	if (w) *w = pw;
	if (h) *h = ph;
}

void fbGetWindowInfo(void *window, int *x, int *y, int *w, int *h, int *bpp, unsigned int *offset)
{
	fbGetWindowGeometry(window, x, y, w, h);
	if (bpp) *bpp = 32;
	if (offset) *offset = 0;
}

void fbDestroyWindow(void *window) { (void)window; }
