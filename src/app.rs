//! Демон: соединение с Wayland, поверхности обоев и меню, главный цикл, команды.
use crate::config::{self, Config};
use crate::draw::Draw;
use crate::gfx::{Egl, Gl};
use crate::jobs::{Done, Pool};
use crate::library::{self, State};
use crate::mpv::VideoOpts;
use crate::picker::{Action, Env, Picker};
use crate::wallpaper::Engine;
use khronos_egl as egl;
use smithay_client_toolkit::reexports::calloop::{self, generic::Generic, timer::{TimeoutAction, Timer}, EventLoop, Interest, LoopHandle, Mode, PostAction};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::protocols::wp::cursor_shape::v1::client::{wp_cursor_shape_device_v1::{Shape as CursorShape, WpCursorShapeDeviceV1}, wp_cursor_shape_manager_v1::WpCursorShapeManagerV1};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::{wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, wp_fractional_scale_v1::{self, WpFractionalScaleV1}};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_egl::WlEglSurface;

/// Отладка задержек: OMAWALL_DEBUG=1 пишет в журнал всё, что держит главный цикл дольше 6 мс.
pub fn slow(label: &str, since: Instant) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("OMAWALL_DEBUG").is_some()) {
        let ms = since.elapsed().as_secs_f32() * 1000.0;
        if ms > 6.0 {
            eprintln!("omawall: slow {label}: {ms:.1} ms");
        }
    }
}

pub struct Surf {
    pub layer: LayerSurface,
    egl_win: Option<WlEglSurface>,
    pub egl_surf: Option<egl::Surface>,
    viewport: Option<WpViewport>,
    frac: Option<WpFractionalScaleV1>,
    pub logical: (u32, u32),
    pub scale120: u32,
    pub phys: (u32, u32),
    pub frame_pending: bool,
    pub dirty: bool,
    pub configured: bool,
}

impl Surf {
    fn new(layer: LayerSurface, viewporter: &Option<WpViewporter>, frac: &Option<WpFractionalScaleManagerV1>, qh: &QueueHandle<App>) -> Surf {
        let wl = layer.wl_surface().clone();
        // Дробный масштаб рисуем сами в физических пикселях; без этих протоколов — целый масштаб.
        let (viewport, frac) = match (viewporter, frac) {
            (Some(v), Some(f)) => (Some(v.get_viewport(&wl, qh, ())), Some(f.get_fractional_scale(&wl, qh, wl.clone()))),
            _ => (None, None),
        };
        Surf { layer, egl_win: None, egl_surf: None, viewport, frac, logical: (0, 0), scale120: 120, phys: (0, 0), frame_pending: false, dirty: false, configured: false }
    }
    pub fn wl(&self) -> &wl_surface::WlSurface {
        self.layer.wl_surface()
    }
    pub fn scale(&self) -> f32 {
        self.scale120 as f32 / 120.0
    }
    fn resize(&mut self, egl: &Egl) {
        let s = self.scale120 as f64 / 120.0;
        let phys = ((self.logical.0 as f64 * s).round() as u32, (self.logical.1 as f64 * s).round() as u32);
        if phys.0 == 0 || phys.1 == 0 {
            return;
        }
        match &self.egl_win {
            Some(w) => {
                if phys != self.phys {
                    w.resize(phys.0 as i32, phys.1 as i32, 0, 0);
                }
            }
            None => {
                if let Ok(w) = WlEglSurface::new(self.wl().id(), phys.0 as i32, phys.1 as i32) {
                    self.egl_surf = egl.create_surface(w.ptr() as *mut c_void);
                    self.egl_win = Some(w);
                }
            }
        }
        match &self.viewport {
            Some(vp) => vp.set_destination(self.logical.0 as i32, self.logical.1 as i32),
            None => self.wl().set_buffer_scale((self.scale120 / 120).max(1) as i32),
        }
        self.phys = phys;
        self.dirty = true;
    }
    fn destroy(mut self, egl: &Egl) {
        if let Some(s) = self.egl_surf.take() {
            egl.destroy_surface(s);
        }
        self.egl_win = None;
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
        if let Some(f) = self.frac.take() {
            f.destroy();
        }
    }
}

pub struct Out {
    pub wl: wl_output::WlOutput,
    pub name: String,
    pub surf: Surf,
}

struct WantImage {
    path: String,
    outputs: Vec<String>,
    instant: bool,
    preload: bool,
}

pub struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    viewporter: Option<WpViewporter>,
    frac_mgr: Option<WpFractionalScaleManagerV1>,
    cursor_mgr: Option<WpCursorShapeManagerV1>,
    cursor_dev: Option<WpCursorShapeDeviceV1>,
    qh: QueueHandle<App>,
    conn: Connection,
    lh: LoopHandle<'static, App>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    mods: Modifiers,

    egl: Rc<Egl>,
    gl: Gl,
    pub draw: Draw,
    pub engine: Engine,
    pub outs: Vec<Out>,
    pub picker: Option<Box<Picker>>,
    picker_surf: Option<Surf>,
    picker_output: Option<String>,

    pub cfg: Config,
    pub state: State,
    pub pool: Pool,

    req: u64,
    wants: HashMap<u64, WantImage>,
    live: Arc<Mutex<HashSet<u64>>>,
    video_wake: Arc<AtomicBool>,
    save_at: Option<Instant>,
    tick_armed: bool,
    trim_due: bool,
    link_target: String,
    theme_name: String,
    on_battery: bool,
    weak: bool,
    last_battery_check: Instant,
    last_click: Option<Instant>,
    scanning: bool,
    converting: bool,
    failed_scenes: HashSet<String>,
    rotation_gen: u64,
    started: Instant,
    exit: bool,
}

