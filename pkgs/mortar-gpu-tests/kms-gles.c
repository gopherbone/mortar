// SPDX-License-Identifier: MIT
// Presentation experiments for the stock PowerVR DDK on the sun4i KMS device.
//
//   kms-gles import   [frames]  zero-copy: KMS dumb buffers -> PRIME dma-buf ->
//                               EGL_EXT_image_dma_buf_import -> GLES FBO,
//                               render directly into scanout memory, page flip.
//                               Without --show only checks pixels via mmap.
//   kms-gles readback [frames]  copy path: render to pbuffer, glReadPixels into
//                               the mmapped dumb buffer, page flip.
// Options: --show (actually modeset + flip on card0; restores CRTC afterwards)
//          --novsync (flip without waiting: drmModeSetCrtc per frame / no event wait)
//          --fmt=xrgb|xbgr|argb|abgr (dumb buffer fourcc, default xrgb)
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <drm_fourcc.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

#define NBUF 3
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %s:%d %s egl=0x%x gl=0x%x errno=%s\n", __FILE__, __LINE__, #x, eglGetError(), glGetError(), strerror(errno)); exit(1); } } while (0)

static double now(void)
{
	struct timespec t;
	clock_gettime(CLOCK_MONOTONIC, &t);
	return t.tv_sec + t.tv_nsec * 1e-9;
}

struct buf {
	uint32_t handle, pitch, fb;
	uint64_t size;
	int fd;
	void *map;
	EGLImageKHR img;
	GLuint tex, fbo;
};

static int drm;
static uint32_t W, H, crtc_id, conn_id;
static drmModeModeInfo mode;
static int flip_pending;

static void flip_handler(int fd, unsigned seq, unsigned s, unsigned us, void *data)
{
	flip_pending = 0;
}

static void wait_flip(void)
{
	drmEventContext ev = { .version = 2, .page_flip_handler = flip_handler };
	while (flip_pending) {
		struct pollfd p = { .fd = drm, .events = POLLIN };
		if (poll(&p, 1, 1000) <= 0) { fprintf(stderr, "flip timeout\n"); flip_pending = 0; break; }
		drmHandleEvent(drm, &ev);
	}
}

static void kms_init(void)
{
	drm = open("/dev/dri/card0", O_RDWR | O_CLOEXEC);
	CHECK(drm >= 0);
	drmModeRes *res = drmModeGetResources(drm);
	CHECK(res);
	for (int i = 0; i < res->count_connectors && !conn_id; i++) {
		drmModeConnector *c = drmModeGetConnector(drm, res->connectors[i]);
		if (c->connection == DRM_MODE_CONNECTED && c->count_modes) {
			conn_id = c->connector_id;
			mode = c->modes[0];
			drmModeEncoder *e = drmModeGetEncoder(drm, c->encoder_id ? c->encoder_id : c->encoders[0]);
			crtc_id = e->crtc_id ? e->crtc_id : res->crtcs[0];
			drmModeFreeEncoder(e);
		}
		drmModeFreeConnector(c);
	}
	CHECK(conn_id);
	W = mode.hdisplay;
	H = mode.vdisplay;
	printf("KMS: connector %u crtc %u mode %s %ux%u@%u\n", conn_id, crtc_id, mode.name, W, H, mode.vrefresh);
}

static void buf_create(struct buf *b, uint32_t fourcc)
{
	struct drm_mode_create_dumb c = { .width = W, .height = H, .bpp = 32 };
	CHECK(drmIoctl(drm, DRM_IOCTL_MODE_CREATE_DUMB, &c) == 0);
	b->handle = c.handle; b->pitch = c.pitch; b->size = c.size;
	uint32_t h[4] = { b->handle }, p[4] = { b->pitch }, o[4] = { 0 };
	CHECK(drmModeAddFB2(drm, W, H, fourcc, h, p, o, &b->fb, 0) == 0);
	CHECK(drmPrimeHandleToFD(drm, b->handle, DRM_CLOEXEC | DRM_RDWR, &b->fd) == 0);
	struct drm_mode_map_dumb m = { .handle = b->handle };
	CHECK(drmIoctl(drm, DRM_IOCTL_MODE_MAP_DUMB, &m) == 0);
	b->map = mmap(NULL, b->size, PROT_READ | PROT_WRITE, MAP_SHARED, drm, m.offset);
	CHECK(b->map != MAP_FAILED);
	memset(b->map, 0x40, b->size);
}

static EGLDisplay dpy;
static EGLContext ctx;
static EGLSurface pbuf = EGL_NO_SURFACE;

