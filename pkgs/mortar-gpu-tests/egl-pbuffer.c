// SPDX-License-Identifier: MIT
// Minimal EGL pbuffer + GLES2 test for the stock PowerVR DDK 1.19 userspace.
// 1. eglInitialize on EGL_DEFAULT_DISPLAY, print vendor/version/extensions
// 2. 1024x768 pbuffer, glClear colour -> glReadPixels check
// 3. shader triangle -> readback check
// 4. throughput: fill-rate (blended full-screen quads), triangles/s, readback cost
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define W 1024
#define H 768

static double now(void)
{
	struct timespec t;
	clock_gettime(CLOCK_MONOTONIC, &t);
	return t.tv_sec + t.tv_nsec * 1e-9;
}

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %s:%d %s egl=0x%x gl=0x%x\n", __FILE__, __LINE__, #x, eglGetError(), glGetError()); exit(1); } } while (0)

static GLuint sh(GLenum type, const char *src)
{
	GLuint s = glCreateShader(type);
	glShaderSource(s, 1, &src, NULL);
	glCompileShader(s);
	GLint ok;
	glGetShaderiv(s, GL_COMPILE_STATUS, &ok);
	if (!ok) { char log[1024]; glGetShaderInfoLog(s, sizeof log, NULL, log); fprintf(stderr, "shader: %s\n", log); exit(1); }
	return s;
}

static GLuint prog(const char *vs, const char *fs)
{
	GLuint p = glCreateProgram();
	glAttachShader(p, sh(GL_VERTEX_SHADER, vs));
	glAttachShader(p, sh(GL_FRAGMENT_SHADER, fs));
	glBindAttribLocation(p, 0, "pos");
	glLinkProgram(p);
	GLint ok;
	glGetProgramiv(p, GL_LINK_STATUS, &ok);
	CHECK(ok);
	return p;
}

static int px_eq(const unsigned char *p, int r, int g, int b)
{
	return abs(p[0] - r) <= 2 && abs(p[1] - g) <= 2 && abs(p[2] - b) <= 2;
}