pub fn run() -> Result<(), String> {
    let sock = config::socket_path();
    if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
        return Err(crate::i18n::tr("демон уже запущен").into());
    }
    let _ = std::fs::remove_file(&sock);
    // Оверлеи вроде MangoHud не должны цепляться к демону обоев.
    std::env::set_var("DISABLE_MANGOHUD", "1");
    std::env::set_var("MANGOHUD", "0");
    // Меньше арен malloc: рабочие потоки не должны раздувать память демона.
    unsafe {
        libc::mallopt(libc::M_ARENA_MAX, 2);
        libc::mallopt(libc::M_MMAP_THRESHOLD, 256 * 1024);
    }
    let conn = Connection::connect_to_env().map_err(|e| crate::trf!("нет соединения с Wayland: {e}", "no Wayland connection: {e}"))?;
    let (globals, event_queue) = registry_queue_init::<App>(&conn).map_err(|e| e.to_string())?;
    let qh = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let lh = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue).insert(lh.clone()).map_err(|e| e.to_string())?;

    let (tx, rx) = calloop::channel::channel::<Done>();
    lh.insert_source(rx, |ev, _, app: &mut App| {
        if let calloop::channel::Event::Msg(d) = ev {
            app.done(d);
        }
    })
    .map_err(|e| e.to_string())?;

    let listener = UnixListener::bind(&sock).map_err(|e| crate::trf!("сокет {}: {e}", "socket {}: {e}", sock.display()))?;
    listener.set_nonblocking(true).ok();
    lh.insert_source(Generic::new(listener, Interest::READ, Mode::Level), |_, l, app: &mut App| {
        while let Ok((stream, _)) = l.accept() {
            app.serve(stream);
        }
        Ok(PostAction::Continue)
    })
    .map_err(|e| e.to_string())?;

    // Следим за состоянием Omarchy: смена фона и темы штатными командами.
    unsafe {
        let fd = libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC);
        if fd >= 0 {
            let dir = std::ffi::CString::new(config::omarchy_state().to_string_lossy().as_bytes()).unwrap();
            libc::inotify_add_watch(fd, dir.as_ptr(), libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE);
            use std::os::fd::FromRawFd;
            let file = std::fs::File::from_raw_fd(fd);
            let _ = lh.insert_source(Generic::new(file, Interest::READ, Mode::Level), |_, f, app: &mut App| {
                use std::io::Read;
                let mut buf = [0u8; 4096];
                while matches!((&**f).read(&mut buf), Ok(n) if n > 0) {}
                // Команды Omarchy меняют несколько файлов подряд — ждём, пока утихнет.
                let _ = app.lh.insert_source(Timer::from_duration(Duration::from_millis(120)), |_, _, app| {
                    app.omarchy_changed();
                    TimeoutAction::Drop
                });
                Ok(PostAction::Continue)
            });
        }
    }

    let (egl, gl) = Egl::new(conn.backend().display_ptr() as *mut c_void)?;
    let egl = Rc::new(egl);
    let cfg = Config::load();
    if !Config::path().exists() {
        cfg.save();
    }
    let state = State::load();
    let mut engine = Engine::new(&gl);
    engine.fill = cfg.wallpaper.fill.clone();
    engine.transition = cfg.wallpaper.transition.clone();
    engine.duration_ms = cfg.wallpaper.duration_ms;

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).map_err(|e| format!("wl_compositor: {e}"))?,
        layer_shell: LayerShell::bind(&globals, &qh).map_err(|e| crate::trf!("композитор не поддерживает wlr-layer-shell: {e}", "the compositor does not support wlr-layer-shell: {e}"))?,
        viewporter: globals.bind::<WpViewporter, App, ()>(&qh, 1..=1, ()).ok(),
        frac_mgr: globals.bind::<WpFractionalScaleManagerV1, App, ()>(&qh, 1..=1, ()).ok(),
        cursor_mgr: globals.bind::<WpCursorShapeManagerV1, App, ()>(&qh, 1..=1, ()).ok(),
        cursor_dev: None,
        qh,
        conn,
        lh: lh.clone(),
        keyboard: None,
        pointer: None,
        mods: Modifiers::default(),
        draw: Draw::new(&gl),
        engine,
        egl,
        gl,
        outs: vec![],
        picker: None,
        picker_surf: None,
        picker_output: None,
        cfg,
        state,
        pool: Pool::new(tx),
        req: 0,
        wants: HashMap::new(),
        live: Arc::new(Mutex::new(HashSet::new())),
        video_wake: Arc::new(AtomicBool::new(false)),
        save_at: None,
        tick_armed: false,
        trim_due: false,
        link_target: String::new(),
        theme_name: std::fs::read_to_string(config::omarchy_state().join("theme.name")).unwrap_or_default().trim().to_string(),
        on_battery: on_battery(),
        weak: config::weak_machine(),
        last_battery_check: Instant::now(),
        last_click: None,
        scanning: false,
        converting: false,
        failed_scenes: HashSet::new(),
        rotation_gen: 0,
        started: Instant::now(),
        exit: false,
    };
    app.update_saver();
    app.link_target = app.read_link();
    app.load_theme_palette();
    app.rescan();
    app.arm_rotation();

    while !app.exit {
        event_loop.dispatch(None, &mut app).map_err(|e| e.to_string())?;
        let t0 = Instant::now();
        app.flush();
        slow(crate::i18n::tr("весь кадр"), t0);
    }
    app.state.save();
    let _ = std::fs::remove_file(config::socket_path());
    // Потоки видео ещё держат mpv и GL; разбирать их по одному незачем — выходим сразу.
    unsafe { libc::_exit(0) }
}

impl App {
    // ---------- обои ----------

    fn video_opts(&self) -> VideoOpts {
        VideoOpts { mute: self.cfg.video.mute, volume: self.cfg.video.volume, hwdec: self.cfg.video.hwdec }
    }

    /// Будильник для потока видео: один сигнал в главный цикл, пока тот его не обработал.
    fn video_notify(&self) -> Arc<dyn Fn() + Send + Sync> {
        let wake = self.video_wake.clone();
        let tx = Mutex::new(self.pool.tx.clone());
        Arc::new(move || {
            if !wake.swap(true, Ordering::AcqRel) {
                let _ = tx.lock().unwrap().send(Done::Frame);
            }
        })
    }

    fn update_saver_from(&mut self, was: bool) {
        self.cfg.saver_now = was;
        self.update_saver();
    }

