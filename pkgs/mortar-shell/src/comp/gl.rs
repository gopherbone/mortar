// SPDX-License-Identifier: GPL-2.0-or-later
//! GLES 2 renderer for the shell's own UI on the PowerVR GE8300 (stock DDK
//! EGL/GLES, loaded with dlopen from `ui.glLibDir`, so the shell runs
//! without them).
//!
//! The proven path (pkgs/mortar-gpu-tests kms-gles, pkgs/mortar-wlegl): an
//! EGL context on the default display with a 16x16 pbuffer; sun4i dumb
//! buffers PRIME-exported and imported with EGL_EXT_image_dma_buf_import,
//! bound with glEGLImageTargetTexture2DOES and attached as FBO colour
//! targets; glFinish before the page flip (no implicit dma-buf fencing in
//! this DDK). GL row 0 is memory row 0, which is the top of the panel: the
//! renderer maps scene y straight to GL y, so frames come out upright.
//! Client dma-bufs (GPU apps) are sampled the same way (EGLImage -> texture),
//! wl_shm buffers are uploaded with glTexSubImage2D.
//!
//! Robustness: every call that can fail is checked; any EGL/GL error makes
//! the caller drop the GPU path for the session (and fall back to the CPU
//! renderer) after an orderly teardown. Nothing here creates EGL window
//! surfaces (the DC path that latched the GPU in early tests).

use crate::scene::{triangulate, Item, Scene, Src, Typo, P};
use mortar_ui::gfx::{Color, Face};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr, CString};
use std::os::fd::RawFd;
use std::path::Path;

type EGLDisplay = *mut c_void;
type EGLContext = *mut c_void;
type EGLSurface = *mut c_void;
type EGLConfig = *mut c_void;
type EGLImage = *mut c_void;
type EGLint = i32;
type EGLBoolean = c_uint;
type GLuint = c_uint;
type GLint = c_int;
type GLenum = c_uint;
type GLsizei = c_int;
type GLfloat = f32;

const EGL_SUCCESS: EGLint = 0x3000;
const EGL_NONE: EGLint = 0x3038;
const EGL_SURFACE_TYPE: EGLint = 0x3033;
const EGL_PBUFFER_BIT: EGLint = 0x0001;
const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
const EGL_RED_SIZE: EGLint = 0x3024;
const EGL_GREEN_SIZE: EGLint = 0x3023;
const EGL_BLUE_SIZE: EGLint = 0x3022;
const EGL_ALPHA_SIZE: EGLint = 0x3021;
const EGL_CONTEXT_CLIENT_VERSION: EGLint = 0x3098;
const EGL_WIDTH: EGLint = 0x3057;
const EGL_HEIGHT: EGLint = 0x3056;
const EGL_OPENGL_ES_API: c_uint = 0x30A0;
const EGL_EXTENSIONS: EGLint = 0x3055;
const EGL_VERSION: EGLint = 0x3054;
const EGL_LINUX_DMA_BUF_EXT: c_uint = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: EGLint = 0x3271;
const EGL_DMA_BUF_PLANE0_FD_EXT: EGLint = 0x3272;
const EGL_DMA_BUF_PLANE0_OFFSET_EXT: EGLint = 0x3273;
const EGL_DMA_BUF_PLANE0_PITCH_EXT: EGLint = 0x3274;

const GL_NO_ERROR: GLenum = 0;
const GL_TEXTURE_2D: GLenum = 0x0DE1;
const GL_FRAMEBUFFER: GLenum = 0x8D40;
const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
const GL_TRIANGLES: GLenum = 0x0004;
const GL_FLOAT: GLenum = 0x1406;
const GL_UNSIGNED_BYTE: GLenum = 0x1401;
const GL_RGBA: GLenum = 0x1908;
const GL_ALPHA: GLenum = 0x1906;
const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
const GL_CLAMP_TO_EDGE: GLint = 0x812F;
const GL_LINEAR: GLint = 0x2601;
const GL_BLEND: GLenum = 0x0BE2;
const GL_ONE: GLenum = 1;
const GL_ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
const GL_COLOR_BUFFER_BIT: c_uint = 0x4000;
const GL_SCISSOR_TEST: GLenum = 0x0C11;
const GL_DEPTH_TEST: GLenum = 0x0B71;
const GL_CULL_FACE: GLenum = 0x0B44;
const GL_VERTEX_SHADER: GLenum = 0x8B31;
const GL_FRAGMENT_SHADER: GLenum = 0x8B30;
const GL_COMPILE_STATUS: GLenum = 0x8B81;
const GL_LINK_STATUS: GLenum = 0x8B82;
const GL_EXTENSIONS: GLenum = 0x1F03;
const GL_RENDERER: GLenum = 0x1F01;
const GL_VERSION: GLenum = 0x1F02;
const GL_UNPACK_ALIGNMENT: GLenum = 0x0CF5;
const GL_TEXTURE0: GLenum = 0x84C0;

