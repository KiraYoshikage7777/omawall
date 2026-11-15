//! Поток видео. Всё, что связано с mpv (создание проигрывателя, первый кадр, отрисовка
//! каждого кадра, закрытие), выполняется здесь, в собственном контексте GL с общими
//! текстурами, — главный цикл с меню и переходами из-за видео никогда не замирает.
use crate::gfx::{Egl, Gl, Target};
use crate::mpv::{Player, VideoOpts};
use glow::HasContext;
use khronos_egl as egl;
use std::ffi::{c_char, c_void, CStr};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

type EglInst = egl::DynamicInstance<egl::EGL1_4>;

/// То, что видит главный поток: три текстуры по кругу и номер готовой.
pub struct Shared {
    pub front: AtomicI32,
    pub in_use: AtomicI32,
    pub tex: [AtomicU32; 3],
    pub size: AtomicU64,
    pub serial: AtomicU64,
    pub failed: AtomicBool,
    pub info: Mutex<String>,
}

impl Shared {
    fn new() -> Arc<Shared> {
        Arc::new(Shared { front: AtomicI32::new(-1), in_use: AtomicI32::new(-1), tex: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)], size: AtomicU64::new(0), serial: AtomicU64::new(0), failed: AtomicBool::new(false), info: Mutex::new(String::new()) })
    }
}

pub enum Cmd {
    Load { id: u64, file: String, opts: VideoOpts, max: (u32, u32), shared: Arc<Shared> },
    Unload,
    Pause(bool),
    Opts(VideoOpts),
    Wake(u64),
    Quit,
}

struct Handles {
    display: egl::Display,
    config: egl::Config,
    ctx: egl::Context,
    wl_display: *mut c_void,
}
unsafe impl Send for Handles {}

extern "C" fn get_proc(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let inst = unsafe { &*(ctx as *const EglInst) };
    let name = unsafe { CStr::from_ptr(name) }.to_str().unwrap_or("");
    inst.get_proc_address(name).map(|p| p as *mut c_void).unwrap_or(std::ptr::null_mut())
}

struct Slot {
    id: u64,
    shared: Arc<Shared>,
    targets: Vec<Target>,
    max: (u32, u32),
    frames: u64,
    player: *const Player,
}

/// Один проигрыватель в своём потоке. Создание контекста отрисовки mpv занимает больше
/// полсекунды, поэтому проигрыватель переживает своё видео и открывает следующее мгновенно,
/// а новый создаётся в новом потоке и не останавливает уже идущее видео.
struct Worker {
    tx: Sender<Cmd>,
    /// Номер видео, которое сейчас играет этот поток (0 — свободен).
    current: Arc<AtomicU64>,
}

pub struct Video {
    workers: Vec<Worker>,
    next: u64,
    display: egl::Display,
    config: egl::Config,
    main_ctx: egl::Context,
    wl_display: *mut c_void,
    notify: Arc<dyn Fn() + Send + Sync>,
}

pub struct Opened {
    pub id: u64,
    pub shared: Arc<Shared>,
    pub tx: Sender<Cmd>,
    pub current: Arc<AtomicU64>,
}

fn worker(handles: Handles, opts: VideoOpts, current: Arc<AtomicU64>, tx: Sender<Cmd>, rx: std::sync::mpsc::Receiver<Cmd>, notify: Arc<dyn Fn() + Send + Sync>) {
    let handles = handles;
    let Ok(lib) = (unsafe { libloading::Library::new("libEGL.so.1") }) else { return };
    let Ok(inst) = (unsafe { EglInst::load_required_from(lib) }) else { return };
    let _ = inst.bind_api(egl::OPENGL_ES_API);
    let mut surface = None;
    if inst.make_current(handles.display, None, None, Some(handles.ctx)).is_err() {
        surface = inst.create_pbuffer_surface(handles.display, handles.config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE]).ok();
        if inst.make_current(handles.display, surface, surface, Some(handles.ctx)).is_err() {
            eprintln!("omawall: could not create a GL context for video");
            return;
        }
    }
    let gl: Gl = Rc::new(unsafe { glow::Context::from_loader_function_cstr(|s| inst.get_proc_address(s.to_str().unwrap_or("")).map(|p| p as *const c_void).unwrap_or(std::ptr::null())) });
    let cur = current.clone();
    let mut player = Player::new(get_proc, &inst as *const EglInst as *mut c_void, handles.wl_display, &opts, Box::new(move || {
        let id = cur.load(Ordering::Acquire);
        if id != 0 {
            let _ = tx.send(Cmd::Wake(id));
        }
    }));
    let mut slot: Option<Slot> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Quit => break,
            Cmd::Load { id, file, opts, max, shared } => match &mut player {
                Some(p) => {
                    p.load(&file, &opts);
                    slot = Some(Slot { id, shared, targets: vec![], max, frames: 0, player: p as *const Player });
                }
                None => {
                    shared.failed.store(true, Ordering::Release);
                    notify();
                }
            },
            Cmd::Unload => {
                slot = None;
                if let Some(p) = &mut player {
                    p.stop();
                }
            }
            Cmd::Pause(paused) => {
                if let Some(p) = &mut player {
                    p.set_paused(paused);
                }
                if let Some(s) = &slot {
                    describe(s);
                }
            }
            Cmd::Opts(opts) => {
                if let Some(p) = &player {
                    p.apply_opts(&opts);
                }
            }
            Cmd::Wake(id) => {
                let (Some(s), Some(p)) = (slot.as_mut().filter(|s| s.id == id), player.as_ref()) else { continue };
                if p.drain_events() {
                    s.shared.failed.store(true, Ordering::Release);
                    notify();
                    continue;
                }
                let Some((vw, vh)) = p.size() else { continue };
                if !p.has_frame() {
                    continue;
                }
                // Кадр готовим не крупнее экрана: 4K-видео на экране 1080p не нужно рисовать в 4K.
                let k = (s.max.0 as f32 / vw as f32).max(s.max.1 as f32 / vh as f32).min(1.0);
                let (w, h) = (((vw as f32 * k).round() as u32).max(2), ((vh as f32 * k).round() as u32).max(2));
                if s.targets.first().map(|t| (t.tex.w, t.tex.h)) != Some((w, h)) {
                    s.shared.front.store(-1, Ordering::Release);
                    s.targets = (0..3).map(|_| Target::new(&gl, w, h)).collect();
                    for (i, t) in s.targets.iter().enumerate() {
                        s.shared.tex[i].store(t.tex.raw(), Ordering::Release);
                    }
                    s.shared.size.store(((w as u64) << 32) | h as u64, Ordering::Release);
                }
                let (front, used) = (s.shared.front.load(Ordering::Acquire), s.shared.in_use.load(Ordering::Acquire));
                let back = (0..3).find(|i| *i != front && *i != used).unwrap_or(0);
                p.render(s.targets[back as usize].fbo_id(), w, h);
                // Главный поток возьмёт текстуру только после того, как кадр дорисован.
                unsafe { gl.finish() };
                s.shared.front.store(back, Ordering::Release);
                s.shared.serial.fetch_add(1, Ordering::AcqRel);
                s.frames += 1;
                if s.frames == 1 {
                    describe(s);
                }
                notify();
            }
        }
    }
    drop(slot);
    drop(player);
    let _ = inst.make_current(handles.display, None, None, None);
    if let Some(s) = surface {
        let _ = inst.destroy_surface(handles.display, s);
    }
    let _ = inst.destroy_context(handles.display, handles.ctx);
}