    /// Пересчитать, нужен ли сейчас экономный режим.
    pub fn update_saver(&mut self) {
        let now = match self.cfg.video.saver.as_str() {
            "on" => true,
            "off" => false,
            _ => self.weak || self.on_battery,
        };
        if now != self.cfg.saver_now {
            self.cfg.saver_now = now;
            self.engine.saver = now;
            self.mark_picker();
        }
    }

    fn output_names(&self) -> Vec<String> {
        self.outs.iter().map(|o| o.name.clone()).collect()
    }

    /// Какие обои должны стоять на выходе.
    pub fn wanted(&self, output: &str) -> Option<String> {
        self.state.wallpapers.get(output).or_else(|| self.state.wallpapers.get("*")).cloned().or_else(|| Some(self.link_target.clone()).filter(|s| !s.is_empty()))
    }

    /// Самый большой размер экрана в физических пикселях: под него готовим картинку.
    fn target_size(&self) -> (u32, u32) {
        let mut best = (0u32, 0u32);
        for o in &self.outs {
            let mut size = o.surf.phys;
            if size.0 == 0 {
                if let Some(info) = self.output_state.info(&o.wl) {
                    if let Some(m) = info.modes.iter().find(|m| m.current) {
                        size = (m.dimensions.0.max(0) as u32, m.dimensions.1.max(0) as u32);
                    }
                }
            }
            best = (best.0.max(size.0), best.1.max(size.1));
        }
        if best.0 == 0 {
            (1920, 1080)
        } else {
            best
        }
    }

