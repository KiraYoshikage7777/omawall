//! Двумерная отрисовка интерфейса: скошенные скруглённые фигуры (SDF) и текст.
use crate::gfx::{Gl, Prog, Tex};
use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont};
use glow::HasContext;
use std::collections::HashMap;

pub type Color = [f32; 4];

pub const BOLD: usize = 0;
pub const MEDIUM: usize = 1;
pub const ICON: usize = 2;

pub fn hex(v: u32) -> Color {
    [((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, 1.0]
}
pub fn alpha(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], c[3] * a]
}
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}
pub fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    u32::from_str_radix(s, 16).ok().map(hex)
}
pub fn to_hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", (c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8)
}
pub fn hsv(h: f32, s: f32, v: f32) -> Color {
    let h = h.rem_euclid(360.0) / 60.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b, 1.0]
}
pub fn to_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-5 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max <= 0.0 { 0.0 } else { d / max }, max)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn has(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x && p.0 < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
    /// Попадание в параллелограмм: skew — на сколько верх сдвинут вправо относительно низа.
    pub fn has_skewed(&self, p: (f32, f32), skew: f32) -> bool {
        let k = if self.h > 0.0 { skew / self.h } else { 0.0 };
        let qx = p.0 + k * (p.1 - (self.y + self.h / 2.0));
        qx >= self.x && qx < self.x + self.w && p.1 >= self.y && p.1 < self.y + self.h
    }
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// Фигура: прямоугольник `rect`, сдвинутый в параллелограмм (`skew` — сдвиг верха
/// относительно низа в пикселях), со скруглением, рамкой и необязательной текстурой.
pub struct Shape<'a> {
    pub rect: Rect,
    pub radius: f32,
    pub skew: f32,
    pub color: Color,
    pub border: f32,
    pub border_color: Color,
    pub tex: Option<&'a Tex>,
    /// u, v центра и приращение uv на пиксель экрана.
    pub uv: [f32; 4],
    pub mode: i32,
    pub alpha: f32,
    /// > 0 — мягкая тень с таким размытием.
    pub soft: f32,
}

impl<'a> Shape<'a> {
    pub fn new(rect: Rect, color: Color) -> Shape<'a> {
        Shape { rect, radius: 0.0, skew: 0.0, color, border: 0.0, border_color: [0.0; 4], tex: None, uv: [0.5, 0.5, 0.0, 0.0], mode: 0, alpha: 1.0, soft: 0.0 }
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
    pub fn skew(mut self, s: f32) -> Self {
        self.skew = s;
        self
    }
    pub fn border(mut self, w: f32, c: Color) -> Self {
        self.border = w;
        self.border_color = c;
        self
    }
    pub fn alpha(mut self, a: f32) -> Self {
        self.alpha = a;
        self
    }
    pub fn soft(mut self, s: f32) -> Self {
        self.soft = s;
        self
    }
    pub fn mode(mut self, m: i32) -> Self {
        self.mode = m;
        self
    }
    /// Текстура «с заполнением» рамки `frame` (её центр совпадает с центром картинки).
    pub fn cover(mut self, tex: &'a Tex, frame: Rect) -> Self {
        let scale = (frame.w / tex.w as f32).max(frame.h / tex.h as f32).max(1e-6);
        let c = (frame.cx() - self.rect.cx(), frame.cy() - self.rect.cy());
        self.uv = [0.5 - c.0 / (tex.w as f32 * scale), 0.5 - c.1 / (tex.h as f32 * scale), 1.0 / (tex.w as f32 * scale), 1.0 / (tex.h as f32 * scale)];
        self.tex = Some(tex);
        self.mode = 1;
        self
    }
    /// Текстура, растянутая ровно на `rect`.
    pub fn stretch(mut self, tex: &'a Tex) -> Self {
        self.uv = [0.5, 0.5, 1.0 / self.rect.w.max(1.0), 1.0 / self.rect.h.max(1.0)];
        self.tex = Some(tex);
        self.mode = 1;
        self
    }
}

