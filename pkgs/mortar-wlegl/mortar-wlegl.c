// SPDX-License-Identifier: GPL-2.0-or-later
/*
 * mortar-wlegl: a Wayland EGL platform for the stock PowerVR EGL (DDK 1.19).
 *
 * The stock userspace has only the NULL WSEGL: window surfaces present
 * through the display-class driver (brick_dc_drm), which drives the CRTC
 * itself and so cannot be used under a compositor. This library sits in
 * front of the stock libEGL/libGLESv2 and gives EGL_PLATFORM_WAYLAND
 * window surfaces (wl_egl_window) to Wayland clients:
 *
 *  - eglGetPlatformDisplay(WAYLAND, wl_display) returns the stock default
 *    display and remembers the wl_display.
 *  - eglCreateWindowSurface(wl_egl_window) returns our own surface. Its
 *    buffers are dma-bufs from the sun4i KMS device (dumb buffers: CMA,
 *    physically contiguous, so the compositor can put them on a display
 *    plane), imported into the GPU as EGLImages
 *    (EGL_EXT_image_dma_buf_import) and into the compositor as wl_buffers
 *    (zwp_linux_dmabuf_v1, XRGB8888, linear).
 *  - The context is made current surfaceless (EGL_KHR_surfaceless_context)
 *    and "framebuffer 0" is redirected to an FBO: glBindFramebuffer(0),
 *    glGetIntegerv(FRAMEBUFFER_BINDING) and friends are wrapped.
 *  - eglSwapBuffers: glFinish (this DDK has no implicit dma-buf fencing),
 *    attach + damage + commit, then pick a released buffer.
 *
 * Orientation. GL's window origin is bottom-left; an FBO texture keeps GL
 * row 0 in memory row 0, so a plain FBO render is upside down for the
 * compositor. Three ways out, best first:
 *   topdown    the app renders the window like a render-target texture
 *              (top row first). Negotiated: glGetString(GL_EXTENSIONS)
 *              lists GL_MORTAR_window_topdown and the app calls
 *              glWindowTopDownMORTAR(GL_TRUE) (sdl3-pvr's GLES2 renderer
 *              does). Zero copies: the FBO renders into the dma-buf.
 *   blit       default: the app renders into a private renderbuffer; at
 *              swap it is blitted into the dma-buf with Y flipped
 *              (glBlitFramebuffer, GLES 3). One GPU copy, no CPU.
 *   transform  the FBO renders into the dma-buf and the surface gets
 *              buffer_transform FLIPPED_180 (a vertical flip). The
 *              compositor must composite (no direct scanout).
 * MORTAR_WLEGL_MODE=topdown|blit|transform forces one.
 *
 * Other env: MORTAR_WLEGL_DRM=/dev/dri/cardN (allocator device),
 * MORTAR_WLEGL_ALLOC=dumb|heap (dumb buffers, default; or the CMA dma-heap),
 * MORTAR_WLEGL_DEBUG=1 (log to stderr).
 *
 * Limits: one wl_display per process; a surface may be current in one
 * context at a time (switching contexts recreates the GL objects);
 * draw and read surfaces must be the same when one is ours; no MSAA.
 */
#define _GNU_SOURCE
#define EGL_NO_PLATFORM_SPECIFIC_TYPES 1
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <linux/dma-heap.h>

#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES3/gl3.h>
#include <GLES2/gl2ext.h>
#include <drm_fourcc.h>
#include <xf86drm.h>
#include <wayland-client.h>
#include <wayland-egl-backend.h>
#include "linux-dmabuf-unstable-v1-client-protocol.h"

#ifndef PVR_LIBDIR
#error "PVR_LIBDIR must point at the stock DDK lib directory"
#endif

#ifndef EGL_PLATFORM_SURFACELESS_MESA
#define EGL_PLATFORM_SURFACELESS_MESA 0x31DD
#endif
#ifndef EGL_BUFFER_AGE_EXT
#define EGL_BUFFER_AGE_EXT 0x313D
#endif

#define NBUF_MAX 4
#define MAGIC 0x6d776c65u /* "mwle" */

/* ------------------------------------------------------------ real libs */

static struct {
	void *egl, *gles;
	PFNEGLGETDISPLAYPROC GetDisplay;
	PFNEGLINITIALIZEPROC Initialize;
	PFNEGLQUERYSTRINGPROC QueryString;
	PFNEGLGETERRORPROC GetError;
	PFNEGLGETPROCADDRESSPROC GetProcAddress;
	PFNEGLCREATEWINDOWSURFACEPROC CreateWindowSurface;
	PFNEGLDESTROYSURFACEPROC DestroySurface;
	PFNEGLMAKECURRENTPROC MakeCurrent;
	PFNEGLSWAPBUFFERSPROC SwapBuffers;
	PFNEGLSWAPINTERVALPROC SwapInterval;
	PFNEGLQUERYSURFACEPROC QuerySurface;
	PFNEGLSURFACEATTRIBPROC SurfaceAttrib;
	PFNEGLGETCURRENTSURFACEPROC GetCurrentSurface;
	PFNEGLGETCURRENTCONTEXTPROC GetCurrentContext;
	PFNEGLGETCONFIGATTRIBPROC GetConfigAttrib;
	PFNEGLCHOOSECONFIGPROC ChooseConfig;
	PFNEGLCREATECONTEXTPROC CreateContext;
	PFNEGLDESTROYCONTEXTPROC DestroyContext;
	PFNEGLRELEASETHREADPROC ReleaseThread;
	PFNEGLCOPYBUFFERSPROC CopyBuffers;
	PFNEGLBINDTEXIMAGEPROC BindTexImage;
	PFNEGLRELEASETEXIMAGEPROC ReleaseTexImage;
	PFNEGLCREATEIMAGEKHRPROC CreateImageKHR;
	PFNEGLDESTROYIMAGEKHRPROC DestroyImageKHR;
	PFNEGLSWAPBUFFERSWITHDAMAGEKHRPROC SwapBuffersWithDamageKHR;
	PFNEGLSWAPBUFFERSWITHDAMAGEEXTPROC SwapBuffersWithDamageEXT;
	PFNGLEGLIMAGETARGETTEXTURE2DOESPROC ImageTargetTexture2DOES;
	PFNGLBINDFRAMEBUFFERPROC glBindFramebuffer;
	PFNGLGETINTEGERVPROC glGetIntegerv;
	PFNGLGETSTRINGPROC glGetString;
	PFNGLDRAWBUFFERSPROC glDrawBuffers;
	PFNGLREADBUFFERPROC glReadBuffer;
	PFNGLINVALIDATEFRAMEBUFFERPROC glInvalidateFramebuffer;
	PFNGLDISCARDFRAMEBUFFEREXTPROC glDiscardFramebufferEXT;
	PFNGLBLITFRAMEBUFFERPROC glBlitFramebuffer;
} R;

static bool debug;
static pthread_once_t once = PTHREAD_ONCE_INIT;

static void dbg(const char *fmt, ...)
{
	va_list ap;
	if (!debug)
		return;
	va_start(ap, fmt);
	fputs("mortar-wlegl: ", stderr);
	vfprintf(stderr, fmt, ap);
	fputc('\n', stderr);
	va_end(ap);
}

static void warn(const char *fmt, ...)
{
	va_list ap;
	va_start(ap, fmt);
	fputs("mortar-wlegl: ", stderr);
	vfprintf(stderr, fmt, ap);
	fputc('\n', stderr);
	va_end(ap);
}

