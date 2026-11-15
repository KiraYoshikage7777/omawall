//! Видео через libmpv (render API поверх нашего контекста GL). Библиотека
//! подгружается только при первом видео: без mpv демон продолжает показывать картинки.
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::OnceLock;

#[repr(C)]
struct RenderParam {
    kind: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct GlInit {
    get_proc: GetProc,
    ctx: *mut c_void,
}
#[repr(C)]
struct GlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    format: c_int,
}
#[repr(C)]
struct Event {
    id: c_int,
    error: c_int,
    userdata: u64,
    data: *mut c_void,
}

type Cb = extern "C" fn(*mut c_void);
pub type GetProc = extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;

struct Api {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> *mut c_void,
    initialize: unsafe extern "C" fn(*mut c_void) -> c_int,
    set_option_string: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    set_property_string: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char) -> c_int,
    get_property_string: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    free: unsafe extern "C" fn(*mut c_void),
    command: unsafe extern "C" fn(*mut c_void, *const *const c_char) -> c_int,
    terminate_destroy: unsafe extern "C" fn(*mut c_void),
    wait_event: unsafe extern "C" fn(*mut c_void, f64) -> *mut Event,
    render_create: unsafe extern "C" fn(*mut *mut c_void, *mut c_void, *mut RenderParam) -> c_int,
    render_set_update_callback: unsafe extern "C" fn(*mut c_void, Option<Cb>, *mut c_void),
    render_update: unsafe extern "C" fn(*mut c_void) -> u64,
    render_render: unsafe extern "C" fn(*mut c_void, *mut RenderParam) -> c_int,
    render_free: unsafe extern "C" fn(*mut c_void),
}

unsafe impl Sync for Api {}
unsafe impl Send for Api {}

static API: OnceLock<Option<Api>> = OnceLock::new();

fn api() -> Option<&'static Api> {
    API.get_or_init(|| unsafe {
        let lib = ["libmpv.so.2", "libmpv.so"].iter().find_map(|n| libloading::Library::new(n).ok())?;
        macro_rules! sym {
            ($n:literal) => {
                *lib.get($n).ok()?
            };
        }
        Some(Api {
            create: sym!(b"mpv_create\0"),
            initialize: sym!(b"mpv_initialize\0"),
            set_option_string: sym!(b"mpv_set_option_string\0"),
            set_property_string: sym!(b"mpv_set_property_string\0"),
            get_property_string: sym!(b"mpv_get_property_string\0"),
            free: sym!(b"mpv_free\0"),
            command: sym!(b"mpv_command\0"),
            terminate_destroy: sym!(b"mpv_terminate_destroy\0"),
            wait_event: sym!(b"mpv_wait_event\0"),
            render_create: sym!(b"mpv_render_context_create\0"),
            render_set_update_callback: sym!(b"mpv_render_context_set_update_callback\0"),
            render_update: sym!(b"mpv_render_context_update\0"),
            render_render: sym!(b"mpv_render_context_render\0"),
            render_free: sym!(b"mpv_render_context_free\0"),
            _lib: lib,
        })
    })
    .as_ref()
}

pub fn available() -> bool {
    api().is_some()
}

extern "C" fn on_update(data: *mut c_void) {
    // Вызывается из потока mpv: только будим главный цикл.
    let wake = unsafe { &*(data as *const Wake) };
    (wake.0)();
}

pub struct Wake(pub Box<dyn Fn() + Send + Sync>);

pub struct Player {
    api: &'static Api,
    mpv: *mut c_void,
    render: *mut c_void,
    _wake: Box<Wake>,
    pub paused: bool,
}

#[derive(Clone)]
pub struct VideoOpts {
    pub mute: bool,
    pub volume: u32,
    pub hwdec: bool,
}