    /// Поставить обои. `target` — имя выхода или None для всех; `commit` — запомнить выбор.
    pub fn apply(&mut self, path: &str, target: Option<&str>, instant: bool, commit: bool) -> bool {
        let (key, file, video, fx) = match self.state.find(path) {
            Some(i) => {
                let it = &self.state.items[i];
                (it.path.clone(), it.file.clone(), it.video, it.fx.clone())
            }
            None => {
                let p = std::path::Path::new(path);
                let real = p.canonicalize().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| path.to_string());
                if !std::path::Path::new(&real).is_file() {
                    return false;
                }
                let video = library::is_video_path(std::path::Path::new(&real));
                (real.clone(), real, video, vec![])
            }
        };
        let outputs: Vec<String> = match target {
            Some(t) => vec![t.to_string()],
            None => self.output_names(),
        };
        if commit {
            match target {
                Some(t) => {
                    self.state.wallpapers.insert(t.to_string(), key.clone());
                }
                None => {
                    self.state.wallpapers.clear();
                    self.state.wallpapers.insert("*".into(), key.clone());
                }
            }
            if let Some(i) = self.state.find(&key) {
                self.state.items[i].applied += 1;
                self.state.items[i].last_applied = library::now();
            }
            self.state.history.retain(|h| h != &key);
            self.state.history.push(key.clone());
            if self.state.history.len() > 50 {
                self.state.history.remove(0);
            }
            if target.is_none() && self.cfg.wallpaper.follow_omarchy {
                self.write_link(&file);
            }
            self.save_soon();
        }
        if outputs.is_empty() {
            return true;
        }
        // Прежние запросы картинок для этих выходов больше не нужны.
        let mut dead = vec![];
        for (id, w) in self.wants.iter_mut() {
            if !w.preload {
                w.outputs.retain(|o| !outputs.contains(o));
                if w.outputs.is_empty() {
                    dead.push(*id);
                }
            }
        }
        for id in dead {
            self.wants.remove(&id);
            self.live.lock().unwrap().remove(&id);
        }
        if let Some(src) = self.engine.find(&key) {
            src.borrow_mut().set_fx(fx);
            self.engine.remember(&src);
            self.engine.show(src, outputs.clone(), instant);
            self.mark(&outputs);
        } else if video {
            let egl = self.egl.clone();
            let display = self.conn.backend().display_ptr() as *mut c_void;
            let (opts, mut max, notify) = (self.video_opts(), self.target_size(), self.video_notify());
            if self.cfg.saver_now {
                // Экономный режим: кадр видео готовим на треть меньше экрана.
                max = (max.0 * 2 / 3, max.1 * 2 / 3);
            }
            let src = self.engine.video_source(&egl, display, &key, &file, &fx, &opts, max, notify);
            self.engine.show(src, outputs, instant);
            self.arm_tick();
        } else {
            // Тот же файл уже декодируется — просто добавим выходы.
            if let Some(w) = self.wants.values_mut().find(|w| w.path == key) {
                for o in outputs {
                    if !w.outputs.contains(&o) {
                        w.outputs.push(o);
                    }
                }
                w.preload = false;
                w.instant = instant;
                return true;
            }
            self.decode(key, file, outputs, instant, false);
        }
        true
    }

    fn decode(&mut self, key: String, file: String, outputs: Vec<String>, instant: bool, preload: bool) {
        self.req += 1;
        let req = self.req;
        self.wants.insert(req, WantImage { path: key.clone(), outputs, instant, preload });
        self.live.lock().unwrap().insert(req);
        let live = self.live.clone();
        let target = self.target_size();
        self.pool.run(!preload, move || {
            if !live.lock().unwrap().contains(&req) {
                return Some(Done::Wall(req, key, None));
            }
            let img = library::decode_image(&file).map(|i| library::fit_cover(i, target));
            Some(Done::Wall(req, key, img))
        });
    }

    /// Заранее загрузить картинку (сосед выбранной в меню), чтобы переход начался сразу.
    pub fn preload(&mut self, path: &str) {
        let Some(i) = self.state.find(path) else { return };
        let it = &self.state.items[i];
        if it.video || self.engine.find(path).is_some() || self.wants.values().any(|w| w.path == path) {
            return;
        }
        let (key, file) = (it.path.clone(), it.file.clone());
        // Незавершённые упреждающие загрузки других картинок отменяем.
        let stale: Vec<u64> = self.wants.iter().filter(|(_, w)| w.preload).map(|(id, _)| *id).collect();
        if stale.len() >= 2 {
            for id in stale {
                self.wants.remove(&id);
                self.live.lock().unwrap().remove(&id);
            }
        }
        self.decode(key, file, vec![], true, true);
    }

    fn mark(&mut self, outputs: &[String]) {
        for o in &mut self.outs {
            if outputs.contains(&o.name) {
                o.surf.dirty = true;
            }
        }
    }

    fn ensure_wallpaper(&mut self, output: &str) {
        let Some(path) = self.wanted(output) else { return };
        let has = self.engine.slots.get(output).and_then(|s| s.cur.as_ref()).is_some_and(|c| c.borrow().path == path);
        if !has {
            let instant = self.started.elapsed() < Duration::from_secs(3);
            if !self.apply(&path, Some(output), instant, false) {
                // Файл пропал — берём фон, на который указывает Omarchy.
                let link = self.link_target.clone();
                if !link.is_empty() && link != path {
                    self.apply(&link, Some(output), instant, false);
                }
            }
        }
    }

    // ---------- Omarchy ----------

    fn read_link(&self) -> String {
        std::fs::canonicalize(config::omarchy_state().join("background")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn write_link(&mut self, file: &str) {
        let link = config::omarchy_state().join("background");
        if !link.parent().is_some_and(|p| p.is_dir()) {
            return;
        }
        let tmp = link.with_extension("omawall");
        let _ = std::fs::remove_file(&tmp);
        if std::os::unix::fs::symlink(file, &tmp).is_ok() && std::fs::rename(&tmp, &link).is_ok() {
            self.link_target = std::fs::canonicalize(file).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| file.to_string());
        }
    }

    fn omarchy_changed(&mut self) {
        let theme = std::fs::read_to_string(config::omarchy_state().join("theme.name")).unwrap_or_default().trim().to_string();
        if theme != self.theme_name {
            self.theme_name = theme;
            self.load_theme_palette();
            if self.cfg.library.theme_backgrounds {
                self.rescan();
            }
            if let Some(p) = &mut self.picker {
                p.theme_changed(&self.cfg);
            }
            self.mark_picker();
        }
        let link = self.read_link();
        if link.is_empty() || link == self.link_target {
            return;
        }
        self.link_target = link.clone();
        if self.cfg.wallpaper.follow_omarchy {
            self.state.wallpapers.clear();
            self.state.wallpapers.insert("*".into(), link.clone());
            self.save_soon();
            self.apply(&link, None, false, false);
            if let Some(p) = &mut self.picker {
                p.external_apply(&link);
            }
        }
    }

    fn load_theme_palette(&mut self) {
        let text = std::fs::read_to_string(config::omarchy_state().join("theme/colors.toml")).unwrap_or_default();
        let mut cols = vec![];
        for line in text.lines() {
            if let Some(i) = line.find('#') {
                if let Some(c) = crate::draw::parse_hex(&line[i..i + 7.min(line.len() - i)]) {
                    cols.push([c[0], c[1], c[2]]);
                }
            }
        }
        self.engine.set_theme_palette(cols);
    }

    // ---------- библиотека ----------

    pub fn rescan(&mut self) {
        if self.scanning {
            return;
        }
        self.scanning = true;
        let cfg = self.cfg.clone();
        self.pool.run(true, move || Some(Done::Scanned(library::scan(&cfg))));
    }

    /// Сцены Wallpaper Engine без ролика записываем по одной в фоне.
    fn convert_next(&mut self) {
        if self.converting || !self.cfg.library.workshop || !crate::we::converter_ready() {
            return;
        }
        let Some((dir, target)) = crate::we::unconverted(&self.cfg).into_iter().find(|(d, _)| !self.failed_scenes.contains(&d.to_string_lossy().into_owned())) else { return };
        self.converting = true;
        self.pool.run(false, move || {
            let ok = crate::we::convert(&dir, &target);
            Some(Done::Converted(dir.to_string_lossy().into_owned(), ok))
        });
    }

    fn queue_thumb(&mut self, i: usize) {
        let it = &self.state.items[i];
        let (path, file, video, thumb) = (it.path.clone(), it.file.clone(), it.video, it.thumb_path());
        self.pool.run(false, move || {
            if thumb.exists() {
                // Миниатюра осталась с прошлого раза, нужен только разбор.
                if let Some(img) = library::load_thumb(&thumb) {
                    let a = library::analyze(&img);
                    let (w, h) = image::image_dimensions(&file).unwrap_or((img.w, img.h));
                    return Some(Done::Made(path, Some(library::ThumbResult { path: file, w, h, duration: 0.0, analysis: Some(a) })));
                }
            }
            Some(Done::Made(path, library::make_thumb(&file, video, &thumb)))
        });
    }

    fn done(&mut self, d: Done) {
        match d {
            Done::Frame => {}
            Done::Wall(req, path, img) => {
                self.live.lock().unwrap().remove(&req);
                let Some(w) = self.wants.remove(&req) else { return };
                let Some(img) = img else {
                    if !w.preload {
                        eprintln!("omawall: could not read {path}");
                    }
                    return;
                };
                let fx = self.state.find(&path).map(|i| self.state.items[i].fx.clone()).unwrap_or_default();
                self.egl.make_current(None);
                let src = self.engine.image_source(&path, &img, &fx);
                drop(img);
                self.engine.remember(&src);
                if !w.outputs.is_empty() {
                    self.engine.show(src, w.outputs.clone(), w.instant);
                    self.mark(&w.outputs);
                }
                self.mark_picker();
            }
            Done::Scanned(found) => {
                self.scanning = false;
                let need = library::merge(&mut self.state, found);
                for i in need {
                    self.queue_thumb(i);
                }
                self.save_soon();
                if let Some(p) = &mut self.picker {
                    p.library_changed(&self.state);
                }
                self.mark_picker();
                self.convert_next();
                let state = self.state.clone();
                self.pool.run(false, move || {
                    library::prune_thumbs(&state);
                    None
                });
            }
            Done::Converted(dir, ok) => {
                self.converting = false;
                if !ok {
                    self.failed_scenes.insert(dir);
                }
                self.rescan();
            }
            Done::Made(path, res) => {
                if let Some(i) = self.state.find(&path) {
                    let it = &mut self.state.items[i];
                    match res {
                        Some(r) => {
                            it.w = r.w;
                            it.h = r.h;
                            if r.duration > 0.0 {
                                it.duration = r.duration;
                            }
                            if let Some(a) = r.analysis {
                                it.hue = a.hue;
                                it.sat = a.sat;
                                it.lum = a.lum;
                                it.rich = a.rich;
                                it.minimal = a.minimal;
                            }
                            it.analyzed = true;
                        }
                        None => it.broken = true,
                    }
                    self.save_soon();
                    if let Some(p) = &mut self.picker {
                        p.item_ready(&self.state, &path);
                    }
                    self.mark_picker();
                }
            }
            other => {
                if let Some(mut p) = self.picker.take() {
                    self.egl.make_current(self.picker_surf.as_ref().and_then(|s| s.egl_surf));
                    let actions = p.done(other, &self.gl, &self.cfg);
                    self.picker = Some(p);
                    self.run_actions(actions);
                    self.mark_picker();
                }
            }
        }
    }

    // ---------- главный цикл ----------

    fn save_soon(&mut self) {
        self.save_at = Some(Instant::now() + Duration::from_millis(400));
        self.arm_tick();
    }

    fn arm_tick(&mut self) {
        if self.tick_armed {
            return;
        }
        self.tick_armed = true;
        let _ = self.lh.insert_source(Timer::from_duration(Duration::from_millis(500)), |_, _, app: &mut App| {
            if app.tick() {
                TimeoutAction::ToDuration(Duration::from_millis(1000))
            } else {
                app.tick_armed = false;
                TimeoutAction::Drop
            }
        });
    }

    /// Редкие фоновые дела. Возвращает false, когда таймер больше не нужен.
    fn tick(&mut self) -> bool {
        let mut busy = false;
        if let Some(at) = self.save_at {
            if Instant::now() >= at {
                self.state.save();
                self.save_at = None;
            } else {
                busy = true;
            }
        }
        if self.engine.has_video() {
            busy = true;
            if (self.cfg.video.pause_on_battery || self.cfg.video.saver == "auto") && self.last_battery_check.elapsed() > Duration::from_secs(10) {
                self.last_battery_check = Instant::now();
                self.on_battery = on_battery();
                self.update_saver();
            }
            let force = self.cfg.video.pause_on_battery && self.on_battery;
            self.engine.idle_check(force);
        }
        if self.picker.is_none() {
            self.engine.trim();
        }
        if !busy || self.trim_due {
            // Вернуть системе память, освободившуюся после декодирования и закрытия меню.
            self.trim_due = false;
            unsafe { libc::malloc_trim(0) };
        }
        busy
    }

    fn mark_picker(&mut self) {
        if let Some(s) = &mut self.picker_surf {
            s.dirty = true;
        }
    }

    /// Отрисовать всё, что изменилось и может быть показано прямо сейчас.
    fn flush(&mut self) {
        if self.engine.has_video() || self.engine.is_loading() {
            self.video_wake.store(false, Ordering::Release);
            let ready: Vec<String> = self.outs.iter().filter(|o| o.surf.configured && !o.surf.frame_pending).map(|o| o.name.clone()).collect();
            let force = self.cfg.video.pause_on_battery && self.on_battery;
            if !force {
                for name in &ready {
                    self.engine.wake_video(name, false);
                }
            }
            let mut dirty = self.engine.pump_videos();
            dirty.extend(self.engine.promote());
            if !dirty.is_empty() {
                self.mark(&dirty);
                self.mark_picker();
            }
        }
        for i in 0..self.outs.len() {
            let o = &self.outs[i];
            if !o.surf.dirty || o.surf.frame_pending || !o.surf.configured {
                continue;
            }
            let Some(es) = o.surf.egl_surf else { continue };
            let (name, size, wl) = (o.name.clone(), o.surf.phys, o.surf.wl().clone());
            let t0 = Instant::now();
            self.egl.make_current(Some(es));
            let animating = self.engine.render(&name, size, &mut self.draw);
            wl.frame(&self.qh, wl.clone());
            self.egl.swap(es);
            slow(crate::i18n::tr("отрисовка обоев"), t0);
            let o = &mut self.outs[i];
            o.surf.frame_pending = true;
            o.surf.dirty = animating;
        }
        self.draw_picker();
        let _ = self.conn.flush();
    }

    fn draw_picker(&mut self) {
        let Some(surf) = &self.picker_surf else { return };
        if !surf.dirty || surf.frame_pending || !surf.configured {
            return;
        }
        let Some(es) = surf.egl_surf else { return };
        let (size, scale, wl) = (surf.phys, surf.scale(), surf.wl().clone());
        let Some(mut p) = self.picker.take() else { return };
        self.egl.make_current(Some(es));
        let preview_src = p.selected_path(&self.state).and_then(|path| self.engine.find(&path));
        if let Some(s) = &preview_src {
            self.engine.bake(s, &mut self.draw);
        }
        self.draw.begin(size.0, size.1, false);
        self.draw.clear([0.0; 4]);
        let mut env = Env { d: &mut self.draw, gl: &self.gl, state: &mut self.state, cfg: &mut self.cfg, pool: &self.pool, size: (size.0 as f32, size.1 as f32), surf_scale: scale, live: preview_src, actions: vec![] };
        let animating = p.frame(&mut env);
        let actions = std::mem::take(&mut env.actions);
        drop(env);
        wl.frame(&self.qh, wl.clone());
        self.egl.swap(es);
        if let Some(s) = &mut self.picker_surf {
            s.frame_pending = true;
            s.dirty = animating;
        }
        self.picker = Some(p);
        self.run_actions(actions);
    }

    fn run_actions(&mut self, actions: Vec<Action>) {
        if actions.is_empty() {
            return;
        }
        for a in actions {
            match a {
                Action::Preview(path) => {
                    let target = self.picker_target();
                    self.apply(&path, target.as_deref(), false, false);
                }
                Action::Commit(path) => {
                    let target = self.picker_target();
                    self.apply(&path, target.as_deref(), false, true);
                }
                Action::Restore => self.restore_wallpapers(),
                Action::Preload(path) => self.preload(&path),
                Action::Close => self.close_picker(),
                Action::SaveState => self.save_soon(),
                Action::SaveConfig => self.config_changed(),
                Action::Rescan => self.rescan(),
                Action::FxChanged(path) => {
                    if let (Some(src), Some(i)) = (self.engine.find(&path), self.state.find(&path)) {
                        src.borrow_mut().set_fx(self.state.items[i].fx.clone());
                        let names = self.output_names();
                        self.mark(&names);
                    }
                    self.save_soon();
                }
                Action::Delete(path) => {
                    if let Some(i) = self.state.find(&path) {
                        let it = self.state.items.remove(i);
                        let _ = std::fs::remove_file(&it.file);
                        let _ = std::fs::remove_file(it.thumb_path());
                        self.save_soon();
                        if let Some(p) = &mut self.picker {
                            p.library_changed(&self.state);
                        }
                    }
                }
                Action::Rotation => self.arm_rotation(),
                Action::Redraw => {}
            }
        }
        if self.picker.is_some() {
            self.mark_picker();
        }
    }

    fn picker_target(&self) -> Option<String> {
        if self.cfg.wallpaper.per_monitor {
            self.picker_output.clone()
        } else {
            None
        }
    }

    /// Вернуть на экраны то, что записано как выбранное (после предпросмотра).
    fn restore_wallpapers(&mut self) {
        for name in self.output_names() {
            self.ensure_wallpaper(&name);
        }
    }

    pub fn config_changed(&mut self) {
        self.cfg.sanitize();
        let was = self.cfg.saver_now;
        self.cfg.saver_now = !was;
        self.update_saver_from(was);
        self.cfg.save();
        self.engine.fill = self.cfg.wallpaper.fill.clone();
        self.engine.transition = self.cfg.wallpaper.transition.clone();
        self.engine.duration_ms = self.cfg.wallpaper.duration_ms;
        let opts = self.video_opts();
        self.engine.apply_video_opts(&opts);
        let names = self.output_names();
        self.mark(&names);
        self.arm_rotation();
        self.arm_tick();
    }

    // ---------- меню ----------

    pub fn open_picker(&mut self) {
        if self.picker_surf.is_some() {
            return;
        }
        let wl = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(&self.qh, wl, Layer::Overlay, Some("omawall-picker"), None);
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        self.picker_surf = Some(Surf::new(layer, &self.viewporter, &self.frac_mgr, &self.qh));
        let current = self.wanted(self.outs.first().map(|o| o.name.as_str()).unwrap_or("*"));
        self.picker = Some(Box::new(Picker::new(&self.state, &self.cfg, current)));
        self.engine.set_cache_limit(4);
        if self.state.items.iter().any(|i| i.video) {
            let egl = self.egl.clone();
            let display = self.conn.backend().display_ptr() as *mut c_void;
            let (opts, notify) = (self.video_opts(), self.video_notify());
            self.engine.prewarm_video(&egl, display, &opts, notify);
        }
        self.rescan();
    }

    pub fn close_picker(&mut self) {
        if let Some(p) = self.picker.take() {
            // Закрыли без «Применить» — возвращаем прежние обои.
            let keep = self.cfg.picker.apply_on_close && self.cfg.picker.live_preview;
            // Закрыли сразу после перелистывания — берём то, на чём остановились.
            let stopped = if keep && p.preview_pending() { p.selected_path(&self.state) } else { None };
            if let Some(path) = stopped.or_else(|| p.previewed_path()) {
                if keep {
                    // Обои уже на экране: только запоминаем выбор, ничего не перезапуская.
                    let target = self.picker_target();
                    self.apply(&path, target.as_deref(), false, true);
                } else {
                    self.restore_wallpapers();
                }
            }
            self.egl.make_current(None);
            drop(p);
        }
        if let Some(s) = self.picker_surf.take() {
            s.destroy(&self.egl);
        }
        self.picker_output = None;
        self.engine.set_cache_limit(0);
        self.engine.trim();
        self.trim_due = true;
        let stale: Vec<u64> = self.wants.iter().filter(|(_, w)| w.preload).map(|(id, _)| *id).collect();
        for id in stale {
            self.wants.remove(&id);
            self.live.lock().unwrap().remove(&id);
        }
        self.save_soon();
    }

    // ---------- автосмена ----------

    fn rotation_list(&self) -> Vec<String> {
        let src = self.cfg.rotation.source.as_str();
        let items = self.state.items.iter().filter(|i| !i.hidden && !i.broken);
        match src {
            "all" => items.map(|i| i.path.clone()).collect(),
            "favourites" => items.filter(|i| i.fav).map(|i| i.path.clone()).collect(),
            name => self.state.playlists.iter().find(|p| p.name == name).map(|p| p.items.iter().filter(|x| self.state.find(x).is_some()).cloned().collect()).unwrap_or_default(),
        }
    }

    pub fn arm_rotation(&mut self) {
        self.rotation_gen += 1;
        if !self.cfg.rotation.enabled {
            return;
        }
        let gen = self.rotation_gen;
        let every = Duration::from_secs(self.cfg.rotation.interval_min as u64 * 60);
        let _ = self.lh.insert_source(Timer::from_duration(every), move |_, _, app: &mut App| {
            if app.rotation_gen != gen || !app.cfg.rotation.enabled {
                return TimeoutAction::Drop;
            }
            // Пока открыто меню, обои под рукой пользователя не меняем.
            if app.picker.is_none() {
                app.step(1, app.cfg.rotation.shuffle);
            }
            TimeoutAction::ToDuration(Duration::from_secs(app.cfg.rotation.interval_min as u64 * 60))
        });
    }

    /// Следующие (dir = 1) или предыдущие (dir = -1) обои из списка автосмены.
    pub fn step(&mut self, dir: i64, shuffle: bool) -> Option<String> {
        let list = self.rotation_list();
        if list.is_empty() {
            return None;
        }
        let cur = self.state.wallpapers.get("*").cloned().unwrap_or_default();
        let pick = if shuffle {
            let mut seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(7) as usize) ^ self.state.rotation_pos.wrapping_mul(2654435761);
            let mut pick = list[seed % list.len()].clone();
            if pick == cur && list.len() > 1 {
                seed = seed.wrapping_add(1);
                pick = list[seed % list.len()].clone();
            }
            pick
        } else {
            let at = list.iter().position(|p| *p == cur).map(|i| i as i64).unwrap_or(-1);
            list[(at + dir).rem_euclid(list.len() as i64) as usize].clone()
        };
        self.state.rotation_pos = self.state.rotation_pos.wrapping_add(1);
        self.apply(&pick, None, false, true);
        Some(pick)
    }

    // ---------- команды ----------

    fn serve(&mut self, stream: std::os::unix::net::UnixStream) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(300)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(300)));
        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            return;
        }
        let args: Vec<String> = line.trim_end_matches('\n').split('\x1f').map(|s| s.to_string()).collect();
        let reply = self.command(&args);
        let mut stream = stream;
        let _ = stream.write_all(reply.as_bytes());
        let _ = stream.write_all(b"\n");
    }

    fn command(&mut self, args: &[String]) -> String {
        let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let opt = |flag: &str| a.iter().position(|x| *x == flag).and_then(|i| a.get(i + 1)).map(|s| s.to_string());
        match a.first().copied().unwrap_or("") {
            "toggle" => {
                if self.picker_surf.is_some() {
                    self.close_picker();
                } else {
                    self.open_picker();
                }
                "ok".into()
            }
            "show" => {
                self.open_picker();
                "ok".into()
            }
            "hide" => {
                self.close_picker();
                "ok".into()
            }
            "apply" => {
                let Some(path) = a.get(1).filter(|p| !p.starts_with('-')) else { return crate::i18n::tr("error: укажите файл").into() };
                let out = opt("-o");
                if let Some(o) = &out {
                    if !self.outs.iter().any(|x| &x.name == o) {
                        return crate::trf!("error: нет монитора {o}", "error: no such monitor: {o}");
                    }
                }
                let path = self.resolve(path);
                if self.apply(&path, out.as_deref(), a.contains(&"--instant"), true) {
                    format!("ok: {path}")
                } else {
                    crate::trf!("error: не найден файл {path}", "error: file not found: {path}")
                }
            }
            "next" | "prev" | "random" => {
                let dir = if a[0] == "prev" { -1 } else { 1 };
                match self.step(dir, a[0] == "random") {
                    Some(p) => format!("ok: {p}"),
                    None => crate::i18n::tr("error: список обоев пуст").into(),
                }
            }
            "current" => {
                let mut m = serde_json::Map::new();
                for o in &self.outs {
                    m.insert(o.name.clone(), serde_json::Value::String(self.wanted(&o.name).unwrap_or_default()));
                }
                serde_json::Value::Object(m).to_string()
            }
            "list" => {
                let items: Vec<serde_json::Value> = self.state.items.iter().map(|i| serde_json::json!({"path": i.path, "name": i.name, "folder": i.folder, "video": i.video, "width": i.w, "height": i.h, "favourite": i.fav, "hidden": i.hidden, "tags": i.tags})).collect();
                serde_json::Value::Array(items).to_string()
            }
            "pause" => {
                self.engine.pause_videos(true);
                "ok".into()
            }
            "resume" => {
                self.engine.pause_videos(false);
                "ok".into()
            }
            "rescan" => {
                self.rescan();
                "ok".into()
            }
            "reload" => {
                self.cfg = Config::load();
                self.config_changed();
                if let Some(p) = &mut self.picker {
                    p.theme_changed(&self.cfg);
                }
                self.mark_picker();
                "ok".into()
            }
            "status" => {
                let rss = std::fs::read_to_string("/proc/self/statm").ok().and_then(|s| s.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok())).unwrap_or(0) * 4096 / 1024;
                serde_json::json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "outputs": self.outs.iter().map(|o| serde_json::json!({"name": o.name, "size": [o.surf.phys.0, o.surf.phys.1], "scale": o.surf.scale(), "wallpaper": self.wanted(&o.name)})).collect::<Vec<_>>(),
                    "library": self.state.items.len(),
                    "picker": self.picker_surf.is_some(),
                    "video": self.engine.video_info(),
                    "mpv": crate::mpv::available(),
                    "rss_kb": rss,
                })
                .to_string()
            }
            "ui" => {
                let Some(mut p) = self.picker.take() else { return crate::i18n::tr("error: меню закрыто").into() };
                let (reply, actions) = p.remote(&a[1..], &mut self.state, &mut self.cfg);
                self.picker = Some(p);
                self.run_actions(actions);
                self.mark_picker();
                reply
            }
            "settings" => {
                self.open_picker();
                if let Some(mut p) = self.picker.take() {
                    let (_, actions) = p.remote(&["open", "settings"], &mut self.state, &mut self.cfg);
                    self.picker = Some(p);
                    self.run_actions(actions);
                    self.mark_picker();
                }
                "ok".into()
            }
            "set" => {
                let (Some(key), Some(val)) = (a.get(1), a.get(2)) else { return "error: set <key> <value>".into() };
                let on = *val == "true" || *val == "on" || *val == "1";
                match *key {
                    "saver" if ["auto", "on", "off"].contains(val) => self.cfg.video.saver = val.to_string(),
                    "apply_on_close" => self.cfg.picker.apply_on_close = on,
                    "live_preview" => self.cfg.picker.live_preview = on,
                    "mute" => self.cfg.video.mute = on,
                    "transition" => self.cfg.wallpaper.transition = val.to_string(),
                    _ => return format!("error: unknown setting {key}"),
                }
                self.config_changed();
                self.mark_picker();
                "ok".into()
            }
            "quit" => {
                self.exit = true;
                "ok".into()
            }
            other => crate::trf!("error: неизвестная команда «{other}»", "error: unknown command “{other}”"),
        }
    }

    /// Путь к файлу или имя обоев из библиотеки.
    fn resolve(&self, what: &str) -> String {
        let p = config::expand(what);
        if p.is_file() {
            return p.canonicalize().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| what.to_string());
        }
        let low = what.to_lowercase();
        self.state.items.iter().find(|i| i.name.to_lowercase() == low).or_else(|| self.state.items.iter().find(|i| i.name.to_lowercase().contains(&low))).map(|i| i.path.clone()).unwrap_or_else(|| what.to_string())
    }

    // ---------- ввод ----------

    fn key(&mut self, event: KeyEvent, repeat: bool) {
        let Some(mut p) = self.picker.take() else { return };
        let actions = p.key(event.keysym, event.raw_code, event.utf8.clone(), self.mods, repeat, &mut self.state, &mut self.cfg);
        self.picker = Some(p);
        self.run_actions(actions);
        self.mark_picker();
    }

    fn surf_of(&mut self, wl: &wl_surface::WlSurface) -> Option<&mut Surf> {
        if self.picker_surf.as_ref().is_some_and(|s| s.wl() == wl) {
            return self.picker_surf.as_mut();
        }
        self.outs.iter_mut().map(|o| &mut o.surf).find(|s| s.wl() == wl)
    }
}