static void *open_lib(const char *name)
{
	char p[512];
	snprintf(p, sizeof(p), "%s/%s", PVR_LIBDIR, name);
	void *h = dlopen(p, RTLD_NOW | RTLD_LOCAL);
	if (!h)
		warn("dlopen %s: %s", p, dlerror());
	return h;
}

static void load_real(void)
{
	debug = getenv("MORTAR_WLEGL_DEBUG") && *getenv("MORTAR_WLEGL_DEBUG") && strcmp(getenv("MORTAR_WLEGL_DEBUG"), "0");
	R.egl = open_lib("libEGL.so.1");
	R.gles = open_lib("libGLESv2.so.2");
	if (!R.egl || !R.gles)
		abort();
#define E(n) R.n = (void *)dlsym(R.egl, "egl" #n)
#define G(n) R.n = (void *)dlsym(R.gles, #n)
	E(GetDisplay); E(Initialize); E(QueryString); E(GetError); E(GetProcAddress);
	E(CreateWindowSurface); E(DestroySurface); E(MakeCurrent); E(SwapBuffers);
	E(SwapInterval); E(QuerySurface); E(SurfaceAttrib); E(GetCurrentSurface);
	E(GetCurrentContext); E(GetConfigAttrib); E(ChooseConfig); E(CreateContext); E(DestroyContext); E(ReleaseThread);
	E(CopyBuffers); E(BindTexImage); E(ReleaseTexImage);
	G(glBindFramebuffer); G(glGetIntegerv); G(glGetString); G(glDrawBuffers);
	G(glReadBuffer); G(glInvalidateFramebuffer); G(glBlitFramebuffer);
#undef E
#undef G
	R.CreateImageKHR = (void *)R.GetProcAddress("eglCreateImageKHR");
	R.DestroyImageKHR = (void *)R.GetProcAddress("eglDestroyImageKHR");
	R.SwapBuffersWithDamageKHR = (void *)R.GetProcAddress("eglSwapBuffersWithDamageKHR");
	R.SwapBuffersWithDamageEXT = (void *)R.GetProcAddress("eglSwapBuffersWithDamageEXT");
	R.ImageTargetTexture2DOES = (void *)R.GetProcAddress("glEGLImageTargetTexture2DOES");
	R.glDiscardFramebufferEXT = (void *)R.GetProcAddress("glDiscardFramebufferEXT");
	if (!R.MakeCurrent || !R.glBindFramebuffer || !R.CreateImageKHR || !R.ImageTargetTexture2DOES) {
		warn("stock EGL/GLES lacks a required entry point");
		abort();
	}
}

static void init_once(void)
{
	pthread_once(&once, load_real);
}

/* ---------------------------------------------------------------- state */

enum mode { MODE_BLIT, MODE_TOPDOWN, MODE_TRANSFORM };

struct wsurf;

struct wbuf {
	struct wsurf *s;
	struct wl_buffer *wl; /* NULL: free slot */
	EGLImageKHR img;
	GLuint tex;           /* in s->ctx */
	int fd, w, h;
	uint32_t pitch;
	uint64_t frame;       /* s->frames when last presented, 0 = never */
	bool busy, stale;
};

struct wsurf {
	uint32_t magic;
	struct wsurf *next;
	struct wl_egl_window *win;
	struct wl_surface *surface;
	struct wl_surface *surf_wrap; /* on our queue, for frame callbacks */
	struct wl_callback *frame;
	EGLConfig cfg;
	int w, h, interval;
	struct wbuf buf[NBUF_MAX];
	struct wbuf *back;
	uint64_t frames;
	int depth, stencil;
	/* GL objects, in ctx */
	EGLContext ctx;
	bool es3, viewport_set;
	GLuint fbo, blit_fbo, color_rb, depth_rb;
	int color_w, color_h, depth_w, depth_h;
	bool color_valid, checked;
	enum mode mode;
	int transform; /* last buffer_transform sent */
};

static struct {
	pthread_mutex_t lock;
	struct wl_display *wl;
	bool own_wl;
	EGLDisplay dpy;
	struct wl_event_queue *q;
	struct wl_display *wrap;
	struct wl_registry *reg;
	struct zwp_linux_dmabuf_v1 *dmabuf;
	bool xrgb_linear, ready, failed;
	int alloc_fd;
	bool heap;
	struct wsurf *surfaces;
	struct {
		EGLContext ctx;
		bool topdown;
	} ctxflags[8];
	char *gl_ext;
} D = { .lock = PTHREAD_RECURSIVE_MUTEX_INITIALIZER_NP, .alloc_fd = -1 };

static __thread struct wsurf *cur;   /* our surface current on this thread */
static __thread EGLint err_pending;  /* error from our code (0: none) */

static void lock(void) { pthread_mutex_lock(&D.lock); }
static void unlock(void) { pthread_mutex_unlock(&D.lock); }

static EGLBoolean fail(EGLint e)
{
	err_pending = e;
	return EGL_FALSE;
}

static EGLBoolean ok(void)
{
	err_pending = EGL_SUCCESS;
	return EGL_TRUE;
}

static struct wsurf *ours(EGLSurface s)
{
	if (s == EGL_NO_SURFACE)
		return NULL;
	for (struct wsurf *w = D.surfaces; w; w = w->next)
		if ((EGLSurface)w == s)
			return w;
	return NULL;
}

static bool ctx_topdown(EGLContext c)
{
	for (size_t i = 0; i < sizeof(D.ctxflags) / sizeof(D.ctxflags[0]); i++)
		if (D.ctxflags[i].ctx == c)
			return D.ctxflags[i].topdown;
	return false;
}

static void set_ctx_topdown(EGLContext c, bool on)
{
	size_t n = sizeof(D.ctxflags) / sizeof(D.ctxflags[0]), freei = n;
	for (size_t i = 0; i < n; i++) {
		if (D.ctxflags[i].ctx == c) {
			D.ctxflags[i].topdown = on;
			return;
		}
		if (!D.ctxflags[i].ctx && freei == n)
			freei = i;
	}
	if (freei < n) {
		D.ctxflags[freei].ctx = c;
		D.ctxflags[freei].topdown = on;
	}
}

static enum mode forced_mode(bool *forced)
{
	const char *m = getenv("MORTAR_WLEGL_MODE");
	*forced = true;
	if (m && !strcmp(m, "topdown"))
		return MODE_TOPDOWN;
	if (m && !strcmp(m, "transform"))
		return MODE_TRANSFORM;
	if (m && !strcmp(m, "blit"))
		return MODE_BLIT;
	*forced = false;
	return MODE_BLIT;
}

/* ------------------------------------------------------------ allocator */

static int open_card(void)
{
	const char *p = getenv("MORTAR_WLEGL_DRM");
	if (p && *p)
		return open(p, O_RDWR | O_CLOEXEC);
	int fallback = -1;
	for (int i = 0; i < 8; i++) {
		char path[32];
		snprintf(path, sizeof(path), "/dev/dri/card%d", i);
		int fd = open(path, O_RDWR | O_CLOEXEC);
		if (fd < 0)
			continue;
		drmVersionPtr v = drmGetVersion(fd);
		uint64_t dumb = 0;
		bool sun4i = v && v->name && !strcmp(v->name, "sun4i-drm");
		bool can = drmGetCap(fd, DRM_CAP_DUMB_BUFFER, &dumb) == 0 && dumb;
		dbg("%s: %s dumb=%d", path, v && v->name ? v->name : "?", (int)dumb);
		if (v)
			drmFreeVersion(v);
		if (sun4i && can) {
			if (fallback >= 0)
				close(fallback);
			return fd;
		}
		if (can && fallback < 0)
			fallback = fd;
		else
			close(fd);
	}
	return fallback;
}