int main(int argc, char **argv)
{
	int frames = argc > 1 ? atoi(argv[1]) : 100;
	EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
	CHECK(dpy != EGL_NO_DISPLAY);
	EGLint maj, min;
	double t0 = now();
	CHECK(eglInitialize(dpy, &maj, &min));
	printf("eglInitialize %d.%d in %.1f ms\n", maj, min, (now() - t0) * 1e3);
	printf("EGL_VENDOR: %s\nEGL_VERSION: %s\nEGL_CLIENT_APIS: %s\nEGL_EXTENSIONS: %s\n",
	       eglQueryString(dpy, EGL_VENDOR), eglQueryString(dpy, EGL_VERSION),
	       eglQueryString(dpy, EGL_CLIENT_APIS), eglQueryString(dpy, EGL_EXTENSIONS));

	EGLint cattr[] = { EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
			   EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_NONE };
	EGLConfig cfg;
	EGLint n;
	CHECK(eglChooseConfig(dpy, cattr, &cfg, 1, &n) && n == 1);
	EGLint all;
	eglGetConfigs(dpy, NULL, 0, &all);
	printf("configs: %d total\n", all);
	{
		EGLConfig cs[64]; EGLint k; eglGetConfigs(dpy, cs, 64, &k);
		for (int i = 0; i < k; i++) {
			EGLint st, r, g, b, a, d, vid;
			eglGetConfigAttrib(dpy, cs[i], EGL_SURFACE_TYPE, &st);
			eglGetConfigAttrib(dpy, cs[i], EGL_RED_SIZE, &r); eglGetConfigAttrib(dpy, cs[i], EGL_GREEN_SIZE, &g);
			eglGetConfigAttrib(dpy, cs[i], EGL_BLUE_SIZE, &b); eglGetConfigAttrib(dpy, cs[i], EGL_ALPHA_SIZE, &a);
			eglGetConfigAttrib(dpy, cs[i], EGL_DEPTH_SIZE, &d); eglGetConfigAttrib(dpy, cs[i], EGL_NATIVE_VISUAL_ID, &vid);
			printf("  cfg%-2d rgba%d%d%d%d d%-2d surf=0x%x%s%s%s visual=0x%x\n", i, r, g, b, a, d, st,
			       st & EGL_WINDOW_BIT ? " WINDOW" : "", st & EGL_PBUFFER_BIT ? " PBUFFER" : "",
			       st & EGL_PIXMAP_BIT ? " PIXMAP" : "", vid);
		}
	}
	EGLint sattr[] = { EGL_WIDTH, W, EGL_HEIGHT, H, EGL_NONE };
	EGLSurface surf = eglCreatePbufferSurface(dpy, cfg, sattr);
	CHECK(surf != EGL_NO_SURFACE);
	EGLint ctxattr[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
	EGLContext ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, ctxattr);
	CHECK(ctx != EGL_NO_CONTEXT);
	CHECK(eglMakeCurrent(dpy, surf, surf, ctx));
	printf("GL_VENDOR: %s\nGL_RENDERER: %s\nGL_VERSION: %s\nGL_SLV: %s\nGL_EXTENSIONS: %s\n",
	       glGetString(GL_VENDOR), glGetString(GL_RENDERER), glGetString(GL_VERSION),
	       glGetString(GL_SHADING_LANGUAGE_VERSION), glGetString(GL_EXTENSIONS));

	static unsigned char buf[W * H * 4];
	// --- clear test
	glClearColor(1.0f, 0.5f, 0.25f, 1.0f);
	glClear(GL_COLOR_BUFFER_BIT);
	glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, buf);
	int okc = px_eq(buf, 255, 128, 64) && px_eq(buf + (W * H - 1) * 4, 255, 128, 64);
	printf("clear test: pixel0=%d,%d,%d,%d -> %s\n", buf[0], buf[1], buf[2], buf[3], okc ? "PASS" : "FAIL");

	// --- triangle test
	GLuint p = prog("attribute vec4 pos; void main(){gl_Position=pos;}",
			"precision mediump float; uniform vec4 c; void main(){gl_FragColor=c;}");
	glUseProgram(p);
	GLint uc = glGetUniformLocation(p, "c");
	static const GLfloat tri[] = { -1, -1, 0, 1,  1, -1, 0, 1,  -1, 1, 0, 1 };
	glVertexAttribPointer(0, 4, GL_FLOAT, GL_FALSE, 0, tri);
	glEnableVertexAttribArray(0);
	glClearColor(0, 0, 0, 1);
	glClear(GL_COLOR_BUFFER_BIT);
	glUniform4f(uc, 0, 1, 0, 1);
	glDrawArrays(GL_TRIANGLES, 0, 3);
	glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, buf);
	unsigned char *bl = buf + (10 * W + 10) * 4, *tr = buf + ((H - 10) * W + (W - 10)) * 4;
	int okt = px_eq(bl, 0, 255, 0) && px_eq(tr, 0, 0, 0);
	printf("triangle test: bottom-left=%d,%d,%d top-right=%d,%d,%d -> %s\n", bl[0], bl[1], bl[2], tr[0], tr[1], tr[2], okt ? "PASS" : "FAIL");

	// --- fill rate: N blended full-screen quads per frame
	static const GLfloat quad[] = { -1, -1, 0, 1,  1, -1, 0, 1,  -1, 1, 0, 1,  1, 1, 0, 1 };
	glVertexAttribPointer(0, 4, GL_FLOAT, GL_FALSE, 0, quad);
	glEnable(GL_BLEND);
	glBlendFunc(GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA);
	int layers = 8;
	glFinish();
	t0 = now();
	for (int f = 0; f < frames; f++) {
		glClear(GL_COLOR_BUFFER_BIT);
		for (int l = 0; l < layers; l++) {
			glUniform4f(uc, l / 8.0f, 0.5f, 1 - l / 8.0f, 0.5f);
			glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
		}
		eglSwapBuffers(dpy, surf); // pbuffer: no-op flush point
		glFinish();
	}
	double dt = now() - t0;
	printf("fill: %d frames x %d blended 1024x768 quads in %.3f s -> %.1f fps, %.1f Mpix/s\n",
	       frames, layers, dt, frames / dt, (double)frames * layers * W * H / dt / 1e6);
	glDisable(GL_BLEND);

	// --- triangle throughput: many small triangles in a VBO
	int ntri = 100000;
	GLfloat *v = malloc(sizeof(GLfloat) * 12 * ntri);
	srand(1);
	for (int i = 0; i < ntri; i++) {
		float x = rand() / (float)RAND_MAX * 2 - 1, y = rand() / (float)RAND_MAX * 2 - 1;
		float s = 0.01f;
		GLfloat t[12] = { x, y, 0, 1, x + s, y, 0, 1, x, y + s, 0, 1 };
		memcpy(v + 12 * i, t, sizeof t);
	}
	GLuint vbo;
	glGenBuffers(1, &vbo);
	glBindBuffer(GL_ARRAY_BUFFER, vbo);
	glBufferData(GL_ARRAY_BUFFER, sizeof(GLfloat) * 12 * ntri, v, GL_STATIC_DRAW);
	glVertexAttribPointer(0, 4, GL_FLOAT, GL_FALSE, 0, 0);
	glFinish();
	int tf = frames / 4 + 1;
	t0 = now();
	for (int f = 0; f < tf; f++) {
		glClear(GL_COLOR_BUFFER_BIT);
		glDrawArrays(GL_TRIANGLES, 0, 3 * ntri);
		glFinish();
	}
	dt = now() - t0;
	printf("triangles: %d x %d small tris in %.3f s -> %.2f Mtri/s\n", tf, ntri, dt, (double)tf * ntri / dt / 1e6);
	glBindBuffer(GL_ARRAY_BUFFER, 0);

	// --- readback cost
	glFinish();
	t0 = now();
	int rb = 30;
	for (int i = 0; i < rb; i++)
		glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, buf);
	dt = now() - t0;
	printf("readback: glReadPixels 1024x768 RGBA %.2f ms each (%.1f MB/s)\n", dt / rb * 1e3, rb * W * H * 4 / dt / 1e6);
	t0 = now();
	for (int i = 0; i < rb; i++) {
		glClear(GL_COLOR_BUFFER_BIT);
		glReadPixels(0, 0, W, H, GL_RGBA, GL_UNSIGNED_BYTE, buf);
	}
	dt = now() - t0;
	printf("readback: clear+glReadPixels %.2f ms each\n", dt / rb * 1e3);

	eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
	eglTerminate(dpy);
	printf("RESULT %s\n", okc && okt ? "PASS" : "FAIL");
	return !(okc && okt);
}
