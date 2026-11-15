//! Движок обоев: источники (картинка или видео), эффекты, переходы по выходам.
use crate::draw::Draw;
use crate::fx;
use crate::gfx::{Egl, Gl, Prog, Target, Tex};
use crate::library::{Fx, Rgba};
use crate::mpv::VideoOpts;
use crate::video::{Cmd, Shared, Video};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use glow::HasContext;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

pub enum Media {
    Image(Tex),
    /// Кадры рисует поток видео в общие текстуры; здесь только ссылка на готовую.
    Video { shared: Arc<Shared>, tx: Sender<Cmd>, current: Arc<std::sync::atomic::AtomicU64>, view: Option<Tex>, seen: u64, paused: bool, failed: bool },
}

pub struct Source {
    pub path: String,
    pub media: Media,
    pub fx: Vec<Fx>,
    baked: Option<Target>,
    scratch: Option<Target>,
    fx_dirty: bool,
    pub last_shown: Instant,
}

pub type Src = Rc<RefCell<Source>>;

impl Source {
    pub fn is_video(&self) -> bool {
        matches!(self.media, Media::Video { .. })
    }
    pub fn raw(&self) -> Option<&Tex> {
        match &self.media {
            Media::Image(t) => Some(t),
            Media::Video { view, .. } => view.as_ref(),
            _ => None,
        }
    }
    /// Текстура, готовая к показу (после эффектов).
    pub fn tex(&self) -> Option<&Tex> {
        if !self.fx.is_empty() {
            if let Some(b) = &self.baked {
                return Some(&b.tex);
            }
        }
        self.raw()
    }
    pub fn ready(&self) -> bool {
        self.raw().is_some()
    }
    pub fn failed(&self) -> bool {
        matches!(self.media, Media::Video { failed: true, .. })
    }
    pub fn set_fx(&mut self, fx: Vec<Fx>) {
        if self.fx != fx {
            self.fx = fx;
            self.fx_dirty = true;
        }
    }
    pub fn set_paused(&mut self, paused: bool) {
        if let Media::Video { tx, paused: was, .. } = &mut self.media {
            if *was != paused {
                *was = paused;
                let _ = tx.send(Cmd::Pause(paused));
            }
        }
    }
    pub fn paused(&self) -> bool {
        matches!(&self.media, Media::Video { paused: true, .. })
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        if let Media::Video { tx, current, .. } = &self.media {
            // Поток освобождается сразу: следующее видео может занять его, не дожидаясь закрытия.
            current.store(0, Ordering::Release);
            let _ = tx.send(Cmd::Unload);
        }
    }
}

#[derive(Default)]
pub struct Slot {
    pub cur: Option<Src>,
    pub prev: Option<Src>,
    start: Option<Instant>,
    dur: f32,
    kind: i32,
    seed: f32,
}

impl Slot {
    pub fn animating(&self) -> bool {
        self.prev.is_some()
    }
}

struct Loading {
    src: Src,
    outputs: Vec<String>,
    instant: bool,
    since: Instant,
}

pub struct Engine {
    gl: Gl,
    wall: Prog,
    fxp: Prog,
    pub slots: HashMap<String, Slot>,
    loading: Vec<Loading>,
    /// Недавние и заранее загруженные источники (живут, пока открыто меню).
    cache: Vec<Src>,
    pub cache_limit: usize,
    seed: u32,
    pub fill: String,
    pub transition: String,
    pub duration_ms: u32,
    palettes_extra: Vec<[f32; 3]>,
    video: Option<Video>,
    /// Экономный режим: тяжёлые переходы заменяются растворением.
    pub saver: bool,
}

fn fit(tex: (u32, u32), surf: (u32, u32), mode: &str) -> [f32; 4] {
    let (tw, th, sw, sh) = (tex.0 as f32, tex.1 as f32, surf.0 as f32, surf.1 as f32);
    let s = match mode {
        "contain" => (sw / tw).min(sh / th),
        "stretch" => return [1.0, 1.0, 0.0, 0.0],
        _ => (sw / tw).max(sh / th),
    };
    [sw / (tw * s), sh / (th * s), 0.0, 0.0]
}

