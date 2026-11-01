// SPDX-License-Identifier: GPL-2.0-or-later
// mortar-wlegl-demo: a fullscreen xdg toplevel rendered with GLES on the
// PowerVR through libmortar-wlegl. A spinning triangle, a white square in
// the top-left corner (orientation check) and a bar that moves every frame.
// Prints fps and process CPU every 2 s.
//
//   mortar-wlegl-demo [FRAMES] [--topdown] [--interval=0|1]
//   (WAYLAND_DISPLAY must name the compositor; MORTAR_WLEGL_DEBUG=1 logs)
#define EGL_NO_PLATFORM_SPECIFIC_TYPES 1
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <math.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <time.h>
#include <wayland-client.h>
#include <wayland-egl.h>
#include "xdg-shell-client-protocol.h"

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %s:%d %s egl=0x%x gl=0x%x\n", __FILE__, __LINE__, #x, eglGetError(), glGetError()); exit(1); } } while (0)

static struct wl_compositor *comp;
static struct xdg_wm_base *wm;
static int cw, ch, configured, closed;

static double now(void)
{
	struct timespec t;
	clock_gettime(CLOCK_MONOTONIC, &t);
	return t.tv_sec + t.tv_nsec * 1e-9;
}

static double cpu(void)
{
	struct rusage u;
	getrusage(RUSAGE_SELF, &u);
	return u.ru_utime.tv_sec + u.ru_stime.tv_sec + (u.ru_utime.tv_usec + u.ru_stime.tv_usec) * 1e-6;
}

static void reg_global(void *d, struct wl_registry *r, uint32_t name, const char *iface, uint32_t ver)
{
	(void)d;
	if (!strcmp(iface, wl_compositor_interface.name))
		comp = wl_registry_bind(r, name, &wl_compositor_interface, ver < 4 ? ver : 4);
	else if (!strcmp(iface, xdg_wm_base_interface.name))
		wm = wl_registry_bind(r, name, &xdg_wm_base_interface, 1);
}
static void reg_remove(void *d, struct wl_registry *r, uint32_t n) { (void)d, (void)r, (void)n; }
static const struct wl_registry_listener reg_l = { reg_global, reg_remove };

static void wm_ping(void *d, struct xdg_wm_base *w, uint32_t serial) { (void)d; xdg_wm_base_pong(w, serial); }
static const struct xdg_wm_base_listener wm_l = { wm_ping };

static void xs_configure(void *d, struct xdg_surface *s, uint32_t serial)
{
	(void)d;
	xdg_surface_ack_configure(s, serial);
	configured = 1;
}
static const struct xdg_surface_listener xs_l = { xs_configure };

static void tl_configure(void *d, struct xdg_toplevel *t, int32_t w, int32_t h, struct wl_array *st)
{
	(void)d, (void)t, (void)st;
	if (w > 0 && h > 0)
		cw = w, ch = h;
}
static void tl_close(void *d, struct xdg_toplevel *t) { (void)d, (void)t; closed = 1; }
static const struct xdg_toplevel_listener tl_l = { .configure = tl_configure, .close = tl_close }; /* xdg_wm_base v1 */

static GLuint shader(GLenum type, const char *src)
{
	GLuint s = glCreateShader(type);
	glShaderSource(s, 1, &src, NULL);
	glCompileShader(s);
	GLint ok;
	glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
	CHECK(ok);
	return s;
}

