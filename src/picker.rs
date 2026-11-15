//! Меню выбора обоев: лента срезов, панель фильтров, оборот карточки, поиск.
use crate::config::Config;
use crate::draw::{alpha, hex, hsv, mix, Color, Draw, Rect, Shape, BOLD, ICON, MEDIUM};
use crate::gfx::{Gl, Tex};
use crate::icons;
use crate::jobs::{Done, Pool};
use crate::library::{self, State, SWATCH_HUES};
use crate::panels::{self, Overlay, PanelCtx};
use crate::ui::{id, Key, Th, Ui, W};
use crate::wallpaper::Src;
use smithay_client_toolkit::seat::keyboard::{Keysym, Modifiers};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub enum Action {
    /// Показать обои на столе, не запоминая выбор.
    Preview(String),
    /// Поставить обои и запомнить.
    Commit(String),
    /// Вернуть на стол запомненные обои.
    Restore,
    Preload(String),
    Close,
    SaveState,
    SaveConfig,
    Rescan,
    FxChanged(String),
    Delete(String),
    Rotation,
    Redraw,
}

pub struct Env<'a> {
    pub d: &'a mut Draw,
    pub gl: &'a Gl,
    pub state: &'a mut State,
    pub cfg: &'a mut Config,
    pub pool: &'a Pool,
    pub size: (f32, f32),
    pub surf_scale: f32,
    /// Источник выбранных обоев, если он уже загружен движком (для живой картинки).
    pub live: Option<Src>,
    pub actions: Vec<Action>,
}

struct Slice {
    k: i64,
    rect: Rect,
    e: f32,
    a: f32,
}