static bool open_alloc(void)
{
	const char *a = getenv("MORTAR_WLEGL_ALLOC");
	if (a && !strcmp(a, "heap")) {
		D.alloc_fd = open("/dev/dma_heap/linux,cma", O_RDWR | O_CLOEXEC);
		if (D.alloc_fd < 0)
			D.alloc_fd = open("/dev/dma_heap/reserved", O_RDWR | O_CLOEXEC);
		D.heap = D.alloc_fd >= 0;
		if (D.heap)
			return true;
		warn("no CMA dma-heap; using KMS dumb buffers");
	}
	D.alloc_fd = open_card();
	if (D.alloc_fd < 0)
		warn("no KMS device for dumb buffers: %s", strerror(errno));
	return D.alloc_fd >= 0;
}

static bool alloc_dmabuf(int w, int h, int *fd, uint32_t *pitch)
{
	if (D.heap) {
		uint32_t p = ((uint32_t)w * 4 + 63) & ~63u;
		struct dma_heap_allocation_data a = { .len = (uint64_t)p * h, .fd_flags = O_RDWR | O_CLOEXEC };
		if (ioctl(D.alloc_fd, DMA_HEAP_IOCTL_ALLOC, &a) < 0) {
			warn("dma-heap alloc %dx%d: %s", w, h, strerror(errno));
			return false;
		}
		*fd = (int)a.fd;
		*pitch = p;
		return true;
	}
	struct drm_mode_create_dumb c = { .width = (uint32_t)w, .height = (uint32_t)h, .bpp = 32 };
	if (drmIoctl(D.alloc_fd, DRM_IOCTL_MODE_CREATE_DUMB, &c) < 0) {
		warn("dumb buffer %dx%d: %s", w, h, strerror(errno));
		return false;
	}
	int r = drmPrimeHandleToFD(D.alloc_fd, c.handle, DRM_CLOEXEC | DRM_RDWR, fd);
	/* the dma-buf holds its own reference to the GEM object */
	struct drm_mode_destroy_dumb d = { .handle = c.handle };
	drmIoctl(D.alloc_fd, DRM_IOCTL_MODE_DESTROY_DUMB, &d);
	if (r < 0) {
		warn("PRIME export: %s", strerror(errno));
		return false;
	}
	*pitch = c.pitch;
	return true;
}

/* -------------------------------------------------------------- wayland */

static void dmabuf_format(void *data, struct zwp_linux_dmabuf_v1 *d, uint32_t format)
{
	(void)data, (void)d;
	if (format == DRM_FORMAT_XRGB8888 && wl_proxy_get_version((struct wl_proxy *)d) < 3)
		D.xrgb_linear = true;
}

static void dmabuf_modifier(void *data, struct zwp_linux_dmabuf_v1 *d, uint32_t format, uint32_t hi, uint32_t lo)
{
	(void)data, (void)d;
	uint64_t m = ((uint64_t)hi << 32) | lo;
	if (format == DRM_FORMAT_XRGB8888 && (m == DRM_FORMAT_MOD_LINEAR || m == DRM_FORMAT_MOD_INVALID))
		D.xrgb_linear = true;
}

static const struct zwp_linux_dmabuf_v1_listener dmabuf_listener = {
	.format = dmabuf_format,
	.modifier = dmabuf_modifier,
};

static void reg_global(void *data, struct wl_registry *r, uint32_t name, const char *iface, uint32_t ver)
{
	(void)data;
	if (!strcmp(iface, zwp_linux_dmabuf_v1_interface.name) && ver >= 2 && !D.dmabuf) {
		D.dmabuf = wl_registry_bind(r, name, &zwp_linux_dmabuf_v1_interface, ver < 3 ? ver : 3);
		zwp_linux_dmabuf_v1_add_listener(D.dmabuf, &dmabuf_listener, NULL);
	}
}

static void reg_remove(void *data, struct wl_registry *r, uint32_t name)
{
	(void)data, (void)r, (void)name;
}

static const struct wl_registry_listener reg_listener = { reg_global, reg_remove };

/* Bind linux-dmabuf on a private queue and open the allocator. */
static bool wl_setup(void)
{
	if (D.ready)
		return true;
	if (D.failed)
		return false;
	D.failed = true;
	D.q = wl_display_create_queue(D.wl);
	D.wrap = wl_proxy_create_wrapper(D.wl);
	if (!D.q || !D.wrap)
		return false;
	wl_proxy_set_queue((struct wl_proxy *)D.wrap, D.q);
	D.reg = wl_display_get_registry(D.wrap);
	wl_registry_add_listener(D.reg, &reg_listener, NULL);
	if (wl_display_roundtrip_queue(D.wl, D.q) < 0 || !D.dmabuf) {
		warn("compositor has no zwp_linux_dmabuf_v1 (version >= 2)");
		return false;
	}
	if (wl_display_roundtrip_queue(D.wl, D.q) < 0)
		return false;
	if (!D.xrgb_linear) {
		warn("compositor does not take linear XRGB8888 dma-bufs");
		return false;
	}
	if (!open_alloc())
		return false;
	D.failed = false;
	D.ready = true;
	dbg("ready: dmabuf v%u, allocator %s", wl_proxy_get_version((struct wl_proxy *)D.dmabuf), D.heap ? "dma-heap" : "dumb");
	return true;
}

static void buf_release(void *data, struct wl_buffer *b)
{
	(void)b;
	((struct wbuf *)data)->busy = false;
}

static const struct wl_buffer_listener buf_listener = { buf_release };

static void frame_done(void *data, struct wl_callback *cb, uint32_t t)
{
	(void)t;
	struct wsurf *s = data;
	wl_callback_destroy(cb);
	if (s->frame == cb)
		s->frame = NULL;
}

static const struct wl_callback_listener frame_listener = { frame_done };

/* ------------------------------------------------------------ GL helpers */

static GLint geti(GLenum p)
{
	GLint v = 0;
	R.glGetIntegerv(p, &v);
	return v;
}

/* We only touch GL while the surface's context is current on this thread. */
static bool gl_ok(struct wsurf *s)
{
	return s->ctx && cur == s && R.GetCurrentContext() == s->ctx;
}

static void buf_free(struct wsurf *s, struct wbuf *b)
{
	if (!b->wl)
		return;
	wl_buffer_destroy(b->wl);
	if (b->img)
		R.DestroyImageKHR(D.dpy, b->img);
	if (b->tex && gl_ok(s))
		glDeleteTextures(1, &b->tex);
	if (b->fd >= 0)
		close(b->fd);
	if (s->back == b)
		s->back = NULL;
	memset(b, 0, sizeof(*b));
	b->fd = -1;
}