static void egl_init(int pbw, int pbh)
{
	dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
	EGLint maj, min;
	CHECK(eglInitialize(dpy, &maj, &min));
	EGLint cattr[] = { EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
			   EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_NONE };
	EGLConfig cfg; EGLint n;
	CHECK(eglChooseConfig(dpy, cattr, &cfg, 1, &n) && n == 1);
	EGLint ca[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
	ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, ca);
	CHECK(ctx != EGL_NO_CONTEXT);
	EGLint sa[] = { EGL_WIDTH, pbw, EGL_HEIGHT, pbh, EGL_NONE };
	pbuf = eglCreatePbufferSurface(dpy, cfg, sa);
	CHECK(pbuf != EGL_NO_SURFACE);
	CHECK(eglMakeCurrent(dpy, pbuf, pbuf, ctx));
}

static GLuint prog, u_t, u_c;
static void gl_scene_init(void)
{
	const char *vs = "attribute vec4 pos; uniform float t; varying vec2 uv;"
			 "void main(){ uv=pos.xy; float c=cos(t), s=sin(t);"
			 " gl_Position=vec4(c*pos.x-s*pos.y, s*pos.x+c*pos.y, 0.0, 1.0); }";
	const char *fs = "precision mediump float; varying vec2 uv; uniform vec4 col;"
			 "void main(){ gl_FragColor=vec4(col.rgb*(0.6+0.4*uv.x), 1.0); }";
	GLuint s1 = glCreateShader(GL_VERTEX_SHADER), s2 = glCreateShader(GL_FRAGMENT_SHADER);
	glShaderSource(s1, 1, &vs, NULL); glCompileShader(s1);
	glShaderSource(s2, 1, &fs, NULL); glCompileShader(s2);
	prog = glCreateProgram();
	glAttachShader(prog, s1); glAttachShader(prog, s2);
	glBindAttribLocation(prog, 0, "pos");
	glLinkProgram(prog);
	GLint ok; glGetProgramiv(prog, GL_LINK_STATUS, &ok); CHECK(ok);
	glUseProgram(prog);
	u_t = glGetUniformLocation(prog, "t");
	u_c = glGetUniformLocation(prog, "col");
}

static void draw_frame(int f)
{
	static const GLfloat tri[] = { -0.6f, -0.5f, 0, 1,  0.6f, -0.5f, 0, 1,  0, 0.7f, 0, 1 };
	glViewport(0, 0, W, H);
	glClearColor(0.1f, 0.1f, 0.3f, 1);
	glClear(GL_COLOR_BUFFER_BIT);
	glVertexAttribPointer(0, 4, GL_FLOAT, GL_FALSE, 0, tri);
	glEnableVertexAttribArray(0);
	glUniform1f(u_t, f * 0.05f);
	glUniform4f(u_c, 1.0f, 0.2f, 0.1f, 1);
	glDrawArrays(GL_TRIANGLES, 0, 3);
}

