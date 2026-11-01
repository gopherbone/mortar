// SPDX-License-Identifier: MIT
// EGL *window* surface on the stock NULL_WSEGL: needs a DC device that can
// scan out (proposal/brick_dc_fbdev.ko). Renders a spinning triangle, swaps,
// reports fps. argv: [frames] [swap_interval] [native window handle as int]
#include <EGL/egl.h>
#include <GLES2/gl2.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#define CHECK(x) do { if (!(x)) { fprintf(stderr, "FAIL %s:%d %s egl=0x%x\n", __FILE__, __LINE__, #x, eglGetError()); exit(1); } } while (0)

static double now(void)
{
	struct timespec t;
	clock_gettime(CLOCK_MONOTONIC, &t);
	return t.tv_sec + t.tv_nsec * 1e-9;
}

/* what SDL2's mali-fbdev backend passes as the native window */
struct fbdev_window { unsigned short width, height; };

int main(int argc, char **argv)
{
	int frames = argc > 1 ? atoi(argv[1]) : 300;
	int interval = argc > 2 ? atoi(argv[2]) : 1;
	int nativemode = argc > 3 ? atoi(argv[3]) : 0;
	static struct fbdev_window fw = { 1024, 768 };
	EGLNativeWindowType win = nativemode ? (EGLNativeWindowType)&fw : (EGLNativeWindowType)0;

	EGLDisplay dpy = eglGetDisplay(EGL_DEFAULT_DISPLAY);
	CHECK(eglInitialize(dpy, NULL, NULL));
	EGLint ca[] = { EGL_SURFACE_TYPE, EGL_WINDOW_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
			EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE };
	EGLConfig cfg; EGLint n;
	CHECK(eglChooseConfig(dpy, ca, &cfg, 1, &n) && n == 1);
	double t0 = now();
	EGLSurface s = eglCreateWindowSurface(dpy, cfg, win, NULL);
	printf("eglCreateWindowSurface(%s) -> %p err 0x%x (%.1f ms)\n", nativemode ? "&fbdev_window" : "NULL", s, eglGetError(), (now() - t0) * 1e3);
	CHECK(s != EGL_NO_SURFACE);
	EGLint cx[] = { EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE };
	EGLContext c = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, cx);
	CHECK(eglMakeCurrent(dpy, s, s, c));
	EGLint w, h;
	eglQuerySurface(dpy, s, EGL_WIDTH, &w);
	eglQuerySurface(dpy, s, EGL_HEIGHT, &h);
	printf("window surface %dx%d, swap interval %d\n", w, h, interval);
	eglSwapInterval(dpy, interval);

	const char *vs = "attribute vec4 pos; uniform float t;"
			 "void main(){ float c=cos(t), s=sin(t); gl_Position=vec4(c*pos.x-s*pos.y, s*pos.x+c*pos.y, 0.0, 1.0); }";
	const char *fs = "precision mediump float; void main(){ gl_FragColor=vec4(1.0,0.6,0.1,1.0); }";
	GLuint a = glCreateShader(GL_VERTEX_SHADER), b = glCreateShader(GL_FRAGMENT_SHADER), p = glCreateProgram();
	glShaderSource(a, 1, &vs, NULL); glCompileShader(a);
	glShaderSource(b, 1, &fs, NULL); glCompileShader(b);
	glAttachShader(p, a); glAttachShader(p, b); glBindAttribLocation(p, 0, "pos"); glLinkProgram(p);
	glUseProgram(p);
	GLint ut = glGetUniformLocation(p, "t");
	static const GLfloat tri[] = { -0.6f, -0.5f, 0, 1, 0.6f, -0.5f, 0, 1, 0, 0.7f, 0, 1 };
	glVertexAttribPointer(0, 4, GL_FLOAT, GL_FALSE, 0, tri);
	glEnableVertexAttribArray(0);
	glViewport(0, 0, w, h);

	t0 = now();
	double worst = 0, last = t0;
	for (int f = 0; f < frames; f++) {
		glClearColor(0.0f, 0.15f, 0.1f, 1);
		glClear(GL_COLOR_BUFFER_BIT);
		glUniform1f(ut, f * 0.05f);
		glDrawArrays(GL_TRIANGLES, 0, 3);
		if (!eglSwapBuffers(dpy, s)) { fprintf(stderr, "eglSwapBuffers failed 0x%x at frame %d\n", eglGetError(), f); break; }
		double t = now();
		if (t - last > worst) worst = t - last;
		last = t;
	}
	double dt = now() - t0;
	printf("window: %d frames %.3f s -> %.1f fps, worst frame %.1f ms\n", frames, dt, frames / dt, worst * 1e3);
	eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
	eglDestroySurface(dpy, s);
	eglTerminate(dpy);
	return 0;
}