pub struct Picker {
    pub ui: Ui,
    th: Th,
    s: f32,
    view: Vec<usize>,
    sel: i64,
    pos: f64,
    thumbs: HashMap<String, Tex>,
    thumb_req: HashSet<String>,
    query: String,
    tags: Vec<String>,
    playlist: Option<String>,
    search_open: bool,
    overlay: Overlay,
    folder_menu: bool,
    flipped: bool,
    flip: f32,
    stashed: bool,
    opened: Instant,
    last: Instant,
    committed: Option<String>,
    previewed: Option<String>,
    preview_due: Option<Instant>,
    nav_dir: i64,
    pending: Vec<Action>,
    delete_confirm: bool,
    tag_input: String,
    playlist_menu: bool,
    cfg_dirty: bool,
    toast: Option<(String, Instant)>,
    tip_since: Option<(Rect, Instant)>,
    drawn: Vec<Slice>,
    why: String,
    frames: u64,
    card: Rect,
    card_skew: f32,
    settings: panels::SettingsSt,
    theme: panels::ThemeSt,
    effects: panels::EffectsSt,
    playlists: panels::PlaylistsSt,
    downloads: panels::DownloadsSt,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn human_size(b: u64) -> String {
    if b >= 1 << 30 {
        crate::trf!("{:.1} ГБ", "{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        crate::trf!("{:.1} МБ", "{:.1} MB", b as f64 / (1u64 << 20) as f64)
    } else {
        crate::trf!("{} КБ", "{} KB", (b / 1024).max(1))
    }
}

pub fn date(ts: i64) -> String {
    // Гражданская дата из unix-времени (алгоритм Говарда Хиннанта), по местному смещению.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = ts as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

fn times(n: u32) -> String {
    if crate::i18n::en() {
        return if n == 1 { "1 time".into() } else { format!("{n} times") };
    }
    let (d, dd) = (n % 10, n % 100);
    let word = if (2..=4).contains(&d) && !(12..=14).contains(&dd) { crate::i18n::tr("раза") } else { crate::i18n::tr("раз") };
    format!("{n} {word}")
}

impl Picker {
    pub fn new(state: &State, cfg: &Config, current: Option<String>) -> Picker {
        let now = Instant::now();
        let mut p = Picker {
            ui: Ui { mouse: (-1.0, -1.0), ..Default::default() },
            th: Th::from(cfg),
            s: 1.0,
            view: vec![],
            sel: 0,
            pos: 0.0,
            thumbs: HashMap::new(),
            thumb_req: HashSet::new(),
            query: String::new(),
            tags: vec![],
            playlist: None,
            search_open: false,
            overlay: Overlay::None,
            folder_menu: false,
            flipped: false,
            flip: 0.0,
            stashed: false,
            opened: now,
            last: now,
            committed: current.clone(),
            previewed: None,
            preview_due: None,
            nav_dir: 1,
            pending: vec![],
            delete_confirm: false,
            tag_input: String::new(),
            playlist_menu: false,
            cfg_dirty: false,
            toast: None,
            tip_since: None,
            drawn: vec![],
            why: String::new(),
            frames: 0,
            card: Rect::default(),
            card_skew: 0.0,
            settings: Default::default(),
            theme: Default::default(),
            effects: Default::default(),
            playlists: Default::default(),
            downloads: Default::default(),
        };
        p.view = library::view(state, "", &[], None);
        // Открываемся на обоях, которые сейчас стоят.
        if let Some(cur) = current {
            if let Some(i) = p.view.iter().position(|&i| state.items[i].path == cur) {
                p.sel = i as i64;
                p.pos = i as f64;
            }
        }
        p
    }

    // ---------- состояние ----------

    fn n(&self) -> i64 {
        self.view.len() as i64
    }
    fn wraps(&self, cfg: &Config) -> bool {
        cfg.picker.wrap && self.n() >= cfg.picker.visible as i64 + 3
    }
    fn item_at(&self, k: i64, cfg: &Config) -> Option<usize> {
        let n = self.n();
        if n == 0 {
            return None;
        }
        if self.wraps(cfg) {
            Some(self.view[k.rem_euclid(n) as usize])
        } else if k >= 0 && k < n {
            Some(self.view[k as usize])
        } else {
            None
        }
    }
    fn sel_item(&self) -> Option<usize> {
        let n = self.n();
        (n > 0).then(|| self.view[self.sel.rem_euclid(n) as usize])
    }
    pub fn selected_path(&self, state: &State) -> Option<String> {
        self.sel_item().and_then(|i| state.items.get(i)).map(|i| i.path.clone())
    }
    /// Обои, показанные на столе для предпросмотра и ещё не закреплённые.
    pub fn previewed_path(&self) -> Option<String> {
        self.previewed.clone().filter(|p| Some(p) != self.committed.as_ref())
    }
    pub fn preview_pending(&self) -> bool {
        self.preview_due.is_some()
    }
    pub fn previewing(&self) -> bool {
        self.previewed.is_some() && self.previewed != self.committed
    }
    pub fn theme_changed(&mut self, cfg: &Config) {
        self.th = Th::from(cfg);
    }
    pub fn external_apply(&mut self, path: &str) {
        self.committed = Some(path.to_string());
        self.previewed = None;
    }
    pub fn library_changed(&mut self, state: &State) {
        self.refresh(state);
    }
    pub fn item_ready(&mut self, state: &State, path: &str) {
        self.thumb_req.remove(path);
        // Разбор мог изменить порядок (сортировка по цвету и т. п.).
        if state.view.sort != "date" && state.view.sort != "recent" && state.view.sort != "pop" {
            self.refresh(state);
        }
    }

    /// Пересобрать ленту, сохранив выбранные обои на месте.
    fn refresh(&mut self, state: &State) {
        let keep = self.sel_item().and_then(|i| state.items.get(i)).map(|i| i.path.clone());
        let old_sel = self.sel;
        self.view = library::view(state, &self.query, &self.tags, self.playlist.as_deref());
        let n = self.n();
        let found = keep.and_then(|p| self.view.iter().position(|&i| state.items[i].path == p));
        self.sel = match found {
            Some(i) => i as i64,
            None => old_sel.clamp(0, (n - 1).max(0)),
        };
        self.pos = self.sel as f64;
        if found.is_none() {
            self.touch_preview();
        }
    }

    fn touch_preview(&mut self) {
        self.preview_due = Some(Instant::now() + Duration::from_millis(80));
        self.flipped = false;
        self.delete_confirm = false;
        self.playlist_menu = false;
    }

    fn nav(&mut self, delta: i64, cfg: &Config) {
        let n = self.n();
        if n == 0 || delta == 0 {
            return;
        }
        let wraps = self.wraps(cfg);
        let mut next = self.sel + delta;
        if !wraps {
            if cfg.picker.wrap && delta.abs() == 1 {
                // Короткий список: с края перескакиваем на другой край.
                next = next.rem_euclid(n);
            } else {
                next = next.clamp(0, n - 1);
            }
        }
        if next != self.sel {
            self.sel = next;
            self.nav_dir = delta.signum();
            if !cfg.picker.animations {
                self.pos = self.sel as f64;
            }
            self.touch_preview();
        }
    }

    fn select_item(&mut self, path: &str, state: &State, cfg: &Config) -> bool {
        let Some(i) = self.view.iter().position(|&i| state.items[i].path == path) else { return false };
        let n = self.n();
        // Ближайшее вхождение при закольцованной ленте.
        let mut target = i as i64;
        if self.wraps(cfg) {
            let base = self.sel - self.sel.rem_euclid(n);
            target = [base + i as i64 - n, base + i as i64, base + i as i64 + n].into_iter().min_by_key(|t| (t - self.sel).abs()).unwrap();
        }
        self.nav(target - self.sel, cfg);
        true
    }

    fn commit(&mut self, state: &State, cfg: &Config, out: &mut Vec<Action>) {
        if let Some(path) = self.selected_path(state) {
            self.committed = Some(path.clone());
            self.previewed = None;
            self.preview_due = None;
            out.push(Action::Commit(path));
            if cfg.picker.close_on_apply {
                out.push(Action::Close);
            }
        }
    }

    fn toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    fn open_overlay(&mut self, o: Overlay) {
        self.overlay = o;
        self.folder_menu = false;
        self.ui.focus = 0;
    }

    fn set_filter(&mut self, state: &mut State, out: &mut Vec<Action>) {
        self.refresh(state);
        out.push(Action::SaveState);
    }

    // ---------- ввод ----------

    pub fn pointer_move(&mut self, pos: (f32, f32)) {
        self.ui.mouse = pos;
    }
    pub fn pointer_button(&mut self, pos: (f32, f32), button: u32, pressed: bool) {
        self.ui.mouse = pos;
        match (button, pressed) {
            (0x110, true) => {
                self.ui.press = true;
                self.ui.down = true;
            }
            (0x110, false) => {
                self.ui.release = true;
                self.ui.down = false;
            }
            (0x111, true) => self.ui.rpress = true,
            _ => {}
        }
    }
    pub fn pointer_wheel(&mut self, steps: f32) {
        self.ui.wheel += steps;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn key(&mut self, sym: Keysym, raw: u32, utf8: Option<String>, mods: Modifiers, _repeat: bool, state: &mut State, cfg: &mut Config) -> Vec<Action> {
        let mut out = vec![];
        let sym = sym.raw();
        if self.stashed {
            self.stashed = false;
            return out;
        }
        if sym == 0xff1b {
            if self.ui.focus != 0 {
                self.ui.focus = 0;
                if self.search_open && self.overlay == Overlay::None && self.query.is_empty() && self.tags.is_empty() {
                    self.search_open = false;
                }
            } else if self.folder_menu || self.playlist_menu {
                self.folder_menu = false;
                self.playlist_menu = false;
            } else if self.overlay != Overlay::None {
                if !panels::back(self) {
                    self.overlay = Overlay::None;
                }
            } else if self.flipped {
                self.flipped = false;
            } else if self.search_open {
                self.search_open = false;
                if !self.query.is_empty() || !self.tags.is_empty() {
                    self.query.clear();
                    self.tags.clear();
                    self.refresh(state);
                }
            } else {
                out.push(Action::Close);
            }
            return out;
        }
        if self.ui.focus != 0 || self.overlay != Overlay::None {
            self.ui.keys.push(Key { sym, raw, text: utf8, ctrl: mods.ctrl, shift: mods.shift });
            return out;
        }
        let step = if mods.shift { 5 } else { 1 };
        match sym {
            0xff51 => self.nav(-step, cfg),
            0xff53 => self.nav(step, cfg),
            0xff50 => self.nav(-self.sel.rem_euclid(self.n().max(1)), cfg),
            0xff57 => self.nav(self.n() - 1 - self.sel.rem_euclid(self.n().max(1)), cfg),
            0xff55 => self.nav(-(cfg.picker.visible as i64 / 2).max(2), cfg),
            0xff56 => self.nav((cfg.picker.visible as i64 / 2).max(2), cfg),
            0xff52 => self.flipped = true,
            0xff54 => self.flipped = false,
            0xff0d | 0xff8d => self.commit(state, cfg, &mut out),
            0xff09 => {
                let kinds = ["", "static", "video"];
                let at = kinds.iter().position(|k| *k == state.view.kind).unwrap_or(0);
                state.view.kind = kinds[(at + if mods.shift { 2 } else { 1 }) % 3].into();
                self.set_filter(state, &mut out);
            }
            0xffff => {
                self.flipped = true;
                self.delete_confirm = true;
            }
            _ => match raw {
                57 => self.flipped = !self.flipped,
                33 if mods.ctrl => self.open_search(),
                53 | 20 => self.open_search(),
                33 => self.toggle_fav(state, &mut out),
                35 => self.toggle_hidden(state, &mut out),
                19 => self.random(state, cfg, &mut out),
                18 => self.open_overlay(Overlay::Effects),
                25 => self.open_overlay(Overlay::Playlists),
                32 => self.open_overlay(Overlay::Downloads),
                31 => self.open_overlay(Overlay::Settings),
                46 if mods.ctrl => self.copy_path(state),
                46 => self.open_overlay(Overlay::Theme),
                48 => {
                    state.view.bar_hidden = !state.view.bar_hidden;
                    out.push(Action::SaveState);
                }
                47 => self.stashed = true,
                24 => {
                    state.view.orient = match state.view.orient.as_str() {
                        "" => "wide".into(),
                        "wide" => "tall".into(),
                        _ => String::new(),
                    };
                    self.set_filter(state, &mut out);
                }
                2..=4 => {
                    state.view.kind = ["", "static", "video"][raw as usize - 2].into();
                    self.set_filter(state, &mut out);
                }
                _ => {}
            },
        }
        out
    }

    fn open_search(&mut self) {
        self.search_open = true;
        self.ui.focus = id("search");
    }

    fn toggle_fav(&mut self, state: &mut State, out: &mut Vec<Action>) {
        if let Some(i) = self.sel_item() {
            state.items[i].fav = !state.items[i].fav;
            let on = state.items[i].fav;
            self.toast(if on { crate::i18n::tr("Добавлено в избранное") } else { crate::i18n::tr("Убрано из избранного") });
            out.push(Action::SaveState);
            if state.view.favourites {
                self.refresh(state);
            }
        }
    }

    fn toggle_hidden(&mut self, state: &mut State, out: &mut Vec<Action>) {
        if let Some(i) = self.sel_item() {
            state.items[i].hidden = !state.items[i].hidden;
            let on = state.items[i].hidden;
            self.toast(if on { crate::i18n::tr("Скрыто из ленты") } else { crate::i18n::tr("Возвращено в ленту") });
            out.push(Action::SaveState);
            self.refresh(state);
        }
    }

    fn random(&mut self, state: &State, cfg: &Config, out: &mut Vec<Action>) {
        let n = self.n();
        if n < 2 {
            return;
        }
        let seed = Instant::now().duration_since(self.opened).as_nanos() as i64 ^ (self.sel * 7919);
        let mut jump = seed.rem_euclid(n - 1) + 1;
        if jump > n / 2 {
            jump -= n;
        }
        self.nav(jump, cfg);
        self.commit(state, cfg, out);
    }

    fn copy_path(&mut self, state: &State) {
        if let Some(i) = self.sel_item() {
            copy_text(&state.items[i].file);
            self.toast(crate::i18n::tr("Путь скопирован"));
        }
    }

    /// Команды для проверки и скриптов: `omawall ui …`.
    pub fn remote(&mut self, a: &[&str], state: &mut State, cfg: &mut Config) -> (String, Vec<Action>) {
        let mut out = vec![];
        let reply = match a.first().copied().unwrap_or("state") {
            "state" => serde_json::json!({
                "count": self.view.len(),
                "selection": self.selected_path(state),
                "committed": self.committed,
                "previewing": self.previewing(),
                "overlay": format!("{:?}", self.overlay),
                "flipped": self.flipped,
                "search": self.search_open,
                "query": self.query,
                "thumbs": self.thumbs.len(),
                "debug": self.why,
            })
            .to_string(),
            "nav" => {
                let d = match a.get(1).copied() {
                    Some("left") => -1,
                    Some("right") => 1,
                    Some(n) => n.parse().unwrap_or(0),
                    None => 0,
                };
                self.nav(d, cfg);
                "ok".into()
            }
            "select" => {
                if a.get(1).is_some_and(|p| self.select_item(p, state, cfg)) {
                    "ok".into()
                } else {
                    crate::i18n::tr("error: нет таких обоев в ленте").into()
                }
            }
            "apply" => {
                self.commit(state, cfg, &mut out);
                "ok".into()
            }
            "flip" => {
                self.flipped = !self.flipped;
                "ok".into()
            }
            "open" => {
                let o = match a.get(1).copied().unwrap_or("") {
                    "settings" => Overlay::Settings,
                    "theme" => Overlay::Theme,
                    "effects" => Overlay::Effects,
                    "playlists" => Overlay::Playlists,
                    "downloads" => Overlay::Downloads,
                    "search" => {
                        self.open_search();
                        Overlay::None
                    }
                    _ => Overlay::None,
                };
                self.open_overlay(o);
                if let Some(n) = a.get(2).and_then(|v| v.parse::<usize>().ok()) {
                    self.settings.section = n;
                    self.effects.selected = n;
                    self.theme.role = n.min(8);
                }
                "ok".into()
            }
            "query" => {
                self.query = a[1..].join(" ");
                self.search_open = true;
                self.refresh(state);
                "ok".into()
            }
            "click" => {
                let x = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let y = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                self.ui.mouse = (x, y);
                self.ui.press = true;
                "ok".into()
            }
            "move" => {
                let x = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                let y = a.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                self.ui.mouse = (x, y);
                "ok".into()
            }
            "wheel" => {
                self.ui.wheel += a.get(1).and_then(|v| v.parse().ok()).unwrap_or(1.0);
                "ok".into()
            }
            "type" => {
                for ch in a[1..].join(" ").chars() {
                    self.ui.keys.push(Key { sym: ch as u32, raw: 0, text: Some(ch.to_string()), ctrl: false, shift: false });
                }
                "ok".into()
            }
            "key" => {
                let raw = a.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                let sym = a.get(2).and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()).unwrap_or(0);
                out = self.key(Keysym::new(sym), raw, None, Modifiers::default(), false, state, cfg);
                "ok".into()
            }
            other => crate::trf!("error: неизвестная команда ui «{other}»", "error: unknown ui command “{other}”"),
        };
        (reply, out)
    }

    /// Результаты фоновых задач, адресованные меню.
    pub fn done(&mut self, d: Done, gl: &Gl, cfg: &Config) -> Vec<Action> {
        let mut out = vec![];
        match d {
            Done::Thumb(path, img) => {
                if let Some(img) = img {
                    self.thumbs.insert(path, Tex::rgba(gl, img.w, img.h, Some(&img.data), true));
                }
            }
            other => panels::done(self, other, gl, cfg, &mut out),
        }
        out
    }

    fn want_thumb(&mut self, state: &State, pool: &Pool, i: usize, urgent: bool) {
        let it = &state.items[i];
        if !it.analyzed || self.thumbs.contains_key(&it.path) || self.thumb_req.contains(&it.path) {
            return;
        }
        self.thumb_req.insert(it.path.clone());
        let (path, file) = (it.path.clone(), it.thumb_path());
        pool.run(urgent, move || Some(Done::Thumb(path, library::load_thumb(&file))));
    }

    // ---------- кадр ----------

    pub fn frame(&mut self, e: &mut Env) -> bool {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.s = e.cfg.picker.scale * (e.size.0 / 1920.0).min(e.size.1 / 1080.0);
        self.th = Th::from(e.cfg);
        self.ui.tip = None;
        self.ui.again = false;
        let mut animating = false;

        // Плавное движение ленты к выбранному срезу.
        let diff = self.sel as f64 - self.pos;
        if diff.abs() > 0.0005 && e.cfg.picker.animations {
            self.pos += diff * (1.0 - (-(dt as f64) * 15.0).exp());
            animating = true;
        } else {
            self.pos = self.sel as f64;
        }
        let ft = if self.flipped { 1.0 } else { 0.0 };
        if (self.flip - ft).abs() > 0.005 {
            self.flip += (ft - self.flip) * (1.0 - (-dt * 18.0).exp());
            animating = true;
        } else {
            self.flip = ft;
        }

        // Предпросмотр на столе: с небольшой задержкой, чтобы не грузить всё подряд при быстром листании.
        if let Some(due) = self.preview_due {
            if now >= due {
                self.preview_due = None;
                if e.cfg.picker.live_preview {
                    if let Some(path) = self.selected_path(e.state) {
                        let shown = self.previewed.as_ref().or(self.committed.as_ref());
                        if shown != Some(&path) {
                            self.previewed = Some(path.clone());
                            e.actions.push(Action::Preview(path));
                        }
                        if let Some(next) = self.item_at(self.sel + self.nav_dir, e.cfg) {
                            e.actions.push(Action::Preload(e.state.items[next].path.clone()));
                        }
                    }
                }
            } else {
                animating = true;
            }
        }
        if !e.cfg.picker.live_preview && self.previewed.is_some() {
            self.previewed = None;
            e.actions.push(Action::Restore);
        }

        if self.stashed {
            if self.ui.press || self.ui.rpress {
                self.stashed = false;
                self.ui.again = true;
            }
            self.ui.end_frame();
            return self.ui.again;
        }

        let fade = ease((now - self.opened).as_secs_f32() / 0.14);
        if fade < 1.0 {
            animating = true;
        }
        let main_active = self.overlay == Overlay::None;
        let mut acts = std::mem::take(&mut self.pending);
        let top = self.draw_bar(e, fade, main_active, &mut acts);
        self.draw_slices(e, top, fade, main_active && !self.folder_menu, &mut acts);
        if self.flip > 0.01 && main_active {
            self.draw_card_back(e, &mut acts);
        }
        if self.search_open && main_active {
            self.draw_search(e, &mut acts);
        }

        // Панели поверх ленты.
        if self.overlay != Overlay::None {
            let sel = self.sel_item();
            let mut want = vec![];
            let mut close = false;
            {
                // На маленьких экранах панели не мельчим: текст должен читаться.
                let mut w = W { ui: &mut self.ui, d: &mut *e.d, th: self.th, s: self.s.max(1.12 * e.surf_scale) };
                let mut c = PanelCtx { state: &mut *e.state, cfg: &mut *e.cfg, pool: e.pool, gl: e.gl, actions: &mut acts, view: e.size, sel, live: &e.live, thumbs: &self.thumbs, toast: &mut self.toast, want: &mut want, close: &mut close, cfg_dirty: &mut self.cfg_dirty, refresh: false, playlist_filter: None };
                match self.overlay {
                    Overlay::Settings => panels::settings(&mut w, &mut self.settings, &mut c),
                    Overlay::Theme => panels::theme(&mut w, &mut self.theme, &mut c),
                    Overlay::Effects => panels::effects(&mut w, &mut self.effects, &mut c),
                    Overlay::Playlists => panels::playlists(&mut w, &mut self.playlists, &mut c),
                    Overlay::Downloads => panels::downloads(&mut w, &mut self.downloads, &mut c),
                    Overlay::None => {}
                }
                let (refresh, pf) = (c.refresh, c.playlist_filter.take());
                if let Some(pf) = pf {
                    self.playlist = pf;
                    self.refresh(e.state);
                } else if refresh {
                    self.refresh(e.state);
                }
            }
            for i in want {
                self.want_thumb(e.state, e.pool, i, true);
            }
            if close {
                self.overlay = Overlay::None;
                self.ui.focus = 0;
            }
            if self.downloads.busy() {
                animating = true;
            }
        }

        // Настройки пишем на диск, когда ползунок отпущен.
        if self.cfg_dirty && !self.ui.down {
            self.cfg_dirty = false;
            acts.push(Action::SaveConfig);
        }

        self.draw_tip(e, now, &mut animating);
        if let Some((text, at)) = self.toast.clone() {
            let age = (now - at).as_secs_f32();
            if age < 2.2 {
                let a = ease((2.2 - age) / 0.3) * ease(age / 0.12);
                let s = self.s;
                let w = e.d.text_w(BOLD, 10.5 * s, &text) + 32.0 * s;
                let r = Rect::new((e.size.0 - w) / 2.0, e.size.1 - 92.0 * s, w, 30.0 * s);
                e.d.shape(&Shape::new(r, alpha(self.th.backdrop, 0.96)).border(s.max(1.0), self.th.accent).alpha(a));
                e.d.text_c(BOLD, 10.5 * s, r, alpha(self.th.text, a), &text);
                animating = true;
            } else {
                self.toast = None;
            }
        }

        if self.ui.focus != 0 {
            // Мигание курсора не нужно: он рисуется всегда, лишних кадров нет.
        }
        self.why = format!("anim={animating} again={} active={} pos={:.4}/{} flip={} due={} frames={}", self.ui.again, self.ui.active, self.pos, self.sel, self.flip, self.preview_due.is_some(), self.frames);
        self.frames += 1;
        animating |= self.ui.again || self.ui.active != 0;
        self.ui.end_frame();
        e.actions.append(&mut acts);
        animating
    }

    fn bar_height(&self) -> f32 {
        24.0 * self.s
    }

    /// Панель фильтров. Возвращает верх области ленты.
    fn draw_bar(&mut self, e: &mut Env, fade: f32, active: bool, acts: &mut Vec<Action>) -> f32 {
        let s = self.s;
        let th = self.th;
        let bar_h = self.bar_height();
        let card_h = e.cfg.picker.height * s;
        let gap = 14.5 * s;
        let block = bar_h + gap + card_h;
        let y0 = ((e.size.1 - block) / 2.0 - 12.0 * s + e.cfg.picker.offset_y * s).max(4.0 * s);
        let cards_top = y0 + bar_h + gap;
        if !e.cfg.picker.bar || e.state.view.bar_hidden {
            return cards_top;
        }
        let skew = 10.0 * s;
        let v = e.state.view.clone();
        #[derive(Clone)]
        enum B {
            Text(&'static str),
            Icon(char),
            Swatch(Color),
            Folder(String),
        }
        struct Btn {
            key: String,
            kind: B,
            w: f32,
            on: bool,
            tip: &'static str,
            gap: f32,
        }
        let mut btns: Vec<Btn> = vec![];
        let px = 9.3 * s;
        let mut text = |d: &mut Draw, key: &str, label: &'static str, on: bool, tip: &'static str| {
            let w = d.text_w(BOLD, px, label) + 2.0 * 13.0 * s + skew;
            btns.push(Btn { key: key.into(), kind: B::Text(label), w, on, tip, gap: 0.0 });
        };
        text(e.d, "type:", crate::i18n::tr("ВСЕ"), v.kind.is_empty(), crate::i18n::tr("Все обои (1)"));
        text(e.d, "type:static", crate::i18n::tr("ФОТО"), v.kind == "static", crate::i18n::tr("Только картинки (2)"));
        text(e.d, "type:video", crate::i18n::tr("ВИДЕО"), v.kind == "video", crate::i18n::tr("Только видео (3)"));
        let orient = match v.orient.as_str() {
            "wide" => crate::i18n::tr("ШИРОКИЕ"),
            "tall" => crate::i18n::tr("ВЫСОКИЕ"),
            _ => crate::i18n::tr("ФОРМА"),
        };
        text(e.d, "orient", orient, !v.orient.is_empty(), crate::i18n::tr("Форма: все, широкие, высокие (O)"));
        let folder_label = if let Some(p) = &self.playlist { format!("♪ {}", p.to_uppercase()) } else if v.folder.is_empty() { crate::i18n::tr("ВСЕ ОБОИ").to_string() } else { v.folder.to_uppercase() };
        btns.push(Btn { key: "folder".into(), kind: B::Folder(folder_label), w: 117.2 * s, on: !v.folder.is_empty() || self.playlist.is_some(), tip: crate::i18n::tr("Папка"), gap: 0.0 });
        let icon_w = 49.4 * s;
        let mut icon = |key: &str, ch: char, on: bool, tip: &'static str, gap: f32| {
            btns.push(Btn { key: key.into(), kind: B::Icon(ch), w: icon_w, on, tip, gap });
        };
        icon("hidden", icons::INCOGNITO, v.show_hidden, crate::i18n::tr("Показать скрытые"), 0.0);
        icon("sort:date", icons::CALENDAR_CLOCK, v.sort == "date", crate::i18n::tr("Сначала новые файлы"), 0.0);
        icon("sort:recent", icons::HISTORY, v.sort == "recent", crate::i18n::tr("Недавно применённые"), 0.0);
        icon("sort:color", icons::PALETTE, v.sort == "color", crate::i18n::tr("По цвету"), 0.0);
        icon("sort:pop", icons::FIRE, v.sort == "pop", crate::i18n::tr("Часто применяемые"), 0.0);
        icon("sort:richness", icons::CARD_TEXT, v.sort == "richness", crate::i18n::tr("Самые красочные"), 0.0);
        icon("sort:minimalist", icons::SQUARE, v.sort == "minimalist", crate::i18n::tr("Самые спокойные"), 0.0);
        icon("sort:res", icons::ASPECT_RATIO, v.sort == "res", crate::i18n::tr("По разрешению"), 0.0);
        icon("favourites", icons::HEART, v.favourites, crate::i18n::tr("Только избранное"), 0.0);
        icon("random", icons::DICE_MULTIPLE, false, crate::i18n::tr("Случайные обои (R)"), 0.0);
        for (i, h) in SWATCH_HUES.iter().enumerate() {
            let c = hsv(*h, 0.79, 0.76);
            btns.push(Btn { key: format!("color:{i}"), kind: B::Swatch(c), w: 28.15 * s, on: v.color == i as i32, tip: "", gap: if i == 0 { 6.0 * s } else { 0.0 } });
        }
        btns.push(Btn { key: "color:-1".into(), kind: B::Swatch(hex(0x707070)), w: 28.15 * s, on: false, tip: crate::i18n::tr("Любой цвет"), gap: 0.0 });
        let mut icon = |key: &str, ch: char, on: bool, tip: &'static str, gap: f32| {
            btns.push(Btn { key: key.into(), kind: B::Icon(ch), w: icon_w, on, tip, gap });
        };
        icon("theme", icons::PALETTE, false, crate::i18n::tr("Дизайнер тем (C)"), 6.0 * s);
        icon("search", icons::MAGNIFY, self.search_open, crate::i18n::tr("Поиск и теги (T или /)"), 0.0);
        icon("download", icons::DOWNLOAD, false, crate::i18n::tr("Скачать обои (D)"), 0.0);
        icon("playlists", icons::PLAYLIST_MUSIC, self.playlist.is_some(), crate::i18n::tr("Плейлисты и автосмена (P)"), 0.0);
        icon("settings", icons::COG, false, crate::i18n::tr("Настройки (S)"), 0.0);
        icon("stash", icons::EYE_OFF, false, crate::i18n::tr("Спрятать меню и посмотреть обои (V)"), 0.0);

        let total: f32 = btns.iter().map(|b| b.w - skew + b.gap).sum::<f32>() + skew;
        let mut x = ((e.size.0 - total) / 2.0).round();
        let y = y0.round();
        let mut clicked: Option<String> = None;
        let mut folder_rect = Rect::default();
        for b in &btns {
            x += b.gap;
            let bbox = Rect::new(x, y, b.w, bar_h);
            let r = Rect::new(x + skew / 2.0, y, b.w - skew, bar_h);
            let hot = active && r.has_skewed(self.ui.mouse, skew);
            let (bg, fg) = match &b.kind {
                B::Swatch(c) => (if hot { mix(*c, [1.0; 4], 0.25) } else { *c }, th.text),
                _ if b.on => (th.accent, th.on_accent),
                _ if hot => (alpha(mix(th.raised, th.text, 0.14), 0.95), th.text),
                _ => (alpha(th.raised, 0.92), th.text),
            };
            e.d.shape(&Shape::new(Rect::new(r.x - 0.5, r.y, r.w + 1.0, r.h), bg).skew(skew).alpha(fade));
            // Тонкая косая черта между соседними кнопками.
            e.d.shape(&Shape::new(Rect::new(r.x - 0.5 * s, r.y, s.max(1.0), r.h), alpha(th.backdrop, 0.30)).skew(skew).alpha(fade));
            match &b.kind {
                B::Text(l) => e.d.text_c(BOLD, px, r, alpha(fg, fade), l),
                B::Icon(ch) => {
                    let mut buf = [0u8; 4];
                    e.d.text_c(ICON, 13.4 * s, r, alpha(fg, fade), ch.encode_utf8(&mut buf));
                }
                B::Swatch(_) => {
                    if b.on {
                        e.d.shape(&Shape::new(Rect::new(r.x, r.bottom() - 3.0 * s, r.w, 3.0 * s), th.text).skew(skew * 3.0 / 24.0).alpha(fade));
                    }
                }
                B::Folder(l) => {
                    folder_rect = bbox;
                    let mut buf = [0u8; 4];
                    e.d.text(ICON, 10.0 * s, r.x + 6.0 * s, r.cy(), alpha(fg, fade), icons::FOLDER.encode_utf8(&mut buf));
                    let label = e.d.fit(BOLD, px, l, r.w - 38.0 * s);
                    e.d.text(BOLD, px, r.x + 21.0 * s, r.cy(), alpha(fg, fade), &label);
                    e.d.text(ICON, 9.0 * s, r.right() - 13.0 * s, r.cy(), alpha(fg, fade), icons::MENU_DOWN.encode_utf8(&mut buf));
                }
            }
            if hot {
                if !b.tip.is_empty() {
                    self.ui.tip = Some((b.tip.to_string(), bbox));
                }
                if self.ui.press {
                    clicked = Some(b.key.clone());
                }
            }
            x += b.w - skew;
        }

        if let Some(key) = clicked {
            self.ui.press = false;
            let view = &mut e.state.view;
            let mut filter = true;
            match key.as_str() {
                "orient" => {
                    view.orient = match view.orient.as_str() {
                        "" => "wide".into(),
                        "wide" => "tall".into(),
                        _ => String::new(),
                    }
                }
                "folder" => {
                    self.folder_menu = !self.folder_menu;
                    filter = false;
                }
                "hidden" => view.show_hidden = !view.show_hidden,
                "favourites" => view.favourites = !view.favourites,
                "random" => {
                    filter = false;
                    let (st, cfg) = (&*e.state, &*e.cfg);
                    self.random(st, cfg, acts);
                }
                "theme" => {
                    filter = false;
                    self.open_overlay(Overlay::Theme);
                }
                "search" => {
                    filter = false;
                    if self.search_open {
                        self.search_open = false;
                        self.ui.focus = 0;
                    } else {
                        self.open_search();
                    }
                }
                "download" => {
                    filter = false;
                    self.open_overlay(Overlay::Downloads);
                }
                "playlists" => {
                    filter = false;
                    self.open_overlay(Overlay::Playlists);
                }
                "settings" => {
                    filter = false;
                    self.open_overlay(Overlay::Settings);
                }
                "stash" => {
                    filter = false;
                    self.stashed = true;
                    self.ui.again = true;
                }
                k if k.starts_with("type:") => view.kind = k[5..].to_string(),
                k if k.starts_with("sort:") => {
                    let mode = &k[5..];
                    if view.sort == mode {
                        view.reverse = !view.reverse;
                    } else {
                        view.sort = mode.to_string();
                        view.reverse = false;
                    }
                }
                k if k.starts_with("color:") => {
                    let c: i32 = k[6..].parse().unwrap_or(-1);
                    view.color = if view.color == c { -1 } else { c };
                }
                _ => filter = false,
            }
            if filter {
                self.folder_menu = false;
                self.set_filter(e.state, acts);
            }
        }

        // Выпадающий список папок.
        if self.folder_menu && active {
            let mut rows: Vec<(String, Option<String>, bool)> = vec![(crate::i18n::tr("Все обои").into(), Some(String::new()), false)];
            for f in e.state.folders() {
                rows.push((f.clone(), Some(f), false));
            }
            for p in &e.state.playlists {
                rows.push((format!("♪ {}", p.name), Some(p.name.clone()), true));
            }
            let row_h = 22.0 * s;
            let w = folder_rect.w.max(150.0 * s);
            let menu = Rect::new(folder_rect.x, folder_rect.bottom() + 3.0 * s, w, row_h * rows.len() as f32 + 6.0 * s);
            e.d.shape(&Shape::new(Rect::new(menu.x, menu.y + 4.0 * s, menu.w, menu.h), [0.0, 0.0, 0.0, 0.45]).soft(10.0 * s));
            e.d.shape(&Shape::new(menu, alpha(th.backdrop, 0.97)).border(s.max(1.0), th.outline));
            let mut pick: Option<(Option<String>, bool)> = None;
            for (i, (label, val, is_pl)) in rows.iter().enumerate() {
                let r = Rect::new(menu.x + 3.0 * s, menu.y + 3.0 * s + row_h * i as f32, menu.w - 6.0 * s, row_h);
                let on = if *is_pl { self.playlist.as_ref() == val.as_ref() } else { self.playlist.is_none() && Some(&e.state.view.folder) == val.as_ref() };
                let hot = self.ui.hover(r);
                if on || hot {
                    e.d.rect(r, if on { mix(th.backdrop, th.accent, 0.4) } else { th.raised });
                }
                let label = e.d.fit(BOLD, 9.6 * s, label, r.w - 16.0 * s);
                e.d.text(BOLD, 9.6 * s, r.x + 8.0 * s, r.cy(), th.text, &label);
                if hot && self.ui.press {
                    pick = Some((val.clone(), *is_pl));
                }
            }
            if let Some((val, is_pl)) = pick {
                if is_pl {
                    self.playlist = val;
                    e.state.view.folder.clear();
                } else {
                    self.playlist = None;
                    e.state.view.folder = val.unwrap_or_default();
                }
                self.folder_menu = false;
                self.ui.press = false;
                self.set_filter(e.state, acts);
            } else if self.ui.press && !menu.has(self.ui.mouse) {
                self.folder_menu = false;
            }
        }
        cards_top
    }

    fn draw_slices(&mut self, e: &mut Env, top: f32, fade: f32, active: bool, acts: &mut Vec<Action>) {
        let s = self.s;
        let th = self.th;
        let p = &e.cfg.picker;
        let h = p.height * s;
        let margin = 15.0 * s;
        let area = Rect::new(margin, top, e.size.0 - 2.0 * margin, h);
        let n = self.n();
        self.drawn.clear();
        if n == 0 {
            let msg = if e.state.items.is_empty() { crate::i18n::tr("В библиотеке пока нет обоев. Добавьте папку в настройках (S) или скачайте обои (D).") } else { crate::i18n::tr("Ничего не найдено. Снимите фильтры или измените запрос.") };
            let w = e.d.text_w(BOLD, 12.0 * s, msg) + 40.0 * s;
            let r = Rect::new((e.size.0 - w) / 2.0, area.cy() - 20.0 * s, w, 40.0 * s);
            e.d.shape(&Shape::new(r, alpha(th.backdrop, 0.94)).border(s.max(1.0), th.outline).alpha(fade));
            e.d.text_c(BOLD, 12.0 * s, r, alpha(th.text, fade), msg);
            return;
        }
        let skew = p.skew * s;
        let base_sel = (p.selected_width * s - skew.abs()).min(area.w * 0.86);
        let visible = p.visible.max(3) as f32;
        let pitch = ((area.w - base_sel) / (visible - 2.0)).max(6.0 * s);
        let radius = p.corner * s;
        let k0 = self.pos.floor() as i64;
        let frac = (self.pos - k0 as f64) as f32;
        let reach = (visible as i64) / 2 + 3;
        let width = |k: i64| -> f32 {
            let e = ease(1.0 - (k as f64 - self.pos).abs() as f32);
            pitch + (base_sel - pitch) * e
        };
        // Левые края относительно среза k0.
        let mut lefts: Vec<(i64, f32, f32)> = vec![];
        let mut x = 0.0;
        for k in k0..=k0 + reach {
            let w = width(k);
            lefts.push((k, x, w));
            x += w;
        }
        let mut x = 0.0;
        for k in (k0 - reach..k0).rev() {
            let w = width(k);
            x -= w;
            lefts.push((k, x, w));
        }
        let c0 = width(k0) / 2.0;
        let c1 = width(k0) + width(k0 + 1) / 2.0;
        let camera = c0 + (c1 - c0) * frac;
        let origin = area.cx() - camera;
        for (k, left, w) in lefts {
            if self.item_at(k, e.cfg).is_none() {
                continue;
            }
            let rect = Rect::new(origin + left, top, w, h);
            let edge = (rect.cx() - area.x).min(area.right() - rect.cx());
            let a = ease((edge + pitch * 0.35) / (pitch * 1.1)) * fade;
            if a <= 0.01 {
                continue;
            }
            let ex = ease(1.0 - (k as f64 - self.pos).abs() as f32);
            self.drawn.push(Slice { k, rect, e: ex, a });
        }
        // Дальние срезы рисуются первыми, выбранный — последним.
        let pos = self.pos;
        self.drawn.sort_by(|a, b| ((b.k as f64 - pos).abs()).partial_cmp(&(a.k as f64 - pos).abs()).unwrap());

        let frame_w = base_sel + skew.abs();
        for idx in 0..self.drawn.len() {
            let (k, rect, ex, a) = { let d = &self.drawn[idx]; (d.k, d.rect, d.e, d.a) };
            let Some(i) = self.item_at(k, e.cfg) else { continue };
            self.want_thumb(e.state, e.pool, i, true);
            let it = &e.state.items[i];
            let frame = Rect::new(rect.cx() - frame_w / 2.0, top, frame_w, h);
            let body = Rect::new(rect.x - 0.5, rect.y, rect.w + 1.0, rect.h);
            if !e.cfg.saver_now {
                e.d.shape(&Shape::new(Rect::new(body.x + 3.0 * s, body.y + 6.0 * s, body.w, body.h), [0.0, 0.0, 0.0, 0.42 * a]).radius(radius).skew(skew).soft(9.0 * s));
            }
            let (bw, bc) = if ex > 0.5 { (2.2 * s * ex, mix(th.accent, [1.0; 4], 0.42)) } else { (0.8 * s, [1.0, 1.0, 1.0, 0.16]) };
            let shade = 0.84 + 0.16 * ex;
            let live = e.live.as_ref().filter(|l| ex > 0.6 && l.borrow().path == it.path && !(e.cfg.saver_now && it.video)).map(|l| l.borrow());
            let tex = live.as_ref().and_then(|l| l.tex()).or_else(|| self.thumbs.get(&it.path));
            let mut sh = Shape::new(body, [shade, shade, shade, 1.0]).radius(radius).skew(skew).border(bw, bc).alpha(a);
            sh = match tex {
                Some(t) => sh.cover(t, frame),
                None => {
                    sh.color = if it.analyzed { let c = hsv(it.hue, (it.sat * 0.9).min(0.6), (it.lum * 0.9 + 0.12).min(0.8)); [c[0], c[1], c[2], 1.0] } else { th.alt };
                    sh
                }
            };
            e.d.shape(&sh);
            drop(live);
            let bottom_right = rect.right() - skew / 2.0;
            let top_right = rect.right() + skew / 2.0;
            if p.badges && !(ex > 0.5 && self.flip > 0.3) {
                let label = if it.video { crate::i18n::tr("ВИДЕО") } else { crate::i18n::tr("ФОТО") };
                let tw = e.d.text_w(BOLD, 6.6 * s, label);
                let bw2 = tw + 12.0 * s;
                let inset = if ex > 0.5 { radius.min(rect.w / 2.0) * 0.45 } else { 2.0 * s };
                let r = Rect::new(bottom_right - bw2 - inset - 2.0 * s, rect.bottom() - 18.0 * s, bw2, 11.0 * s);
                e.d.shape(&Shape::new(r, alpha(th.backdrop, 0.74)).skew(4.0 * s).alpha(a));
                e.d.text_c(BOLD, 6.6 * s, r, alpha(th.text, 0.88 * a), label);
                if it.video {
                    let inset = if ex > 0.5 { radius.min(rect.w / 2.0) * 0.5 } else { 0.0 };
                    let c = Rect::new(top_right - 20.0 * s - inset, rect.y + 9.0 * s, 13.0 * s, 13.0 * s);
                    e.d.shape(&Shape::new(c, alpha(th.backdrop, 0.55)).radius(7.0 * s).border(s.max(1.0), alpha(th.accent, 0.8)).alpha(a));
                    let mut buf = [0u8; 4];
                    e.d.text_c(ICON, 7.5 * s, c, alpha(th.text, 0.9 * a), icons::PLAY.encode_utf8(&mut buf));
                }
            }
            if it.fav {
                let inset = if ex > 0.5 { radius.min(rect.w / 2.0) * 0.5 } else { 0.0 };
                let mut buf = [0u8; 4];
                e.d.text(ICON, 9.0 * s, rect.x + skew / 2.0 + 8.0 * s + inset, rect.y + 16.0 * s, alpha(th.accent2, a), icons::HEART.encode_utf8(&mut buf));
            }
            if ex > 0.5 {
                self.card = rect;
                self.card_skew = skew;
            }
        }

        // Миниатюры далеко от выбранного места выгружаем из видеопамяти.
        if self.thumbs.len() > 60 && self.overlay == Overlay::None {
            let keep: HashSet<String> = (self.sel - 24..=self.sel + 24).filter_map(|k| self.item_at(k, e.cfg)).map(|i| e.state.items[i].path.clone()).collect();
            self.thumbs.retain(|k, _| keep.contains(k));
            self.thumb_req.retain(|k| keep.contains(k));
        }
        if !active {
            return;
        }
        // Мышь: щелчок по срезу выбирает, по выбранному — применяет; колесо листает.
        let hit = self.drawn.iter().rev().find(|d| d.rect.has_skewed(self.ui.mouse, skew)).map(|d| (d.k, d.e));
        if self.ui.wheel != 0.0 && area.has(self.ui.mouse) && self.flip < 0.5 {
            let steps = if self.ui.wheel > 0.0 { self.ui.wheel.ceil() } else { self.ui.wheel.floor() } as i64;
            self.nav(steps, e.cfg);
            self.ui.wheel = 0.0;
            self.ui.again = true;
        }
        if let Some((k, ex)) = hit {
            if self.ui.press && self.flip < 0.5 {
                if k == self.sel && ex > 0.5 {
                    let (st, cfg) = (&*e.state, &*e.cfg);
                    self.commit(st, cfg, acts);
                } else {
                    self.nav(k - self.sel, e.cfg);
                }
                self.ui.again = true;
            } else if self.ui.rpress {
                if k == self.sel {
                    self.flipped = !self.flipped;
                } else {
                    self.nav(k - self.sel, e.cfg);
                    self.flipped = true;
                }
                self.ui.again = true;
            } else if e.cfg.picker.hover_select && k != self.sel && self.flip < 0.5 {
                self.nav(k - self.sel, e.cfg);
                self.ui.again = true;
            }
        } else if self.ui.press && !self.search_open && self.flip < 0.5 && !self.bar_zone(e).has(self.ui.mouse) {
            // Щелчок мимо всего закрывает меню.
            acts.push(Action::Close);
        }
    }

    fn bar_zone(&self, e: &Env) -> Rect {
        Rect::new(0.0, 0.0, e.size.0, e.size.1 * 0.26 + self.bar_height() + 40.0 * self.s)
    }

    /// Оборот карточки: сведения об обоях и действия.
    fn draw_card_back(&mut self, e: &mut Env, acts: &mut Vec<Action>) {
        let Some(i) = self.sel_item() else { return };
        let s = self.s;
        let th = self.th;
        let a = ease(self.flip);
        let card = self.card;
        let radius = e.cfg.picker.corner * s;
        e.d.shape(&Shape::new(card, alpha(th.backdrop, 0.90)).radius(radius).skew(self.card_skew).border(2.2 * s, mix(th.accent, [1.0; 4], 0.42)).alpha(a));
        if a < 0.6 {
            return;
        }
        let pad = self.card_skew.abs() / 2.0 + radius * 0.42 + 6.0 * s;
        let r = Rect::new(card.x + pad, card.y + 22.0 * s, card.w - 2.0 * pad, card.h - 44.0 * s);
        let it = e.state.items[i].clone();
        let path = it.path.clone();
        let mut w = W { ui: &mut self.ui, d: &mut *e.d, th, s };
        let kicker = crate::trf!("Библиотека / {}", "Library / {}", if it.video { crate::i18n::tr("ВИДЕО") } else { crate::i18n::tr("ИЗОБРАЖЕНИЕ") });
        w.text(BOLD, 8.0, r.x, r.y + 8.0 * s, th.accent, &kicker);
        let b = 24.0 * s;
        if w.icon_button(Rect::new(r.right() - b, r.y - 4.0 * s, b, b), if it.fav { icons::HEART } else { icons::HEART_OUTLINE }, it.fav, crate::i18n::tr("Избранное (F)")) {
            e.state.items[i].fav = !it.fav;
            acts.push(Action::SaveState);
        }
        if w.icon_button(Rect::new(r.right() - 2.0 * b - 4.0 * s, r.y - 4.0 * s, b, b), icons::INCOGNITO, it.hidden, crate::i18n::tr("Скрыть из ленты (H)")) {
            e.state.items[i].hidden = !it.hidden;
            acts.push(Action::SaveState);
            self.pending.push(Action::Redraw);
            self.flipped = false;
            let st = &*e.state;
            self.view = library::view(st, &self.query, &self.tags, self.playlist.as_deref());
            self.sel = self.sel.clamp(0, (self.view.len() as i64 - 1).max(0));
            self.pos = self.sel as f64;
            return;
        }
        let name = w.d.fit(BOLD, 17.0 * s, &it.name, r.w);
        w.text(BOLD, 17.0, r.x, r.y + 44.0 * s, th.text, &name);
        let folder = w.d.fit(MEDIUM, 9.0 * s, &crate::config::shorten(&it.file), r.w);
        w.text(MEDIUM, 9.0, r.x, r.y + 66.0 * s, th.dim, &folder);

        // Сведения в четыре колонки.
        let dur = if it.duration > 0.0 { format!("{}:{:02}", it.duration as u32 / 60, it.duration as u32 % 60) } else { String::new() };
        let facts = [
            (crate::i18n::tr("РАЗРЕШЕНИЕ"), if it.w > 0 { format!("{} × {}", it.w, it.h) } else { "—".into() }),
            (crate::i18n::tr("РАЗМЕР"), human_size(it.size)),
            (crate::i18n::tr("ИЗМЕНЕНО"), date(it.mtime)),
            (if it.video { crate::i18n::tr("ДЛИНА") } else { crate::i18n::tr("ПРИМЕНЕНО") }, if it.video { dur } else { times(it.applied) }),
        ];
        let col = r.w / 4.0;
        let fy = r.y + 96.0 * s;
        for (n, (label, val)) in facts.iter().enumerate() {
            let x = r.x + col * n as f32;
            w.text(BOLD, 7.0, x, fy, th.accent, label);
            w.text(BOLD, 10.5, x, fy + 18.0 * s, th.text, val);
        }

        // Теги.
        let ty = fy + 46.0 * s;
        w.text(BOLD, 7.5, r.x, ty, th.dim, &crate::trf!("Теги / {}", "Tags / {}", it.tags.len()));
        let mut x = r.x;
        let cy = ty + 12.0 * s;
        let ch = 20.0 * s;
        let mut remove: Option<usize> = None;
        for (n, t) in it.tags.iter().enumerate() {
            let cw = w.text_w(BOLD, 9.0, t) + 30.0 * s;
            if x + cw > r.x + r.w * 0.46 {
                break;
            }
            let cr = Rect::new(x, cy, cw, ch);
            let hot = w.ui.hover(cr);
            w.d.shape(&Shape::new(cr, if hot { th.raised } else { th.alt }).border(s.max(1.0), th.outline));
            w.text(BOLD, 9.0, cr.x + 8.0 * s, cr.cy(), th.text, t);
            let mut buf = [0u8; 4];
            w.text(ICON, 8.0, cr.right() - 15.0 * s, cr.cy(), th.dim, icons::CLOSE.encode_utf8(&mut buf));
            if hot && w.ui.press {
                remove = Some(n);
            }
            x += cw + 5.0 * s;
        }
        if let Some(n) = remove {
            e.state.items[i].tags.remove(n);
            acts.push(Action::SaveState);
        }
        let field = Rect::new(x, cy, (r.x + r.w * 0.46 - x).max(110.0 * s), ch);
        if w.text_field(field, id("tag"), &mut self.tag_input, crate::i18n::tr("+ Добавить тег")) {
            let t = self.tag_input.trim().to_lowercase();
            if !t.is_empty() && !it.tags.contains(&t) {
                e.state.items[i].tags.push(t);
                acts.push(Action::SaveState);
            }
            self.tag_input.clear();
        }

        // Действия.
        let ax = r.x + r.w * 0.52;
        w.text(BOLD, 7.5, ax, ty, th.dim, crate::i18n::tr("Действия"));
        let bw = (r.right() - ax - 2.0 * 5.0 * s) / 3.0;
        let bh = 22.0 * s;
        let cell = |n: usize| Rect::new(ax + (bw + 5.0 * s) * (n % 3) as f32, cy + (bh + 5.0 * s) * (n / 3) as f32, bw, bh);
        if w.button(cell(0), crate::i18n::tr("Эффекты")) {
            self.overlay = Overlay::Effects;
        }
        if w.button(cell(1), crate::i18n::tr("Показать в папке")) {
            show_in_folder(&it.file);
            acts.push(Action::Close);
        }
        if w.button(cell(2), crate::i18n::tr("Плейлист")) {
            self.playlist_menu = !self.playlist_menu;
        }
        if w.button(cell(3), crate::i18n::tr("Копировать путь")) {
            copy_text(&it.file);
            self.toast = Some((crate::i18n::tr("Путь скопирован").into(), Instant::now()));
        }
        if w.button(cell(4), if it.video { crate::i18n::tr("Копировать файл") } else { crate::i18n::tr("Копировать картинку") }) {
            copy_file(&it.file);
            self.toast = Some((crate::i18n::tr("Скопировано в буфер обмена").into(), Instant::now()));
        }
        if self.delete_confirm {
            if w.button_danger(cell(5), crate::i18n::tr("Точно удалить файл?")) {
                acts.push(Action::Delete(path.clone()));
                self.flipped = false;
                self.delete_confirm = false;
            }
        } else if w.button_ghost(cell(5), crate::i18n::tr("Удалить"), true) {
            self.delete_confirm = true;
        }

        if self.playlist_menu {
            let anchor = cell(2);
            let lists: Vec<(String, bool)> = e.state.playlists.iter().map(|p| (p.name.clone(), p.items.contains(&path))).collect();
            let row_h = 22.0 * s;
            let rows = lists.len().max(1) as f32;
            let menu = Rect::new(anchor.x, anchor.y - row_h * rows - 9.0 * s, anchor.w.max(170.0 * s), row_h * rows + 6.0 * s);
            w.d.shape(&Shape::new(menu, th.backdrop).border(s.max(1.0), th.outline));
            if lists.is_empty() {
                w.text(MEDIUM, 9.0, menu.x + 8.0 * s, menu.cy(), th.dim, crate::i18n::tr("Плейлистов пока нет (P)"));
            }
            for (n, (name, has)) in lists.iter().enumerate() {
                let rr = Rect::new(menu.x + 3.0 * s, menu.y + 3.0 * s + row_h * n as f32, menu.w - 6.0 * s, row_h);
                let hot = w.ui.hover(rr);
                if hot {
                    w.d.rect(rr, th.raised);
                }
                let mut buf = [0u8; 4];
                if *has {
                    w.text(ICON, 9.0, rr.x + 6.0 * s, rr.cy(), th.accent, icons::CHECK.encode_utf8(&mut buf));
                }
                let label = w.d.fit(BOLD, 9.6 * s, name, rr.w - 30.0 * s);
                w.text(BOLD, 9.6, rr.x + 22.0 * s, rr.cy(), th.text, &label);
                if hot && w.ui.press {
                    let pl = &mut e.state.playlists[n];
                    if *has {
                        pl.items.retain(|x| x != &path);
                    } else {
                        pl.items.push(path.clone());
                    }
                    acts.push(Action::SaveState);
                    acts.push(Action::Rotation);
                }
            }
        }
        // Щелчок по карточке мимо кнопок ничего не применяет.
        if self.ui.press && !card.has_skewed(self.ui.mouse, self.card_skew) && !self.bar_zone(e).has(self.ui.mouse) {
            self.flipped = false;
            self.playlist_menu = false;
        }
    }

    /// Панель поиска: запрос и облако тегов.
    fn draw_search(&mut self, e: &mut Env, acts: &mut Vec<Action>) {
        let s = self.s;
        let th = self.th;
        let all = e.state.all_tags();
        let w0 = 760.0 * s;
        let rows = if all.is_empty() { 0.0 } else { 2.0 };
        let hgt = (52.0 + rows * 26.0 + if all.is_empty() { 0.0 } else { 14.0 }) * s;
        let r = Rect::new((e.size.0 - w0) / 2.0, (self.card.bottom() + 28.0 * s).min(e.size.1 - hgt - 8.0 * s), w0, hgt);
        e.d.shape(&Shape::new(Rect::new(r.x, r.y + 5.0 * s, r.w, r.h), [0.0, 0.0, 0.0, 0.4]).soft(12.0 * s));
        e.d.shape(&Shape::new(r, alpha(th.backdrop, 0.95)).border(s.max(1.0), th.outline));
        let before = (self.query.clone(), self.tags.clone());
        let mut w = W { ui: &mut self.ui, d: &mut *e.d, th, s };
        let mut buf = [0u8; 4];
        w.text(ICON, 12.0, r.x + 14.0 * s, r.y + 26.0 * s, th.accent, icons::MAGNIFY.encode_utf8(&mut buf));
        let count = crate::trf!("{} из {}", "{} of {}", self.view.len(), e.state.items.iter().filter(|i| !i.broken).count());
        let cw = w.text_w(BOLD, 8.5, &count);
        let field = Rect::new(r.x + 38.0 * s, r.y + 12.0 * s, r.w - 62.0 * s - cw, 28.0 * s);
        if w.text_field(field, id("search"), &mut self.query, crate::i18n::tr("Название, папка или тег")) {
            w.ui.focus = 0;
        }
        w.text(BOLD, 8.5, r.right() - cw - 12.0 * s, field.cy(), th.dim, &count);
        if !all.is_empty() {
            let mut x = r.x + 14.0 * s;
            let mut y = r.y + 52.0 * s;
            for (t, n) in all.iter() {
                let label = format!("{t}  {n}");
                let cw = w.text_w(BOLD, 9.0, &label) + 18.0 * s;
                if x + cw > r.right() - 14.0 * s {
                    x = r.x + 14.0 * s;
                    y += 26.0 * s;
                    if y > r.bottom() - 22.0 * s {
                        break;
                    }
                }
                let on = self.tags.contains(t);
                if w.chip(Rect::new(x, y, cw, 20.0 * s), &label, on) {
                    if on {
                        self.tags.retain(|x| x != t);
                    } else {
                        self.tags.push(t.clone());
                    }
                }
                x += cw + 6.0 * s;
            }
        }
        if before != (self.query.clone(), self.tags.clone()) {
            self.refresh(e.state);
            acts.push(Action::Redraw);
            self.ui.again = true;
        }
        if self.ui.press && r.has(self.ui.mouse) {
            self.ui.press = false;
        }
    }

    fn draw_tip(&mut self, e: &mut Env, now: Instant, animating: &mut bool) {
        let Some((text, anchor)) = self.ui.tip.clone() else {
            self.tip_since = None;
            return;
        };
        let since = match self.tip_since {
            Some((r, t)) if r == anchor => t,
            _ => {
                self.tip_since = Some((anchor, now));
                now
            }
        };
        if (now - since).as_secs_f32() < 0.45 {
            *animating = true;
            return;
        }
        let s = self.s;
        let w = e.d.text_w(BOLD, 9.0 * s, &text) + 16.0 * s;
        let x = (anchor.cx() - w / 2.0).clamp(4.0 * s, e.size.0 - w - 4.0 * s);
        let mut y = anchor.bottom() + 6.0 * s;
        if y + 22.0 * s > e.size.1 {
            y = anchor.y - 28.0 * s;
        }
        let r = Rect::new(x, y, w, 22.0 * s);
        e.d.shape(&Shape::new(r, alpha(self.th.backdrop, 0.97)).border(s.max(1.0), self.th.outline));
        e.d.text_c(BOLD, 9.0 * s, r, self.th.text, &text);
    }

    // Доступ для панелей.
    pub(crate) fn overlay(&self) -> Overlay {
        self.overlay
    }
    pub(crate) fn panel_states(&mut self) -> (&mut panels::SettingsSt, &mut panels::ThemeSt, &mut panels::EffectsSt, &mut panels::PlaylistsSt, &mut panels::DownloadsSt, &mut Option<(String, Instant)>) {
        (&mut self.settings, &mut self.theme, &mut self.effects, &mut self.playlists, &mut self.downloads, &mut self.toast)
    }
}

pub fn copy_text(text: &str) {
    use std::io::Write;
    if let Ok(mut c) = std::process::Command::new("wl-copy").stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut i) = c.stdin.take() {
            let _ = i.write_all(text.as_bytes());
        }
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

pub fn copy_file(path: &str) {
    // Список URI понимают файловые менеджеры и мессенджеры: вставится сам файл.
    use std::io::Write;
    let uri = format!("file://{}\n", path.split('/').map(urlencode).collect::<Vec<_>>().join("/"));
    if let Ok(mut c) = std::process::Command::new("wl-copy").args(["-t", "text/uri-list"]).stdin(std::process::Stdio::piped()).spawn() {
        if let Some(mut i) = c.stdin.take() {
            let _ = i.write_all(uri.as_bytes());
        }
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

pub fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

pub fn show_in_folder(file: &str) {
    let uri = format!("file://{}", file.split('/').map(urlencode).collect::<Vec<_>>().join("/"));
    let ok = std::process::Command::new("gdbus")
        .args(["call", "--session", "--dest", "org.freedesktop.FileManager1", "--object-path", "/org/freedesktop/FileManager1", "--method", "org.freedesktop.FileManager1.ShowItems", &format!("['{}']", uri.replace('\'', "%27")), ""])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        if let Some(dir) = std::path::Path::new(file).parent() {
            let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
        }
    }
}