int main(int argc, char **argv)
{
	const char *what = argc > 1 ? argv[1] : "import";
	int frames = 300, show = 0, novsync = 0;
	uint32_t fourcc = DRM_FORMAT_XRGB8888;
	for (int i = 2; i < argc; i++) {
		if (!strcmp(argv[i], "--show")) show = 1;
		else if (!strcmp(argv[i], "--novsync")) novsync = 1;
		else if (!strcmp(argv[i], "--fmt=xbgr")) fourcc = DRM_FORMAT_XBGR8888;
		else if (!strcmp(argv[i], "--fmt=argb")) fourcc = DRM_FORMAT_ARGB8888;
		else if (!strcmp(argv[i], "--fmt=abgr")) fourcc = DRM_FORMAT_ABGR8888;
		else frames = atoi(argv[i]);
	}
	int import = !strcmp(what, "import");
	kms_init();
	struct buf b[NBUF];
	for (int i = 0; i < NBUF; i++)
		buf_create(&b[i], fourcc);
	printf("dumb buffers: %ux%u pitch %u size %llu fourcc %.4s, dma-buf fds %d %d %d\n", W, H, b[0].pitch,
	       (unsigned long long)b[0].size, (char *)&fourcc, b[0].fd, b[1].fd, b[2].fd);

	egl_init(import ? 16 : W, import ? 16 : H);
	printf("EGL_EXTENSIONS: %s\n", eglQueryString(dpy, EGL_EXTENSIONS));
	gl_scene_init();

	if (import) {
		PFNEGLCREATEIMAGEKHRPROC createImage = (void *)eglGetProcAddress("eglCreateImageKHR");
		PFNGLEGLIMAGETARGETTEXTURE2DOESPROC targetTex = (void *)eglGetProcAddress("glEGLImageTargetTexture2DOES");
		CHECK(createImage && targetTex);
		for (int i = 0; i < NBUF; i++) {
			EGLint at[] = { EGL_WIDTH, W, EGL_HEIGHT, H, EGL_LINUX_DRM_FOURCC_EXT, fourcc,
					EGL_DMA_BUF_PLANE0_FD_EXT, b[i].fd, EGL_DMA_BUF_PLANE0_OFFSET_EXT, 0,
					EGL_DMA_BUF_PLANE0_PITCH_EXT, b[i].pitch, EGL_NONE };
			double t0 = now();
			b[i].img = createImage(dpy, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, NULL, at);
			printf("eglCreateImageKHR(dma-buf %d): %p (0x%x) %.2f ms\n", i, b[i].img, eglGetError(), (now() - t0) * 1e3);
			CHECK(b[i].img != EGL_NO_IMAGE_KHR);
			glGenTextures(1, &b[i].tex);
			glBindTexture(GL_TEXTURE_2D, b[i].tex);
			targetTex(GL_TEXTURE_2D, b[i].img);
			CHECK(glGetError() == GL_NO_ERROR);
			glGenFramebuffers(1, &b[i].fbo);
			glBindFramebuffer(GL_FRAMEBUFFER, b[i].fbo);
			glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, b[i].tex, 0);
			GLenum st = glCheckFramebufferStatus(GL_FRAMEBUFFER);
			printf("FBO %d status 0x%x %s\n", i, st, st == GL_FRAMEBUFFER_COMPLETE ? "COMPLETE" : "INCOMPLETE");
			CHECK(st == GL_FRAMEBUFFER_COMPLETE);
		}
		// correctness: clear buffer 0 to known colour, check the CPU view
		glBindFramebuffer(GL_FRAMEBUFFER, b[0].fbo);
		glViewport(0, 0, W, H);
		glClearColor(1.0f, 0.0f, 0.0f, 1.0f); // red
		glClear(GL_COLOR_BUFFER_BIT);
		glClearColor(0.0f, 1.0f, 0.0f, 1.0f);
		glEnable(GL_SCISSOR_TEST);
		glScissor(0, 0, W / 2, H / 2); // bottom-left quarter green
		glClear(GL_COLOR_BUFFER_BIT);
		glDisable(GL_SCISSOR_TEST);
		glFinish();
		unsigned char *m = b[0].map;
		unsigned char *top = m, *bot = m + (size_t)(H - 1) * b[0].pitch;
		printf("dma-buf CPU view after GPU clear: row0[0]=%02x %02x %02x %02x  rowH-1[0]=%02x %02x %02x %02x\n",
		       top[0], top[1], top[2], top[3], bot[0], bot[1], bot[2], bot[3]);
		printf("  (xrgb bytes are B,G,R,X: red=00 00 ff ff, green=00 ff 00 ff; GL row0 is bottom unless flipped)\n");
		int red_any = (top[2] > 200 && top[1] < 50) || (bot[2] > 200 && bot[1] < 50);
		int green_any = (top[1] > 200 && top[2] < 50) || (bot[1] > 200 && bot[2] < 50);
		printf("IMPORT-RENDER %s\n", (fourcc == DRM_FORMAT_XRGB8888 || fourcc == DRM_FORMAT_ARGB8888) ? (red_any && green_any ? "PASS" : "FAIL") : "(check bytes manually)");
	}

	drmModeCrtc *saved = NULL;
	if (show) {
		saved = drmModeGetCrtc(drm, crtc_id);
		CHECK(drmSetMaster(drm) == 0 || errno == EBUSY || 1);
		CHECK(drmModeSetCrtc(drm, crtc_id, b[0].fb, 0, 0, &conn_id, 1, &mode) == 0);
	}
	double t0 = now(), tgpu = 0, tcopy = 0;
	int cur = 1;
	for (int f = 0; f < frames; f++) {
		double a = now();
		if (import) {
			glBindFramebuffer(GL_FRAMEBUFFER, b[cur].fbo);
			draw_frame(f);
			glFinish();
			tgpu += now() - a;
		} else {
			draw_frame(f);
			glFinish();
			double c = now();
			tgpu += c - a;
			glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, b[cur].map); // pitch == W*4
			tcopy += now() - c;
		}
		if (show) {
			if (novsync) {
				drmModeSetCrtc(drm, crtc_id, b[cur].fb, 0, 0, &conn_id, 1, &mode);
			} else {
				wait_flip();
				if (drmModePageFlip(drm, crtc_id, b[cur].fb, DRM_MODE_PAGE_FLIP_EVENT, NULL) == 0)
					flip_pending = 1;
				else
					fprintf(stderr, "pageflip: %s\n", strerror(errno));
			}
		}
		cur = (cur + 1) % NBUF;
	}
	if (show) wait_flip();
	double dt = now() - t0;
	printf("%s: %d frames %.3f s -> %.1f fps (render+finish %.2f ms/frame%s%s)\n", what, frames, dt, frames / dt,
	       tgpu / frames * 1e3, import ? "" : ", glReadPixels into dumb buffer ", "");
	if (!import) printf("  readback %.2f ms/frame\n", tcopy / frames * 1e3);
	if (show && saved) {
		drmModeSetCrtc(drm, saved->crtc_id, saved->buffer_id, saved->x, saved->y, &conn_id, 1, &saved->mode);
		drmModeFreeCrtc(saved);
	}
	return 0;
}