macro_rules! fns {
    ($name:ident { $($f:ident : fn($($a:ty),*) $(-> $r:ty)?;)* }) => {
        #[allow(non_snake_case)]
        struct $name { $($f: unsafe extern "C" fn($($a),*) $(-> $r)?,)* }
        impl $name {
            /// # Safety
            /// `h` is a dlopen handle of a library exporting these symbols
            /// with these C signatures.
            unsafe fn load(h: *mut c_void, get: &dyn Fn(&str) -> *mut c_void) -> Result<$name, String> {
                Ok($name { $($f: {
                    let n = CString::new(stringify!($f)).unwrap();
                    // SAFETY: dlsym on a valid handle
                    let mut p = unsafe { libc::dlsym(h, n.as_ptr()) };
                    if p.is_null() { p = get(stringify!($f)); }
                    if p.is_null() { return Err(format!("missing {}", stringify!($f))); }
                    // SAFETY: the caller guarantees the signature
                    unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($a),*) $(-> $r)?>(p) }
                },)* })
            }
        }
    };
}

fns!(Egl {
    eglGetDisplay: fn(*mut c_void) -> EGLDisplay;
    eglInitialize: fn(EGLDisplay, *mut EGLint, *mut EGLint) -> EGLBoolean;
    eglTerminate: fn(EGLDisplay) -> EGLBoolean;
    eglBindAPI: fn(c_uint) -> EGLBoolean;
    eglChooseConfig: fn(EGLDisplay, *const EGLint, *mut EGLConfig, EGLint, *mut EGLint) -> EGLBoolean;
    eglCreateContext: fn(EGLDisplay, EGLConfig, EGLContext, *const EGLint) -> EGLContext;
    eglDestroyContext: fn(EGLDisplay, EGLContext) -> EGLBoolean;
    eglCreatePbufferSurface: fn(EGLDisplay, EGLConfig, *const EGLint) -> EGLSurface;
    eglDestroySurface: fn(EGLDisplay, EGLSurface) -> EGLBoolean;
    eglMakeCurrent: fn(EGLDisplay, EGLSurface, EGLSurface, EGLContext) -> EGLBoolean;
    eglGetError: fn() -> EGLint;
    eglQueryString: fn(EGLDisplay, EGLint) -> *const c_char;
    eglGetProcAddress: fn(*const c_char) -> *mut c_void;
});

fns!(Gles {
    glGetString: fn(GLenum) -> *const c_char;
    glGetError: fn() -> GLenum;
    glViewport: fn(GLint, GLint, GLsizei, GLsizei);
    glClearColor: fn(GLfloat, GLfloat, GLfloat, GLfloat);
    glClear: fn(c_uint);
    glEnable: fn(GLenum);
    glDisable: fn(GLenum);
    glBlendFunc: fn(GLenum, GLenum);
    glScissor: fn(GLint, GLint, GLsizei, GLsizei);
    glGenTextures: fn(GLsizei, *mut GLuint);
    glDeleteTextures: fn(GLsizei, *const GLuint);
    glBindTexture: fn(GLenum, GLuint);
    glActiveTexture: fn(GLenum);
    glTexParameteri: fn(GLenum, GLenum, GLint);
    glTexImage2D: fn(GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, *const c_void);
    glTexSubImage2D: fn(GLenum, GLint, GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *const c_void);
    glPixelStorei: fn(GLenum, GLint);
    glGenFramebuffers: fn(GLsizei, *mut GLuint);
    glDeleteFramebuffers: fn(GLsizei, *const GLuint);
    glBindFramebuffer: fn(GLenum, GLuint);
    glFramebufferTexture2D: fn(GLenum, GLenum, GLenum, GLuint, GLint);
    glCheckFramebufferStatus: fn(GLenum) -> GLenum;
    glCreateShader: fn(GLenum) -> GLuint;
    glDeleteShader: fn(GLuint);
    glShaderSource: fn(GLuint, GLsizei, *const *const c_char, *const GLint);
    glCompileShader: fn(GLuint);
    glGetShaderiv: fn(GLuint, GLenum, *mut GLint);
    glGetShaderInfoLog: fn(GLuint, GLsizei, *mut GLsizei, *mut c_char);
    glCreateProgram: fn() -> GLuint;
    glDeleteProgram: fn(GLuint);
    glAttachShader: fn(GLuint, GLuint);
    glBindAttribLocation: fn(GLuint, GLuint, *const c_char);
    glLinkProgram: fn(GLuint);
    glGetProgramiv: fn(GLuint, GLenum, *mut GLint);
    glUseProgram: fn(GLuint);
    glGetUniformLocation: fn(GLuint, *const c_char) -> GLint;
    glUniform1i: fn(GLint, GLint);
    glUniform1f: fn(GLint, GLfloat);
    glUniform2f: fn(GLint, GLfloat, GLfloat);
    glVertexAttribPointer: fn(GLuint, GLint, GLenum, u8, GLsizei, *const c_void);
    glEnableVertexAttribArray: fn(GLuint);
    glDisableVertexAttribArray: fn(GLuint);
    glDrawArrays: fn(GLenum, GLint, GLsizei);
    glFinish: fn();
});

type ImageTargetTex = unsafe extern "C" fn(GLenum, EGLImage);
type CreateImage = unsafe extern "C" fn(EGLDisplay, EGLContext, c_uint, *mut c_void, *const EGLint) -> EGLImage;
type DestroyImage = unsafe extern "C" fn(EGLDisplay, EGLImage) -> EGLBoolean;
type FbTexMs = unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint, GLint, GLsizei);

/// A render target: a scanout buffer imported as an FBO colour attachment.
pub struct Target {
    img: EGLImage,
    tex: GLuint,
    fbo: GLuint,
    pub w: i32,
    pub h: i32,
}