static bool buf_alloc(struct wsurf *s, struct wbuf *b, int w, int h)
{
	int fd;
	uint32_t pitch;
	if (!alloc_dmabuf(w, h, &fd, &pitch))
		return false;
	EGLint at[] = {
		EGL_WIDTH, w, EGL_HEIGHT, h, EGL_LINUX_DRM_FOURCC_EXT, DRM_FORMAT_XRGB8888,
		EGL_DMA_BUF_PLANE0_FD_EXT, fd, EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
		EGL_DMA_BUF_PLANE0_PITCH_EXT, (EGLint)pitch, EGL_NONE,
	};
	EGLImageKHR img = R.CreateImageKHR(D.dpy, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, NULL, at);
	if (img == EGL_NO_IMAGE_KHR) {
		warn("eglCreateImageKHR(dma-buf %dx%d): 0x%x", w, h, R.GetError());
		close(fd);
		return false;
	}
	struct zwp_linux_buffer_params_v1 *p = zwp_linux_dmabuf_v1_create_params(D.dmabuf);
	zwp_linux_buffer_params_v1_add(p, fd, 0, 0, pitch, (uint32_t)(DRM_FORMAT_MOD_LINEAR >> 32),
				       (uint32_t)(DRM_FORMAT_MOD_LINEAR & 0xffffffff));
	struct wl_buffer *wb = zwp_linux_buffer_params_v1_create_immed(p, w, h, DRM_FORMAT_XRGB8888, 0);
	zwp_linux_buffer_params_v1_destroy(p);
	if (!wb) {
		R.DestroyImageKHR(D.dpy, img);
		close(fd);
		return false;
	}
	memset(b, 0, sizeof(*b));
	*b = (struct wbuf){ .s = s, .wl = wb, .img = img, .fd = fd, .w = w, .h = h, .pitch = pitch };
	wl_buffer_add_listener(wb, &buf_listener, b);
	dbg("buffer %ld: %dx%d pitch %u", (long)(b - s->buf), w, h, pitch);
	return true;
}

/* Read whatever the compositor already sent (releases), without blocking. */
static int read_nonblock(void)
{
	while (wl_display_prepare_read_queue(D.wl, D.q) != 0)
		if (wl_display_dispatch_queue_pending(D.wl, D.q) < 0)
			return -1;
	wl_display_flush(D.wl);
	struct pollfd p = { .fd = wl_display_get_fd(D.wl), .events = POLLIN };
	if (poll(&p, 1, 0) > 0) {
		if (wl_display_read_events(D.wl) < 0)
			return -1;
	} else {
		wl_display_cancel_read(D.wl);
	}
	return wl_display_dispatch_queue_pending(D.wl, D.q);
}

/* A buffer the compositor is done with, at the current size. */
static struct wbuf *get_back(struct wsurf *s)
{
	bool polled = false;
	for (;;) {
		if (wl_display_dispatch_queue_pending(D.wl, D.q) < 0)
			return NULL;
		struct wbuf *best = NULL, *empty = NULL;
		for (int i = 0; i < NBUF_MAX; i++) {
			struct wbuf *b = &s->buf[i];
			if (b->wl && !b->busy && (b->stale || b->w != s->w || b->h != s->h))
				buf_free(s, b);
			if (!b->wl) {
				if (!empty)
					empty = b;
				continue;
			}
			/* the most recently shown free buffer: smallest age */
			if (!b->busy && (!best || b->frame > best->frame))
				best = b;
		}
		if (best)
			return best;
		if (!polled) {
			/* a release may be waiting in the socket: don't allocate for nothing */
			polled = true;
			if (read_nonblock() < 0)
				return NULL;
			continue;
		}
		if (empty)
			return buf_alloc(s, empty, s->w, s->h) ? empty : NULL;
		if (wl_display_dispatch_queue(D.wl, D.q) < 0)
			return NULL;
	}
}

static void ensure_tex(struct wbuf *b)
{
	if (b->tex)
		return;
	GLint old = geti(GL_TEXTURE_BINDING_2D);
	glGenTextures(1, &b->tex);
	glBindTexture(GL_TEXTURE_2D, b->tex);
	R.ImageTargetTexture2DOES(GL_TEXTURE_2D, b->img);
	glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
	glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
	glBindTexture(GL_TEXTURE_2D, (GLuint)old);
}

static void ensure_rb(GLuint *rb, int *rw, int *rh, GLenum fmt, int w, int h)
{
	if (*rb && *rw == w && *rh == h)
		return;
	GLint old = geti(GL_RENDERBUFFER_BINDING);
	if (!*rb)
		glGenRenderbuffers(1, rb);
	glBindRenderbuffer(GL_RENDERBUFFER, *rb);
	glRenderbufferStorage(GL_RENDERBUFFER, fmt, w, h);
	glBindRenderbuffer(GL_RENDERBUFFER, (GLuint)old);
	*rw = w;
	*rh = h;
}

static bool direct(struct wsurf *s)
{
	return s->mode != MODE_BLIT;
}

/* Point the FBO at what the app renders into next. */
static void attach(struct wsurf *s)
{
	struct wbuf *b = s->back;
	if (!b)
		return;
	GLint draw = geti(GL_FRAMEBUFFER_BINDING);
	GLint read = s->es3 ? geti(GL_READ_FRAMEBUFFER_BINDING) : draw;
	R.glBindFramebuffer(GL_FRAMEBUFFER, s->fbo);
	if (direct(s)) {
		ensure_tex(b);
		glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, b->tex, 0);
	} else {
		if (s->color_w != s->w || s->color_h != s->h)
			s->color_valid = false;
		ensure_rb(&s->color_rb, &s->color_w, &s->color_h, s->es3 ? GL_RGBA8 : GL_RGBA8_OES, s->w, s->h);
		glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, s->color_rb);
	}
	if (s->depth || s->stencil) {
		ensure_rb(&s->depth_rb, &s->depth_w, &s->depth_h, GL_DEPTH24_STENCIL8_OES, s->w, s->h);
		glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_RENDERBUFFER, s->depth_rb);
		glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_STENCIL_ATTACHMENT, GL_RENDERBUFFER, s->depth_rb);
	}
	if (!s->checked || debug) {
		GLenum st = glCheckFramebufferStatus(GL_FRAMEBUFFER);
		if (st != GL_FRAMEBUFFER_COMPLETE)
			warn("window FBO incomplete: 0x%x", st);
		s->checked = true;
	}
	if (s->es3) {
		R.glBindFramebuffer(GL_DRAW_FRAMEBUFFER, (GLuint)draw);
		R.glBindFramebuffer(GL_READ_FRAMEBUFFER, (GLuint)read);
	} else {
		R.glBindFramebuffer(GL_FRAMEBUFFER, (GLuint)draw);
	}
}

/* Copy the app's frame into the dma-buf, flipping Y (blit mode). */
static void blit(struct wsurf *s, struct wbuf *b)
{
	GLint draw = geti(GL_DRAW_FRAMEBUFFER_BINDING), read = geti(GL_READ_FRAMEBUFFER_BINDING);
	GLboolean sc = glIsEnabled(GL_SCISSOR_TEST), rd = glIsEnabled(GL_RASTERIZER_DISCARD);
	ensure_tex(b);
	if (!s->blit_fbo)
		glGenFramebuffers(1, &s->blit_fbo);
	R.glBindFramebuffer(GL_DRAW_FRAMEBUFFER, s->blit_fbo);
	glFramebufferTexture2D(GL_DRAW_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, b->tex, 0);
	R.glBindFramebuffer(GL_READ_FRAMEBUFFER, s->fbo);
	if (sc)
		glDisable(GL_SCISSOR_TEST);
	if (rd)
		glDisable(GL_RASTERIZER_DISCARD);
	/* the renderbuffer and the back buffer both have the surface size */
	R.glBlitFramebuffer(0, 0, b->w, b->h, 0, b->h, b->w, 0, GL_COLOR_BUFFER_BIT, GL_NEAREST);
	if (sc)
		glEnable(GL_SCISSOR_TEST);
	if (rd)
		glEnable(GL_RASTERIZER_DISCARD);
	R.glBindFramebuffer(GL_DRAW_FRAMEBUFFER, (GLuint)draw);
	R.glBindFramebuffer(GL_READ_FRAMEBUFFER, (GLuint)read);
}

