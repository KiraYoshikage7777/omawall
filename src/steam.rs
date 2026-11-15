//! Мастерская Steam для Wallpaper Engine: поиск и подписка через запущенный клиент Steam.
//! Работает отдельным коротким процессом (`omawall steam …`): пока он жив, Steam считает,
//! что запущен Wallpaper Engine, а сбой библиотеки Steam не заденет демон обоев.
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const APP: u32 = 431960;

type Ugc = *mut c_void;

struct Api {
    _lib: libloading::Library,
    ugc: Ugc,
    pipe: i32,
    run_frame: unsafe extern "C" fn(i32),
    next_callback: unsafe extern "C" fn(i32, *mut CallbackMsg) -> bool,
    free_callback: unsafe extern "C" fn(i32),
    call_result: unsafe extern "C" fn(i32, u64, *mut c_void, i32, i32, *mut bool) -> bool,
    shutdown: unsafe extern "C" fn(),
    create_query: unsafe extern "C" fn(Ugc, i32, i32, u32, u32, u32) -> u64,
    set_search: unsafe extern "C" fn(Ugc, u64, *const c_char) -> bool,
    add_tag: unsafe extern "C" fn(Ugc, u64, *const c_char) -> bool,
    add_excluded: unsafe extern "C" fn(Ugc, u64, *const c_char) -> bool,
    trend_days: unsafe extern "C" fn(Ugc, u64, u32) -> bool,
    send_query: unsafe extern "C" fn(Ugc, u64) -> u64,
    result: unsafe extern "C" fn(Ugc, u64, u32, *mut u8) -> bool,
    preview_url: unsafe extern "C" fn(Ugc, u64, u32, *mut c_char, u32) -> bool,
    num_tags: Option<unsafe extern "C" fn(Ugc, u64, u32) -> u32>,
    tag: Option<unsafe extern "C" fn(Ugc, u64, u32, u32, *mut c_char, u32) -> bool>,
    release_query: unsafe extern "C" fn(Ugc, u64) -> bool,
    subscribe: unsafe extern "C" fn(Ugc, u64) -> u64,
    unsubscribe: unsafe extern "C" fn(Ugc, u64) -> u64,
    download: unsafe extern "C" fn(Ugc, u64, bool) -> bool,
    item_state: unsafe extern "C" fn(Ugc, u64) -> u32,
    download_info: unsafe extern "C" fn(Ugc, u64, *mut u64, *mut u64) -> bool,
    install_info: unsafe extern "C" fn(Ugc, u64, *mut u64, *mut c_char, u32, *mut u32) -> bool,
}

#[repr(C)]
struct CallbackMsg {
    user: i32,
    callback: i32,
    param: *mut u8,
    size: i32,
}

/// Где искать libsteam_api.so: своя копия, пакет skwd, игры из библиотеки Steam.
fn candidates() -> Vec<PathBuf> {
    let home = crate::config::home();
    let mut v = vec![];
    if let Ok(p) = std::env::var("OMAWALL_STEAM_API") {
        v.push(PathBuf::from(p));
    }
    v.push(home.join(".local/share/omawall/libsteam_api.so"));
    v.push(PathBuf::from("/usr/lib/skwd-deck/libsteam_api.so"));
    for root in [home.join(".local/share/Steam/steamapps/common"), home.join(".steam/steam/steamapps/common")] {
        let mut stack = vec![(root, 0)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && depth < 3 {
                    stack.push((p, depth + 1));
                } else if p.file_name().is_some_and(|n| n == "libsteam_api.so") {
                    v.push(p);
                }
            }
        }
    }
    v
}

pub fn library_path() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