/// A sampled image: an imported client dma-buf or an uploaded texture.
pub struct Tex {
    img: EGLImage,
    pub tex: GLuint,
    pub w: i32,
    pub h: i32,
    /// Bytes are B,G,R,A in an RGBA texture (uploaded wl_shm XRGB).
    pub swizzle: bool,
    /// Ignore alpha (XRGB).
    pub opaque: bool,
}

const VS: &str = "attribute vec2 a_pos; attribute vec2 a_uv; attribute vec4 a_col;
uniform vec2 u_size; varying vec2 v_uv; varying vec4 v_col;
void main() { v_uv = a_uv; v_col = a_col;
  gl_Position = vec4(a_pos.x / u_size.x * 2.0 - 1.0, a_pos.y / u_size.y * 2.0 - 1.0, 0.0, 1.0); }";
const FS_SOLID: &str = "precision mediump float; varying vec4 v_col; void main() { gl_FragColor = v_col; }";
const FS_MASK: &str = "precision mediump float; uniform sampler2D u_tex; varying vec2 v_uv; varying vec4 v_col;
void main() { gl_FragColor = v_col * texture2D(u_tex, v_uv).a; }";
const FS_IMAGE: &str = "precision mediump float; uniform sampler2D u_tex; uniform float u_swz; uniform float u_opaque;
varying vec2 v_uv; varying vec4 v_col;
void main() { vec4 t = texture2D(u_tex, v_uv); vec3 c = mix(t.rgb, t.bgr, u_swz);
  float a = mix(t.a, 1.0, u_opaque); gl_FragColor = vec4(c, a) * v_col.a; }";

struct Prog {
    id: GLuint,
    size: GLint,
    tex: GLint,
    swz: GLint,
    opaque: GLint,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Solid,
    Mask,
    Image(GLuint, bool, bool),
}

struct Atlas {
    tex: GLuint,
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    row_h: i32,
    /// (face, glyph, size*64) -> (x, y, w, h, xmin, ymin)
    map: HashMap<(Face, u16, u32), [i32; 6]>,
}

pub struct Gl {
    lib_egl: *mut c_void,
    lib_gles: *mut c_void,
    egl: Egl,
    gl: Gles,
    dpy: EGLDisplay,
    ctx: EGLContext,
    surf: EGLSurface,
    create_image: CreateImage,
    destroy_image: DestroyImage,
    target_tex: ImageTargetTex,
    fb_tex_ms: Option<FbTexMs>,
    msaa: i32,
    progs: [Prog; 3],
    atlas: Atlas,
    verts: Vec<f32>,
    kind: Kind,
    live: bool,
    pub info: String,
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: EGL/GL strings are NUL-terminated and static
        unsafe { CStr::from_ptr(p).to_string_lossy().into_owned() }
    }
}