fn cs(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

impl Player {
    /// Контекст GL должен быть текущим. `wake` будит главный цикл, когда готов новый кадр.
    pub fn new(get_proc: GetProc, proc_ctx: *mut c_void, wl_display: *mut c_void, opts: &VideoOpts, wake: Box<dyn Fn() + Send + Sync>) -> Option<Player> {
        let api = api()?;
        unsafe {
            let mpv = (api.create)();
            if mpv.is_null() {
                return None;
            }
            let set = |k: &str, v: &str| {
                (api.set_option_string)(mpv, cs(k).as_ptr(), cs(v).as_ptr());
            };
            set("config", "no");
            set("terminal", "no");
            set("vo", "libmpv");
            set("profile", "fast");
            set("hwdec", if opts.hwdec { "nvdec,vaapi,auto-safe" } else { "no" });
            set("loop-file", "inf");
            set("keep-open", "yes");
            set("idle", "yes");
            set("input-default-bindings", "no");
            set("input-vo-keyboard", "no");
            set("osc", "no");
            set("osd-level", "0");
            set("sub", "no");
            set("load-scripts", "no");
            set("ytdl", "no");
            set("audio-display", "no");
            set("image-display-duration", "inf");
            set("video-timing-offset", "0");
            set("cache", "no");
            set("demuxer-max-bytes", "32MiB");
            set("demuxer-max-back-bytes", "0");
            set("demuxer-readahead-secs", "2");
            set("vd-lavc-dr", "yes");
            set("audio-client-name", "omawall");
            if opts.mute {
                // Без звуковой дорожки mpv не декодирует звук и не открывает устройство.
                set("aid", "no");
            } else {
                set("volume", &opts.volume.to_string());
            }
            let t0 = std::time::Instant::now();
            let r_init = (api.initialize)(mpv);
            crate::app::slow("mpv_initialize", t0);
            if r_init < 0 {
                (api.terminate_destroy)(mpv);
                return None;
            }
            let mut init = GlInit { get_proc, ctx: proc_ctx };
            let api_type = cs("opengl");
            let mut params = [
                RenderParam { kind: 1, data: api_type.as_ptr() as *mut c_void },
                RenderParam { kind: 2, data: &mut init as *mut _ as *mut c_void },
                RenderParam { kind: 9, data: wl_display },
                RenderParam { kind: 0, data: std::ptr::null_mut() },
            ];
            let mut render = std::ptr::null_mut();
            let t0 = std::time::Instant::now();
            let r_rc = (api.render_create)(&mut render, mpv, params.as_mut_ptr());
            crate::app::slow("mpv_render_context_create", t0);
            if r_rc < 0 || render.is_null() {
                (api.terminate_destroy)(mpv);
                return None;
            }
            let wake = Box::new(Wake(wake));
            (api.render_set_update_callback)(render, Some(on_update), &*wake as *const Wake as *mut c_void);
            Some(Player { api, mpv, render, _wake: wake, paused: false })
        }
    }

    pub fn command(&self, args: &[&str]) {
        let owned: Vec<CString> = args.iter().map(|a| cs(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        unsafe { (self.api.command)(self.mpv, ptrs.as_ptr()) };
    }

    pub fn set(&self, key: &str, value: &str) {
        unsafe { (self.api.set_property_string)(self.mpv, cs(key).as_ptr(), cs(value).as_ptr()) };
    }

    pub fn get(&self, key: &str) -> Option<String> {
        unsafe {
            let p = (self.api.get_property_string)(self.mpv, cs(key).as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            (self.api.free)(p as *mut c_void);
            Some(s)
        }
    }

    /// Открыть файл в этом проигрывателе (прежний при этом закрывается).
    pub fn load(&mut self, path: &str, opts: &VideoOpts) {
        self.apply_opts(opts);
        self.paused = false;
        self.set("pause", "no");
        self.command(&["loadfile", path, "replace"]);
    }

    pub fn apply_opts(&self, opts: &VideoOpts) {
        self.set("hwdec", if opts.hwdec { "nvdec,vaapi,auto-safe" } else { "no" });
        if opts.mute {
            self.set("aid", "no");
        } else {
            self.set("aid", "auto");
            self.set("volume", &opts.volume.to_string());
        }
    }

    pub fn stop(&mut self) {
        self.command(&["stop"]);
    }

    pub fn set_paused(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            self.set("pause", if paused { "yes" } else { "no" });
        }
    }

    /// Размер кадра, когда он уже известен.
    pub fn size(&self) -> Option<(u32, u32)> {
        let w: u32 = self.get("dwidth")?.parse().ok()?;
        let h: u32 = self.get("dheight")?.parse().ok()?;
        (w > 0 && h > 0).then_some((w, h))
    }

    /// Прочитать накопившиеся события mpv; true — файл не открылся.
    pub fn drain_events(&self) -> bool {
        let mut failed = false;
        unsafe {
            loop {
                let ev = (self.api.wait_event)(self.mpv, 0.0);
                if ev.is_null() || (*ev).id == 0 {
                    break;
                }
                // END_FILE с ошибкой: data → { int reason; int error; ... }, reason 4 = ошибка.
                if (*ev).id == 7 && !(*ev).data.is_null() && *((*ev).data as *const c_int) == 4 {
                    failed = true;
                }
            }
        }
        failed
    }

    /// Есть ли новый кадр для отрисовки.
    pub fn has_frame(&self) -> bool {
        unsafe { (self.api.render_update)(self.render) & 1 != 0 }
    }

    /// Нарисовать текущий кадр в FBO (верх кадра — в v = 0 текстуры).
    pub fn render(&self, fbo: i32, w: u32, h: u32) {
        let mut target = GlFbo { fbo, w: w as c_int, h: h as c_int, format: 0 };
        let mut flip: c_int = 0;
        let mut block: c_int = 0;
        let mut params = [
            RenderParam { kind: 3, data: &mut target as *mut _ as *mut c_void },
            RenderParam { kind: 4, data: &mut flip as *mut _ as *mut c_void },
            RenderParam { kind: 12, data: &mut block as *mut _ as *mut c_void },
            RenderParam { kind: 0, data: std::ptr::null_mut() },
        ];
        unsafe { (self.api.render_render)(self.render, params.as_mut_ptr()) };
    }
}

impl Player {
    /// Освободить всё, что связано с GL. Вызывать в потоке с текущим контекстом.
    pub fn release_gl(&mut self) {
        if !self.render.is_null() {
            unsafe {
                (self.api.render_set_update_callback)(self.render, None, std::ptr::null_mut());
                (self.api.render_free)(self.render);
            }
            self.render = std::ptr::null_mut();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.release_gl();
        unsafe { (self.api.terminate_destroy)(self.mpv) };
    }
}