/* The surface was made current with ctx: (re)create GL objects there. */
static bool gl_setup(struct wsurf *s, EGLContext ctx)
{
	if (s->ctx != ctx) {
		/* objects of another context can't be used here; let them go */
		s->ctx = ctx;
		s->fbo = s->blit_fbo = s->color_rb = s->depth_rb = 0;
		s->color_w = s->color_h = s->depth_w = s->depth_h = 0;
		s->color_valid = false;
		s->viewport_set = false;
		for (int i = 0; i < NBUF_MAX; i++)
			s->buf[i].tex = 0;
		const char *v = (const char *)R.glGetString(GL_VERSION);
		int maj = 0;
		if (v && sscanf(v, "OpenGL ES %d", &maj) == 1)
			s->es3 = maj >= 3;
		bool forced;
		enum mode m = forced_mode(&forced);
		if (!forced)
			m = ctx_topdown(ctx) ? MODE_TOPDOWN : MODE_BLIT;
		if (m == MODE_BLIT && (!s->es3 || !R.glBlitFramebuffer))
			m = MODE_TRANSFORM;
		s->mode = m;
		dbg("surface %p in context %p: %s, mode %s", (void *)s, ctx, v ? v : "?",
		    m == MODE_BLIT ? "blit" : m == MODE_TOPDOWN ? "topdown" : "transform");
	}
	if (!s->fbo)
		glGenFramebuffers(1, &s->fbo);
	if (!s->back)
		s->back = get_back(s);
	if (!s->back)
		return false;
	attach(s);
	if (!s->viewport_set) {
		glViewport(0, 0, s->w, s->h);
		glScissor(0, 0, s->w, s->h);
		s->viewport_set = true;
	}
	return true;
}

/* -------------------------------------------------------- EGL: displays */

static EGLDisplay wl_display_for(void *native)
{
	struct wl_display *wl = native;
	lock();
	if (!wl) {
		if (!D.wl) {
			wl = wl_display_connect(NULL);
			if (!wl) {
				unlock();
				err_pending = EGL_BAD_PARAMETER;
				return EGL_NO_DISPLAY;
			}
			D.own_wl = true;
		} else {
			wl = D.wl;
		}
	}
	if (D.wl && D.wl != wl) {
		warn("only one wl_display per process is supported");
		unlock();
		err_pending = EGL_BAD_PARAMETER;
		return EGL_NO_DISPLAY;
	}
	D.wl = wl;
	D.dpy = R.GetDisplay(EGL_DEFAULT_DISPLAY);
	unlock();
	err_pending = EGL_SUCCESS;
	return D.dpy;
}

static bool is_wl_display(void *native)
{
	return native && *(const void *const *)native == &wl_display_interface;
}

EGLDisplay eglGetDisplay(EGLNativeDisplayType native)
{
	init_once();
	if (native != EGL_DEFAULT_DISPLAY && is_wl_display(native))
		return wl_display_for(native);
	return R.GetDisplay(native);
}

static EGLDisplay get_platform_display(EGLenum platform, void *native)
{
	init_once();
	switch (platform) {
	case EGL_PLATFORM_WAYLAND_KHR:
		return wl_display_for(native);
	case EGL_PLATFORM_SURFACELESS_MESA:
		err_pending = EGL_SUCCESS;
		return R.GetDisplay(EGL_DEFAULT_DISPLAY);
	default:
		err_pending = EGL_BAD_PARAMETER;
		return EGL_NO_DISPLAY;
	}
}

static EGLDisplay EGLAPIENTRY wrap_eglGetPlatformDisplayEXT(EGLenum platform, void *native, const EGLint *attrs)
{
	(void)attrs;
	return get_platform_display(platform, native);
}

static EGLDisplay EGLAPIENTRY wrap_eglGetPlatformDisplay(EGLenum platform, void *native, const EGLAttrib *attrs)
{
	(void)attrs;
	return get_platform_display(platform, native);
}

EGLBoolean eglInitialize(EGLDisplay dpy, EGLint *major, EGLint *minor)
{
	init_once();
	if (!R.Initialize(dpy, major, minor))
		return EGL_FALSE;
	lock();
	if (D.wl && dpy == D.dpy && !wl_setup())
		warn("Wayland window surfaces unavailable");
	unlock();
	return EGL_TRUE;
}

static const char client_ext[] =
	"EGL_EXT_client_extensions EGL_EXT_platform_base EGL_KHR_platform_wayland "
	"EGL_EXT_platform_wayland EGL_MESA_platform_surfaceless";

const char *eglQueryString(EGLDisplay dpy, EGLint name)
{
	init_once();
	if (dpy == EGL_NO_DISPLAY && name == EGL_EXTENSIONS) {
		err_pending = EGL_SUCCESS;
		return client_ext;
	}
	return R.QueryString(dpy, name);
}

EGLint eglGetError(void)
{
	init_once();
	EGLint real = R.GetError();
	EGLint e = err_pending;
	err_pending = 0;
	/* our own failure wins; our successes don't hide a stock failure */
	return e && e != EGL_SUCCESS ? e : real;
}

EGLBoolean eglChooseConfig(EGLDisplay dpy, const EGLint *attrs, EGLConfig *configs, EGLint size, EGLint *n)
{
	init_once();
	EGLBoolean r = R.ChooseConfig(dpy, attrs, configs, size, n);
	if (debug) {
		char buf[512];
		size_t o = 0;
		for (const EGLint *a = attrs; a && *a != EGL_NONE && o < sizeof(buf) - 32; a += 2)
			o += (size_t)snprintf(buf + o, sizeof(buf) - o, " 0x%x=%d", a[0], a[1]);
		buf[o] = 0;
		dbg("eglChooseConfig(%s): %s, %d configs", buf, r ? "ok" : "failed", n ? *n : -1);
	}
	return r;
}

EGLContext eglCreateContext(EGLDisplay dpy, EGLConfig cfg, EGLContext share, const EGLint *attrs)
{
	init_once();
	EGLContext c = R.CreateContext(dpy, cfg, share, attrs);
	if (debug) {
		char buf[256];
		size_t o = 0;
		for (const EGLint *a = attrs; a && *a != EGL_NONE && o < sizeof(buf) - 32; a += 2)
			o += (size_t)snprintf(buf + o, sizeof(buf) - o, " 0x%x=%d", a[0], a[1]);
		buf[o] = 0;
		EGLint e = c ? EGL_SUCCESS : R.GetError();
		err_pending = c ? 0 : e; /* keep it for the app's eglGetError */
		dbg("eglCreateContext(%s): %p (0x%x)", buf, c, e);
	}
	return c;
}

/* -------------------------------------------------------- EGL: surfaces */

static void win_resize(struct wl_egl_window *w, void *data)
{
	(void)w, (void)data; /* sizes are read at the next swap */
}

static void win_destroy(void *data)
{
	lock();
	struct wsurf *s = data;
	for (struct wsurf *x = D.surfaces; x; x = x->next)
		if (x == s)
			s->win = NULL;
	unlock();
}