impl Engine {
    pub fn new(gl: &Gl) -> Engine {
        Engine {
            gl: gl.clone(),
            wall: Prog::new(gl, fx::FULL_VS, fx::WALL_FS),
            fxp: Prog::new(gl, fx::FULL_VS, fx::FX_FS),
            slots: HashMap::new(),
            loading: vec![],
            cache: vec![],
            cache_limit: 0,
            seed: std::process::id(),
            fill: "cover".into(),
            transition: "random".into(),
            duration_ms: 700,
            palettes_extra: vec![],
            video: None,
            saver: false,
        }
    }

    /// Цвета темы Omarchy — отдельной палитрой для эффекта «Палитра».
    pub fn set_theme_palette(&mut self, colors: Vec<[f32; 3]>) {
        self.palettes_extra = colors;
    }

    /// Уже загруженный источник для этого пути (на экране, в загрузке или в кеше).
    pub fn find(&self, path: &str) -> Option<Src> {
        let hit = |s: &Src| s.borrow().path == path;
        self.slots.values().filter_map(|s| s.cur.as_ref()).find(|s| hit(s)).or_else(|| self.cache.iter().find(|s| hit(s))).or_else(|| self.loading.iter().map(|l| &l.src).find(|s| hit(s))).cloned()
    }

    pub fn image_source(&mut self, path: &str, img: &Rgba, fx: &[Fx]) -> Src {
        unsafe { self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0) };
        let tex = Tex::rgba(&self.gl, img.w, img.h, Some(&img.data), true);
        Rc::new(RefCell::new(Source { path: path.into(), media: Media::Image(tex), fx: fx.to_vec(), baked: None, scratch: None, fx_dirty: !fx.is_empty(), last_shown: Instant::now() }))
    }

    /// Открыть видео. Вся работа с mpv идёт в потоке видео, главный цикл не ждёт.
    #[allow(clippy::too_many_arguments)]
    pub fn video_source(&mut self, egl: &Egl, wl_display: *mut std::ffi::c_void, path: &str, file: &str, fx: &[Fx], opts: &VideoOpts, max: (u32, u32), notify: Arc<dyn Fn() + Send + Sync>) -> Src {
        if self.video.is_none() && crate::mpv::available() {
            self.video = Video::start(egl, wl_display, notify);
        }
        let browsing = self.cache_limit > 0;
        let media = match self.video.as_mut().and_then(|v| {
            let o = v.open(egl, file, opts, max);
            // Пока открыто меню, держим наготове ещё один проигрыватель для следующего видео.
            if browsing {
                v.prewarm(egl, opts);
            }
            o
        }) {
            Some(o) => Media::Video { shared: o.shared, tx: o.tx, current: o.current, view: None, seen: 0, paused: false, failed: false },
            None => {
                let (tx, _) = std::sync::mpsc::channel();
                Media::Video { shared: Arc::new(Shared { front: (-1).into(), in_use: (-1).into(), tex: [0.into(), 0.into(), 0.into()], size: 0.into(), serial: 0.into(), failed: true.into(), info: Default::default() }), tx, current: Default::default(), view: None, seen: 0, paused: false, failed: true }
            }
        };
        Rc::new(RefCell::new(Source { path: path.into(), media, fx: fx.to_vec(), baked: None, scratch: None, fx_dirty: true, last_shown: Instant::now() }))
    }

    /// Заранее поднять проигрыватель, чтобы первое видео открылось без задержки.
    pub fn prewarm_video(&mut self, egl: &Egl, wl_display: *mut std::ffi::c_void, opts: &VideoOpts, notify: Arc<dyn Fn() + Send + Sync>) {
        if self.video.is_none() && crate::mpv::available() {
            self.video = Video::start(egl, wl_display, notify);
        }
        if let Some(v) = &mut self.video {
            v.prewarm(egl, opts);
        }
    }

    pub fn remember(&mut self, src: &Src) {
        if self.cache_limit == 0 || src.borrow().is_video() {
            return;
        }
        self.cache.retain(|s| !Rc::ptr_eq(s, src));
        self.cache.push(src.clone());
        while self.cache.len() > self.cache_limit {
            self.cache.remove(0);
        }
    }

    pub fn set_cache_limit(&mut self, n: usize) {
        self.cache_limit = n;
        while self.cache.len() > n {
            self.cache.remove(0);
        }
    }

    /// Показать источник на выходах: сразу, если готов, иначе — когда появится первый кадр.
    pub fn show(&mut self, src: Src, outputs: Vec<String>, instant: bool) {
        // Новый запрос для выхода отменяет прежние ожидания.
        for l in &mut self.loading {
            l.outputs.retain(|o| !outputs.contains(o));
        }
        self.loading.retain(|l| !l.outputs.is_empty());
        if src.borrow().ready() {
            for o in outputs {
                self.set_slot(&o, src.clone(), instant);
            }
        } else {
            self.loading.push(Loading { src, outputs, instant, since: Instant::now() });
        }
    }

    fn set_slot(&mut self, output: &str, src: Src, instant: bool) {
        self.seed = self.seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let mut kind = fx::transition_index(&self.transition, self.seed >> 8);
        if self.saver && [6, 7, 8, 12, 13, 15, 16].contains(&kind) {
            kind = 0;
        }
        let slot = self.slots.entry(output.to_string()).or_default();
        if slot.cur.as_ref().is_some_and(|c| Rc::ptr_eq(c, &src)) {
            return;
        }
        let animate = !instant && kind >= 0 && self.duration_ms > 0 && slot.cur.is_some();
        slot.prev = if animate { slot.cur.take() } else { None };
        slot.cur = Some(src);
        slot.start = None;
        slot.dur = self.duration_ms as f32 / 1000.0;
        slot.kind = kind;
        slot.seed = (self.seed >> 8) as f32 / 16777216.0;
    }

    pub fn remove_output(&mut self, name: &str) {
        self.slots.remove(name);
        for l in &mut self.loading {
            l.outputs.retain(|o| o != name);
        }
        self.loading.retain(|l| !l.outputs.is_empty());
    }

    /// Все уникальные видео (на экране и в загрузке).
    fn videos(&self) -> Vec<Src> {
        let mut v: Vec<Src> = vec![];
        let all = self.slots.values().flat_map(|s| s.cur.iter().chain(s.prev.iter())).chain(self.loading.iter().map(|l| &l.src));
        for s in all {
            if s.borrow().is_video() && !v.iter().any(|x| Rc::ptr_eq(x, s)) {
                v.push(s.clone());
            }
        }
        v
    }

    /// Сведения о видео на экране: для `omawall status`.
    pub fn video_info(&self) -> Vec<String> {
        self.videos()
            .iter()
            .map(|v| match &v.borrow().media {
                Media::Video { shared, failed: false, .. } => shared.info.lock().unwrap().clone(),
                _ => crate::i18n::tr("не открылось").into(),
            })
            .collect()
    }

    pub fn has_video(&self) -> bool {
        !self.videos().is_empty()
    }

    /// Подхватить новые кадры от потока видео. Возвращает имена выходов, которые надо перерисовать.
    pub fn pump_videos(&mut self) -> Vec<String> {
        let mut dirty = vec![];
        for src in self.videos() {
            let mut s = src.borrow_mut();
            let s = &mut *s;
            let Media::Video { shared, view, seen, failed, .. } = &mut s.media else { continue };
            if shared.failed.load(Ordering::Acquire) {
                *failed = true;
                continue;
            }
            let serial = shared.serial.load(Ordering::Acquire);
            if serial == *seen {
                continue;
            }
            *seen = serial;
            let front = shared.front.load(Ordering::Acquire);
            if front < 0 {
                continue;
            }
            shared.in_use.store(front, Ordering::Release);
            let size = shared.size.load(Ordering::Acquire);
            *view = Tex::borrowed(&self.gl, shared.tex[front as usize].load(Ordering::Acquire), (size >> 32) as u32, size as u32);
            s.fx_dirty = true;
            for (n, slot) in &self.slots {
                if slot.cur.as_ref().is_some_and(|c| Rc::ptr_eq(c, &src)) || slot.prev.as_ref().is_some_and(|c| Rc::ptr_eq(c, &src)) {
                    dirty.push(n.clone());
                }
            }
        }
        dirty
    }

    /// Готовые источники из очереди загрузки отправить на экран. Возвращает затронутые выходы.
    pub fn promote(&mut self) -> Vec<String> {
        let mut dirty = vec![];
        let mut i = 0;
        while i < self.loading.len() {
            let (ready, failed) = {
                let s = self.loading[i].src.borrow();
                (s.ready(), s.failed() || self.loading[i].since.elapsed().as_secs() > 15)
            };
            if ready {
                let l = self.loading.remove(i);
                for o in l.outputs {
                    self.set_slot(&o, l.src.clone(), l.instant);
                    dirty.push(o);
                }
            } else if failed {
                eprintln!("omawall: could not open {}", self.loading[i].src.borrow().path);
                self.loading.remove(i);
            } else {
                i += 1;
            }
        }
        dirty
    }

    pub fn is_loading(&self) -> bool {
        !self.loading.is_empty()
    }

    /// Применить цепочку эффектов (для картинки — один раз, для видео — на каждый кадр).
    pub fn bake(&mut self, src: &Src, draw: &mut Draw) {
        let mut s = src.borrow_mut();
        let s = &mut *s;
        if !s.fx_dirty {
            return;
        }
        s.fx_dirty = false;
        if s.fx.is_empty() {
            s.baked = None;
            s.scratch = None;
            return;
        }
        let Some((w, h, raw_id)) = (match &s.media {
            Media::Image(t) => Some((t.w, t.h, t.id)),
            Media::Video { view: Some(t), .. } => Some((t.w, t.h, t.id)),
            _ => None,
        }) else {
            s.fx_dirty = true;
            return;
        };
        // Размытие — два прохода (по горизонтали и по вертикали).
        let mut passes: Vec<(i32, [f32; 4], [[f32; 3]; 2])> = vec![];
        for f in &s.fx {
            let Some(d) = fx::def(&f.id) else { continue };
            passes.push((d.code, f.p, f.c));
            if d.code == 25 {
                passes.push((26, f.p, f.c));
            }
        }
        if passes.is_empty() {
            s.baked = None;
            return;
        }
        for t in [&mut s.baked, &mut s.scratch] {
            if t.as_ref().map(|t| (t.tex.w, t.tex.h)) != Some((w, h)) {
                *t = Some(Target::new(&self.gl, w, h));
            }
        }
        let gl = &self.gl;
        let p = &mut self.fxp;
        p.bind();
        p.f1("u_flip", -1.0);
        p.i1("u_tex", 0);
        p.f2("u_px", 1.0 / w as f32, 1.0 / h as f32);
        unsafe {
            gl.disable(glow::BLEND);
            gl.active_texture(glow::TEXTURE0);
        }
        // Последний проход всегда попадает в baked.
        let n = passes.len();
        let mut input = raw_id;
        for (i, (code, pp, cc)) in passes.iter().enumerate() {
            let to_baked = (n - 1 - i) % 2 == 0;
            let dst = if to_baked { s.baked.as_ref().unwrap() } else { s.scratch.as_ref().unwrap() };
            dst.bind();
            p.i1("u_fx", *code);
            p.f4("u_p", *pp);
            p.f3v("u_c0", &cc[0]);
            p.f3v("u_c1", &cc[1]);
            if *code == 1 {
                let idx = pp[0].round().max(0.0) as usize;
                let cols: Vec<[f32; 3]> = if idx < fx::PALETTES.len() {
                    fx::PALETTES[idx].1.iter().map(|&v| { let c = crate::draw::hex(v); [c[0], c[1], c[2]] }).collect()
                } else if !self.palettes_extra.is_empty() {
                    self.palettes_extra.clone()
                } else {
                    vec![[0.0; 3], [1.0; 3]]
                };
                let flat: Vec<f32> = cols.iter().take(16).flatten().copied().collect();
                p.f3v("u_pal", &flat);
                p.i1("u_paln", (flat.len() / 3) as i32);
            }
            unsafe { gl.bind_texture(glow::TEXTURE_2D, Some(input)) };
            draw.quad();
            input = dst.tex.id;
        }
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.enable(glow::BLEND);
        }
        if !s.is_video() {
            s.scratch = None;
        }
    }

    /// Нарисовать обои выхода в текущую поверхность. true — переход ещё идёт.
    pub fn render(&mut self, output: &str, size: (u32, u32), draw: &mut Draw) -> bool {
        let now = Instant::now();
        let (cur, prev, t, kind, seed) = {
            let Some(slot) = self.slots.get_mut(output) else {
                draw.begin(size.0, size.1, false);
                draw.clear([0.0, 0.0, 0.0, 1.0]);
                return false;
            };
            // Отсчёт перехода начинается с первого реально показанного кадра.
            let start = *slot.start.get_or_insert(now);
            let mut t = if slot.dur > 0.0 { (now - start).as_secs_f32() / slot.dur } else { 1.0 };
            if slot.prev.is_none() {
                t = 1.0;
            }
            if t >= 1.0 {
                slot.prev = None;
                t = 1.0;
            }
            (slot.cur.clone(), slot.prev.clone(), t, slot.kind, slot.seed)
        };
        for s in [&cur, &prev].into_iter().flatten() {
            self.bake(s, draw);
            s.borrow_mut().last_shown = now;
        }
        draw.begin(size.0, size.1, false);
        let cb = cur.as_ref().map(|s| s.borrow());
        let pb = prev.as_ref().map(|s| s.borrow());
        let tb = cb.as_ref().and_then(|s| s.tex());
        let ta = pb.as_ref().and_then(|s| s.tex());
        if tb.is_none() && ta.is_none() {
            draw.clear([0.0, 0.0, 0.0, 1.0]);
            return false;
        }
        let gl = &self.gl;
        let p = &mut self.wall;
        p.bind();
        p.f1("u_flip", 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        p.f1("u_t", if ta.is_some() { e } else { 1.0 });
        p.i1("u_type", kind);
        p.f1("u_ratio", size.0 as f32 / size.1.max(1) as f32);
        p.f1("u_seed", seed);
        unsafe {
            gl.disable(glow::BLEND);
            for (unit, tex, name, fname, hname) in [(0u32, ta, "u_a", "u_fa", "u_has_a"), (1, tb, "u_b", "u_fb", "u_has_b")] {
                gl.active_texture(glow::TEXTURE0 + unit);
                match tex {
                    Some(tx) => {
                        gl.bind_texture(glow::TEXTURE_2D, Some(tx.id));
                        p.f4(fname, fit((tx.w, tx.h), size, &self.fill));
                        p.f1(hname, 1.0);
                    }
                    None => {
                        gl.bind_texture(glow::TEXTURE_2D, None);
                        p.f1(hname, 0.0);
                    }
                }
                p.i1(name, unit as i32);
            }
            gl.active_texture(glow::TEXTURE0);
        }
        draw.quad();
        unsafe { gl.enable(glow::BLEND) };
        t < 1.0
    }

    /// Остановить или продолжить все видео (пауза по батарее или когда стол не виден).
    pub fn pause_videos(&mut self, paused: bool) {
        for v in self.videos() {
            v.borrow_mut().set_paused(paused);
        }
    }

    /// Остановить видео, которые давно никто не видел, и разбудить те, что снова нужны.
    pub fn idle_check(&mut self, force_pause: bool) {
        for v in self.videos() {
            let mut s = v.borrow_mut();
            let hidden = s.last_shown.elapsed().as_secs_f32() > 2.5;
            let loading = self.loading.iter().any(|l| Rc::ptr_eq(&l.src, &v));
            s.set_paused(force_pause || (hidden && !loading));
        }
    }

    pub fn wake_video(&mut self, output: &str, force_pause: bool) {
        if force_pause {
            return;
        }
        if let Some(slot) = self.slots.get(output) {
            for s in slot.cur.iter().chain(slot.prev.iter()) {
                let mut s = s.borrow_mut();
                if s.paused() {
                    s.last_shown = Instant::now();
                    s.set_paused(false);
                }
            }
        }
    }

    pub fn apply_video_opts(&mut self, opts: &VideoOpts) {
        if let Some(v) = &self.video {
            v.set_opts(opts);
        }
    }

    /// Сбросить всё, что держит видеопамять сверх показанного.
    pub fn trim(&mut self) {
        self.cache.clear();
        if let Some(v) = &mut self.video {
            v.trim(0);
        }
    }

    pub fn drop_all(&mut self) {
        self.slots.clear();
        self.loading.clear();
        self.cache.clear();
    }
}