impl Video {
    /// `notify` будит главный цикл, когда готов новый кадр.
    pub fn start(main: &Egl, wl_display: *mut c_void, notify: Arc<dyn Fn() + Send + Sync>) -> Option<Video> {
        Some(Video { workers: vec![], next: 1, display: main.display, config: main.config, main_ctx: main.ctx, wl_display, notify })
    }

    fn spawn(&mut self, main: &Egl, opts: &VideoOpts) -> Option<usize> {
        let ctx = main.inst.create_context(self.display, self.config, Some(self.main_ctx), &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]).ok()?;
        let handles = Handles { display: self.display, config: self.config, ctx, wl_display: self.wl_display };
        let (tx, rx) = channel::<Cmd>();
        let current = Arc::new(AtomicU64::new(0));
        let (cur, tx_thread, notify, opts) = (current.clone(), tx.clone(), self.notify.clone(), opts.clone());
        std::thread::Builder::new().name("omawall-video".into()).spawn(move || worker(handles, opts, cur, tx_thread, rx, notify)).ok()?;
        self.workers.push(Worker { tx, current });
        Some(self.workers.len() - 1)
    }

    pub fn open(&mut self, main: &Egl, file: &str, opts: &VideoOpts, max: (u32, u32)) -> Option<Opened> {
        let id = self.next;
        self.next += 1;
        let i = match self.workers.iter().position(|w| w.current.load(Ordering::Acquire) == 0) {
            Some(i) => i,
            None => self.spawn(main, opts)?,
        };
        let w = &self.workers[i];
        let shared = Shared::new();
        w.current.store(id, Ordering::Release);
        let _ = w.tx.send(Cmd::Load { id, file: file.to_string(), opts: opts.clone(), max, shared: shared.clone() });
        Some(Opened { id, shared, tx: w.tx.clone(), current: w.current.clone() })
    }

    /// Заранее поднять свободный проигрыватель, чтобы следующее видео открылось сразу.
    pub fn prewarm(&mut self, main: &Egl, opts: &VideoOpts) {
        if !self.workers.iter().any(|w| w.current.load(Ordering::Acquire) == 0) && self.workers.len() < 3 {
            self.spawn(main, opts);
        }
    }

    /// Закрыть свободные проигрыватели: каждый держит декодер и память.
    pub fn trim(&mut self, keep: usize) {
        let mut idle = 0;
        self.workers.retain(|w| {
            if w.current.load(Ordering::Acquire) != 0 {
                return true;
            }
            idle += 1;
            if idle > keep {
                let _ = w.tx.send(Cmd::Quit);
                return false;
            }
            true
        });
    }

    pub fn set_opts(&self, opts: &VideoOpts) {
        for w in &self.workers {
            let _ = w.tx.send(Cmd::Opts(opts.clone()));
        }
    }
}

fn describe(s: &Slot) {
    let p = unsafe { &*s.player };
    let text = format!(
        "{}x{} {} fps, {}, {}",
        p.get("dwidth").unwrap_or_default(),
        p.get("dheight").unwrap_or_default(),
        p.get("container-fps").map(|f| f.chars().take(5).collect::<String>()).unwrap_or_default(),
        p.get("hwdec-current").filter(|h| h != "no").unwrap_or_else(|| crate::i18n::tr("процессор").into()),
        if p.paused { crate::i18n::tr("пауза") } else { crate::i18n::tr("играет") }
    );
    *s.shared.info.lock().unwrap() = text;
}

impl Drop for Video {
    fn drop(&mut self) {
        for w in &self.workers {
            let _ = w.tx.send(Cmd::Quit);
        }
    }
}
