//! EGL/GLES3: один контекст на весь демон, поверхности — по одной на слой Wayland.
use glow::HasContext;
use khronos_egl as egl;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

pub type Gl = Rc<glow::Context>;
type EglInst = egl::DynamicInstance<egl::EGL1_4>;

pub struct Egl {
    pub inst: EglInst,
    pub display: egl::Display,
    pub config: egl::Config,
    pub ctx: egl::Context,
    fallback: Option<egl::Surface>,
}

impl Egl {
    pub fn new(wl_display: *mut c_void) -> Result<(Egl, Gl), String> {
        let lib = unsafe { libloading::Library::new("libEGL.so.1") }.map_err(|e| format!("libEGL: {e}"))?;
        let inst = unsafe { EglInst::load_required_from(lib) }.map_err(|e| format!("EGL: {e}"))?;
        let display = unsafe { inst.get_display(wl_display as egl::NativeDisplayType) }.ok_or(crate::i18n::tr("нет EGL-дисплея"))?;
        inst.initialize(display).map_err(|e| format!("eglInitialize: {e}"))?;
        inst.bind_api(egl::OPENGL_ES_API).map_err(|e| format!("eglBindAPI: {e}"))?;
        let base = [egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8, egl::ALPHA_SIZE, 8, egl::RENDERABLE_TYPE, 0x40];
        let exts = inst.query_string(Some(display), egl::EXTENSIONS).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let surfaceless = exts.contains("EGL_KHR_surfaceless_context");
        let mut attrs = base.to_vec();
        attrs.extend([egl::SURFACE_TYPE, if surfaceless { egl::WINDOW_BIT } else { egl::WINDOW_BIT | egl::PBUFFER_BIT }, egl::NONE]);
        let config = inst.choose_first_config(display, &attrs).map_err(|e| format!("eglChooseConfig: {e}"))?.ok_or(crate::i18n::tr("нет подходящей конфигурации EGL (нужен OpenGL ES 3)"))?;
        let ctx = inst
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE])
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        let fallback = if surfaceless {
            None
        } else {
            Some(inst.create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE]).map_err(|e| format!("pbuffer: {e}"))?)
        };
        let me = Egl { inst, display, config, ctx, fallback };
        me.make_current(None);
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|s| me.inst.get_proc_address(s.to_str().unwrap_or("")).map(|p| p as *const c_void).unwrap_or(std::ptr::null()))
        };
        Ok((me, Rc::new(gl)))
    }

    pub fn make_current(&self, surf: Option<egl::Surface>) {
        let s = surf.or(self.fallback);
        let _ = self.inst.make_current(self.display, s, s, Some(self.ctx));
    }

    pub fn create_surface(&self, win: *mut c_void) -> Option<egl::Surface> {
        unsafe { self.inst.create_window_surface(self.display, self.config, win as egl::NativeWindowType, None) }.ok()
    }

    pub fn destroy_surface(&self, s: egl::Surface) {
        self.make_current(None);
        let _ = self.inst.destroy_surface(self.display, s);
    }

    /// Кадр уходит без ожидания vblank: темп задают frame-callback'и Wayland,
    /// иначе скрытая поверхность подвесила бы весь цикл.
    pub fn swap(&self, s: egl::Surface) {
        let _ = self.inst.swap_interval(self.display, 0);
        let _ = self.inst.swap_buffers(self.display, s);
    }

    pub fn proc_address(&self, name: &str) -> *mut c_void {
        self.inst.get_proc_address(name).map(|p| p as *mut c_void).unwrap_or(std::ptr::null_mut())
    }
}

pub struct Tex {
    gl: Gl,
    owned: bool,
    pub id: glow::Texture,
    pub w: u32,
    pub h: u32,
}