fn open() -> Result<Api, String> {
    // Клиент Steam узнаёт приложение по этим переменным.
    std::env::set_var("SteamAppId", APP.to_string());
    std::env::set_var("SteamGameId", APP.to_string());
    let mut last = crate::i18n::tr("не найдена библиотека libsteam_api.so (она есть в любой игре Steam для Linux)").to_string();
    for path in candidates() {
        if !path.is_file() {
            continue;
        }
        // 32-битные копии просто не загрузятся — идём дальше.
        let Ok(lib) = (unsafe { libloading::Library::new(&path) }) else { continue };
        unsafe {
            macro_rules! sym {
                ($n:expr) => {
                    match lib.get($n) {
                        Ok(s) => *s,
                        Err(_) => {
                            last = crate::trf!("в {} нет нужных функций", "{} lacks the needed functions", path.display());
                            continue;
                        }
                    }
                };
            }
            let manual: unsafe extern "C" fn() = sym!(b"SteamAPI_ManualDispatch_Init\0");
            let get_pipe: unsafe extern "C" fn() -> i32 = sym!(b"SteamAPI_GetHSteamPipe\0");
            // В новых библиотеках вход — InitFlat, в старых — Init или InitSafe.
            let ok = if let Ok(f) = lib.get::<unsafe extern "C" fn(*mut c_char) -> i32>(b"SteamAPI_InitFlat\0") {
                let mut err = vec![0u8; 1024];
                f(err.as_mut_ptr() as *mut c_char) == 0
            } else if let Ok(f) = lib.get::<unsafe extern "C" fn() -> bool>(b"SteamAPI_Init\0") {
                f()
            } else if let Ok(f) = lib.get::<unsafe extern "C" fn() -> bool>(b"SteamAPI_InitSafe\0") {
                f()
            } else {
                last = crate::trf!("в {} нет входа в Steam API", "{} has no Steam API entry point", path.display());
                continue;
            };
            if !ok {
                return Err(crate::i18n::tr("Steam не запущен или в библиотеке нет Wallpaper Engine. Запустите Steam и повторите.").into());
            }
            manual();
            // Номер версии интерфейса зависит от возраста библиотеки.
            let mut ugc: Ugc = std::ptr::null_mut();
            for v in (12..=40).rev() {
                let name = format!("SteamAPI_SteamUGC_v{v:03}\0");
                if let Ok(f) = lib.get::<unsafe extern "C" fn() -> Ugc>(name.as_bytes()) {
                    ugc = f();
                    break;
                }
            }
            if ugc.is_null() {
                last = crate::i18n::tr("библиотека Steam не дала доступ к Мастерской").into();
                continue;
            }
            return Ok(Api {
                ugc,
                pipe: get_pipe(),
                run_frame: sym!(b"SteamAPI_ManualDispatch_RunFrame\0"),
                next_callback: sym!(b"SteamAPI_ManualDispatch_GetNextCallback\0"),
                free_callback: sym!(b"SteamAPI_ManualDispatch_FreeLastCallback\0"),
                call_result: sym!(b"SteamAPI_ManualDispatch_GetAPICallResult\0"),
                shutdown: sym!(b"SteamAPI_Shutdown\0"),
                create_query: sym!(b"SteamAPI_ISteamUGC_CreateQueryAllUGCRequestPage\0"),
                set_search: sym!(b"SteamAPI_ISteamUGC_SetSearchText\0"),
                add_tag: sym!(b"SteamAPI_ISteamUGC_AddRequiredTag\0"),
                add_excluded: sym!(b"SteamAPI_ISteamUGC_AddExcludedTag\0"),
                trend_days: sym!(b"SteamAPI_ISteamUGC_SetRankedByTrendDays\0"),
                send_query: sym!(b"SteamAPI_ISteamUGC_SendQueryUGCRequest\0"),
                result: sym!(b"SteamAPI_ISteamUGC_GetQueryUGCResult\0"),
                preview_url: sym!(b"SteamAPI_ISteamUGC_GetQueryUGCPreviewURL\0"),
                num_tags: lib.get(b"SteamAPI_ISteamUGC_GetQueryUGCNumTags\0").ok().map(|s| *s),
                tag: lib.get(b"SteamAPI_ISteamUGC_GetQueryUGCTag\0").ok().map(|s| *s),
                release_query: sym!(b"SteamAPI_ISteamUGC_ReleaseQueryUGCRequest\0"),
                subscribe: sym!(b"SteamAPI_ISteamUGC_SubscribeItem\0"),
                unsubscribe: sym!(b"SteamAPI_ISteamUGC_UnsubscribeItem\0"),
                download: sym!(b"SteamAPI_ISteamUGC_DownloadItem\0"),
                item_state: sym!(b"SteamAPI_ISteamUGC_GetItemState\0"),
                download_info: sym!(b"SteamAPI_ISteamUGC_GetItemDownloadInfo\0"),
                install_info: sym!(b"SteamAPI_ISteamUGC_GetItemInstallInfo\0"),
                _lib: lib,
            });
        }
    }
    Err(last)
}