static EGLSurface create_window(EGLDisplay dpy, EGLConfig cfg, void *native)
{
	struct wl_egl_window *w = native;
	init_once();
	lock();
	if (!D.wl || dpy != D.dpy) {
		unlock();
		return EGL_NO_SURFACE;
	}
	if (!D.ready && !wl_setup()) {
		unlock();
		err_pending = EGL_BAD_ALLOC;
		return EGL_NO_SURFACE;
	}
	if (!w || w->version < 3 || !w->surface || w->driver_private) {
		unlock();
		err_pending = EGL_BAD_NATIVE_WINDOW;
		return EGL_NO_SURFACE;
	}
	struct wsurf *s = calloc(1, sizeof(*s));
	if (!s) {
		unlock();
		err_pending = EGL_BAD_ALLOC;
		return EGL_NO_SURFACE;
	}
	s->magic = MAGIC;
	s->win = w;
	s->surface = w->surface;
	s->surf_wrap = wl_proxy_create_wrapper(w->surface);
	wl_proxy_set_queue((struct wl_proxy *)s->surf_wrap, D.q);
	s->cfg = cfg;
	s->w = w->width > 0 ? w->width : 1;
	s->h = w->height > 0 ? w->height : 1;
	s->interval = 1;
	for (int i = 0; i < NBUF_MAX; i++)
		s->buf[i].fd = -1;
	R.GetConfigAttrib(dpy, cfg, EGL_DEPTH_SIZE, &s->depth);
	R.GetConfigAttrib(dpy, cfg, EGL_STENCIL_SIZE, &s->stencil);
	w->driver_private = s;
	w->resize_callback = win_resize;
	w->destroy_window_callback = win_destroy;
	s->next = D.surfaces;
	D.surfaces = s;
	dbg("window surface %p: %dx%d depth %d stencil %d", (void *)s, s->w, s->h, s->depth, s->stencil);
	unlock();
	err_pending = EGL_SUCCESS;
	return (EGLSurface)s;
}

EGLSurface eglCreateWindowSurface(EGLDisplay dpy, EGLConfig cfg, EGLNativeWindowType win, const EGLint *attrs)
{
	init_once();
	if (D.wl && dpy == D.dpy)
		return create_window(dpy, cfg, (void *)win);
	return R.CreateWindowSurface(dpy, cfg, win, attrs);
}

static EGLSurface EGLAPIENTRY wrap_eglCreatePlatformWindowSurfaceEXT(EGLDisplay dpy, EGLConfig cfg, void *win, const EGLint *attrs)
{
	(void)attrs;
	init_once();
	if (D.wl && dpy == D.dpy)
		return create_window(dpy, cfg, win);
	err_pending = EGL_BAD_NATIVE_WINDOW;
	return EGL_NO_SURFACE;
}

static EGLSurface EGLAPIENTRY wrap_eglCreatePlatformWindowSurface(EGLDisplay dpy, EGLConfig cfg, void *win, const EGLAttrib *attrs)
{
	(void)attrs;
	return wrap_eglCreatePlatformWindowSurfaceEXT(dpy, cfg, win, NULL);
}

EGLBoolean eglDestroySurface(EGLDisplay dpy, EGLSurface surface)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	if (!s) {
		unlock();
		return R.DestroySurface(dpy, surface);
	}
	for (struct wsurf **p = &D.surfaces; *p; p = &(*p)->next)
		if (*p == s) {
			*p = s->next;
			break;
		}
	bool gl = gl_ok(s);
	for (int i = 0; i < NBUF_MAX; i++)
		buf_free(s, &s->buf[i]);
	if (gl) {
		if (geti(GL_FRAMEBUFFER_BINDING) == (GLint)s->fbo)
			R.glBindFramebuffer(GL_FRAMEBUFFER, 0);
		GLuint fb[2] = { s->fbo, s->blit_fbo }, rb[2] = { s->color_rb, s->depth_rb };
		glDeleteFramebuffers(2, fb);
		glDeleteRenderbuffers(2, rb);
	}
	if (s->frame)
		wl_callback_destroy(s->frame);
	if (s->surf_wrap)
		wl_proxy_wrapper_destroy(s->surf_wrap);
	if (s->win) {
		s->win->driver_private = NULL;
		s->win->resize_callback = NULL;
		s->win->destroy_window_callback = NULL;
	}
	if (cur == s)
		cur = NULL;
	s->magic = 0;
	free(s);
	unlock();
	return ok();
}

EGLBoolean eglMakeCurrent(EGLDisplay dpy, EGLSurface draw, EGLSurface read, EGLContext ctx)
{
	init_once();
	lock();
	struct wsurf *sd = ours(draw), *sr = ours(read);
	struct wsurf *prev = cur;
	if (sd && sd == sr && sd == prev && sd->ctx == ctx && R.GetCurrentContext() == ctx) {
		unlock();
		return ok(); /* already current */
	}
	/* leaving one of ours: don't leave our FBO bound as "0" */
	if (prev && gl_ok(prev) && geti(GL_FRAMEBUFFER_BINDING) == (GLint)prev->fbo && sd != prev)
		R.glBindFramebuffer(GL_FRAMEBUFFER, 0);
	if (!sd && !sr) {
		unlock();
		EGLBoolean r = R.MakeCurrent(dpy, draw, read, ctx);
		if (r)
			cur = NULL;
		return r;
	}
	if (sd != sr) {
		unlock();
		return fail(EGL_BAD_MATCH);
	}
	if (ctx == EGL_NO_CONTEXT) {
		unlock();
		return fail(EGL_BAD_MATCH);
	}
	if (!R.MakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, ctx)) {
		unlock();
		return EGL_FALSE;
	}
	cur = sd;
	GLint bound = geti(GL_FRAMEBUFFER_BINDING);
	bool rebind = sd->ctx != ctx || bound == 0 || (prev && bound == (GLint)prev->fbo);
	if (!gl_setup(sd, ctx)) {
		cur = NULL;
		R.MakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
		unlock();
		return fail(EGL_BAD_ALLOC);
	}
	if (rebind)
		R.glBindFramebuffer(GL_FRAMEBUFFER, sd->fbo);
	unlock();
	return ok();
}

EGLBoolean eglReleaseThread(void)
{
	init_once();
	cur = NULL;
	return R.ReleaseThread();
}

EGLBoolean eglDestroyContext(EGLDisplay dpy, EGLContext ctx)
{
	init_once();
	lock();
	for (struct wsurf *s = D.surfaces; s; s = s->next)
		if (s->ctx == ctx) {
			s->ctx = EGL_NO_CONTEXT; /* its GL objects die with it */
			for (int i = 0; i < NBUF_MAX; i++)
				s->buf[i].tex = 0;
			s->fbo = s->blit_fbo = s->color_rb = s->depth_rb = 0;
			if (cur == s)
				cur = NULL;
		}
	for (size_t i = 0; i < sizeof(D.ctxflags) / sizeof(D.ctxflags[0]); i++)
		if (D.ctxflags[i].ctx == ctx)
			D.ctxflags[i].ctx = EGL_NO_CONTEXT;
	unlock();
	return R.DestroyContext(dpy, ctx);
}

EGLSurface eglGetCurrentSurface(EGLint which)
{
	init_once();
	if (cur && R.GetCurrentContext() == cur->ctx)
		return (EGLSurface)cur;
	return R.GetCurrentSurface(which);
}

EGLBoolean eglSwapInterval(EGLDisplay dpy, EGLint interval)
{
	init_once();
	if (cur) {
		cur->interval = interval > 0 ? 1 : 0;
		return ok();
	}
	return R.SwapInterval(dpy, interval);
}