impl Tex {
    pub fn rgba(gl: &Gl, w: u32, h: u32, data: Option<&[u8]>, mip: bool) -> Tex {
        unsafe {
            let id = gl.create_texture().expect("texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(id));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(data));
            let min = if mip { glow::LINEAR_MIPMAP_LINEAR } else { glow::LINEAR };
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            if mip {
                gl.generate_mipmap(glow::TEXTURE_2D);
            }
            Tex { gl: gl.clone(), owned: true, id, w, h }
        }
    }

    /// Чужая текстура (её рисует поток видео в общем пространстве имён GL): не удаляем.
    pub fn borrowed(gl: &Gl, raw: u32, w: u32, h: u32) -> Option<Tex> {
        let id = glow::NativeTexture(std::num::NonZeroU32::new(raw)?);
        Some(Tex { gl: gl.clone(), owned: false, id, w, h })
    }

    pub fn raw(&self) -> u32 {
        self.id.0.get()
    }

    pub fn alpha(gl: &Gl, w: u32, h: u32) -> Tex {
        unsafe {
            let id = gl.create_texture().expect("texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(id));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            let zero = vec![0u8; (w * h) as usize];
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::R8 as i32, w as i32, h as i32, 0, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&zero)));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            Tex { gl: gl.clone(), owned: true, id, w, h }
        }
    }

    pub fn sub_alpha(&self, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.id));
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, x as i32, y as i32, w as i32, h as i32, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(data)));
        }
    }

    pub fn mipmap(&self) {
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.id));
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
            self.gl.generate_mipmap(glow::TEXTURE_2D);
        }
    }
}

impl Drop for Tex {
    fn drop(&mut self) {
        if self.owned {
            unsafe { self.gl.delete_texture(self.id) }
        }
    }
}

/// Текстура, в которую можно рисовать.
pub struct Target {
    gl: Gl,
    pub fbo: glow::Framebuffer,
    pub tex: Tex,
}

impl Target {
    pub fn new(gl: &Gl, w: u32, h: u32) -> Target {
        let tex = Tex::rgba(gl, w.max(1), h.max(1), None, false);
        unsafe {
            let fbo = gl.create_framebuffer().expect("fbo");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex.id), 0);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            Target { gl: gl.clone(), fbo, tex }
        }
    }
    pub fn bind(&self) {
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.fbo));
            self.gl.viewport(0, 0, self.tex.w as i32, self.tex.h as i32);
        }
    }
    pub fn fbo_id(&self) -> i32 {
        self.fbo.0.get() as i32
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        unsafe { self.gl.delete_framebuffer(self.fbo) }
    }
}

pub struct Prog {
    gl: Gl,
    pub id: glow::Program,
    locs: HashMap<&'static str, Option<glow::UniformLocation>>,
}

impl Prog {
    pub fn new(gl: &Gl, vs: &str, fs: &str) -> Prog {
        unsafe {
            let id = gl.create_program().expect("program");
            let mut shaders = vec![];
            for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
                let s = gl.create_shader(kind).expect("shader");
                gl.shader_source(s, &format!("#version 300 es\nprecision highp float;\nprecision highp int;\n{src}"));
                gl.compile_shader(s);
                if !gl.get_shader_compile_status(s) {
                    eprintln!("omawall: shader error: {}", gl.get_shader_info_log(s));
                }
                gl.attach_shader(id, s);
                shaders.push(s);
            }
            gl.bind_attrib_location(id, 0, "a_pos");
            gl.link_program(id);
            if !gl.get_program_link_status(id) {
                eprintln!("omawall: shader link error: {}", gl.get_program_info_log(id));
            }
            for s in shaders {
                gl.detach_shader(id, s);
                gl.delete_shader(s);
            }
            Prog { gl: gl.clone(), id, locs: HashMap::new() }
        }
    }
    pub fn bind(&self) {
        unsafe { self.gl.use_program(Some(self.id)) }
    }
    fn loc(&mut self, name: &'static str) -> Option<glow::UniformLocation> {
        let (gl, id) = (&self.gl, self.id);
        self.locs.entry(name).or_insert_with(|| unsafe { gl.get_uniform_location(id, name) }).clone()
    }
    pub fn f1(&mut self, n: &'static str, v: f32) {
        let l = self.loc(n);
        unsafe { self.gl.uniform_1_f32(l.as_ref(), v) }
    }
    pub fn f2(&mut self, n: &'static str, a: f32, b: f32) {
        let l = self.loc(n);
        unsafe { self.gl.uniform_2_f32(l.as_ref(), a, b) }
    }
    pub fn f4(&mut self, n: &'static str, v: [f32; 4]) {
        let l = self.loc(n);
        unsafe { self.gl.uniform_4_f32(l.as_ref(), v[0], v[1], v[2], v[3]) }
    }
    pub fn i1(&mut self, n: &'static str, v: i32) {
        let l = self.loc(n);
        unsafe { self.gl.uniform_1_i32(l.as_ref(), v) }
    }
    pub fn f3v(&mut self, n: &'static str, v: &[f32]) {
        let l = self.loc(n);
        unsafe { self.gl.uniform_3_f32_slice(l.as_ref(), v) }
    }
}