impl Api {
    /// Прокачать очередь событий Steam; вернуть ответ на вызов `call`, если он пришёл.
    fn pump(&self, call: u64, buf: &mut [u8]) -> Option<bool> {
        let mut done = None;
        unsafe {
            (self.run_frame)(self.pipe);
            let mut msg = CallbackMsg { user: 0, callback: 0, param: std::ptr::null_mut(), size: 0 };
            while (self.next_callback)(self.pipe, &mut msg) {
                if std::env::var_os("OMAWALL_DEBUG").is_some() {
                    eprintln!("steam callback {} size {}", msg.callback, msg.size);
                }
                // 703 — SteamAPICallCompleted_t { u64 вызов; i32 тип ответа; u32 размер }.
                if msg.callback == 703 && !msg.param.is_null() && msg.size >= 16 {
                    let id = std::ptr::read_unaligned(msg.param as *const u64);
                    let kind = std::ptr::read_unaligned(msg.param.add(8) as *const i32);
                    let size = std::ptr::read_unaligned(msg.param.add(12) as *const u32) as usize;
                    if id == call && call != 0 && size <= buf.len() {
                        let mut failed = false;
                        let ok = (self.call_result)(self.pipe, call, buf.as_mut_ptr() as *mut c_void, size as i32, kind, &mut failed);
                        done = Some(ok && !failed);
                    }
                }
                (self.free_callback)(self.pipe);
            }
        }
        done
    }