static EGLBoolean swap(struct wsurf *s)
{
	if (!gl_ok(s))
		return fail(EGL_BAD_SURFACE);
	if (!s->win)
		return fail(EGL_BAD_NATIVE_WINDOW);
	struct wbuf *b = s->back;
	if (!b)
		return fail(EGL_BAD_ALLOC);
	/* depth/stencil are undefined after a swap (EGL 3.9.1): don't let the
	   tiler store them */
	if ((s->depth || s->stencil) && s->es3 && R.glInvalidateFramebuffer) {
		static const GLenum ds[2] = { GL_DEPTH_ATTACHMENT, GL_STENCIL_ATTACHMENT };
		GLint draw = geti(GL_DRAW_FRAMEBUFFER_BINDING);
		R.glBindFramebuffer(GL_DRAW_FRAMEBUFFER, s->fbo);
		R.glInvalidateFramebuffer(GL_DRAW_FRAMEBUFFER, 2, ds);
		R.glBindFramebuffer(GL_DRAW_FRAMEBUFFER, (GLuint)draw);
	}
	if (s->mode == MODE_BLIT)
		blit(s, b);
	glFinish(); /* no implicit fencing in this DDK: done before the compositor reads it */

	if (s->interval > 0)
		while (s->frame)
			if (wl_display_dispatch_queue(D.wl, D.q) < 0)
				return fail(EGL_BAD_NATIVE_WINDOW);
	if (s->interval > 0) {
		s->frame = wl_surface_frame(s->surf_wrap);
		wl_callback_add_listener(s->frame, &frame_listener, s);
	}
	int t = s->mode == MODE_TRANSFORM ? WL_OUTPUT_TRANSFORM_FLIPPED_180 : WL_OUTPUT_TRANSFORM_NORMAL;
	if (t != s->transform) {
		wl_surface_set_buffer_transform(s->surface, t);
		s->transform = t;
	}
	uint32_t ver = wl_proxy_get_version((struct wl_proxy *)s->surface);
	int dx = s->win->dx, dy = s->win->dy;
	if (ver >= 5) {
		wl_surface_attach(s->surface, b->wl, 0, 0);
		if (dx || dy)
			wl_surface_offset(s->surface, dx, dy);
	} else {
		wl_surface_attach(s->surface, b->wl, dx, dy);
	}
	s->win->dx = s->win->dy = 0;
	if (ver >= 4)
		wl_surface_damage_buffer(s->surface, 0, 0, INT32_MAX, INT32_MAX);
	else
		wl_surface_damage(s->surface, 0, 0, INT32_MAX, INT32_MAX);
	wl_surface_commit(s->surface);
	wl_display_flush(D.wl);
	b->busy = true;
	b->frame = ++s->frames;
	s->win->attached_width = b->w;
	s->win->attached_height = b->h;

	if (s->win->width > 0 && s->win->height > 0 && (s->win->width != s->w || s->win->height != s->h)) {
		dbg("resize %dx%d -> %dx%d", s->w, s->h, s->win->width, s->win->height);
		s->w = s->win->width;
		s->h = s->win->height;
		for (int i = 0; i < NBUF_MAX; i++)
			s->buf[i].stale = true;
	}
	s->back = NULL;
	s->back = get_back(s);
	if (!s->back)
		return fail(EGL_BAD_ALLOC);
	attach(s);
	if (s->mode == MODE_BLIT)
		s->color_valid = true;
	return ok();
}

EGLBoolean eglSwapBuffers(EGLDisplay dpy, EGLSurface surface)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	if (!s) {
		unlock();
		return R.SwapBuffers(dpy, surface);
	}
	EGLBoolean r = swap(s);
	unlock();
	return r;
}

static EGLBoolean EGLAPIENTRY wrap_eglSwapBuffersWithDamageKHR(EGLDisplay dpy, EGLSurface surface, const EGLint *rects, EGLint n)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	unlock();
	if (s)
		return eglSwapBuffers(dpy, surface);
	if (R.SwapBuffersWithDamageKHR)
		return R.SwapBuffersWithDamageKHR(dpy, surface, (EGLint *)rects, n);
	return R.SwapBuffers(dpy, surface);
}

static EGLBoolean EGLAPIENTRY wrap_eglSwapBuffersWithDamageEXT(EGLDisplay dpy, EGLSurface surface, const EGLint *rects, EGLint n)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	unlock();
	if (s)
		return eglSwapBuffers(dpy, surface);
	if (R.SwapBuffersWithDamageEXT)
		return R.SwapBuffersWithDamageEXT(dpy, surface, (EGLint *)rects, n);
	return R.SwapBuffers(dpy, surface);
}

EGLBoolean eglQuerySurface(EGLDisplay dpy, EGLSurface surface, EGLint attr, EGLint *v)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	if (!s) {
		unlock();
		return R.QuerySurface(dpy, surface, attr, v);
	}
	EGLBoolean r = EGL_TRUE;
	switch (attr) {
	case EGL_WIDTH: *v = s->w; break;
	case EGL_HEIGHT: *v = s->h; break;
	case EGL_CONFIG_ID: R.GetConfigAttrib(dpy, s->cfg, EGL_CONFIG_ID, v); break;
	case EGL_RENDER_BUFFER: *v = EGL_BACK_BUFFER; break;
	case EGL_SWAP_BEHAVIOR: *v = EGL_BUFFER_DESTROYED; break;
	case EGL_MULTISAMPLE_RESOLVE: *v = EGL_MULTISAMPLE_RESOLVE_DEFAULT; break;
	case EGL_TEXTURE_FORMAT: *v = EGL_NO_TEXTURE; break;
	case EGL_TEXTURE_TARGET: *v = EGL_NO_TEXTURE; break;
	case EGL_HORIZONTAL_RESOLUTION:
	case EGL_VERTICAL_RESOLUTION:
	case EGL_PIXEL_ASPECT_RATIO: *v = EGL_UNKNOWN; break;
	case EGL_LARGEST_PBUFFER:
	case EGL_MIPMAP_TEXTURE:
	case EGL_MIPMAP_LEVEL: *v = 0; break;
	case EGL_BUFFER_AGE_EXT:
		if (s->mode == MODE_BLIT)
			*v = s->color_valid ? 1 : 0;
		else
			*v = s->back && s->back->frame ? (EGLint)(s->frames + 1 - s->back->frame) : 0;
		break;
	default: r = fail(EGL_BAD_ATTRIBUTE);
	}
	unlock();
	if (r)
		err_pending = EGL_SUCCESS;
	return r;
}

EGLBoolean eglSurfaceAttrib(EGLDisplay dpy, EGLSurface surface, EGLint attr, EGLint value)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	unlock();
	if (!s)
		return R.SurfaceAttrib(dpy, surface, attr, value);
	if (attr == EGL_SWAP_BEHAVIOR && value == EGL_BUFFER_PRESERVED)
		return fail(EGL_BAD_MATCH);
	return ok();
}

EGLBoolean eglCopyBuffers(EGLDisplay dpy, EGLSurface surface, EGLNativePixmapType target)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	unlock();
	if (s)
		return fail(EGL_BAD_NATIVE_PIXMAP);
	return R.CopyBuffers(dpy, surface, target);
}

EGLBoolean eglBindTexImage(EGLDisplay dpy, EGLSurface surface, EGLint buffer)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	unlock();
	if (s)
		return fail(EGL_BAD_SURFACE);
	return R.BindTexImage(dpy, surface, buffer);
}

EGLBoolean eglReleaseTexImage(EGLDisplay dpy, EGLSurface surface, EGLint buffer)
{
	init_once();
	lock();
	struct wsurf *s = ours(surface);
	unlock();
	if (s)
		return fail(EGL_BAD_SURFACE);
	return R.ReleaseTexImage(dpy, surface, buffer);
}

/* --------------------------------------------- GL: "framebuffer 0" fixes */

static bool bound_ours(GLenum binding)
{
	return cur && cur->fbo && geti(binding) == (GLint)cur->fbo;
}