fn on_battery() -> bool {
    let Ok(rd) = std::fs::read_dir("/sys/class/power_supply") else { return false };
    rd.flatten().any(|e| std::fs::read_to_string(e.path().join("status")).is_ok_and(|s| s.trim() == "Discharging"))
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        let egl = self.egl.clone();
        if let Some(s) = self.surf_of(surface) {
            if s.frac.is_none() && factor > 0 && s.scale120 != factor as u32 * 120 {
                s.scale120 = factor as u32 * 120;
                if s.configured {
                    s.resize(&egl);
                }
            }
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(s) = self.surf_of(surface) {
            s.frame_pending = false;
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, output: &wl_output::WlOutput) {
        if self.picker_surf.as_ref().is_some_and(|s| s.wl() == surface) {
            self.picker_output = self.outs.iter().find(|o| &o.wl == output).map(|o| o.name.clone());
        }
    }
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let name = self.output_state.info(&output).and_then(|i| i.name).unwrap_or_else(|| format!("output-{}", output.id().protocol_id()));
        let wl = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(qh, wl, Layer::Background, Some("omawall"), Some(&output));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        layer.commit();
        let surf = Surf::new(layer, &self.viewporter, &self.frac_mgr, qh);
        self.outs.push(Out { wl: output, name, surf });
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if let Some(i) = self.outs.iter().position(|o| o.wl == output) {
            let o = self.outs.remove(i);
            self.engine.remove_output(&o.name);
            o.surf.destroy(&self.egl);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.picker_surf.as_ref().is_some_and(|s| &s.layer == layer) {
            self.close_picker();
        } else if let Some(i) = self.outs.iter().position(|o| &o.surf.layer == layer) {
            let o = self.outs.remove(i);
            self.engine.remove_output(&o.name);
            o.surf.destroy(&self.egl);
        }
    }
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        let egl = self.egl.clone();
        let wl = layer.wl_surface().clone();
        let Some(s) = self.surf_of(&wl) else { return };
        if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            s.logical = configure.new_size;
        }
        s.configured = true;
        s.resize(&egl);
        if let Some(name) = self.outs.iter().find(|o| o.surf.wl() == &wl).map(|o| o.name.clone()) {
            self.ensure_wallpaper(&name);
        }
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard_with_repeat(qh, &seat, None, self.lh.clone(), Box::new(|app: &mut App, _, event| app.key(event, true))).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
            if let (Some(mgr), Some(p)) = (&self.cursor_mgr, &self.pointer) {
                self.cursor_dev = Some(mgr.get_pointer(p, qh, ()));
            }
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(d) = self.cursor_dev.take() {
                d.destroy();
            }
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        self.key(event, false);
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, modifiers: Modifiers, _: u32) {
        self.mods = modifiers;
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            let on_picker = self.picker_surf.as_ref().is_some_and(|s| s.wl() == &ev.surface);
            if let PointerEventKind::Enter { serial } = ev.kind {
                if let Some(d) = &self.cursor_dev {
                    d.set_shape(serial, CursorShape::Default);
                }
            }
            if on_picker {
                let scale = self.picker_surf.as_ref().map(|s| s.scale()).unwrap_or(1.0);
                let pos = (ev.position.0 as f32 * scale, ev.position.1 as f32 * scale);
                if let Some(p) = &mut self.picker {
                    match ev.kind {
                        PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => p.pointer_move(pos),
                        PointerEventKind::Leave { .. } => p.pointer_move((-1.0, -1.0)),
                        PointerEventKind::Press { button, .. } => p.pointer_button(pos, button, true),
                        PointerEventKind::Release { button, .. } => p.pointer_button(pos, button, false),
                        PointerEventKind::Axis { vertical, horizontal, .. } => {
                            let steps = if vertical.discrete != 0 || horizontal.discrete != 0 { (vertical.discrete + horizontal.discrete) as f32 } else { (vertical.absolute + horizontal.absolute) as f32 / 30.0 };
                            p.pointer_wheel(steps);
                        }
                    }
                }
                self.mark_picker();
            } else if let PointerEventKind::Press { button: 0x110, .. } = ev.kind {
                // Двойной щелчок по обоям открывает меню — как штатный фон Omarchy.
                let now = Instant::now();
                if self.last_click.is_some_and(|t| now - t < Duration::from_millis(400)) {
                    self.last_click = None;
                    self.open_picker();
                } else {
                    self.last_click = Some(now);
                }
            }
        }
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_layer!(App);
delegate_registry!(App);

macro_rules! no_events {
    ($t:ty) => {
        impl Dispatch<$t, ()> for App {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    };
}
no_events!(WpViewporter);
no_events!(WpViewport);
no_events!(WpFractionalScaleManagerV1);
no_events!(WpCursorShapeManagerV1);
no_events!(WpCursorShapeDeviceV1);

impl Dispatch<WpFractionalScaleV1, wl_surface::WlSurface> for App {
    fn event(app: &mut Self, _: &WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, surface: &wl_surface::WlSurface, _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let egl = app.egl.clone();
            if let Some(s) = app.surf_of(surface) {
                if s.scale120 != scale {
                    s.scale120 = scale;
                    if s.configured {
                        s.resize(&egl);
                    }
                }
            }
        }
    }
}