    fn wait(&self, call: u64, buf: &mut [u8], secs: u64) -> bool {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if let Some(ok) = self.pump(call, buf) {
                return ok;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    }
}

fn cstr(buf: &[u8]) -> String {
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// `omawall steam search <порядок> <тип> <страница> [запрос]` → JSON в stdout.
fn search(args: &[String]) -> Result<String, String> {
    let api = open()?;
    let sort = args.first().map(|s| s.as_str()).unwrap_or("trend");
    let kind = args.get(1).map(|s| s.as_str()).unwrap_or("video");
    let page: u32 = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(1).max(1);
    let query = args.get(3..).map(|q| q.join(" ")).unwrap_or_default();
    // 0 — по оценкам, 1 — по дате, 3 — в тренде, 11 — по тексту, 12 — по числу подписчиков.
    let mode = match (sort, query.trim().is_empty()) {
        ("new", _) => 1,
        ("top", _) => 0,
        ("subscribed", _) => 12,
        (_, false) => 11,
        _ => 3,
    };
    unsafe {
        let q = (api.create_query)(api.ugc, mode, 2, APP, APP, page);
        if q == u64::MAX || q == 0 {
            (api.shutdown)();
            return Err(crate::i18n::tr("Steam не принял запрос").into());
        }
        if !query.trim().is_empty() {
            (api.set_search)(api.ugc, q, CString::new(query.trim().replace('\0', "")).unwrap().as_ptr());
        }
        if mode == 3 {
            (api.trend_days)(api.ugc, q, 30);
        }
        match kind {
            "video" => {
                (api.add_tag)(api.ugc, q, c"Video".as_ptr());
            }
            "scene" => {
                (api.add_tag)(api.ugc, q, c"Scene".as_ptr());
            }
            _ => {}
        }
        // Веб-обои и приложения здесь показать нечем.
        (api.add_excluded)(api.ugc, q, c"Web".as_ptr());
        (api.add_excluded)(api.ugc, q, c"Application".as_ptr());
        (api.add_excluded)(api.ugc, q, c"Mature".as_ptr());
        (api.add_excluded)(api.ugc, q, c"Questionable".as_ptr());
        let call = (api.send_query)(api.ugc, q);
        // SteamUGCQueryCompleted_t { u64 запрос; i32 итог; u32 получено; u32 всего; … }.
        let mut reply = vec![0u8; 4096];
        if !api.wait(call, &mut reply, 20) {
            (api.shutdown)();
            return Err(crate::i18n::tr("Steam не ответил на запрос. Проверьте, что он в сети.").into());
        }
        let count = std::ptr::read_unaligned(reply.as_ptr().add(12) as *const u32).min(50);
        let total = std::ptr::read_unaligned(reply.as_ptr().add(16) as *const u32);
        let mut items = vec![];
        // SteamUGCDetails_t: u64 id; i32 итог; … название с 24-го байта (129 байт).
        let mut details = vec![0u8; 32768];
        for i in 0..count {
            details.fill(0);
            if !(api.result)(api.ugc, q, i, details.as_mut_ptr()) {
                continue;
            }
            let id = std::ptr::read_unaligned(details.as_ptr() as *const u64);
            let status = std::ptr::read_unaligned(details.as_ptr().add(8) as *const i32);
            if id == 0 || status != 1 {
                continue;
            }
            let title = cstr(&details[24..24 + 129]);
            let mut url = vec![0u8; 1024];
            (api.preview_url)(api.ugc, q, i, url.as_mut_ptr() as *mut c_char, 1024);
            let mut tags = vec![];
            if let (Some(num), Some(tag)) = (api.num_tags, api.tag) {
                for t in 0..num(api.ugc, q, i).min(24) {
                    let mut buf = vec![0u8; 256];
                    if tag(api.ugc, q, i, t, buf.as_mut_ptr() as *mut c_char, 256) {
                        tags.push(cstr(&buf));
                    }
                }
            }
            let state = (api.item_state)(api.ugc, id);
            items.push(serde_json::json!({"id": id.to_string(), "title": title, "preview": cstr(&url), "tags": tags, "installed": state & 4 != 0}));
        }
        (api.release_query)(api.ugc, q);
        (api.shutdown)();
        Ok(serde_json::json!({"total": total, "items": items}).to_string())
    }
}

/// `omawall steam get <id>`: подписаться и дождаться загрузки. Ход печатается построчно.
fn get(id: u64) -> Result<String, String> {
    let api = open()?;
    let out = std::io::stdout();
    unsafe {
        (api.subscribe)(api.ugc, id);
        let mut scratch = vec![0u8; 4096];
        let started = Instant::now();
        let mut asked = false;
        let mut last_print = Instant::now() - Duration::from_secs(1);
        loop {
            api.pump(0, &mut scratch);
            let state = (api.item_state)(api.ugc, id);
            let (installed, needs_update, busy) = (state & 4 != 0, state & 8 != 0, state & (16 | 32) != 0);
            if installed && !needs_update && !busy {
                break;
            }
            if !asked && !busy && started.elapsed() > Duration::from_millis(600) {
                (api.download)(api.ugc, id, true);
                asked = true;
            }
            let (mut have, mut total) = (0u64, 0u64);
            if (api.download_info)(api.ugc, id, &mut have, &mut total) && total > 0 && last_print.elapsed() > Duration::from_millis(250) {
                last_print = Instant::now();
                let _ = writeln!(out.lock(), "progress {have} {total}");
                let _ = out.lock().flush();
            }
            if started.elapsed() > Duration::from_secs(1800) {
                (api.shutdown)();
                return Err(crate::i18n::tr("загрузка не закончилась за полчаса").into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut folder = vec![0u8; 4096];
        let (mut size, mut stamp) = (0u64, 0u32);
        (api.install_info)(api.ugc, id, &mut size, folder.as_mut_ptr() as *mut c_char, 4096, &mut stamp);
        (api.shutdown)();
        let folder = CStr::from_bytes_until_nul(&folder).map(|c| c.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(format!("done {folder}"))
    }
}

fn drop_item(id: u64) -> Result<String, String> {
    let api = open()?;
    unsafe {
        let call = (api.unsubscribe)(api.ugc, id);
        let mut buf = vec![0u8; 4096];
        api.wait(call, &mut buf, 10);
        (api.shutdown)();
    }
    Ok("done".into())
}

/// Точка входа `omawall steam …`.
pub fn main(args: &[String]) -> i32 {
    let res = match args.first().map(|s| s.as_str()) {
        Some("search") => search(&args[1..]),
        Some("get") => args.get(1).and_then(|v| v.parse().ok()).ok_or(crate::i18n::tr("нужен номер обоев").to_string()).and_then(get),
        Some("drop") => args.get(1).and_then(|v| v.parse().ok()).ok_or(crate::i18n::tr("нужен номер обоев").to_string()).and_then(drop_item),
        Some("lib") => library_path().map(|p| p.display().to_string()).ok_or(crate::i18n::tr("не найдена").to_string()),
        _ => Err(crate::i18n::tr("использование: omawall steam search <trend|new|top|subscribed> <video|scene|all> <страница> [запрос] | get <id> | drop <id>").into()),
    };
    match res {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            println!("error {e}");
            1
        }
    }
}
