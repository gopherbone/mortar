/* SPDX-License-Identifier: MIT
 * mortar_glue.c: the Android glue API (app/src/main/cpp/android/android_glue.h)
 * for the Mortar build, which compiles the port with TARGET_ANDROID's code
 * paths (OpenGL ES 3 context, ES-compatible renderer) the way the Switch
 * build does with switch_glue.c.
 *
 *  - android_bootstrap(): the launcher (bin/acgc) has already changed into
 *    the data directory, so only make the folders and unbuffer the logs
 *    (they go to the shell's journal).
 *  - ac_glShaderSource(): desktop GLSL (#version 330 core) -> GLSL ES 3.00
 *    with precision qualifiers, as android_glue.c.
 *  - gladLoadGL(): every GLES entry point through SDL_GL_GetProcAddress
 *    (compat/glad/gl.h), so the port draws with whatever EGL/GLES SDL
 *    loaded: mortar-wlegl inside mortar-shell, the stock PowerVR libs on the
 *    panel, or Mesa as the software fallback.
 *  - the touch overlay's inputs, which stay zero.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#include <SDL.h>
#include <glad/gl.h>

#include "android_glue.h"

#define AC_GLES_DEFINE(name, NAME) PFNGL##NAME##PROC mgl_gl##name;
AC_GLES_FUNCS(AC_GLES_DEFINE)
#undef AC_GLES_DEFINE

int gladLoadGL(GLADloadfunc load) {
    int missing = 0;
#define AC_GLES_LOAD(name, NAME)                                         \
    mgl_gl##name = (PFNGL##NAME##PROC)load("gl" #name);                 \
    if (!mgl_gl##name) {                                                 \
        missing++;                                                       \
        fprintf(stderr, "[AC/Mortar] GLES entry point gl" #name " not found\n"); \
    }
    AC_GLES_FUNCS(AC_GLES_LOAD)
#undef AC_GLES_LOAD
    if (!mgl_glGetString) return 0;
    printf("[AC/Mortar] GL: %s / %s / %s (%d entry points missing)\n",
           (const char*)mgl_glGetString(GL_VERSION), (const char*)mgl_glGetString(GL_RENDERER),
           (const char*)mgl_glGetString(GL_SHADING_LANGUAGE_VERSION), missing);
    return 1;
}

void android_bootstrap(void) {
    mkdir("rom", 0755);
    mkdir("save", 0755);
    mkdir("save/card_a", 0755);
    mkdir("save/card_b", 0755);
    mkdir("texture_pack", 0755);
    setvbuf(stdout, NULL, _IOLBF, 0);
    setvbuf(stderr, NULL, _IONBF, 0);
}

static char* rewrite_glsl(const char* src) {
    static const char prefix[] =
        "#version 300 es\n"
        "precision highp float;\n"
        "precision highp int;\n"
        "precision highp sampler2D;\n"
        /* GX lighting per vertex, as the GameCube does it and as the Switch
         * build does (default.vert/.frag): per pixel it costs fill rate,
         * which is what the GE8300 has least of */
        "#define AC_VERTEX_LIGHTING 1\n";
    const char* body = src;
    if (strncmp(src, "#version", 8) == 0) {
        const char* nl = strchr(src, '\n');
        body = nl ? nl + 1 : src + strlen(src);
    }
    size_t n = strlen(prefix) + strlen(body) + 1;
    char* out = (char*)malloc(n);
    if (!out) return NULL;
    memcpy(out, prefix, sizeof(prefix) - 1);
    strcpy(out + sizeof(prefix) - 1, body);
    return out;
}

int ac_glShaderSource(GLuint shader, GLsizei count, const GLchar* const* string, const GLint* length) {
    if (count <= 0 || !string) return -1;
    size_t total = 1;
    for (GLsizei i = 0; i < count; i++)
        total += (length && length[i] >= 0) ? (size_t)length[i] : strlen(string[i]);
    char* src = (char*)malloc(total);
    if (!src) return -1;
    size_t off = 0;
    for (GLsizei i = 0; i < count; i++) {
        size_t n = (length && length[i] >= 0) ? (size_t)length[i] : strlen(string[i]);
        memcpy(src + off, string[i], n);
        off += n;
    }
    src[off] = '\0';
    char* rw = rewrite_glsl(src);
    free(src);
    if (!rw) return -1;
    const GLchar* p = rw;
    mgl_glShaderSource(shader, 1, &p, NULL);
    free(rw);
    return 0;
}

/* pc_pad.c reads the Android touch overlay; there is none here. */
volatile int g_android_touch_buttons = 0;
volatile int g_android_touch_stick_x = 0;
volatile int g_android_touch_stick_y = 0;
volatile int g_android_touch_cstick_x = 0;
volatile int g_android_touch_cstick_y = 0;