const SHAPE_VS: &str = r#"
layout(location=0) in vec2 a_pos;
uniform vec2 u_center; uniform vec2 u_ext; uniform vec2 u_view; uniform float u_flip;
out vec2 v_p;
void main(){ v_p = a_pos*u_ext; vec2 c = (u_center+v_p)/u_view*2.0-1.0; gl_Position = vec4(c.x, -c.y*u_flip, 0.0, 1.0); }
"#;

const SHAPE_FS: &str = r#"
uniform vec2 u_half; uniform float u_radius; uniform float u_skew;
uniform vec4 u_color; uniform vec4 u_bcolor; uniform float u_border; uniform float u_soft;
uniform int u_mode; uniform sampler2D u_tex; uniform vec4 u_uv; uniform float u_alpha;
in vec2 v_p; out vec4 o;
float sdrr(vec2 q, vec2 b, float r){ vec2 d = abs(q)-b+r; return length(max(d,0.0))+min(max(d.x,d.y),0.0)-r; }
vec3 hsv2rgb(vec3 c){ vec3 p = abs(fract(c.xxx+vec3(1.0,2.0/3.0,1.0/3.0))*6.0-3.0); return c.z*mix(vec3(1.0),clamp(p-1.0,0.0,1.0),c.y); }
void main(){
  vec2 q = vec2(v_p.x + u_skew*v_p.y, v_p.y);
  float r = min(u_radius, min(u_half.x,u_half.y));
  float d = sdrr(q,u_half,r);
  if(u_soft>0.0){ float a = 1.0-smoothstep(-u_soft,u_soft,d); o = vec4(u_color.rgb,1.0)*(u_color.a*a*u_alpha); return; }
  float a = clamp(0.5-d,0.0,1.0);
  vec4 c = u_color;
  if(u_mode==1){ vec2 uv = u_uv.xy + v_p*u_uv.zw; c = vec4(texture(u_tex,uv).rgb*u_color.rgb, u_color.a); }
  else if(u_mode==2){ // кольцо оттенков
    float ang = atan(v_p.y,v_p.x)/6.2831853; float rr = length(v_p)/u_half.x;
    float m = smoothstep(0.78,0.80,rr)*(1.0-smoothstep(0.98,1.0,rr));
    c = vec4(hsv2rgb(vec3(ang,1.0,1.0)), m);
  } else if(u_mode==3){ // поле насыщенность/яркость, оттенок в u_color.x
    vec2 t = v_p/u_half*0.5+0.5; c = vec4(hsv2rgb(vec3(u_color.x, t.x, 1.0-t.y)),1.0);
  } else if(u_mode==4){ vec2 uv = u_uv.xy + v_p*u_uv.zw; vec4 t = texture(u_tex,uv); c = vec4(t.rgb,1.0)*u_color; }
  if(u_border>0.0){ float bm = clamp(d+u_border+0.5,0.0,1.0); c = mix(c,vec4(u_bcolor.rgb,1.0),bm*u_bcolor.a); }
  o = vec4(c.rgb*c.a, c.a)*(a*u_alpha);
}
"#;

const TEXT_VS: &str = r#"
layout(location=0) in vec4 a_pos; uniform vec2 u_view; uniform float u_flip; out vec2 v_uv;
void main(){ v_uv = a_pos.zw; vec2 c = a_pos.xy/u_view*2.0-1.0; gl_Position = vec4(c.x,-c.y*u_flip,0.0,1.0); }
"#;
const TEXT_FS: &str = r#"
uniform sampler2D u_tex; uniform vec4 u_color; in vec2 v_uv; out vec4 o;
void main(){ float a = texture(u_tex,v_uv).r*u_color.a; o = vec4(u_color.rgb*a,a); }
"#;

#[derive(Clone, Copy)]
struct Glyph {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    left: f32,
    top: f32,
}

const ATLAS: u32 = 1024;

pub struct Draw {
    pub gl: Gl,
    shape: Prog,
    text: Prog,
    quad: glow::VertexArray,
    text_vao: glow::VertexArray,
    text_vbo: glow::Buffer,
    fonts: Vec<FontArc>,
    fallback: HashMap<u32, Option<usize>>,
    atlas: Tex,
    glyphs: HashMap<(u8, u16, u16), Option<Glyph>>,
    pen: (u32, u32, u32),
    pub view: (f32, f32),
    flip: f32,
    verts: Vec<f32>,
}