int main(int argc, char **argv)
{
	int frames = 600, topdown = 0, interval = 1;
	for (int i = 1; i < argc; i++) {
		if (!strcmp(argv[i], "--topdown")) topdown = 1;
		else if (!strncmp(argv[i], "--interval=", 11)) interval = atoi(argv[i] + 11);
		else frames = atoi(argv[i]);
	}
	struct wl_display *wl = wl_display_connect(NULL);
	CHECK(wl);
	struct wl_registry *reg = wl_display_get_registry(wl);
	wl_registry_add_listener(reg, &reg_l, NULL);
	wl_display_roundtrip(wl);
	CHECK(comp && wm);
	xdg_wm_base_add_listener(wm, &wm_l, NULL);
	struct wl_surface *surf = wl_compositor_create_surface(comp);
	struct xdg_surface *xs = xdg_wm_base_get_xdg_surface(wm, surf);
	xdg_surface_add_listener(xs, &xs_l, NULL);
	struct xdg_toplevel *tl = xdg_surface_get_toplevel(xs);
	xdg_toplevel_add_listener(tl, &tl_l, NULL);
	xdg_toplevel_set_title(tl, "mortar-wlegl-demo");
	xdg_toplevel_set_fullscreen(tl, NULL);
	wl_surface_commit(surf);
	while (!configured)
		CHECK(wl_display_dispatch(wl) >= 0);
	if (cw <= 0) cw = 1024, ch = 768;
	struct wl_egl_window *win = wl_egl_window_create(surf, cw, ch);
	CHECK(win);

	PFNEGLGETPLATFORMDISPLAYEXTPROC gpd = (void *)eglGetProcAddress("eglGetPlatformDisplayEXT");
	CHECK(gpd);
	EGLDisplay dpy = gpd(EGL_PLATFORM_WAYLAND_KHR, wl, NULL);
	CHECK(dpy != EGL_NO_DISPLAY);
	EGLint maj, min;
	CHECK(eglInitialize(dpy, &maj, &min));
	printf("EGL %s, client extensions: %s\n", eglQueryString(dpy, EGL_VERSION), eglQueryString(EGL_NO_DISPLAY, EGL_EXTENSIONS));
	EGLint ca[] = { EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
			EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE };
	EGLConfig cfg;
	EGLint n;
	CHECK(eglChooseConfig(dpy, ca, &cfg, 1, &n) && n == 1);
	EGLint cta[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
	EGLContext ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, cta);
	CHECK(ctx != EGL_NO_CONTEXT);
	EGLSurface es = eglCreateWindowSurface(dpy, cfg, (EGLNativeWindowType)win, NULL);
	CHECK(es != EGL_NO_SURFACE);
	CHECK(eglMakeCurrent(dpy, es, es, ctx));
	CHECK(eglSwapInterval(dpy, interval));
	printf("GL_RENDERER %s, GL_VERSION %s\n", glGetString(GL_RENDERER), glGetString(GL_VERSION));
	float ysign = 1.0f;
	if (topdown) {
		const char *ext = (const char *)glGetString(GL_EXTENSIONS);
		void (*td)(GLboolean) = (void *)eglGetProcAddress("glWindowTopDownMORTAR");
		if (ext && strstr(ext, "GL_MORTAR_window_topdown") && td) {
			td(GL_TRUE);
			ysign = -1.0f; /* we now draw the top row first */
			printf("top-down window (no flip at present)\n");
		} else {
			printf("GL_MORTAR_window_topdown not available\n");
		}
	}

	const char *vs = "attribute vec2 pos; uniform mat2 rot; uniform vec2 off; uniform float ys;"
			 "void main(){ vec2 p = rot*pos + off; gl_Position = vec4(p.x, p.y*ys, 0.0, 1.0); }";
	const char *fs = "precision mediump float; uniform vec4 col; void main(){ gl_FragColor = col; }";
	GLuint prog = glCreateProgram();
	glAttachShader(prog, shader(GL_VERTEX_SHADER, vs));
	glAttachShader(prog, shader(GL_FRAGMENT_SHADER, fs));
	glBindAttribLocation(prog, 0, "pos");
	glLinkProgram(prog);
	glUseProgram(prog);
	GLint u_rot = glGetUniformLocation(prog, "rot"), u_off = glGetUniformLocation(prog, "off");
	GLint u_col = glGetUniformLocation(prog, "col"), u_ys = glGetUniformLocation(prog, "ys");
	glUniform1f(u_ys, ysign);
	static const GLfloat tri[] = { -0.5f, -0.4f, 0.5f, -0.4f, 0.0f, 0.6f };
	static const GLfloat quad[] = { -1, -1, 1, -1, -1, 1, 1, 1 };
	glEnableVertexAttribArray(0);

	double t0 = now(), c0 = cpu(), tl0 = t0;
	int last = 0;
	for (int f = 0; f < frames && !closed; f++) {
		wl_display_dispatch_pending(wl);
		wl_egl_window_resize(win, cw, ch, 0, 0); /* follows the toplevel configure */
		glViewport(0, 0, cw, ch);
		glClearColor(0.1f, 0.1f, 0.3f, 1);
		glClear(GL_COLOR_BUFFER_BIT);
		float a = f * 0.03f, c = cosf(a), s = sinf(a);
		GLfloat rot[4] = { c, s, -s, c };
		glUniformMatrix2fv(u_rot, 1, GL_FALSE, rot);
		glUniform2f(u_off, 0, 0);
		glUniform4f(u_col, 1.0f, 0.3f, 0.1f, 1);
		glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, tri);
		glDrawArrays(GL_TRIANGLES, 0, 3);
		/* white square, top-left (GL +y is up) */
		GLfloat sq[4] = { 0.06f, 0, 0, 0.08f };
		glUniformMatrix2fv(u_rot, 1, GL_FALSE, sq);
		glUniform2f(u_off, -0.92f, 0.9f);
		glUniform4f(u_col, 1, 1, 1, 1);
		glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, quad);
		glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
		/* moving lime bar along the bottom */
		GLfloat bar[4] = { 0.02f, 0, 0, 0.05f };
		glUniformMatrix2fv(u_rot, 1, GL_FALSE, bar);
		glUniform2f(u_off, -1.0f + fmodf(f * 0.01f, 2.0f), -0.93f);
		glUniform4f(u_col, 0.8f, 0.94f, 0.35f, 1);
		glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
		CHECK(eglSwapBuffers(dpy, es));
		double t = now();
		if (t - tl0 >= 2.0) {
			double c1 = cpu();
			printf("%.1f fps, CPU %.0f%% (%dx%d)\n", (f - last) / (t - tl0), (c1 - c0) / (t - tl0) * 100, cw, ch);
			fflush(stdout);
			tl0 = t, c0 = c1, last = f;
		}
	}
	printf("%d frames in %.1f s\n", frames, now() - t0);
	eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
	eglDestroySurface(dpy, es);
	wl_egl_window_destroy(win);
	eglDestroyContext(dpy, ctx);
	eglTerminate(dpy);
	xdg_toplevel_destroy(tl);
	xdg_surface_destroy(xs);
	wl_surface_destroy(surf);
	wl_display_disconnect(wl);
	return 0;
}