impl Gl {
    /// Load the stock libs from `dir` and set up a context. `msaa`: 0 or 4.
    pub fn new(dir: &Path, msaa: u32) -> Result<Gl, String> {
        let open = |n: &str| -> Result<*mut c_void, String> {
            let p = CString::new(dir.join(n).to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
            // SAFETY: dlopen of a path
            let h = unsafe { libc::dlopen(p.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if h.is_null() {
                // SAFETY: dlerror returns a C string or NULL
                return Err(format!("dlopen {}: {}", n, cstr(unsafe { libc::dlerror() })));
            }
            Ok(h)
        };
        let lib_egl = open("libEGL.so.1")?;
        let lib_gles = open("libGLESv2.so.2")?;
        let none = |_: &str| std::ptr::null_mut();
        // SAFETY: the stock libEGL exports the EGL 1.4 entry points
        let egl = unsafe { Egl::load(lib_egl, &none)? };
        let gpa = |n: &str| {
            let c = CString::new(n).unwrap();
            // SAFETY: eglGetProcAddress takes a C string
            unsafe { (egl.eglGetProcAddress)(c.as_ptr()) }
        };
        // SAFETY: the stock libGLESv2 exports the GLES 2 entry points
        let gl = unsafe { Gles::load(lib_gles, &gpa)? };
        let (ci, di, tt) = (gpa("eglCreateImageKHR"), gpa("eglDestroyImageKHR"), gpa("glEGLImageTargetTexture2DOES"));
        if ci.is_null() || di.is_null() || tt.is_null() {
            return Err("EGL image extensions missing".into());
        }
        let mut me = Gl {
            lib_egl,
            lib_gles,
            egl,
            gl,
            dpy: std::ptr::null_mut(),
            ctx: std::ptr::null_mut(),
            surf: std::ptr::null_mut(),
            // SAFETY: non-null entry points with the extensions' signatures
            create_image: unsafe { std::mem::transmute::<*mut c_void, CreateImage>(ci) },
            destroy_image: unsafe { std::mem::transmute::<*mut c_void, DestroyImage>(di) },
            target_tex: unsafe { std::mem::transmute::<*mut c_void, ImageTargetTex>(tt) },
            fb_tex_ms: None,
            msaa: 0,
            progs: [Prog { id: 0, size: -1, tex: -1, swz: -1, opaque: -1 }, Prog { id: 0, size: -1, tex: -1, swz: -1, opaque: -1 }, Prog { id: 0, size: -1, tex: -1, swz: -1, opaque: -1 }],
            atlas: Atlas { tex: 0, w: 2048, h: 1024, x: 0, y: 0, row_h: 0, map: HashMap::new() },
            verts: Vec::with_capacity(64 * 1024),
            kind: Kind::Solid,
            live: false,
            info: String::new(),
        };
        if let Err(e) = me.init(msaa) {
            me.teardown();
            return Err(e);
        }
        Ok(me)
    }

    fn egl_err(&self, what: &str) -> String {
        // SAFETY: plain query
        format!("{what}: EGL error 0x{:x}", unsafe { (self.egl.eglGetError)() })
    }

    fn init(&mut self, msaa: u32) -> Result<(), String> {
        let e = &self.egl;
        // SAFETY: EGL calls in the documented order with valid arguments
        unsafe {
            self.dpy = (e.eglGetDisplay)(std::ptr::null_mut());
            if self.dpy.is_null() {
                return Err(self.egl_err("eglGetDisplay"));
            }
            let (mut maj, mut min) = (0, 0);
            if (e.eglInitialize)(self.dpy, &mut maj, &mut min) == 0 {
                self.dpy = std::ptr::null_mut();
                return Err(self.egl_err("eglInitialize (is the DC module loaded?)"));
            }
            (e.eglBindAPI)(EGL_OPENGL_ES_API);
            let attrs = [EGL_SURFACE_TYPE, EGL_PBUFFER_BIT, EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT, EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8, EGL_NONE];
            let mut cfg: EGLConfig = std::ptr::null_mut();
            let mut n = 0;
            if (e.eglChooseConfig)(self.dpy, attrs.as_ptr(), &mut cfg, 1, &mut n) == 0 || n < 1 {
                return Err(self.egl_err("eglChooseConfig"));
            }
            let ca = [EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE];
            self.ctx = (e.eglCreateContext)(self.dpy, cfg, std::ptr::null_mut(), ca.as_ptr());
            if self.ctx.is_null() {
                return Err(self.egl_err("eglCreateContext"));
            }
            let sa = [EGL_WIDTH, 16, EGL_HEIGHT, 16, EGL_NONE];
            self.surf = (e.eglCreatePbufferSurface)(self.dpy, cfg, sa.as_ptr());
            if self.surf.is_null() {
                return Err(self.egl_err("eglCreatePbufferSurface"));
            }
            if (e.eglMakeCurrent)(self.dpy, self.surf, self.surf, self.ctx) == 0 {
                return Err(self.egl_err("eglMakeCurrent"));
            }
            self.live = true;
            let exts = cstr((self.gl.glGetString)(GL_EXTENSIONS));
            let has = |x: &str| exts.split_whitespace().any(|e| e == x);
            if msaa > 0 && has("GL_EXT_multisampled_render_to_texture") {
                let p = (e.eglGetProcAddress)(c"glFramebufferTexture2DMultisampleEXT".as_ptr());
                if !p.is_null() {
                    self.fb_tex_ms = Some(std::mem::transmute::<*mut c_void, FbTexMs>(p));
                    self.msaa = msaa.min(4) as i32;
                }
            }
            self.info = format!(
                "EGL {} / {} / {} / msaa {}",
                cstr((e.eglQueryString)(self.dpy, EGL_VERSION)),
                cstr((self.gl.glGetString)(GL_RENDERER)),
                cstr((self.gl.glGetString)(GL_VERSION)),
                self.msaa
            );
            let egl_exts = cstr((e.eglQueryString)(self.dpy, EGL_EXTENSIONS));
            if !egl_exts.contains("EGL_EXT_image_dma_buf_import") {
                return Err("no EGL_EXT_image_dma_buf_import".into());
            }
        }
        self.progs[0] = self.program(FS_SOLID)?;
        self.progs[1] = self.program(FS_MASK)?;
        self.progs[2] = self.program(FS_IMAGE)?;
        // glyph atlas
        let g = &self.gl;
        // SAFETY: GL calls on the current context
        unsafe {
            let mut t = 0;
            (g.glGenTextures)(1, &mut t);
            (g.glBindTexture)(GL_TEXTURE_2D, t);
            (g.glPixelStorei)(GL_UNPACK_ALIGNMENT, 1);
            let zero = vec![0u8; (self.atlas.w * self.atlas.h) as usize];
            (g.glTexImage2D)(GL_TEXTURE_2D, 0, GL_ALPHA as GLint, self.atlas.w, self.atlas.h, 0, GL_ALPHA, GL_UNSIGNED_BYTE, zero.as_ptr() as *const c_void);
            tex_params(g);
            self.atlas.tex = t;
        }
        self.check("init")
    }

    fn program(&self, fs: &str) -> Result<Prog, String> {
        let g = &self.gl;
        let compile = |kind: GLenum, src: &str| -> Result<GLuint, String> {
            let c = CString::new(src).unwrap();
            // SAFETY: GL shader calls with a NUL-terminated source
            unsafe {
                let s = (g.glCreateShader)(kind);
                let p = c.as_ptr();
                (g.glShaderSource)(s, 1, &p, std::ptr::null());
                (g.glCompileShader)(s);
                let mut ok = 0;
                (g.glGetShaderiv)(s, GL_COMPILE_STATUS, &mut ok);
                if ok == 0 {
                    let mut buf = vec![0 as c_char; 1024];
                    let mut n = 0;
                    (g.glGetShaderInfoLog)(s, 1024, &mut n, buf.as_mut_ptr());
                    (g.glDeleteShader)(s);
                    return Err(format!("shader: {}", cstr(buf.as_ptr())));
                }
                Ok(s)
            }
        };
        let vs = compile(GL_VERTEX_SHADER, VS)?;
        let fsh = compile(GL_FRAGMENT_SHADER, fs)?;
        // SAFETY: GL program calls on the current context
        unsafe {
            let p = (g.glCreateProgram)();
            (g.glAttachShader)(p, vs);
            (g.glAttachShader)(p, fsh);
            (g.glBindAttribLocation)(p, 0, c"a_pos".as_ptr());
            (g.glBindAttribLocation)(p, 1, c"a_uv".as_ptr());
            (g.glBindAttribLocation)(p, 2, c"a_col".as_ptr());
            (g.glLinkProgram)(p);
            (g.glDeleteShader)(vs);
            (g.glDeleteShader)(fsh);
            let mut ok = 0;
            (g.glGetProgramiv)(p, GL_LINK_STATUS, &mut ok);
            if ok == 0 {
                (g.glDeleteProgram)(p);
                return Err("program link failed".into());
            }
            Ok(Prog {
                id: p,
                size: (g.glGetUniformLocation)(p, c"u_size".as_ptr()),
                tex: (g.glGetUniformLocation)(p, c"u_tex".as_ptr()),
                swz: (g.glGetUniformLocation)(p, c"u_swz".as_ptr()),
                opaque: (g.glGetUniformLocation)(p, c"u_opaque".as_ptr()),
            })
        }
    }

    /// GL errors since the last check, as an Err.
    pub fn check(&self, what: &str) -> Result<(), String> {
        let mut errs = Vec::new();
        for _ in 0..8 {
            // SAFETY: plain query
            let e = unsafe { (self.gl.glGetError)() };
            if e == GL_NO_ERROR {
                break;
            }
            errs.push(format!("0x{e:x}"));
        }
        // SAFETY: plain query
        let ee = unsafe { (self.egl.eglGetError)() };
        if ee != EGL_SUCCESS {
            errs.push(format!("EGL 0x{ee:x}"));
        }
        if errs.is_empty() { Ok(()) } else { Err(format!("{what}: GL error {}", errs.join(" "))) }
    }

    fn import_image(&self, fd: RawFd, w: i32, h: i32, stride: u32, offset: u32, fourcc: u32) -> Result<EGLImage, String> {
        let at = [
            EGL_WIDTH,
            w,
            EGL_HEIGHT,
            h,
            EGL_LINUX_DRM_FOURCC_EXT,
            fourcc as EGLint,
            EGL_DMA_BUF_PLANE0_FD_EXT,
            fd,
            EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            offset as EGLint,
            EGL_DMA_BUF_PLANE0_PITCH_EXT,
            stride as EGLint,
            EGL_NONE,
        ];
        // SAFETY: attribute list as kms-gles uses it; the fd stays open
        let img = unsafe { (self.create_image)(self.dpy, std::ptr::null_mut(), EGL_LINUX_DMA_BUF_EXT, std::ptr::null_mut(), at.as_ptr()) };
        if img.is_null() {
            return Err(self.egl_err("eglCreateImageKHR"));
        }
        Ok(img)
    }

    /// A scanout buffer (dma-buf of a dumb buffer) as a render target.
    pub fn target(&self, fd: RawFd, w: i32, h: i32, stride: u32, fourcc: u32) -> Result<Target, String> {
        let img = self.import_image(fd, w, h, stride, 0, fourcc)?;
        let g = &self.gl;
        // SAFETY: GL calls on the current context with a valid image
        unsafe {
            let (mut tex, mut fbo) = (0, 0);
            (g.glGenTextures)(1, &mut tex);
            (g.glBindTexture)(GL_TEXTURE_2D, tex);
            (self.target_tex)(GL_TEXTURE_2D, img);
            tex_params(g);
            (g.glGenFramebuffers)(1, &mut fbo);
            (g.glBindFramebuffer)(GL_FRAMEBUFFER, fbo);
            let mut ok = false;
            if let Some(ms) = self.fb_tex_ms {
                ms(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0, self.msaa);
                ok = (g.glCheckFramebufferStatus)(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE && (g.glGetError)() == GL_NO_ERROR;
                if !ok {
                    tracing::warn!("multisampled render-to-texture incomplete; rendering without MSAA");
                }
            }
            if !ok {
                (g.glFramebufferTexture2D)(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);
                let st = (g.glCheckFramebufferStatus)(GL_FRAMEBUFFER);
                if st != GL_FRAMEBUFFER_COMPLETE {
                    (g.glBindFramebuffer)(GL_FRAMEBUFFER, 0);
                    (g.glDeleteFramebuffers)(1, &fbo);
                    (g.glDeleteTextures)(1, &tex);
                    (self.destroy_image)(self.dpy, img);
                    return Err(format!("FBO incomplete: 0x{st:x}"));
                }
            }
            (g.glBindFramebuffer)(GL_FRAMEBUFFER, 0);
            let t = Target { img, tex, fbo, w, h };
            if let Err(e) = self.check("render target") {
                self.drop_target(t);
                return Err(e);
            }
            Ok(t)
        }
    }

    pub fn drop_target(&self, t: Target) {
        // SAFETY: objects created by `target`, not in use (callers finish first)
        unsafe {
            (self.gl.glDeleteFramebuffers)(1, &t.fbo);
            (self.gl.glDeleteTextures)(1, &t.tex);
            (self.destroy_image)(self.dpy, t.img);
        }
    }

    /// A client dma-buf (single plane) as a texture.
    pub fn import_dmabuf(&self, fd: RawFd, w: i32, h: i32, stride: u32, offset: u32, fourcc: u32) -> Result<Tex, String> {
        let img = self.import_image(fd, w, h, stride, offset, fourcc)?;
        let g = &self.gl;
        // SAFETY: GL calls on the current context
        unsafe {
            let mut tex = 0;
            (g.glGenTextures)(1, &mut tex);
            (g.glBindTexture)(GL_TEXTURE_2D, tex);
            (self.target_tex)(GL_TEXTURE_2D, img);
            tex_params(g);
            let t = Tex { img, tex, w, h, swizzle: false, opaque: true };
            if let Err(e) = self.check("import client buffer") {
                self.drop_tex(t);
                return Err(e);
            }
            Ok(t)
        }
    }

    /// Upload pixels (4 bytes each, `stride` bytes a row) into `t` (new if
    /// None or the size changed). `swizzle`: bytes are B,G,R,X.
    pub fn upload(&self, t: Option<Tex>, w: i32, h: i32, stride: usize, data: &[u8], swizzle: bool, opaque: bool) -> Result<Tex, String> {
        let g = &self.gl;
        let tight;
        let px: &[u8] = if stride == w as usize * 4 {
            data
        } else {
            let mut v = Vec::with_capacity(w as usize * h as usize * 4);
            for y in 0..h as usize {
                v.extend_from_slice(&data[y * stride..y * stride + w as usize * 4]);
            }
            tight = v;
            &tight
        };
        if px.len() < w as usize * h as usize * 4 {
            return Err("short pixel data".into());
        }
        // SAFETY: GL calls with a buffer of w*h*4 bytes
        unsafe {
            let t = match t {
                Some(t) if t.w == w && t.h == h && t.img.is_null() => {
                    (g.glBindTexture)(GL_TEXTURE_2D, t.tex);
                    (g.glPixelStorei)(GL_UNPACK_ALIGNMENT, 4);
                    (g.glTexSubImage2D)(GL_TEXTURE_2D, 0, 0, 0, w, h, GL_RGBA, GL_UNSIGNED_BYTE, px.as_ptr() as *const c_void);
                    Tex { swizzle, opaque, ..t }
                }
                other => {
                    if let Some(o) = other {
                        self.drop_tex(o);
                    }
                    let mut tex = 0;
                    (g.glGenTextures)(1, &mut tex);
                    (g.glBindTexture)(GL_TEXTURE_2D, tex);
                    (g.glPixelStorei)(GL_UNPACK_ALIGNMENT, 4);
                    (g.glTexImage2D)(GL_TEXTURE_2D, 0, GL_RGBA as GLint, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, px.as_ptr() as *const c_void);
                    tex_params(g);
                    Tex { img: std::ptr::null_mut(), tex, w, h, swizzle, opaque }
                }
            };
            self.check("texture upload")?;
            Ok(t)
        }
    }

    pub fn drop_tex(&self, t: Tex) {
        // SAFETY: objects created here, not in use (callers finish first)
        unsafe {
            (self.gl.glDeleteTextures)(1, &t.tex);
            if !t.img.is_null() {
                (self.destroy_image)(self.dpy, t.img);
            }
        }
    }

    pub fn finish(&self) {
        // SAFETY: plain call
        unsafe { (self.gl.glFinish)() }
    }

    // ------------------------------------------------------- drawing ----

    fn flush(&mut self) {
        if self.verts.is_empty() {
            return;
        }
        let g = &self.gl;
        let n = (self.verts.len() / 8) as GLsizei;
        let (pi, tex, swz, opq) = match self.kind {
            Kind::Solid => (0, 0, false, false),
            Kind::Mask => (1, self.atlas.tex, false, false),
            Kind::Image(t, s, o) => (2, t, s, o),
        };
        let p = &self.progs[pi];
        // SAFETY: client-side vertex arrays from self.verts, alive during the draw
        unsafe {
            (g.glUseProgram)(p.id);
            if p.tex >= 0 {
                (g.glActiveTexture)(GL_TEXTURE0);
                (g.glBindTexture)(GL_TEXTURE_2D, tex);
                (g.glUniform1i)(p.tex, 0);
            }
            if p.swz >= 0 {
                (g.glUniform1f)(p.swz, if swz { 1.0 } else { 0.0 });
            }
            if p.opaque >= 0 {
                (g.glUniform1f)(p.opaque, if opq { 1.0 } else { 0.0 });
            }
            let base = self.verts.as_ptr() as *const u8;
            (g.glVertexAttribPointer)(0, 2, GL_FLOAT, 0, 32, base as *const c_void);
            (g.glVertexAttribPointer)(1, 2, GL_FLOAT, 0, 32, base.add(8) as *const c_void);
            (g.glVertexAttribPointer)(2, 4, GL_FLOAT, 0, 32, base.add(16) as *const c_void);
            (g.glEnableVertexAttribArray)(0);
            (g.glEnableVertexAttribArray)(1);
            (g.glEnableVertexAttribArray)(2);
            (g.glDrawArrays)(GL_TRIANGLES, 0, n);
        }
        self.verts.clear();
    }

    fn kind(&mut self, k: Kind) {
        if self.kind != k {
            self.flush();
            self.kind = k;
        }
    }

    fn vert(&mut self, p: P, uv: (f32, f32), c: [f32; 4]) {
        self.verts.extend_from_slice(&[p.0, p.1, uv.0, uv.1, c[0], c[1], c[2], c[3]]);
    }

    fn tri(&mut self, a: P, b: P, c: P, col: [f32; 4]) {
        self.vert(a, (0.0, 0.0), col);
        self.vert(b, (0.0, 0.0), col);
        self.vert(c, (0.0, 0.0), col);
    }

    fn glyph(&mut self, typo: &Typo, key: (Face, u16, u32)) -> Option<[i32; 6]> {
        if let Some(g) = self.atlas.map.get(&key) {
            return Some(*g);
        }
        let (m, cov) = typo.font(key.0).rasterize_indexed(key.1, key.2 as f32 / 64.0);
        let (w, h) = (m.width as i32, m.height as i32);
        if w == 0 || h == 0 {
            let e = [0, 0, 0, 0, m.xmin, m.ymin];
            self.atlas.map.insert(key, e);
            return Some(e);
        }
        if w + 2 > self.atlas.w || h + 2 > self.atlas.h {
            return None;
        }
        if self.atlas.x + w + 1 > self.atlas.w {
            self.atlas.x = 0;
            self.atlas.y += self.atlas.row_h + 1;
            self.atlas.row_h = 0;
        }
        if self.atlas.y + h + 1 > self.atlas.h {
            // full: start over (glyphs already batched this frame are flushed first)
            self.flush();
            self.atlas.map.clear();
            self.atlas.x = 0;
            self.atlas.y = 0;
            self.atlas.row_h = 0;
        }
        let (x, y) = (self.atlas.x, self.atlas.y);
        // SAFETY: cov holds w*h bytes; the region is inside the atlas
        unsafe {
            (self.gl.glBindTexture)(GL_TEXTURE_2D, self.atlas.tex);
            (self.gl.glPixelStorei)(GL_UNPACK_ALIGNMENT, 1);
            (self.gl.glTexSubImage2D)(GL_TEXTURE_2D, 0, x, y, w, h, GL_ALPHA, GL_UNSIGNED_BYTE, cov.as_ptr() as *const c_void);
        }
        self.atlas.x += w + 1;
        self.atlas.row_h = self.atlas.row_h.max(h);
        let e = [x, y, w, h, m.xmin, m.ymin];
        self.atlas.map.insert(key, e);
        Some(e)
    }

    /// Draw `scene` into `t`. `tex` resolves image sources.
    pub fn render(&mut self, t: &Target, scene: &Scene, typo: &Typo, tex: &dyn Fn(Src) -> Option<(GLuint, bool, bool, i32, i32)>) -> Result<(), String> {
        let g = &self.gl;
        // SAFETY: GL state setup on the current context
        unsafe {
            (g.glBindFramebuffer)(GL_FRAMEBUFFER, t.fbo);
            (g.glViewport)(0, 0, t.w, t.h);
            (g.glDisable)(GL_DEPTH_TEST);
            (g.glDisable)(GL_CULL_FACE);
            (g.glDisable)(GL_SCISSOR_TEST);
            let c = pm(scene.clear);
            (g.glClearColor)(c[0], c[1], c[2], 1.0);
            (g.glClear)(GL_COLOR_BUFFER_BIT);
            (g.glEnable)(GL_BLEND);
            (g.glBlendFunc)(GL_ONE, GL_ONE_MINUS_SRC_ALPHA);
            for p in &self.progs {
                (g.glUseProgram)(p.id);
                (g.glUniform2f)(p.size, t.w as f32, t.h as f32);
            }
        }
        self.kind = Kind::Solid;
        let (aw, ah) = (self.atlas.w as f32, self.atlas.h as f32);
        for it in &scene.items {
            match it {
                Item::Clip(r) => {
                    self.flush();
                    // SAFETY: scissor state
                    unsafe {
                        match r {
                            Some(r) => {
                                (self.gl.glEnable)(GL_SCISSOR_TEST);
                                (self.gl.glScissor)(r[0].round() as GLint, r[1].round() as GLint, r[2].round().max(0.0) as GLsizei, r[3].round().max(0.0) as GLsizei);
                            }
                            None => (self.gl.glDisable)(GL_SCISSOR_TEST),
                        }
                    }
                }
                Item::Fill { pts, color, convex } => {
                    self.kind(Kind::Solid);
                    let c = pm(*color);
                    if *convex {
                        for i in 1..pts.len().saturating_sub(1) {
                            self.tri(pts[0], pts[i], pts[i + 1], c);
                        }
                    } else {
                        let tris = triangulate(pts);
                        for t in tris.chunks_exact(3) {
                            self.tri(t[0], t[1], t[2], c);
                        }
                    }
                }
                Item::Stroke { pts, closed, width, color } => {
                    self.kind(Kind::Solid);
                    let c = pm(*color);
                    let n = pts.len();
                    let segs = if *closed { n } else { n - 1 };
                    let hw = width / 2.0;
                    for i in 0..segs {
                        let (a, b) = (pts[i], pts[(i + 1) % n]);
                        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                        let l = (dx * dx + dy * dy).sqrt();
                        if l < 1e-4 {
                            continue;
                        }
                        let (ux, uy) = (dx / l * hw, dy / l * hw);
                        let (nx, ny) = (-uy, ux);
                        // extended by half the width: square joins
                        let a0 = (a.0 - ux + nx, a.1 - uy + ny);
                        let a1 = (a.0 - ux - nx, a.1 - uy - ny);
                        let b0 = (b.0 + ux + nx, b.1 + uy + ny);
                        let b1 = (b.0 + ux - nx, b.1 + uy - ny);
                        self.tri(a0, b0, b1, c);
                        self.tri(a0, b1, a1, c);
                    }
                }
                Item::Shape { shape, xf, color } => {
                    self.kind(Kind::Solid);
                    let c = pm(*color);
                    for t in shape.tris.chunks_exact(3) {
                        self.tri(xf.apply(t[0]), xf.apply(t[1]), xf.apply(t[2]), c);
                    }
                }
                Item::Text(run) => {
                    self.kind(Kind::Mask);
                    let c = pm(run.color);
                    let skey = (run.size * 64.0).round() as u32;
                    let fast = run.xf.is_translate();
                    for (idx, pen) in typo.layout(&run.s, run.face, run.size, run.tracking) {
                        let Some([gx, gy, gw, gh, xmin, ymin]) = self.glyph(typo, (run.face, idx, skey)) else { continue };
                        if gw == 0 {
                            continue;
                        }
                        self.kind(Kind::Mask);
                        let mut x0 = run.x + pen + xmin as f32;
                        let mut y0 = run.base - gh as f32 - ymin as f32;
                        if fast {
                            x0 = (x0 + run.xf.e).round() - run.xf.e;
                            y0 = (y0 + run.xf.f).round() - run.xf.f;
                        }
                        let (x1, y1) = (x0 + gw as f32, y0 + gh as f32);
                        let q = [run.xf.apply((x0, y0)), run.xf.apply((x1, y0)), run.xf.apply((x1, y1)), run.xf.apply((x0, y1))];
                        let (u0, v0, u1, v1) = (gx as f32 / aw, gy as f32 / ah, (gx + gw) as f32 / aw, (gy + gh) as f32 / ah);
                        self.vert(q[0], (u0, v0), c);
                        self.vert(q[1], (u1, v0), c);
                        self.vert(q[2], (u1, v1), c);
                        self.vert(q[0], (u0, v0), c);
                        self.vert(q[2], (u1, v1), c);
                        self.vert(q[3], (u0, v1), c);
                    }
                }
                Item::Image { src, quad, uv, alpha } => {
                    let Some((id, swz, opq, _, _)) = tex(*src) else { continue };
                    self.kind(Kind::Image(id, swz, opq));
                    let c = [*alpha, *alpha, *alpha, *alpha];
                    let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
                    self.vert(quad[0], (u0, v0), c);
                    self.vert(quad[1], (u1, v0), c);
                    self.vert(quad[2], (u1, v1), c);
                    self.vert(quad[0], (u0, v0), c);
                    self.vert(quad[2], (u1, v1), c);
                    self.vert(quad[3], (u0, v1), c);
                }
            }
        }
        self.flush();
        // SAFETY: state reset
        unsafe {
            (self.gl.glDisable)(GL_SCISSOR_TEST);
            (self.gl.glDisableVertexAttribArray)(0);
            (self.gl.glDisableVertexAttribArray)(1);
            (self.gl.glDisableVertexAttribArray)(2);
            (self.gl.glBindFramebuffer)(GL_FRAMEBUFFER, 0);
        }
        self.check("render")
    }

    #[allow(dead_code)]
    fn unload(&mut self) {
        // SAFETY: handles from dlopen, closed once; no GL objects remain
        unsafe {
            if !self.lib_gles.is_null() {
                libc::dlclose(self.lib_gles);
            }
            if !self.lib_egl.is_null() {
                libc::dlclose(self.lib_egl);
            }
        }
        self.lib_gles = std::ptr::null_mut();
        self.lib_egl = std::ptr::null_mut();
    }

    /// Orderly shutdown: wait for the GPU, free GL objects, release the
    /// context and the display. Callers drop targets and textures first.
    pub fn teardown(&mut self) {
        // SAFETY: objects created by this context; EGL teardown order
        unsafe {
            if self.live {
                (self.gl.glFinish)();
                if self.atlas.tex != 0 {
                    (self.gl.glDeleteTextures)(1, &self.atlas.tex);
                }
                for p in &self.progs {
                    if p.id != 0 {
                        (self.gl.glDeleteProgram)(p.id);
                    }
                }
                (self.gl.glFinish)();
            }
            if !self.dpy.is_null() {
                (self.egl.eglMakeCurrent)(self.dpy, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
                if !self.surf.is_null() {
                    (self.egl.eglDestroySurface)(self.dpy, self.surf);
                }
                if !self.ctx.is_null() {
                    (self.egl.eglDestroyContext)(self.dpy, self.ctx);
                }
                (self.egl.eglTerminate)(self.dpy);
            }
        }
        self.live = false;
        self.dpy = std::ptr::null_mut();
        self.ctx = std::ptr::null_mut();
        self.surf = std::ptr::null_mut();
        // the stock libs stay loaded: unloading a driver with threads is unsafe
    }
}

/// Straight-alpha colour to premultiplied floats.
fn pm(c: Color) -> [f32; 4] {
    let a = c.3 as f32 / 255.0;
    [c.0 as f32 / 255.0 * a, c.1 as f32 / 255.0 * a, c.2 as f32 / 255.0 * a, a]
}

/// # Safety
/// A texture is bound on the current context.
unsafe fn tex_params(g: &Gles) {
    // SAFETY: per the function contract
    unsafe {
        (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
        (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
        (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
        (g.glTexParameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
    }
}
