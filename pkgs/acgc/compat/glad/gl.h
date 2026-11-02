/* SPDX-License-Identifier: MIT
 * glad/gl.h for the Mortar (TrimUI Brick) build: OpenGL ES 3 through a
 * run-time loader.
 *
 * The port includes <glad/gl.h> (glad2, desktop GL 3.3). The Android and
 * Switch builds swap in a shim that includes <GLES3/gl3.h> and links GLES
 * directly. Here GLES must come from whatever SDL loaded (the stock PowerVR
 * libGLESv2 on the panel, libmortar-wlegl.so inside mortar-shell, which
 * wraps framebuffer 0, or Mesa as the software fallback), so every entry
 * point is a pointer filled by gladLoadGL(SDL_GL_GetProcAddress) once the
 * context exists (mortar_glue.c). Calling a name missing from AC_GLES_FUNCS
 * fails to compile (no prototypes), so the list cannot silently fall short.
 */
#ifndef AC_MORTAR_GLAD_GL_H
#define AC_MORTAR_GLAD_GL_H

#define GL_GLES_PROTOTYPES 0
#include <GLES3/gl32.h>
#include <GLES2/gl2ext.h> /* EXT_buffer_storage */

#ifdef __cplusplus
extern "C" {
#endif

#define AC_GLES_FUNCS(X) \
    X(ActiveTexture, ACTIVETEXTURE) \
    X(AttachShader, ATTACHSHADER) \
    X(BindAttribLocation, BINDATTRIBLOCATION) \
    X(BindBuffer, BINDBUFFER) \
    X(BindBufferBase, BINDBUFFERBASE) \
    X(BindFramebuffer, BINDFRAMEBUFFER) \
    X(BindRenderbuffer, BINDRENDERBUFFER) \
    X(BindTexture, BINDTEXTURE) \
    X(BindVertexArray, BINDVERTEXARRAY) \
    X(BlendEquation, BLENDEQUATION) \
    X(BlendEquationSeparate, BLENDEQUATIONSEPARATE) \
    X(BlendFunc, BLENDFUNC) \
    X(BlendFuncSeparate, BLENDFUNCSEPARATE) \
    X(BlitFramebuffer, BLITFRAMEBUFFER) \
    X(BufferData, BUFFERDATA) \
    X(BufferStorageEXT, BUFFERSTORAGEEXT) \
    X(BufferSubData, BUFFERSUBDATA) \
    X(CheckFramebufferStatus, CHECKFRAMEBUFFERSTATUS) \
    X(Clear, CLEAR) \
    X(ClearColor, CLEARCOLOR) \
    X(ClearDepthf, CLEARDEPTHF) \
    X(ClearStencil, CLEARSTENCIL) \
    X(ClientWaitSync, CLIENTWAITSYNC) \
    X(ColorMask, COLORMASK) \
    X(CompileShader, COMPILESHADER) \
    X(CompressedTexImage2D, COMPRESSEDTEXIMAGE2D) \
    X(CreateProgram, CREATEPROGRAM) \
    X(CreateShader, CREATESHADER) \
    X(CullFace, CULLFACE) \
    X(DeleteBuffers, DELETEBUFFERS) \
    X(DeleteFramebuffers, DELETEFRAMEBUFFERS) \
    X(DeleteProgram, DELETEPROGRAM) \
    X(DeleteRenderbuffers, DELETERENDERBUFFERS) \
    X(DeleteShader, DELETESHADER) \
    X(DeleteSync, DELETESYNC) \
    X(DeleteTextures, DELETETEXTURES) \
    X(DeleteVertexArrays, DELETEVERTEXARRAYS) \
    X(DepthFunc, DEPTHFUNC) \
    X(DepthMask, DEPTHMASK) \
    X(DepthRangef, DEPTHRANGEF) \
    X(DetachShader, DETACHSHADER) \
    X(Disable, DISABLE) \
    X(DisableVertexAttribArray, DISABLEVERTEXATTRIBARRAY) \
    X(DrawArrays, DRAWARRAYS) \
    X(DrawBuffers, DRAWBUFFERS) \
    X(DrawElements, DRAWELEMENTS) \
    X(DrawElementsBaseVertex, DRAWELEMENTSBASEVERTEX) \
    X(Enable, ENABLE) \
    X(EnableVertexAttribArray, ENABLEVERTEXATTRIBARRAY) \
    X(FenceSync, FENCESYNC) \
    X(Finish, FINISH) \
    X(Flush, FLUSH) \
    X(FramebufferRenderbuffer, FRAMEBUFFERRENDERBUFFER) \
    X(FramebufferTexture2D, FRAMEBUFFERTEXTURE2D) \
    X(FrontFace, FRONTFACE) \
    X(GenBuffers, GENBUFFERS) \
    X(GenerateMipmap, GENERATEMIPMAP) \
    X(GenFramebuffers, GENFRAMEBUFFERS) \
    X(GenRenderbuffers, GENRENDERBUFFERS) \
    X(GenTextures, GENTEXTURES) \
    X(GenVertexArrays, GENVERTEXARRAYS) \
    X(GetAttribLocation, GETATTRIBLOCATION) \
    X(GetError, GETERROR) \
    X(GetFloatv, GETFLOATV) \
    X(GetIntegerv, GETINTEGERV) \
    X(GetProgramBinary, GETPROGRAMBINARY) \
    X(GetProgramInfoLog, GETPROGRAMINFOLOG) \
    X(GetProgramiv, GETPROGRAMIV) \
    X(GetShaderInfoLog, GETSHADERINFOLOG) \
    X(GetShaderiv, GETSHADERIV) \
    X(GetShaderSource, GETSHADERSOURCE) \
    X(GetString, GETSTRING) \
    X(GetStringi, GETSTRINGI) \
    X(GetUniformBlockIndex, GETUNIFORMBLOCKINDEX) \
    X(GetUniformLocation, GETUNIFORMLOCATION) \
    X(Hint, HINT) \
    X(InvalidateFramebuffer, INVALIDATEFRAMEBUFFER) \
    X(IsEnabled, ISENABLED) \
    X(LineWidth, LINEWIDTH) \
    X(LinkProgram, LINKPROGRAM) \
    X(MapBufferRange, MAPBUFFERRANGE) \
    X(PixelStorei, PIXELSTOREI) \
    X(PolygonOffset, POLYGONOFFSET) \
    X(ProgramBinary, PROGRAMBINARY) \
    X(ReadBuffer, READBUFFER) \
    X(ReadPixels, READPIXELS) \
    X(RenderbufferStorage, RENDERBUFFERSTORAGE) \
    X(Scissor, SCISSOR) \
    X(ShaderSource, SHADERSOURCE) \
    X(StencilFunc, STENCILFUNC) \
    X(StencilMask, STENCILMASK) \
    X(StencilOp, STENCILOP) \
    X(TexImage2D, TEXIMAGE2D) \
    X(TexParameterf, TEXPARAMETERF) \
    X(TexParameteri, TEXPARAMETERI) \
    X(TexStorage2D, TEXSTORAGE2D) \
    X(TexSubImage2D, TEXSUBIMAGE2D) \
    X(Uniform1f, UNIFORM1F) \
    X(Uniform1fv, UNIFORM1FV) \
    X(Uniform1i, UNIFORM1I) \
    X(Uniform1iv, UNIFORM1IV) \
    X(Uniform2f, UNIFORM2F) \
    X(Uniform2fv, UNIFORM2FV) \
    X(Uniform2i, UNIFORM2I) \
    X(Uniform2iv, UNIFORM2IV) \
    X(Uniform3f, UNIFORM3F) \
    X(Uniform3fv, UNIFORM3FV) \
    X(Uniform3i, UNIFORM3I) \
    X(Uniform3iv, UNIFORM3IV) \
    X(Uniform4f, UNIFORM4F) \
    X(Uniform4fv, UNIFORM4FV) \
    X(Uniform4i, UNIFORM4I) \
    X(Uniform4iv, UNIFORM4IV) \
    X(UniformBlockBinding, UNIFORMBLOCKBINDING) \
    X(UniformMatrix3fv, UNIFORMMATRIX3FV) \
    X(UniformMatrix4fv, UNIFORMMATRIX4FV) \
    X(UnmapBuffer, UNMAPBUFFER) \
    X(UseProgram, USEPROGRAM) \
    X(VertexAttrib4f, VERTEXATTRIB4F) \
    X(VertexAttribIPointer, VERTEXATTRIBIPOINTER) \
    X(VertexAttribPointer, VERTEXATTRIBPOINTER) \
    X(Viewport, VIEWPORT)

#define AC_GLES_DECLARE(name, NAME) extern PFNGL##NAME##PROC mgl_gl##name;
AC_GLES_FUNCS(AC_GLES_DECLARE)
#undef AC_GLES_DECLARE

/* every gl* call in the port goes through the pointers */
#define glActiveTexture mgl_glActiveTexture
#define glAttachShader mgl_glAttachShader
#define glBindAttribLocation mgl_glBindAttribLocation
#define glBindBuffer mgl_glBindBuffer
#define glBindBufferBase mgl_glBindBufferBase
#define glBindFramebuffer mgl_glBindFramebuffer
#define glBindRenderbuffer mgl_glBindRenderbuffer
#define glBindTexture mgl_glBindTexture
#define glBindVertexArray mgl_glBindVertexArray
#define glBlendEquation mgl_glBlendEquation
#define glBlendEquationSeparate mgl_glBlendEquationSeparate
#define glBlendFunc mgl_glBlendFunc
#define glBlendFuncSeparate mgl_glBlendFuncSeparate
#define glBlitFramebuffer mgl_glBlitFramebuffer
#define glBufferData mgl_glBufferData
#define glBufferStorageEXT mgl_glBufferStorageEXT
#define glBufferSubData mgl_glBufferSubData
#define glCheckFramebufferStatus mgl_glCheckFramebufferStatus
#define glClear mgl_glClear
#define glClearColor mgl_glClearColor
#define glClearDepthf mgl_glClearDepthf
#define glClearStencil mgl_glClearStencil
#define glClientWaitSync mgl_glClientWaitSync
#define glColorMask mgl_glColorMask
/* compile errors go to the log in full (mortar_glue.c) */
void ac_glCompileShader(GLuint shader);
#define glCompileShader ac_glCompileShader
#define glCompressedTexImage2D mgl_glCompressedTexImage2D
#define glCreateProgram mgl_glCreateProgram
#define glCreateShader mgl_glCreateShader
#define glCullFace mgl_glCullFace
#define glDeleteBuffers mgl_glDeleteBuffers
#define glDeleteFramebuffers mgl_glDeleteFramebuffers
#define glDeleteProgram mgl_glDeleteProgram
#define glDeleteRenderbuffers mgl_glDeleteRenderbuffers
#define glDeleteShader mgl_glDeleteShader
#define glDeleteSync mgl_glDeleteSync
#define glDeleteTextures mgl_glDeleteTextures
#define glDeleteVertexArrays mgl_glDeleteVertexArrays
#define glDepthFunc mgl_glDepthFunc
#define glDepthMask mgl_glDepthMask
#define glDepthRangef mgl_glDepthRangef
#define glDetachShader mgl_glDetachShader
#define glDisable mgl_glDisable
#define glDisableVertexAttribArray mgl_glDisableVertexAttribArray
#define glDrawArrays mgl_glDrawArrays
#define glDrawBuffers mgl_glDrawBuffers
#define glDrawElements mgl_glDrawElements
#define glDrawElementsBaseVertex mgl_glDrawElementsBaseVertex
#define glEnable mgl_glEnable
#define glEnableVertexAttribArray mgl_glEnableVertexAttribArray
#define glFenceSync mgl_glFenceSync
#define glFinish mgl_glFinish
#define glFlush mgl_glFlush
#define glFramebufferRenderbuffer mgl_glFramebufferRenderbuffer
#define glFramebufferTexture2D mgl_glFramebufferTexture2D
#define glFrontFace mgl_glFrontFace
#define glGenBuffers mgl_glGenBuffers
#define glGenerateMipmap mgl_glGenerateMipmap
#define glGenFramebuffers mgl_glGenFramebuffers
#define glGenRenderbuffers mgl_glGenRenderbuffers
#define glGenTextures mgl_glGenTextures
#define glGenVertexArrays mgl_glGenVertexArrays
#define glGetAttribLocation mgl_glGetAttribLocation
#define glGetError mgl_glGetError
#define glGetFloatv mgl_glGetFloatv
#define glGetIntegerv mgl_glGetIntegerv
#define glGetProgramBinary mgl_glGetProgramBinary
#define glGetProgramInfoLog mgl_glGetProgramInfoLog
#define glGetProgramiv mgl_glGetProgramiv
#define glGetShaderInfoLog mgl_glGetShaderInfoLog
#define glGetShaderiv mgl_glGetShaderiv
#define glGetString mgl_glGetString
#define glGetStringi mgl_glGetStringi
#define glGetUniformBlockIndex mgl_glGetUniformBlockIndex
#define glGetUniformLocation mgl_glGetUniformLocation
#define glHint mgl_glHint
#define glInvalidateFramebuffer mgl_glInvalidateFramebuffer
#define glIsEnabled mgl_glIsEnabled
#define glLineWidth mgl_glLineWidth
#define glLinkProgram mgl_glLinkProgram
#define glMapBufferRange mgl_glMapBufferRange
#define glPixelStorei mgl_glPixelStorei
#define glPolygonOffset mgl_glPolygonOffset
#define glProgramBinary mgl_glProgramBinary
#define glReadBuffer mgl_glReadBuffer
#define glReadPixels mgl_glReadPixels
#define glRenderbufferStorage mgl_glRenderbufferStorage
#define glScissor mgl_glScissor
#define glStencilFunc mgl_glStencilFunc
#define glStencilMask mgl_glStencilMask
#define glStencilOp mgl_glStencilOp
#define glTexImage2D mgl_glTexImage2D
#define glTexParameterf mgl_glTexParameterf
#define glTexParameteri mgl_glTexParameteri
#define glTexStorage2D mgl_glTexStorage2D
#define glTexSubImage2D mgl_glTexSubImage2D
#define glUniform1f mgl_glUniform1f
#define glUniform1fv mgl_glUniform1fv
#define glUniform1i mgl_glUniform1i
#define glUniform1iv mgl_glUniform1iv
#define glUniform2f mgl_glUniform2f
#define glUniform2fv mgl_glUniform2fv
#define glUniform2i mgl_glUniform2i
#define glUniform2iv mgl_glUniform2iv
#define glUniform3f mgl_glUniform3f
#define glUniform3fv mgl_glUniform3fv
#define glUniform3i mgl_glUniform3i
#define glUniform3iv mgl_glUniform3iv
#define glUniform4f mgl_glUniform4f
#define glUniform4fv mgl_glUniform4fv
#define glUniform4i mgl_glUniform4i
#define glUniform4iv mgl_glUniform4iv
#define glUniformBlockBinding mgl_glUniformBlockBinding
#define glUniformMatrix3fv mgl_glUniformMatrix3fv
#define glUniformMatrix4fv mgl_glUniformMatrix4fv
#define glUnmapBuffer mgl_glUnmapBuffer
#define glUseProgram mgl_glUseProgram
#define glVertexAttrib4f mgl_glVertexAttrib4f
#define glVertexAttribIPointer mgl_glVertexAttribIPointer
#define glVertexAttribPointer mgl_glVertexAttribPointer
#define glViewport mgl_glViewport

/* The port calls the uniform uploads through glad2's prefixed names too. */
#define glad_glUniform1f mgl_glUniform1f
#define glad_glUniform1i mgl_glUniform1i
#define glad_glUniform1iv mgl_glUniform1iv
#define glad_glUniform2f mgl_glUniform2f
#define glad_glUniform2i mgl_glUniform2i
#define glad_glUniform2iv mgl_glUniform2iv
#define glad_glUniform3f mgl_glUniform3f
#define glad_glUniform3fv mgl_glUniform3fv
#define glad_glUniform3i mgl_glUniform3i
#define glad_glUniform3iv mgl_glUniform3iv
#define glad_glUniform4f mgl_glUniform4f
#define glad_glUniform4fv mgl_glUniform4fv
#define glad_glUniform4i mgl_glUniform4i
#define glad_glUniform4iv mgl_glUniform4iv
#define glad_glUniformMatrix3fv mgl_glUniformMatrix3fv
#define glad_glUniformMatrix4fv mgl_glUniformMatrix4fv

/* Desktop-only entry points -> ES equivalents (as the Android shim) */
#define glClearDepth(depth) mgl_glClearDepthf((GLfloat)(depth))
#define glDepthRange(n, f) mgl_glDepthRangef((GLfloat)(n), (GLfloat)(f))
#define glPointSize(size) ((void)0)

/* Desktop GLSL (#version 330 core) -> GLSL ES 3.00, in mortar_glue.c */
int ac_glShaderSource(GLuint shader, GLsizei count, const GLchar* const* string, const GLint* length);
#undef glShaderSource
#define glShaderSource(shader, count, string, length) ac_glShaderSource((shader), (count), (string), (length))

/* glad2's loader entry: fills the pointers (mortar_glue.c) */
typedef void* (*GLADloadfunc)(const char* name);
int gladLoadGL(GLADloadfunc load);

#ifdef __cplusplus
}
#endif

#endif /* AC_MORTAR_GLAD_GL_H */