GL_APICALL void GL_APIENTRY glBindFramebuffer(GLenum target, GLuint framebuffer)
{
	init_once();
	if (framebuffer == 0 && cur && cur->fbo)
		framebuffer = cur->fbo;
	R.glBindFramebuffer(target, framebuffer);
}

GL_APICALL void GL_APIENTRY glGetIntegerv(GLenum pname, GLint *data)
{
	init_once();
	R.glGetIntegerv(pname, data);
	if (!cur || !cur->fbo || !data)
		return;
	switch (pname) {
	case GL_FRAMEBUFFER_BINDING: /* == GL_DRAW_FRAMEBUFFER_BINDING */
	case GL_READ_FRAMEBUFFER_BINDING:
		if (*data == (GLint)cur->fbo)
			*data = 0;
		break;
	case GL_READ_BUFFER:
		if (*data == GL_COLOR_ATTACHMENT0 && bound_ours(GL_READ_FRAMEBUFFER_BINDING))
			*data = GL_BACK;
		break;
	case GL_DRAW_BUFFER0:
		if (*data == GL_COLOR_ATTACHMENT0 && bound_ours(GL_DRAW_FRAMEBUFFER_BINDING))
			*data = GL_BACK;
		break;
	}
}

static const char topdown_ext[] = "GL_MORTAR_window_topdown";

GL_APICALL const GLubyte *GL_APIENTRY glGetString(GLenum name)
{
	init_once();
	const GLubyte *r = R.glGetString(name);
	if (name != GL_EXTENSIONS || !r || !cur)
		return r;
	lock();
	if (!D.gl_ext) {
		size_t n = strlen((const char *)r);
		D.gl_ext = malloc(n + sizeof(topdown_ext) + 2);
		if (D.gl_ext)
			snprintf(D.gl_ext, n + sizeof(topdown_ext) + 2, "%s%s%s", (const char *)r,
				 n && r[n - 1] != ' ' ? " " : "", topdown_ext);
	}
	unlock();
	return D.gl_ext ? (const GLubyte *)D.gl_ext : r;
}

GL_APICALL void GL_APIENTRY glDrawBuffers(GLsizei n, const GLenum *bufs)
{
	init_once();
	if (n == 1 && bufs && bufs[0] == GL_BACK && bound_ours(GL_DRAW_FRAMEBUFFER_BINDING)) {
		GLenum a = GL_COLOR_ATTACHMENT0;
		R.glDrawBuffers(1, &a);
		return;
	}
	R.glDrawBuffers(n, bufs);
}

GL_APICALL void GL_APIENTRY glReadBuffer(GLenum src)
{
	init_once();
	if (src == GL_BACK && bound_ours(GL_READ_FRAMEBUFFER_BINDING))
		src = GL_COLOR_ATTACHMENT0;
	R.glReadBuffer(src);
}

static GLenum map_attachment(GLenum a)
{
	switch (a) {
	case GL_COLOR: return GL_COLOR_ATTACHMENT0;
	case GL_DEPTH: return GL_DEPTH_ATTACHMENT;
	case GL_STENCIL: return GL_STENCIL_ATTACHMENT;
	default: return a;
	}
}

static void invalidate(void (GL_APIENTRY *fn)(GLenum, GLsizei, const GLenum *), GLenum target, GLsizei n, const GLenum *att)
{
	GLenum binding = target == GL_READ_FRAMEBUFFER ? GL_READ_FRAMEBUFFER_BINDING : GL_FRAMEBUFFER_BINDING;
	if (!fn)
		return;
	if (n > 0 && n <= 8 && att && bound_ours(binding)) {
		GLenum m[8];
		for (GLsizei i = 0; i < n; i++)
			m[i] = map_attachment(att[i]);
		fn(target, n, m);
		return;
	}
	fn(target, n, att);
}

GL_APICALL void GL_APIENTRY glInvalidateFramebuffer(GLenum target, GLsizei n, const GLenum *att)
{
	init_once();
	invalidate(R.glInvalidateFramebuffer, target, n, att);
}

static void GL_APIENTRY wrap_glDiscardFramebufferEXT(GLenum target, GLsizei n, const GLenum *att)
{
	init_once();
	invalidate(R.glDiscardFramebufferEXT, target, n, att);
}

/* The app renders the window top-down (like a render-target texture):
   scan it out without a flip. Applies to the current surface and every
   surface later made current with this context. */
GL_APICALL void GL_APIENTRY glWindowTopDownMORTAR(GLboolean on)
{
	init_once();
	lock();
	EGLContext c = R.GetCurrentContext();
	if (c != EGL_NO_CONTEXT) {
		set_ctx_topdown(c, on);
		bool forced;
		forced_mode(&forced);
		if (cur && gl_ok(cur) && !forced) {
			enum mode m = on ? MODE_TOPDOWN : (cur->es3 && R.glBlitFramebuffer ? MODE_BLIT : MODE_TRANSFORM);
			if (m != cur->mode) {
				dbg("surface %p: mode %s", (void *)cur, on ? "topdown" : "blit");
				cur->mode = m;
				attach(cur);
			}
		}
	}
	unlock();
}

/* ------------------------------------------------------ eglGetProcAddress */

static const struct {
	const char *name;
	void (*fn)(void);
} procs[] = {
#define P(n) { #n, (void (*)(void))n }
#define W(n) { #n, (void (*)(void))wrap_##n }
	P(eglGetDisplay), P(eglInitialize), P(eglChooseConfig), P(eglCreateContext), P(eglQueryString), P(eglGetError),
	P(eglCreateWindowSurface), P(eglDestroySurface), P(eglMakeCurrent),
	P(eglReleaseThread), P(eglDestroyContext), P(eglGetCurrentSurface),
	P(eglSwapInterval), P(eglSwapBuffers), P(eglQuerySurface), P(eglSurfaceAttrib),
	P(eglCopyBuffers), P(eglBindTexImage), P(eglReleaseTexImage),
	W(eglGetPlatformDisplayEXT), W(eglGetPlatformDisplay),
	W(eglCreatePlatformWindowSurfaceEXT), W(eglCreatePlatformWindowSurface),
	W(eglSwapBuffersWithDamageKHR), W(eglSwapBuffersWithDamageEXT),
	P(glBindFramebuffer), P(glGetIntegerv), P(glGetString), P(glDrawBuffers),
	P(glReadBuffer), P(glInvalidateFramebuffer), W(glDiscardFramebufferEXT),
	P(glWindowTopDownMORTAR),
#undef P
#undef W
};

__eglMustCastToProperFunctionPointerType eglGetProcAddress(const char *name)
{
	init_once();
	if (!name)
		return NULL;
	for (size_t i = 0; i < sizeof(procs) / sizeof(procs[0]); i++)
		if (!strcmp(procs[i].name, name))
			return (__eglMustCastToProperFunctionPointerType)procs[i].fn;
	return R.GetProcAddress(name);
}

/* Exported for SDL_GL_GetProcAddress (dlsym on this library). */
EGLAPI EGLDisplay EGLAPIENTRY eglGetPlatformDisplayEXT(EGLenum platform, void *native, const EGLint *attrs)
{
	return wrap_eglGetPlatformDisplayEXT(platform, native, attrs);
}

EGLAPI EGLSurface EGLAPIENTRY eglCreatePlatformWindowSurfaceEXT(EGLDisplay dpy, EGLConfig cfg, void *win, const EGLint *attrs)
{
	return wrap_eglCreatePlatformWindowSurfaceEXT(dpy, cfg, win, attrs);
}