impl Draw {
    pub fn new(gl: &Gl) -> Draw {
        unsafe {
            let quad = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(quad));
            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let v: [f32; 12] = [-1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0];
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, std::slice::from_raw_parts(v.as_ptr() as *const u8, 48), glow::STATIC_DRAW);
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
            let text_vao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(text_vao));
            let text_vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(text_vbo));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 4, glow::FLOAT, false, 16, 0);
            gl.bind_vertex_array(None);
            let fonts = vec![
                FontArc::try_from_slice(include_bytes!("../assets/text-bold.ttf")).unwrap(),
                FontArc::try_from_slice(include_bytes!("../assets/text-medium.ttf")).unwrap(),
                FontArc::try_from_slice(include_bytes!("../assets/icons.ttf")).unwrap(),
            ];
            Draw {
                gl: gl.clone(),
                shape: Prog::new(gl, SHAPE_VS, SHAPE_FS),
                text: Prog::new(gl, TEXT_VS, TEXT_FS),
                quad,
                text_vao,
                text_vbo,
                fonts,
                fallback: HashMap::new(),
                atlas: Tex::alpha(gl, ATLAS, ATLAS),
                glyphs: HashMap::new(),
                pen: (1, 1, 0),
                view: (1.0, 1.0),
                flip: 1.0,
                verts: Vec::new(),
            }
        }
    }

    /// Подготовить состояние GL к рисованию в текущую цель размером w×h.
    pub fn begin(&mut self, w: u32, h: u32, to_texture: bool) {
        self.view = (w as f32, h as f32);
        self.flip = if to_texture { -1.0 } else { 1.0 };
        unsafe {
            self.gl.viewport(0, 0, w as i32, h as i32);
            self.gl.disable(glow::DEPTH_TEST);
            self.gl.disable(glow::CULL_FACE);
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.enable(glow::BLEND);
            self.gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }
    }

    pub fn clear(&self, c: Color) {
        unsafe {
            self.gl.clear_color(c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }

    pub fn clip(&self, r: Option<Rect>) {
        unsafe {
            match r {
                Some(r) => {
                    self.gl.enable(glow::SCISSOR_TEST);
                    self.gl.scissor(r.x.floor() as i32, (self.view.1 - r.y - r.h).floor() as i32, r.w.ceil().max(0.0) as i32, r.h.ceil().max(0.0) as i32);
                }
                None => self.gl.disable(glow::SCISSOR_TEST),
            }
        }
    }

    /// Нарисовать единичный квадрат текущей программой (для полноэкранных проходов).
    pub fn quad(&self) {
        unsafe {
            self.gl.bind_vertex_array(Some(self.quad));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }
    }

    pub fn shape(&mut self, s: &Shape) {
        if s.alpha <= 0.001 || s.rect.w <= 0.0 || s.rect.h <= 0.0 {
            return;
        }
        let half = (s.rect.w / 2.0, s.rect.h / 2.0);
        let k = s.skew / s.rect.h;
        let pad = 1.0 + s.soft;
        let p = &mut self.shape;
        p.bind();
        p.f2("u_view", self.view.0, self.view.1);
        p.f1("u_flip", self.flip);
        p.f2("u_center", s.rect.cx(), s.rect.cy());
        p.f2("u_ext", half.0 + s.skew.abs() / 2.0 + pad, half.1 + pad);
        p.f2("u_half", half.0, half.1);
        p.f1("u_radius", s.radius);
        p.f1("u_skew", k);
        p.f4("u_color", s.color);
        p.f4("u_bcolor", s.border_color);
        p.f1("u_border", s.border);
        p.f1("u_soft", s.soft);
        p.i1("u_mode", s.mode);
        p.f4("u_uv", s.uv);
        p.f1("u_alpha", s.alpha);
        unsafe {
            if let Some(t) = s.tex {
                self.gl.active_texture(glow::TEXTURE0);
                self.gl.bind_texture(glow::TEXTURE_2D, Some(t.id));
                p.i1("u_tex", 0);
            }
            self.gl.bind_vertex_array(Some(self.quad));
            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }
    }

    pub fn rect(&mut self, r: Rect, c: Color) {
        self.shape(&Shape::new(r, c));
    }
    pub fn rrect(&mut self, r: Rect, radius: f32, c: Color) {
        self.shape(&Shape::new(r, c).radius(radius));
    }
    pub fn frame(&mut self, r: Rect, radius: f32, w: f32, c: Color) {
        self.shape(&Shape::new(r, [0.0; 4]).radius(radius).border(w, c));
    }

    fn em(font: &FontArc, px: f32) -> PxScale {
        PxScale::from(px * font.height_unscaled() / font.units_per_em().unwrap_or(1000.0))
    }

    /// Шрифт, в котором есть этот символ: запрошенный, иначе системный (через fontconfig).
    fn font_for(&mut self, pref: usize, ch: char) -> (usize, GlyphId) {
        let id = self.fonts[pref].glyph_id(ch);
        if id.0 != 0 || ch.is_whitespace() || ch.is_control() {
            return (pref, id);
        }
        let block = ch as u32 >> 8;
        let slot = match self.fallback.get(&block) {
            Some(s) => *s,
            None => {
                let found = std::process::Command::new("fc-match")
                    .args(["-f", "%{file}\n%{index}", &format!(":charset={:x}", ch as u32)])
                    .output()
                    .ok()
                    .and_then(|o| {
                        let text = String::from_utf8_lossy(&o.stdout).into_owned();
                        let mut lines = text.lines();
                        let file = lines.next()?.to_string();
                        let index: u32 = lines.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                        let data = std::fs::read(file).ok()?;
                        ab_glyph::FontVec::try_from_vec_and_index(data, index).ok().map(FontArc::new)
                    })
                    .filter(|f| f.glyph_id(ch).0 != 0)
                    .map(|f| {
                        self.fonts.push(f);
                        self.fonts.len() - 1
                    });
                self.fallback.insert(block, found);
                found
            }
        };
        match slot {
            Some(f) => (f, self.fonts[f].glyph_id(ch)),
            None => (pref, id),
        }
    }

    fn glyph(&mut self, font: usize, px: f32, id: GlyphId) -> Option<Glyph> {
        let key = (font as u8, (px * 4.0).round() as u16, id.0);
        if let Some(g) = self.glyphs.get(&key) {
            return *g;
        }
        let f = &self.fonts[font];
        let scale = Self::em(f, px);
        let out = f.outline_glyph(id.with_scale_and_position(scale, ab_glyph::point(0.0, 0.0))).and_then(|o| {
            let b = o.px_bounds();
            let (w, h) = (b.width().ceil() as u32, b.height().ceil() as u32);
            if w == 0 || h == 0 || w > 400 || h > 400 {
                return None;
            }
            let mut buf = vec![0u8; (w * h) as usize];
            o.draw(|x, y, c| {
                if x < w && y < h {
                    buf[(y * w + x) as usize] = (c.clamp(0.0, 1.0) * 255.0) as u8;
                }
            });
            Some((w, h, b.min.x, b.min.y, buf))
        });
        let g = out.map(|(w, h, left, top, buf)| {
            if self.pen.0 + w + 1 > ATLAS {
                self.pen = (1, self.pen.1 + self.pen.2 + 1, 0);
            }
            if self.pen.1 + h + 1 > ATLAS {
                // Атлас полон: начинаем заново, старые глифы растеризуются по требованию.
                self.glyphs.clear();
                self.pen = (1, 1, 0);
            }
            let g = Glyph { x: self.pen.0, y: self.pen.1, w, h, left, top };
            self.atlas.sub_alpha(g.x, g.y, w, h, &buf);
            self.pen.0 += w + 1;
            self.pen.2 = self.pen.2.max(h);
            g
        });
        self.glyphs.insert(key, g);
        g
    }

    pub fn text_w(&mut self, font: usize, px: f32, s: &str) -> f32 {
        let mut x = 0.0;
        let mut prev: Option<(usize, GlyphId)> = None;
        for ch in s.chars() {
            let (f, id) = self.font_for(font, ch);
            let sf = self.fonts[f].as_scaled(Self::em(&self.fonts[f], px));
            if let Some((pf, pid)) = prev {
                if pf == f {
                    x += sf.kern(pid, id);
                }
            }
            x += sf.h_advance(id);
            prev = Some((f, id));
        }
        x
    }

    /// Текст; `y` — середина строки по высоте прописных букв. Возвращает ширину.
    pub fn text(&mut self, font: usize, px: f32, x: f32, y: f32, color: Color, s: &str) -> f32 {
        if color[3] <= 0.003 || s.is_empty() {
            return self.text_w(font, px, s);
        }
        let base = if font == ICON { (y + px * 0.36).round() } else { (y + px * 0.355).round() };
        let mut pen = x.round();
        let start = pen;
        let mut prev: Option<(usize, GlyphId)> = None;
        self.verts.clear();
        for ch in s.chars() {
            let (f, id) = self.font_for(font, ch);
            let (adv, kern) = {
                let sf = self.fonts[f].as_scaled(Self::em(&self.fonts[f], px));
                (sf.h_advance(id), prev.filter(|p| p.0 == f).map(|p| sf.kern(p.1, id)).unwrap_or(0.0))
            };
            pen += kern;
            if let Some(g) = self.glyph(f, px, id) {
                let (x0, y0) = ((pen + g.left).round(), base + g.top.round());
                let (x1, y1) = (x0 + g.w as f32, y0 + g.h as f32);
                let a = ATLAS as f32;
                let (u0, v0, u1, v1) = (g.x as f32 / a, g.y as f32 / a, (g.x + g.w) as f32 / a, (g.y + g.h) as f32 / a);
                self.verts.extend_from_slice(&[x0, y0, u0, v0, x1, y0, u1, v0, x1, y1, u1, v1, x0, y0, u0, v0, x1, y1, u1, v1, x0, y1, u0, v1]);
            }
            pen += adv;
            prev = Some((f, id));
        }
        if !self.verts.is_empty() {
            let p = &mut self.text;
            p.bind();
            p.f2("u_view", self.view.0, self.view.1);
            p.f1("u_flip", self.flip);
            p.f4("u_color", color);
            p.i1("u_tex", 0);
            unsafe {
                self.gl.active_texture(glow::TEXTURE0);
                self.gl.bind_texture(glow::TEXTURE_2D, Some(self.atlas.id));
                self.gl.bind_vertex_array(Some(self.text_vao));
                self.gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.text_vbo));
                let bytes = std::slice::from_raw_parts(self.verts.as_ptr() as *const u8, self.verts.len() * 4);
                self.gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
                self.gl.draw_arrays(glow::TRIANGLES, 0, (self.verts.len() / 4) as i32);
            }
        }
        pen - start
    }

    /// Текст по центру прямоугольника.
    pub fn text_c(&mut self, font: usize, px: f32, r: Rect, color: Color, s: &str) {
        let w = self.text_w(font, px, s);
        self.text(font, px, r.cx() - w / 2.0, r.cy(), color, s);
    }

    /// Обрезать строку с многоточием, чтобы влезла в ширину.
    pub fn fit(&mut self, font: usize, px: f32, s: &str, max_w: f32) -> String {
        if self.text_w(font, px, s) <= max_w {
            return s.to_string();
        }
        let mut out: Vec<char> = s.chars().collect();
        while !out.is_empty() {
            out.pop();
            let t: String = out.iter().collect::<String>() + "…";
            if self.text_w(font, px, &t) <= max_w {
                return t;
            }
        }
        String::new()
    }

    /// Перенос по словам; возвращает строки.
    pub fn wrap(&mut self, font: usize, px: f32, s: &str, max_w: f32) -> Vec<String> {
        let mut lines = vec![];
        for para in s.split('\n') {
            let mut cur = String::new();
            for word in para.split_whitespace() {
                let probe = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
                if self.text_w(font, px, &probe) > max_w && !cur.is_empty() {
                    lines.push(std::mem::take(&mut cur));
                    cur = word.to_string();
                } else {
                    cur = probe;
                }
            }
            lines.push(cur);
        }
        lines
    }
}
