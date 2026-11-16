//! Панели поверх ленты: настройки, дизайнер тем, эффекты, плейлисты, загрузки.
use crate::config::{self, Config, Palette};
use crate::draw::{alpha, hex, hsv, mix, parse_hex, to_hex, to_hsv, Rect, Shape, BOLD, ICON, MEDIUM};
use crate::fx;
use crate::gfx::{Gl, Tex};
use crate::icons;
use crate::jobs::{self, Done, Pool};
use crate::library::{self, State};
use crate::picker::{Action, Picker};
use crate::ui::{id, W};
use crate::wallpaper::Src;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Overlay {
    None,
    Settings,
    Theme,
    Effects,
    Playlists,
    Downloads,
}

pub struct PanelCtx<'a> {
    pub state: &'a mut State,
    pub cfg: &'a mut Config,
    pub pool: &'a Pool,
    pub gl: &'a Gl,
    pub actions: &'a mut Vec<Action>,
    pub view: (f32, f32),
    pub sel: Option<usize>,
    pub live: &'a Option<Src>,
    pub thumbs: &'a HashMap<String, Tex>,
    pub toast: &'a mut Option<(String, Instant)>,
    pub want: &'a mut Vec<usize>,
    pub close: &'a mut bool,
    pub cfg_dirty: &'a mut bool,
    pub refresh: bool,
    pub playlist_filter: Option<Option<String>>,
}

pub fn back(_p: &mut Picker) -> bool {
    false
}

const SIDE: f32 = 300.0;

/// Общий каркас: окно, левая колонка с заголовком. Возвращает (окно, колонка, содержимое).
fn shell(w: &mut W, view: (f32, f32), title: &str, sub: &str) -> (Rect, Rect, Rect) {
    let s = w.s;
    let win = w.window(view, 16.0);
    let side = Rect::new(win.x, win.y, (SIDE * s).min(win.w * 0.3), win.h);
    w.d.rect(side.inset(s), alpha(w.th.panel, 0.9));
    w.text(BOLD, 17.0, side.x + 22.0 * s, side.y + 34.0 * s, w.th.text, title);
    w.para(MEDIUM, 9.0, side.x + 22.0 * s, side.y + 52.0 * s, side.w - 44.0 * s, w.th.dim, sub);
    let body = Rect::new(side.right() + 36.0 * s, win.y, win.w - side.w - 72.0 * s, win.h);
    (win, side, body)
}

// ---------------------------------------------------------------- настройки

#[derive(Default)]
pub struct SettingsSt {
    pub section: usize,
    scroll: f32,
    new_folder: String,
}

const SECTIONS: [(&str, &str); 8] = [
    ("Обои", "Как картинка ложится на экран и как сменяется."),
    ("Видео", "Звук и экономия ресурсов для живых обоев."),
    ("Лента", "Размер, форма и поведение меню выбора."),
    ("Библиотека", "Откуда брать обои."),
    ("Автосмена", "Смена обоев по таймеру."),
    ("Загрузки", "Поиск обоев в интернете."),
    ("Клавиши", "Управление с клавиатуры и мыши."),
    ("О программе", "Версия, файлы и команды."),
];

struct Rows<'a, 'b> {
    w: &'a mut W<'b>,
    x: f32,
    y: f32,
    width: f32,
}

impl Rows<'_, '_> {
    /// Строка настройки: название и пояснение слева, место под элемент управления справа.
    fn row(&mut self, title: &str, desc: &str) -> Rect {
        let s = self.w.s;
        let ctl_w = 250.0 * s;
        let th = self.w.th;
        self.w.text(BOLD, 11.5, self.x, self.y + 13.0 * s, th.text, title);
        let bottom = self.w.para(MEDIUM, 8.8, self.x, self.y + 25.0 * s, self.width - ctl_w - 40.0 * s, th.dim, desc);
        let h = (bottom - self.y + 10.0 * s).max(50.0 * s);
        let r = Rect::new(self.x + self.width - ctl_w, self.y + 6.0 * s, ctl_w, 26.0 * s);
        self.w.d.rect(Rect::new(self.x, self.y + h - s, self.width, s.max(1.0)), alpha(th.outline, 0.45));
        self.y += h;
        r
    }
    fn toggle(&mut self, title: &str, desc: &str, v: &mut bool) -> bool {
        let r = self.row(title, desc);
        let tr = Rect::new(r.right() - 32.0 * self.w.s, r.y, 32.0 * self.w.s, r.h);
        if self.w.toggle(tr, *v) {
            *v = !*v;
            return true;
        }
        false
    }
    fn slider(&mut self, title: &str, desc: &str, key: &str, v: &mut f32, min: f32, max: f32, show: impl Fn(f32) -> String) -> bool {
        let r = self.row(title, desc);
        let s = self.w.s;
        let th = self.w.th;
        let vw = 58.0 * s;
        let shown = show(*v);
        self.w.text(BOLD, 9.6, r.right() - vw + 8.0 * s, r.cy(), th.accent, &shown);
        self.w.slider(Rect::new(r.x, r.y, r.w - vw, r.h), id(key), v, min, max)
    }
    fn cycle(&mut self, title: &str, desc: &str, label: &str) -> i32 {
        let r = self.row(title, desc);
        self.w.cycle(r, label)
    }
    fn note(&mut self, text: &str) {
        let s = self.w.s;
        let th = self.w.th;
        self.y = self.w.para(MEDIUM, 9.2, self.x, self.y + 10.0 * s, self.width, th.dim, text) + 8.0 * s;
    }
}

fn step<T: PartialEq + Clone>(list: &[T], cur: &T, dir: i32) -> T {
    let at = list.iter().position(|x| x == cur).unwrap_or(0) as i32;
    list[(at + dir).rem_euclid(list.len() as i32) as usize].clone()
}

pub fn settings(w: &mut W, st: &mut SettingsSt, c: &mut PanelCtx) {
    let s = w.s;
    let th = w.th;
    let (win, side, body) = shell(w, c.view, crate::i18n::tr("Настройки"), crate::i18n::tr("Здесь только то, что действительно работает. Изменения применяются сразу."));
    for (i, (name, _)) in SECTIONS.iter().map(|(a, b)| (crate::i18n::tr(a), crate::i18n::tr(b))).enumerate() {
        let r = Rect::new(side.x + 14.0 * s, side.y + 104.0 * s + 30.0 * s * i as f32, side.w - 28.0 * s, 27.0 * s);
        if w.nav_row(r, name, "", st.section == i) {
            st.section = i;
            st.scroll = 0.0;
            w.ui.focus = 0;
        }
    }
    st.section = st.section.min(SECTIONS.len() - 1);
    let (title, sub) = (crate::i18n::tr(SECTIONS[st.section].0), crate::i18n::tr(SECTIONS[st.section].1));
    let top = w.heading(body.x, body.y + 34.0 * s, crate::i18n::tr("НАСТРОЙКИ"), title, sub, body.w - 60.0 * s) + 8.0 * s;
    if w.close_button(win) {
        *c.close = true;
    }
    let area = Rect::new(body.x, top, body.w, win.bottom() - top - 14.0 * s);
    w.d.rect(Rect::new(area.x, area.y - s, area.w, s.max(1.0)), alpha(th.outline, 0.6));
    w.d.clip(Some(area));
    let start = area.y - st.scroll;
    let mut changed = false;
    let mut r = Rows { w, x: area.x, y: start, width: area.w - 14.0 * s };
    let cfg = &mut *c.cfg;
    match st.section {
        0 => {
            let fills = ["cover".to_string(), "contain".into(), "stretch".into()];
            let label = match cfg.wallpaper.fill.as_str() {
                "contain" => crate::i18n::tr("Вписать целиком"),
                "stretch" => crate::i18n::tr("Растянуть"),
                _ => crate::i18n::tr("Заполнить экран"),
            };
            let d = r.cycle(crate::i18n::tr("Размещение"), crate::i18n::tr("Заполнить — картинка закрывает весь экран, лишнее обрезается. Вписать — видна целиком, по краям чёрные поля."), label);
            if d != 0 {
                cfg.wallpaper.fill = step(&fills, &cfg.wallpaper.fill, d);
                changed = true;
            }
            let mut names: Vec<String> = vec!["random".into(), "none".into()];
            names.extend(fx::TRANSITIONS.iter().map(|t| t.0.to_string()));
            let d = r.cycle(crate::i18n::tr("Переход"), crate::i18n::tr("Анимация смены обоев. «Случайный» каждый раз выбирает новую."), fx::transition_label(&cfg.wallpaper.transition));
            if d != 0 {
                cfg.wallpaper.transition = step(&names, &cfg.wallpaper.transition, d);
                changed = true;
            }
            let mut v = cfg.wallpaper.duration_ms as f32;
            if r.slider(crate::i18n::tr("Длительность перехода"), crate::i18n::tr("Сколько длится смена обоев."), "dur", &mut v, 100.0, 3000.0, |v| crate::trf!("{:.1} с", "{:.1} s", v / 1000.0)) {
                cfg.wallpaper.duration_ms = (v / 50.0).round() as u32 * 50;
                changed = true;
            }
            changed |= r.toggle(crate::i18n::tr("Только монитор с меню"), crate::i18n::tr("Ставить обои на тот монитор, где открыто меню, а не на все сразу."), &mut cfg.wallpaper.per_monitor);
            changed |= r.toggle(crate::i18n::tr("Заодно с Omarchy"), crate::i18n::tr("Смена темы и команды «omarchy theme bg …» меняют обои здесь, а выбранные здесь обои видит экран блокировки."), &mut cfg.wallpaper.follow_omarchy);
        }
        1 => {
            if !crate::mpv::available() {
                r.note(crate::i18n::tr("Библиотека libmpv не найдена — видеообои показываться не будут. Установите пакет mpv."));
            }
            changed |= r.toggle(crate::i18n::tr("Без звука"), crate::i18n::tr("Звуковая дорожка не декодируется вовсе — это дешевле, чем просто убрать громкость."), &mut cfg.video.mute);
            if !cfg.video.mute {
                let mut v = cfg.video.volume as f32;
                if r.slider(crate::i18n::tr("Громкость"), crate::i18n::tr("Громкость звука видеообоев."), "vol", &mut v, 0.0, 100.0, |v| format!("{v:.0} %")) {
                    cfg.video.volume = v.round() as u32;
                    changed = true;
                }
            }
            changed |= r.toggle(crate::i18n::tr("Аппаратное декодирование"), crate::i18n::tr("Видео декодирует видеокарта. Выключайте, только если видео показывается с искажениями. Действует со следующей смены обоев."), &mut cfg.video.hwdec);
            let label = match cfg.video.saver.as_str() {
                "on" => crate::i18n::tr("Всегда"),
                "off" => crate::i18n::tr("Никогда"),
                _ => crate::i18n::tr("Само на батарее и слабых ПК"),
            };
            let d = r.cycle(crate::i18n::tr("Экономный режим"), crate::i18n::tr("Видео рисуется меньше экрана, тяжёлые переходы заменяются растворением, у меню нет теней. Заметно разгружает слабые ноутбуки."), label);
            if d != 0 {
                cfg.video.saver = step(&["auto".to_string(), "on".into(), "off".into()], &cfg.video.saver, d);
                changed = true;
            }
            if cfg.saver_now {
                r.note(crate::i18n::tr("Сейчас экономный режим включён."));
            }
            changed |= r.toggle(crate::i18n::tr("Пауза от батареи"), crate::i18n::tr("Останавливать видео, когда ноутбук работает от аккумулятора."), &mut cfg.video.pause_on_battery);
            r.note(crate::i18n::tr("Когда рабочий стол закрыт полноэкранным окном, видео останавливается само и не тратит ресурсы."));
        }
        2 => {
            let p = &mut cfg.picker;
            changed |= r.slider(crate::i18n::tr("Масштаб"), crate::i18n::tr("Общий размер меню."), "scale", &mut p.scale, 0.7, 2.2, |v| format!("{v:.2}×"));
            let mut v = p.visible as f32;
            if r.slider(crate::i18n::tr("Срезов на экране"), crate::i18n::tr("Сколько обоев видно одновременно."), "vis", &mut v, 5.0, 25.0, |v| format!("{}", v.round() as u32)) {
                p.visible = v.round() as u32;
                changed = true;
            }
            changed |= r.slider(crate::i18n::tr("Ширина выбранного"), crate::i18n::tr("Ширина раскрытой карточки."), "selw", &mut p.selected_width, 300.0, 1100.0, |v| format!("{v:.0}"));
            changed |= r.slider(crate::i18n::tr("Высота"), crate::i18n::tr("Высота ленты."), "h", &mut p.height, 180.0, 620.0, |v| format!("{v:.0}"));
            changed |= r.slider(crate::i18n::tr("Наклон"), crate::i18n::tr("Насколько срезы наклонены."), "skew", &mut p.skew, -70.0, 70.0, |v| format!("{v:.0}"));
            changed |= r.slider(crate::i18n::tr("Скругление"), crate::i18n::tr("Радиус углов карточек."), "corner", &mut p.corner, 0.0, 90.0, |v| format!("{v:.0}"));
            changed |= r.slider(crate::i18n::tr("Сдвиг по вертикали"), crate::i18n::tr("Поднять или опустить меню на экране."), "offy", &mut p.offset_y, -250.0, 250.0, |v| format!("{v:.0}"));
            changed |= r.toggle(crate::i18n::tr("Предпросмотр на столе"), crate::i18n::tr("Выбранные в ленте обои сразу показываются на рабочем столе. Закрыли меню без применения — вернутся прежние."), &mut p.live_preview);
            changed |= r.toggle(crate::i18n::tr("Оставлять обои при закрытии"), crate::i18n::tr("Закрыли меню — остаются те обои, на которых остановились, Enter не нужен. Работает вместе с предпросмотром на столе."), &mut p.apply_on_close);
            changed |= r.toggle(crate::i18n::tr("Панель фильтров"), crate::i18n::tr("Показывать панель с типами, сортировкой и цветами."), &mut p.bar);
            changed |= r.toggle(crate::i18n::tr("Метки «ФОТО» и «ВИДЕО»"), crate::i18n::tr("Подписи типа на срезах."), &mut p.badges);
            changed |= r.toggle(crate::i18n::tr("Выбор наведением"), crate::i18n::tr("Срез выбирается, когда на него наведён указатель."), &mut p.hover_select);
            changed |= r.toggle(crate::i18n::tr("Закрывать после применения"), crate::i18n::tr("Enter или щелчок по выбранной карточке ставит обои и закрывает меню."), &mut p.close_on_apply);
            changed |= r.toggle(crate::i18n::tr("Лента по кругу"), crate::i18n::tr("После последних обоев снова идут первые."), &mut p.wrap);
            changed |= r.toggle(crate::i18n::tr("Анимация ленты"), crate::i18n::tr("Плавное движение при листании."), &mut p.animations);
            let rr = r.row(crate::i18n::tr("Вид по умолчанию"), crate::i18n::tr("Вернуть размеры и форму ленты к исходным."));
            if r.w.button(Rect::new(rr.right() - 120.0 * s, rr.y, 120.0 * s, rr.h), crate::i18n::tr("Сбросить")) {
                let keep = (p.live_preview, p.close_on_apply, p.hover_select, p.apply_on_close);
                *p = Default::default();
                (p.live_preview, p.close_on_apply, p.hover_select, p.apply_on_close) = keep;
                changed = true;
            }
        }
        3 => {
            let total = c.state.items.iter().filter(|i| !i.broken).count();
            let videos = c.state.items.iter().filter(|i| i.video && !i.broken).count();
            r.note(&crate::trf!("В библиотеке {total} обоев, из них видео — {videos}. Вложенные папки читаются тоже; новые файлы подхватываются при каждом открытии меню.", "The library has {total} wallpapers, {videos} of them videos. Subfolders are read too; new files are picked up every time the menu opens."));
            let mut remove = None;
            for (i, f) in cfg.library.folders.iter().enumerate() {
                let rr = r.row(&config::shorten(f), if config::expand(f).is_dir() { crate::i18n::tr("Папка с обоями") } else { crate::i18n::tr("Папка не найдена") });
                if r.w.button_ghost(Rect::new(rr.right() - 100.0 * s, rr.y, 100.0 * s, rr.h), crate::i18n::tr("Убрать"), true) {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove {
                cfg.library.folders.remove(i);
                changed = true;
                c.actions.push(Action::Rescan);
            }
            let rr = r.row(crate::i18n::tr("Добавить папку"), crate::i18n::tr("Впишите путь и нажмите Enter. Ctrl+V вставляет из буфера обмена."));
            if r.w.text_field(rr, id("folder"), &mut st.new_folder, "~/Pictures/…") {
                let p = st.new_folder.trim().to_string();
                if !p.is_empty() && config::expand(&p).is_dir() {
                    let short = config::shorten(&config::expand(&p).to_string_lossy());
                    if !cfg.library.folders.contains(&short) {
                        cfg.library.folders.push(short);
                    }
                    st.new_folder.clear();
                    changed = true;
                    c.actions.push(Action::Rescan);
                } else {
                    *c.toast = Some((crate::i18n::tr("Такой папки нет").into(), Instant::now()));
                }
            }
            if r.toggle(crate::i18n::tr("Фоны темы Omarchy"), crate::i18n::tr("Показывать в ленте фоны текущей темы (папка «Тема Omarchy»)."), &mut cfg.library.theme_backgrounds) {
                changed = true;
                c.actions.push(Action::Rescan);
            }
            if r.toggle(crate::i18n::tr("Видео из Steam Workshop"), crate::i18n::tr("Показывать видеообои, скачанные через Wallpaper Engine в Steam."), &mut cfg.library.workshop) {
                changed = true;
                c.actions.push(Action::Rescan);
            }
            let rr = r.row(crate::i18n::tr("Перечитать библиотеку"), crate::i18n::tr("Заново обойти папки прямо сейчас."));
            if r.w.button(Rect::new(rr.right() - 140.0 * s, rr.y, 140.0 * s, rr.h), crate::i18n::tr("Пересканировать")) {
                c.actions.push(Action::Rescan);
                *c.toast = Some((crate::i18n::tr("Библиотека перечитывается").into(), Instant::now()));
            }
        }
        4 => {
            let ro = &mut cfg.rotation;
            changed |= r.toggle(crate::i18n::tr("Менять обои по таймеру"), crate::i18n::tr("Меню при этом открывать не нужно. Пока меню открыто, автосмена ждёт."), &mut ro.enabled);
            let steps = [1u32, 5, 10, 15, 30, 60, 120, 180, 360, 720, 1440];
            let label = if ro.interval_min >= 60 && ro.interval_min % 60 == 0 { crate::trf!("{} ч", "{} h", ro.interval_min / 60) } else { crate::trf!("{} мин", "{} min", ro.interval_min) };
            let d = r.cycle(crate::i18n::tr("Как часто"), crate::i18n::tr("Промежуток между сменами."), &label);
            if d != 0 {
                let at = steps.iter().position(|x| *x >= ro.interval_min).unwrap_or(4) as i32;
                ro.interval_min = steps[(at + d).clamp(0, steps.len() as i32 - 1) as usize];
                changed = true;
            }
            changed |= r.toggle(crate::i18n::tr("В случайном порядке"), crate::i18n::tr("Иначе обои идут по порядку списка."), &mut ro.shuffle);
            let mut sources: Vec<String> = vec!["all".into(), "favourites".into()];
            sources.extend(c.state.playlists.iter().map(|p| p.name.clone()));
            let label = match ro.source.as_str() {
                "all" => crate::i18n::tr("Вся библиотека").to_string(),
                "favourites" => crate::i18n::tr("Избранное").to_string(),
                n => crate::trf!("Плейлист «{n}»", "Playlist “{n}”"),
            };
            let d = r.cycle(crate::i18n::tr("Откуда брать"), crate::i18n::tr("Вся библиотека, избранное или плейлист (плейлисты собираются кнопкой P)."), &label);
            if d != 0 {
                ro.source = step(&sources, &ro.source, d);
                changed = true;
            }
            r.note(crate::i18n::tr("Сменить обои вручную можно командами «omawall next», «omawall prev» и «omawall random» — их удобно повесить на клавиши."));
        }
        5 => {
            let so = &mut cfg.sources;
            let rr = r.row(crate::i18n::tr("Папка для скачанного"), crate::i18n::tr("Имя папки внутри библиотеки или полный путь."));
            if r.w.text_field(rr, id("dldir"), &mut so.download_dir, "Wallhaven") {
                r.w.ui.focus = 0;
                changed = true;
            }
            changed |= r.toggle(crate::i18n::tr("Wallhaven: общее"), crate::i18n::tr("Пейзажи, абстракция, техника."), &mut so.wallhaven_general);
            changed |= r.toggle(crate::i18n::tr("Wallhaven: аниме"), crate::i18n::tr("Рисованные обои."), &mut so.wallhaven_anime);
            changed |= r.toggle(crate::i18n::tr("Wallhaven: люди"), crate::i18n::tr("Фотографии людей."), &mut so.wallhaven_people);
            let sizes = ["".to_string(), "1920x1080".into(), "2560x1440".into(), "3840x2160".into()];
            let label = if so.wallhaven_min_res.is_empty() { crate::i18n::tr("Любое").to_string() } else { crate::trf!("от {}", "from {}", so.wallhaven_min_res.replace('x', " × ")) };
            let d = r.cycle(crate::i18n::tr("Разрешение"), crate::i18n::tr("Не показывать картинки меньше этого размера."), &label);
            if d != 0 {
                so.wallhaven_min_res = step(&sizes, &so.wallhaven_min_res, d);
                changed = true;
            }
            r.note(crate::i18n::tr("Поиск идёт только по безопасному содержимому (SFW). Сайту отправляется лишь сам поисковый запрос."));
        }
        6 => {
            let keys = [
                ("← →", crate::i18n::tr("Листать ленту (с Shift — по пять)")),
                ("Enter", crate::i18n::tr("Поставить выбранные обои")),
                ("Esc", crate::i18n::tr("Закрыть меню; без применения вернутся прежние обои (или останутся новые — см. «Оставлять обои при закрытии»)")),
                (crate::i18n::tr("Пробел, ↑ ↓"), crate::i18n::tr("Перевернуть карточку: сведения, теги, действия")),
                ("F", crate::i18n::tr("В избранное и обратно")),
                ("H", crate::i18n::tr("Скрыть из ленты и вернуть")),
                ("R", crate::i18n::tr("Случайные обои")),
                (crate::i18n::tr("T или /"), crate::i18n::tr("Поиск и теги")),
                ("1 2 3, Tab", crate::i18n::tr("Все, только фото, только видео")),
                ("O", crate::i18n::tr("Форма: все, широкие, высокие")),
                ("E", crate::i18n::tr("Эффекты для выбранных обоев")),
                ("P", crate::i18n::tr("Плейлисты и автосмена")),
                ("D", crate::i18n::tr("Скачать обои")),
                ("C", crate::i18n::tr("Дизайнер тем")),
                ("S", crate::i18n::tr("Настройки")),
                ("B", crate::i18n::tr("Спрятать и показать панель фильтров")),
                ("V", crate::i18n::tr("Спрятать меню, чтобы рассмотреть обои")),
                ("Ctrl+C", crate::i18n::tr("Скопировать путь к файлу")),
                ("Delete", crate::i18n::tr("Удалить файл (с подтверждением)")),
                ("Home, End, PgUp, PgDn", crate::i18n::tr("В начало, в конец, на полэкрана")),
                (crate::i18n::tr("Мышь"), crate::i18n::tr("Щелчок выбирает, щелчок по выбранной — применяет, правая кнопка переворачивает, колесо листает, щелчок мимо закрывает")),
                (crate::i18n::tr("Рабочий стол"), crate::i18n::tr("Двойной щелчок по обоям открывает меню")),
            ];
            for (k, what) in keys {
                let y = r.y + 15.0 * s;
                r.w.text(BOLD, 10.0, r.x, y, th.accent, k);
                let b = r.w.para(MEDIUM, 9.6, r.x + 190.0 * s, r.y + 5.0 * s, r.width - 200.0 * s, th.text, what);
                r.y = b.max(r.y + 26.0 * s) + 3.0 * s;
            }
        }
        _ => {
            let rss = std::fs::read_to_string("/proc/self/statm").ok().and_then(|t| t.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok())).unwrap_or(0) * 4096;
            let lines = [
                (crate::i18n::tr("Версия"), format!("omawall {}", env!("CARGO_PKG_VERSION"))),
                (crate::i18n::tr("Память демона"), crate::picker::human_size(rss)),
                (crate::i18n::tr("Настройки"), config::shorten(&Config::path().to_string_lossy())),
                (crate::i18n::tr("Библиотека и отметки"), config::shorten(&State::path().to_string_lossy())),
                (crate::i18n::tr("Миниатюры"), config::shorten(&config::cache_dir().join("thumbs").to_string_lossy())),
                (crate::i18n::tr("Видео"), if crate::mpv::available() { crate::i18n::tr("libmpv найдена").into() } else { crate::i18n::tr("libmpv не найдена").into() }),
            ];
            for (k, v) in lines {
                let y = r.y + 15.0 * s;
                r.w.text(BOLD, 10.0, r.x, y, th.dim, k);
                r.w.text(BOLD, 10.0, r.x + 190.0 * s, y, th.text, &v);
                r.y += 28.0 * s;
            }
            r.note(crate::i18n::tr("Команды: omawall toggle — открыть или закрыть меню; apply <файл> [-o монитор]; next, prev, random; pause, resume; rescan; reload — перечитать config.toml; status."));
        }
    }
    let content_h = r.y - start + 20.0 * s;
    w.d.clip(None);
    w.scroll(area, content_h, &mut st.scroll);
    if changed {
        *c.cfg_dirty = true;
    }
}

// ---------------------------------------------------------------- дизайнер тем

#[derive(Default)]
pub struct ThemeSt {
    pub role: usize,
    hex: String,
    hex_for: Option<(usize, String)>,
    name: String,
    preset: usize,
}

pub fn theme(w: &mut W, st: &mut ThemeSt, c: &mut PanelCtx) {
    let s = w.s;
    let (win, side, body) = shell(w, c.view, crate::i18n::tr("Дизайнер тем"), crate::i18n::tr("Девять цветов, из которых собрано меню. Щёлкните цвет, чтобы изменить его."));
    let th = w.th;
    let omarchy = c.cfg.theme.source == "omarchy";
    let mut changed = false;
    st.role = st.role.min(8);
    for (i, name) in Palette::ROLES.iter().map(|r| crate::i18n::tr(r.0)).enumerate() {
        let r = Rect::new(side.x + 14.0 * s, side.y + 104.0 * s + 32.0 * s * i as f32, side.w - 28.0 * s, 29.0 * s);
        let on = st.role == i;
        let hot = w.ui.hover(r);
        if on || hot {
            w.d.rect(r, if on { mix(th.panel, th.accent, 0.38) } else { mix(th.panel, th.text, 0.07) });
        }
        let col = parse_hex(c.cfg.theme.palette.get(i)).unwrap_or(hex(0));
        let live = [th.accent, th.on_accent, th.accent2, th.panel, th.text, th.alt, th.raised, th.backdrop, th.outline][i];
        w.d.shape(&Shape::new(Rect::new(r.x + 9.0 * s, r.y + 6.0 * s, 26.0 * s, 17.0 * s), if omarchy { live } else { col }).border(s.max(1.0), th.outline));
        w.text(BOLD, 10.0, r.x + 44.0 * s, r.cy(), th.text, name);
        let code = if omarchy { to_hex(live) } else { c.cfg.theme.palette.get(i).to_string() };
        let cw = w.text_w(BOLD, 8.0, &code);
        w.text(BOLD, 8.0, r.right() - cw - 8.0 * s, r.cy(), th.dim, &code);
        if hot && w.ui.press {
            st.role = i;
            w.ui.focus = 0;
        }
    }
    let top = w.heading(body.x, body.y + 34.0 * s, crate::i18n::tr("ОФОРМЛЕНИЕ МЕНЮ"), crate::i18n::tr("Палитра"), crate::i18n::tr("Меняется только вид этого меню. Тему всей системы по-прежнему задаёт Omarchy."), body.w - 60.0 * s) + 10.0 * s;
    if w.close_button(win) {
        *c.close = true;
    }

    // Источник цветов.
    let bw = 170.0 * s;
    if w.button_on(Rect::new(body.x, top, bw, 26.0 * s), crate::i18n::tr("Своя палитра"), !omarchy) && omarchy {
        c.cfg.theme.source = "palette".into();
        changed = true;
    }
    if w.button_on(Rect::new(body.x + bw + 6.0 * s, top, bw, 26.0 * s), crate::i18n::tr("Как тема Omarchy"), omarchy) && !omarchy {
        c.cfg.theme.source = "omarchy".into();
        changed = true;
    }
    let y = top + 44.0 * s;
    if omarchy {
        w.para(MEDIUM, 10.0, body.x, y, body.w * 0.6, th.dim, crate::i18n::tr("Цвета берутся из текущей темы Omarchy и меняются вместе с ней. Чтобы настроить цвета вручную, выберите «Своя палитра»."));
    } else {
        // Цветовой круг: кольцо оттенков и поле насыщенности и яркости.
        let (role_name, role_desc) = (crate::i18n::tr(Palette::ROLES[st.role].0), crate::i18n::tr(Palette::ROLES[st.role].1));
        w.text(BOLD, 13.0, body.x, y + 8.0 * s, th.text, role_name);
        w.text(MEDIUM, 9.0, body.x, y + 26.0 * s, th.dim, role_desc);
        let cur = parse_hex(c.cfg.theme.palette.get(st.role)).unwrap_or(hex(0x888888));
        let (mut h, mut sv, mut v) = to_hsv(cur);
        let dsz = (body.h - (y - body.y) - 200.0 * s).clamp(150.0 * s, 250.0 * s);
        let ring = Rect::new(body.x, y + 44.0 * s, dsz, dsz);
        w.d.shape(&Shape::new(ring, [0.0; 4]).mode(2));
        let sq = dsz * 0.5;
        let field = Rect::new(ring.cx() - sq / 2.0, ring.cy() - sq / 2.0, sq, sq);
        let mut sh = Shape::new(field, [h / 360.0, 0.0, 0.0, 1.0]);
        sh.mode = 3;
        w.d.shape(&sh);
        let (mx, my) = w.ui.mouse;
        let (dx, dy) = (mx - ring.cx(), my - ring.cy());
        let dist = (dx * dx + dy * dy).sqrt() / (dsz / 2.0);
        if w.ui.press {
            if field.has(w.ui.mouse) {
                w.ui.active = id("sv");
            } else if (0.74..=1.04).contains(&dist) {
                w.ui.active = id("hue");
            }
        }
        let mut picked = false;
        if w.ui.down && w.ui.active == id("hue") {
            h = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            if sv < 0.02 {
                sv = 0.5;
            }
            if v < 0.02 {
                v = 0.7;
            }
            picked = true;
        } else if w.ui.down && w.ui.active == id("sv") {
            sv = ((mx - field.x) / sq).clamp(0.0, 1.0);
            v = 1.0 - ((my - field.y) / sq).clamp(0.0, 1.0);
            picked = true;
        }
        if picked {
            c.cfg.theme.palette.set(st.role, to_hex(hsv(h, sv, v)));
            c.cfg.theme.palette.name = crate::i18n::tr("Своя").into();
            changed = true;
        }
        let ang = h.to_radians();
        let rr = dsz / 2.0 * 0.89;
        let knob = |w: &mut W, x: f32, y: f32, col| {
            w.d.shape(&Shape::new(Rect::new(x - 7.0 * s, y - 7.0 * s, 14.0 * s, 14.0 * s), col).radius(7.0 * s).border(2.0 * s, [1.0; 4]));
        };
        knob(w, ring.cx() + ang.cos() * rr, ring.cy() + ang.sin() * rr, hsv(h, 1.0, 1.0));
        knob(w, field.x + sv * sq, field.y + (1.0 - v) * sq, hsv(h, sv, v));

        // Точный код цвета.
        let cur_hex = c.cfg.theme.palette.get(st.role).to_string();
        if st.hex_for.as_ref() != Some(&(st.role, cur_hex.clone())) && w.ui.focus != id("hex") {
            st.hex = cur_hex.clone();
            st.hex_for = Some((st.role, cur_hex.clone()));
        }
        let fy = ring.bottom() + 16.0 * s;
        w.d.shape(&Shape::new(Rect::new(body.x, fy, 40.0 * s, 28.0 * s), hsv(h, sv, v)).border(s.max(1.0), th.outline));
        if w.text_field(Rect::new(body.x + 46.0 * s, fy, 120.0 * s, 28.0 * s), id("hex"), &mut st.hex, "#88c0d0") {
            if let Some(col) = parse_hex(&st.hex) {
                c.cfg.theme.palette.set(st.role, to_hex(col));
                c.cfg.theme.palette.name = crate::i18n::tr("Своя").into();
                changed = true;
            } else {
                *c.toast = Some((crate::i18n::tr("Нужен код вида #88c0d0").into(), Instant::now()));
            }
            w.ui.focus = 0;
        }
        w.text(MEDIUM, 8.8, body.x + 176.0 * s, fy + 14.0 * s, th.dim, crate::i18n::tr("Код цвета, Enter — применить"));

        // Правая колонка: готовые и сохранённые палитры.
        let cx = body.x + (dsz + 60.0 * s).max(body.w * 0.46);
        let cw = body.right() - cx - 20.0 * s;
        w.text(BOLD, 13.0, cx, y + 8.0 * s, th.text, crate::i18n::tr("Готовые палитры"));
        let presets = Palette::presets();
        st.preset = presets.iter().position(|p| p.name == c.cfg.theme.palette.name).unwrap_or(st.preset.min(presets.len() - 1));
        let mut py = y + 30.0 * s;
        let swatch = |w: &mut W, p: &Palette, r: Rect| {
            let n = 9.0;
            for i in 0..9 {
                let col = parse_hex(p.get(i)).unwrap_or(hex(0));
                w.d.rect(Rect::new(r.x + r.w / n * i as f32, r.y, r.w / n + 0.5, r.h), col);
            }
        };
        let col_w = (cw - 8.0 * s) / 2.0;
        for (i, p) in presets.iter().enumerate() {
            let r = Rect::new(cx + (col_w + 8.0 * s) * (i % 2) as f32, py + 34.0 * s * (i / 2) as f32, col_w, 28.0 * s);
            let on = *p == c.cfg.theme.palette;
            let hot = w.ui.hover(r);
            w.d.shape(&Shape::new(r, if hot { th.raised } else { th.alt }).border(s.max(1.0), if on { th.accent } else { th.outline }));
            w.text(BOLD, 9.6, r.x + 8.0 * s, r.cy(), th.text, &crate::i18n::tr_dyn(&p.name));
            swatch(w, p, Rect::new(r.right() - 96.0 * s, r.y + 8.0 * s, 88.0 * s, 12.0 * s));
            if hot && w.ui.press {
                c.cfg.theme.palette = p.clone();
                changed = true;
            }
        }
        py += 34.0 * s * presets.len().div_ceil(2) as f32 + 18.0 * s;
        w.text(BOLD, 13.0, cx, py + 6.0 * s, th.text, crate::i18n::tr("Свои палитры"));
        py += 24.0 * s;
        if w.text_field(Rect::new(cx, py, cw - 126.0 * s, 26.0 * s), id("palname"), &mut st.name, crate::i18n::tr("Название палитры")) | w.button(Rect::new(cx + cw - 120.0 * s, py, 120.0 * s, 26.0 * s), crate::i18n::tr("Сохранить")) {
            let name = st.name.trim().to_string();
            if name.is_empty() {
                *c.toast = Some((crate::i18n::tr("Сначала назовите палитру").into(), Instant::now()));
            } else {
                let mut p = c.cfg.theme.palette.clone();
                p.name = name.clone();
                c.cfg.theme.saved.retain(|x| x.name != name);
                c.cfg.theme.saved.push(p.clone());
                c.cfg.theme.palette = p;
                st.name.clear();
                w.ui.focus = 0;
                changed = true;
            }
        }
        py += 34.0 * s;
        let mut del = None;
        for (i, p) in c.cfg.theme.saved.clone().iter().enumerate() {
            let r = Rect::new(cx, py, cw - 34.0 * s, 28.0 * s);
            if r.bottom() > win.bottom() - 16.0 * s {
                break;
            }
            let on = *p == c.cfg.theme.palette;
            let hot = w.ui.hover(r);
            w.d.shape(&Shape::new(r, if hot { th.raised } else { th.alt }).border(s.max(1.0), if on { th.accent } else { th.outline }));
            let label = w.d.fit(BOLD, 9.6 * s, &crate::i18n::tr_dyn(&p.name), r.w - 120.0 * s);
            w.text(BOLD, 9.6, r.x + 8.0 * s, r.cy(), th.text, &label);
            swatch(w, p, Rect::new(r.right() - 96.0 * s, r.y + 8.0 * s, 88.0 * s, 12.0 * s));
            if hot && w.ui.press {
                c.cfg.theme.palette = p.clone();
                changed = true;
            }
            if w.icon_button(Rect::new(r.right() + 6.0 * s, py, 28.0 * s, 28.0 * s), icons::DELETE, false, crate::i18n::tr("Удалить палитру")) {
                del = Some(i);
            }
            py += 34.0 * s;
        }
        if let Some(i) = del {
            c.cfg.theme.saved.remove(i);
            changed = true;
        }
        if c.cfg.theme.saved.is_empty() {
            w.text(MEDIUM, 9.2, cx, py + 8.0 * s, th.dim, crate::i18n::tr("Сохранённых палитр пока нет."));
        }
    }
    if changed {
        *c.cfg_dirty = true;
        w.ui.again = true;
    }
}

// ---------------------------------------------------------------- эффекты

#[derive(Default)]
pub struct EffectsSt {
    pub selected: usize,
    scroll: f32,
}

const FX_COLORS: [u32; 16] = [0x11131f, 0x2e3440, 0x5e81ac, 0x88c0d0, 0x8fbcbb, 0xa3be8c, 0xebcb8b, 0xd08770, 0xbf616a, 0xb48ead, 0xf5c2e7, 0xf2ccb0, 0xeceff4, 0xffffff, 0x7aa2f7, 0x1a1b26];

pub fn effects(w: &mut W, st: &mut EffectsSt, c: &mut PanelCtx) {
    let s = w.s;
    let th = w.th;
    let (win, side, body) = shell(w, c.view, crate::i18n::tr("Эффекты"), crate::i18n::tr("Фильтры для выбранных обоев. Добавляйте несколько — они применяются по очереди и запоминаются для этих обоев."));
    st.selected = st.selected.min(fx::FX.len() - 1);
    let Some(i) = c.sel else {
        w.text(BOLD, 12.0, body.x, body.y + 60.0 * s, th.text, crate::i18n::tr("Сначала выберите обои в ленте."));
        if w.close_button(win) {
            *c.close = true;
        }
        return;
    };
    // Список эффектов по группам.
    let list = Rect::new(side.x + 8.0 * s, side.y + 108.0 * s, side.w - 16.0 * s, side.h - 118.0 * s);
    w.d.clip(Some(list));
    let mut y = list.y - st.scroll;
    let chain_ids: Vec<String> = c.state.items[i].fx.iter().map(|f| f.id.clone()).collect();
    for g in fx::GROUPS {
        w.text(BOLD, 8.0, list.x + 10.0 * s, y + 12.0 * s, th.accent, &crate::i18n::tr(g).to_uppercase());
        y += 24.0 * s;
        for (n, f) in fx::FX.iter().enumerate().filter(|(_, f)| f.group == g) {
            let r = Rect::new(list.x + 4.0 * s, y, list.w - 12.0 * s, 25.0 * s);
            let mark = if chain_ids.iter().any(|x| x == f.id) { crate::i18n::tr("вкл") } else { "" };
            if w.nav_row(r, crate::i18n::tr(f.name), mark, st.selected == n) && list.has(w.ui.mouse) {
                st.selected = n;
            }
            y += 27.0 * s;
        }
        y += 8.0 * s;
    }
    let content = y + st.scroll - list.y;
    w.d.clip(None);
    w.scroll(list, content, &mut st.scroll);

    let def = &fx::FX[st.selected];
    let top = w.heading(body.x, body.y + 34.0 * s, &crate::i18n::tr(def.group).to_uppercase(), crate::i18n::tr(def.name), crate::i18n::tr(def.desc), body.w - 60.0 * s) + 6.0 * s;
    if w.close_button(win) {
        *c.close = true;
    }
    let path = c.state.items[i].path.clone();
    let mut changed = false;
    let at = c.state.items[i].fx.iter().position(|f| f.id == def.id);

    // Кнопки цепочки.
    let bh = 26.0 * s;
    let mut bx = body.x;
    if at.is_some() {
        if w.button_on(Rect::new(bx, top, 150.0 * s, bh), crate::i18n::tr("Убрать эффект"), true) {
            c.state.items[i].fx.retain(|f| f.id != def.id);
            changed = true;
        }
    } else if w.button(Rect::new(bx, top, 150.0 * s, bh), crate::i18n::tr("Добавить эффект")) {
        c.state.items[i].fx.push(fx::instance(def));
        changed = true;
    }
    bx += 156.0 * s;
    if w.button_ghost(Rect::new(bx, top, 150.0 * s, bh), crate::i18n::tr("Сбросить все"), !c.state.items[i].fx.is_empty()) && !c.state.items[i].fx.is_empty() {
        c.state.items[i].fx.clear();
        changed = true;
    }
    bx += 166.0 * s;
    let chain: Vec<&str> = c.state.items[i].fx.iter().filter_map(|f| fx::def(&f.id).map(|d| crate::i18n::tr(d.name))).collect();
    let line = if chain.is_empty() { crate::i18n::tr("Эффектов нет").to_string() } else { crate::trf!("Цепочка: {}", "Chain: {}", chain.join(" → ")) };
    let line = w.d.fit(MEDIUM, 9.6 * s, &line, body.right() - bx - 50.0 * s);
    w.text(MEDIUM, 9.6, bx, top + bh / 2.0, th.dim, &line);

    // Параметры выбранного эффекта.
    let at = c.state.items[i].fx.iter().position(|f| f.id == def.id);
    let params_h = if at.is_some() { (def.params.len() as f32 * 34.0 + def.colors.len() as f32 * 40.0 + 20.0) * s } else { 0.0 };
    let prev = Rect::new(body.x, top + bh + 14.0 * s, body.w - 14.0 * s, win.bottom() - top - bh - 30.0 * s - params_h);
    let live = c.live.as_ref().filter(|l| l.borrow().path == path).map(|l| l.borrow());
    let tex = live.as_ref().and_then(|l| l.tex()).or_else(|| c.thumbs.get(&path));
    w.d.rect(prev, th.alt);
    if let Some(t) = tex {
        // Картинка вписывается целиком.
        let k = (prev.w / t.w as f32).min(prev.h / t.h as f32);
        let r = Rect::new(prev.cx() - t.w as f32 * k / 2.0, prev.cy() - t.h as f32 * k / 2.0, t.w as f32 * k, t.h as f32 * k);
        w.d.shape(&Shape::new(r, [1.0; 4]).stretch(t));
    }
    if live.is_none() {
        let note = crate::i18n::tr("Результат виден на рабочем столе, когда включён предпросмотр или обои применены.");
        let nw = w.text_w(BOLD, 9.0, note) + 20.0 * s;
        let r = Rect::new(prev.x + 10.0 * s, prev.bottom() - 34.0 * s, nw, 24.0 * s);
        w.d.rect(r, alpha(th.backdrop, 0.9));
        w.text(BOLD, 9.0, r.x + 10.0 * s, r.cy(), th.text, note);
    }
    drop(live);
    if let Some(at) = at {
        let mut y = prev.bottom() + 14.0 * s;
        let mut f = c.state.items[i].fx[at].clone();
        for (n, p) in def.params.iter().enumerate().take(4) {
            w.text(BOLD, 10.0, body.x, y + 13.0 * s, th.text, crate::i18n::tr(p.name));
            let ctl = Rect::new(body.x + 200.0 * s, y, (body.w - 300.0 * s).min(520.0 * s), 26.0 * s);
            if def.id == "palette" && n == 0 {
                let count = fx::PALETTES.len() as i32 + 1;
                let idx = (f.p[0].round() as i32).clamp(0, count - 1);
                let label = if (idx as usize) < fx::PALETTES.len() { crate::i18n::tr(fx::PALETTES[idx as usize].0) } else { crate::i18n::tr("Тема Omarchy") };
                let d = w.cycle(Rect::new(ctl.x, ctl.y, 260.0 * s, ctl.h), label);
                if d != 0 {
                    f.p[0] = (idx + d).rem_euclid(count) as f32;
                }
            } else {
                let mut v = f.p[n];
                if w.slider(ctl, id(&format!("fx{n}")), &mut v, p.min, p.max) {
                    f.p[n] = if p.max - p.min >= 12.0 { v.round() } else { v };
                }
                let show = if p.max - p.min >= 12.0 { format!("{:.0}", f.p[n]) } else { format!("{:.2}", f.p[n]) };
                w.text(BOLD, 9.6, ctl.right() + 14.0 * s, ctl.cy(), th.accent, &show);
            }
            y += 34.0 * s;
        }
        for (n, (name, _)) in def.colors.iter().enumerate().take(2) {
            w.text(BOLD, 10.0, body.x, y + 13.0 * s, th.text, name);
            for (k, col) in FX_COLORS.iter().enumerate() {
                let cc = hex(*col);
                let r = Rect::new(body.x + 200.0 * s + 30.0 * s * k as f32, y + 2.0 * s, 26.0 * s, 22.0 * s);
                let on = (f.c[n][0] - cc[0]).abs() + (f.c[n][1] - cc[1]).abs() + (f.c[n][2] - cc[2]).abs() < 0.02;
                w.d.shape(&Shape::new(r, cc).border(if on { 2.0 * s } else { s.max(1.0) }, if on { th.accent } else { th.outline }));
                if w.ui.clicked(r) {
                    f.c[n] = [cc[0], cc[1], cc[2]];
                }
            }
            y += 40.0 * s;
        }
        if f != c.state.items[i].fx[at] {
            c.state.items[i].fx[at] = f;
            changed = true;
        }
    }
    if changed {
        c.actions.push(Action::FxChanged(path));
        w.ui.again = true;
    }
}

// ---------------------------------------------------------------- плейлисты

#[derive(Default)]
pub struct PlaylistsSt {
    selected: usize,
    new_name: String,
    scroll: f32,
    confirm_delete: bool,
}

pub fn playlists(w: &mut W, st: &mut PlaylistsSt, c: &mut PanelCtx) {
    let s = w.s;
    let th = w.th;
    let (win, side, body) = shell(w, c.view, crate::i18n::tr("Плейлисты"), crate::i18n::tr("Свои наборы обоев. Плейлист можно показать в ленте или пустить в автосмену."));
    let mut y = side.y + 104.0 * s;
    st.selected = st.selected.min(c.state.playlists.len().saturating_sub(1));
    if c.state.playlists.is_empty() {
        w.text(BOLD, 10.5, side.x + 22.0 * s, y + 10.0 * s, th.text, crate::i18n::tr("Плейлистов пока нет"));
        w.text(MEDIUM, 9.0, side.x + 22.0 * s, y + 28.0 * s, th.dim, crate::i18n::tr("Создайте первый ниже."));
    }
    for (i, p) in c.state.playlists.iter().enumerate() {
        let r = Rect::new(side.x + 14.0 * s, y, side.w - 28.0 * s, 27.0 * s);
        let auto = c.cfg.rotation.enabled && c.cfg.rotation.source == p.name;
        let right = if auto { crate::trf!("авто · {}", "auto · {}", p.items.len()) } else { p.items.len().to_string() };
        if w.nav_row(r, &p.name, &right, st.selected == i) {
            st.selected = i;
            st.scroll = 0.0;
            st.confirm_delete = false;
        }
        y += 30.0 * s;
    }
    let ny = side.bottom() - 52.0 * s;
    w.text(BOLD, 8.0, side.x + 22.0 * s, ny - 14.0 * s, th.accent, crate::i18n::tr("НОВЫЙ ПЛЕЙЛИСТ"));
    let add = w.text_field(Rect::new(side.x + 22.0 * s, ny, side.w - 82.0 * s, 28.0 * s), id("plname"), &mut st.new_name, crate::i18n::tr("Название")) | w.icon_button(Rect::new(side.right() - 54.0 * s, ny, 32.0 * s, 28.0 * s), icons::PLUS, false, crate::i18n::tr("Создать плейлист"));
    if add {
        let name = st.new_name.trim().to_string();
        if name.is_empty() || name == "all" || name == "favourites" || c.state.playlists.iter().any(|p| p.name == name) {
            *c.toast = Some((if name.is_empty() { crate::i18n::tr("Введите название") } else { crate::i18n::tr("Такое название уже занято") }.into(), Instant::now()));
        } else {
            c.state.playlists.push(library::Playlist { name, items: vec![] });
            st.selected = c.state.playlists.len() - 1;
            st.new_name.clear();
            w.ui.focus = 0;
            c.actions.push(Action::SaveState);
        }
    }
    if w.close_button(win) {
        *c.close = true;
    }
    if c.state.playlists.is_empty() {
        w.heading(body.x, body.y + 34.0 * s, crate::i18n::tr("ПЛЕЙЛИСТЫ"), crate::i18n::tr("Соберите набор"), crate::i18n::tr("Назовите плейлист слева, затем добавляйте в него обои: кнопкой здесь или кнопкой «Плейлист» на обороте карточки."), body.w - 60.0 * s);
        return;
    }
    let pi = st.selected;
    let name = c.state.playlists[pi].name.clone();
    let sub = crate::trf!("Обоев в плейлисте: {}", "Wallpapers in the playlist: {}", c.state.playlists[pi].items.len());
    let top = w.heading(body.x, body.y + 34.0 * s, crate::i18n::tr("ПЛЕЙЛИСТ"), &name, &sub, body.w - 60.0 * s) + 6.0 * s;
    let bh = 26.0 * s;
    let mut bx = body.x;
    let mut btn = |w: &mut W, label: &str, width: f32, on: Option<bool>| -> bool {
        let r = Rect::new(bx, top, width * s, bh);
        bx += (width + 6.0) * s;
        match on {
            Some(v) => w.button_on(r, label, v),
            None => w.button(r, label),
        }
    };
    if btn(w, crate::i18n::tr("Показать в ленте"), 150.0, None) {
        c.playlist_filter = Some(Some(name.clone()));
        *c.close = true;
    }
    let sel_path = c.sel.map(|i| c.state.items[i].path.clone());
    let has_sel = sel_path.as_ref().is_some_and(|p| c.state.playlists[pi].items.contains(p));
    if btn(w, if has_sel { crate::i18n::tr("Убрать выбранные обои") } else { crate::i18n::tr("Добавить выбранные обои") }, 190.0, Some(has_sel)) {
        if let Some(p) = sel_path {
            let pl = &mut c.state.playlists[pi];
            if has_sel {
                pl.items.retain(|x| x != &p);
            } else {
                pl.items.push(p);
            }
            c.actions.push(Action::SaveState);
            c.actions.push(Action::Rotation);
        }
    }
    let auto = c.cfg.rotation.enabled && c.cfg.rotation.source == name;
    if btn(w, if auto { crate::i18n::tr("Автосмена включена") } else { crate::i18n::tr("Включить автосмену") }, 170.0, Some(auto)) {
        if auto {
            c.cfg.rotation.enabled = false;
        } else {
            c.cfg.rotation.enabled = true;
            c.cfg.rotation.source = name.clone();
        }
        *c.cfg_dirty = true;
    }
    if st.confirm_delete {
        if w.button_danger(Rect::new(bx, top, 170.0 * s, bh), crate::i18n::tr("Точно удалить?")) {
            c.state.playlists.remove(pi);
            if c.cfg.rotation.source == name {
                c.cfg.rotation.source = "all".into();
                c.cfg.rotation.enabled = false;
                *c.cfg_dirty = true;
            }
            c.playlist_filter = Some(None);
            st.confirm_delete = false;
            c.actions.push(Action::SaveState);
            return;
        }
    } else if w.button_ghost(Rect::new(bx, top, 170.0 * s, bh), crate::i18n::tr("Удалить плейлист"), true) {
        st.confirm_delete = true;
    }
    if auto {
        let label = if c.cfg.rotation.interval_min >= 60 && c.cfg.rotation.interval_min % 60 == 0 { crate::trf!("каждые {} ч", "every {} h", c.cfg.rotation.interval_min / 60) } else { crate::trf!("каждые {} мин", "every {} min", c.cfg.rotation.interval_min) };
        let note = crate::trf!("Обои меняются {label}{}. Интервал и порядок — в настройках, раздел «Автосмена».", "The wallpaper changes {label}{}. Interval and order are in Settings, “Rotation”.", if c.cfg.rotation.shuffle { crate::i18n::tr(", в случайном порядке") } else { "" });
        w.text(MEDIUM, 9.2, body.x, top + bh + 16.0 * s, th.dim, &note);
    }

    // Сетка обоев плейлиста.
    let area = Rect::new(body.x, top + bh + 34.0 * s, body.w, win.bottom() - top - bh - 48.0 * s);
    let cols = ((area.w / (210.0 * s)).floor() as usize).max(2);
    let gap = 8.0 * s;
    let cw = (area.w - 14.0 * s - gap * (cols as f32 - 1.0)) / cols as f32;
    let chh = cw * 9.0 / 16.0;
    let items = c.state.playlists[pi].items.clone();
    if items.is_empty() {
        w.text(MEDIUM, 10.0, area.x, area.y + 12.0 * s, th.dim, crate::i18n::tr("Пусто. Выберите обои в ленте и нажмите «Добавить выбранные обои»."));
    }
    w.d.clip(Some(area));
    let mut remove = None;
    for (n, path) in items.iter().enumerate() {
        let r = Rect::new(area.x + (cw + gap) * (n % cols) as f32, area.y + (chh + gap) * (n / cols) as f32 - st.scroll, cw, chh);
        if r.bottom() < area.y || r.y > area.bottom() {
            continue;
        }
        let idx = c.state.find(path);
        match c.thumbs.get(path) {
            Some(t) => w.d.shape(&Shape::new(r, [1.0; 4]).cover(t, r)),
            None => {
                w.d.rect(r, th.alt);
                if let Some(i) = idx {
                    c.want.push(i);
                }
            }
        }
        let label = idx.map(|i| c.state.items[i].name.clone()).unwrap_or_else(|| crate::i18n::tr("файл не найден").into());
        if w.ui.hover(r) && area.has(w.ui.mouse) {
            w.d.rect(Rect::new(r.x, r.bottom() - 22.0 * s, r.w, 22.0 * s), alpha(th.backdrop, 0.86));
            let label = w.d.fit(BOLD, 9.0 * s, &label, r.w - 16.0 * s);
            w.text(BOLD, 9.0, r.x + 8.0 * s, r.bottom() - 11.0 * s, th.text, &label);
            if w.icon_button(Rect::new(r.right() - 26.0 * s, r.y + 4.0 * s, 22.0 * s, 22.0 * s), icons::CLOSE, false, crate::i18n::tr("Убрать из плейлиста")) {
                remove = Some(n);
            }
        }
    }
    w.d.clip(None);
    let rows = items.len().div_ceil(cols) as f32;
    w.scroll(area, rows * (chh + gap), &mut st.scroll);
    if let Some(n) = remove {
        c.state.playlists[pi].items.remove(n);
        c.actions.push(Action::SaveState);
        c.actions.push(Action::Rotation);
        c.refresh = true;
    }
}

// ---------------------------------------------------------------- загрузки

#[derive(Clone)]
struct Remote {
    id: String,
    thumb: String,
    full: String,
    info: String,
    title: String,
}

#[derive(Default)]
pub struct DownloadsSt {
    source: usize,
    query: String,
    sort: usize,
    page: u32,
    last_page: u32,
    items: Vec<Remote>,
    loading: bool,
    error: Option<String>,
    texs: HashMap<String, Tex>,
    asked: HashSet<String>,
    saving: HashSet<String>,
    saved: HashSet<String>,
    gen: u64,
    started: bool,
    scroll: f32,
    kind: usize,
    progress: HashMap<String, (u64, u64)>,
}

impl DownloadsSt {
    pub fn busy(&self) -> bool {
        self.loading || !self.saving.is_empty()
    }
}

const ST_SORTS: [(&str, &str); 4] = [("trend", "В тренде за месяц"), ("subscribed", "Самые популярные"), ("new", "Новые"), ("top", "Лучшие по оценкам")];
const ST_KINDS: [(&str, &str); 3] = [("video", "Видео"), ("scene", "Сцены"), ("all", "Все")];

const WH_SORTS: [(&str, &str); 6] = [("toplist", "Лучшие за месяц"), ("date_added", "Новые"), ("hot", "Популярные сейчас"), ("relevance", "По запросу"), ("views", "По просмотрам"), ("random", "Случайные")];

fn search(st: &mut DownloadsSt, c: &mut PanelCtx) {
    st.gen += 1;
    st.loading = true;
    st.error = None;
    st.items.clear();
    st.texs.clear();
    st.asked.clear();
    st.scroll = 0.0;
    let tag = format!("dl:{}", st.gen);
    if st.source == 2 {
        // Мастерская Steam: спрашиваем через запущенный клиент Steam отдельным процессом.
        let args = vec!["steam".to_string(), "search".into(), ST_SORTS[st.sort.min(3)].0.into(), ST_KINDS[st.kind.min(2)].0.into(), st.page.max(1).to_string(), st.query.trim().to_string()];
        c.pool.run(true, move || {
            let out = std::env::current_exe().ok().and_then(|exe| std::process::Command::new(exe).args(&args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok());
            Some(Done::Net(tag, out.map(|o| o.stdout)))
        });
        return;
    }
    let url = if st.source == 0 {
        let so = &c.cfg.sources;
        let cats = format!("{}{}{}", so.wallhaven_general as u8, so.wallhaven_anime as u8, so.wallhaven_people as u8);
        let cats = if cats == "000" { "110".to_string() } else { cats };
        let mut sort = WH_SORTS[st.sort.min(5)].0;
        if !st.query.trim().is_empty() && sort == "toplist" {
            sort = "relevance";
        }
        let mut u = format!("https://wallhaven.cc/api/v1/search?categories={cats}&purity=100&sorting={sort}&order=desc&topRange=1M&page={}", st.page.max(1));
        if !st.query.trim().is_empty() {
            u += &format!("&q={}", crate::picker::urlencode(st.query.trim()));
        }
        if !so.wallhaven_min_res.is_empty() {
            u += &format!("&atleast={}", so.wallhaven_min_res);
        }
        u
    } else {
        "https://www.bing.com/HPImageArchive.aspx?format=js&idx=0&n=8&mkt=en-US".to_string()
    };
    c.pool.run(true, move || Some(Done::Net(tag, jobs::http_get(&url, 20))));
}

pub fn downloads(w: &mut W, st: &mut DownloadsSt, c: &mut PanelCtx) {
    let s = w.s;
    let th = w.th;
    let (win, side, body) = shell(w, c.view, crate::i18n::tr("Скачать обои"), crate::i18n::tr("Поиск по открытым источникам. Скачанное сразу попадает в библиотеку."));
    if !st.started {
        st.started = true;
        st.page = 1;
        // Начинаем с Мастерской Wallpaper Engine, если до неё можно дотянуться.
        st.source = if crate::steam::library_path().is_some() { 2 } else { 0 };
        search(st, c);
    }
    let mut again = false;
    let x = side.x + 22.0 * s;
    let sw = side.w - 44.0 * s;
    let mut y = side.y + 108.0 * s;
    for (name, i) in [("Wallpaper Engine (Steam)", 2usize), ("Wallhaven", 0), (crate::i18n::tr("Bing: фото дня"), 1)] {
        if w.nav_row(Rect::new(side.x + 14.0 * s, y, side.w - 28.0 * s, 27.0 * s), name, "", st.source == i) && st.source != i {
            st.source = i;
            st.page = 1;
            st.sort = 0;
            again = true;
        }
        y += 30.0 * s;
    }
    y += 14.0 * s;
    if st.source == 0 {
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("ЗАПРОС"));
        y += 12.0 * s;
        if w.text_field(Rect::new(x, y, sw, 28.0 * s), id("dlq"), &mut st.query, crate::i18n::tr("Например: mountains, city night")) {
            st.page = 1;
            again = true;
            w.ui.focus = 0;
        }
        y += 44.0 * s;
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("ПОРЯДОК"));
        y += 12.0 * s;
        let d = w.cycle(Rect::new(x, y, sw, 26.0 * s), crate::i18n::tr(WH_SORTS[st.sort.min(5)].1));
        if d != 0 {
            st.sort = (st.sort as i32 + d).rem_euclid(WH_SORTS.len() as i32) as usize;
            st.page = 1;
            again = true;
        }
        y += 42.0 * s;
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("КАТЕГОРИИ"));
        y += 12.0 * s;
        let so = &mut c.cfg.sources;
        let cw = (sw - 12.0 * s) / 3.0;
        for (i, (label, v)) in [(crate::i18n::tr("Общее"), &mut so.wallhaven_general), (crate::i18n::tr("Аниме"), &mut so.wallhaven_anime), (crate::i18n::tr("Люди"), &mut so.wallhaven_people)].into_iter().enumerate() {
            if w.chip(Rect::new(x + (cw + 6.0 * s) * i as f32, y, cw, 24.0 * s), label, *v) {
                *v = !*v;
                *c.cfg_dirty = true;
                st.page = 1;
                again = true;
            }
        }
        y += 42.0 * s;
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("СТРАНИЦА"));
        y += 12.0 * s;
        let label = if st.last_page > 0 { crate::trf!("{} из {}", "{} of {}", st.page, st.last_page) } else { st.page.to_string() };
        let d = w.cycle(Rect::new(x, y, sw, 26.0 * s), &label);
        if d != 0 {
            let max = if st.last_page > 0 { st.last_page } else { 9999 };
            let np = (st.page as i32 + d).clamp(1, max as i32) as u32;
            if np != st.page {
                st.page = np;
                again = true;
            }
        }
    } else if st.source == 2 {
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("ЗАПРОС"));
        y += 12.0 * s;
        if w.text_field(Rect::new(x, y, sw, 28.0 * s), id("dlq"), &mut st.query, crate::i18n::tr("Например: cyberpunk, miku, rain")) {
            st.page = 1;
            again = true;
            w.ui.focus = 0;
        }
        y += 44.0 * s;
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("ТИП"));
        y += 12.0 * s;
        let cw = (sw - 12.0 * s) / 3.0;
        for (i, label) in ST_KINDS.iter().map(|k| crate::i18n::tr(k.1)).enumerate() {
            if w.chip(Rect::new(x + (cw + 6.0 * s) * i as f32, y, cw, 24.0 * s), label, st.kind == i) && st.kind != i {
                st.kind = i;
                st.page = 1;
                again = true;
            }
        }
        y += 42.0 * s;
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("ПОРЯДОК"));
        y += 12.0 * s;
        let d = w.cycle(Rect::new(x, y, sw, 26.0 * s), crate::i18n::tr(ST_SORTS[st.sort.min(3)].1));
        if d != 0 {
            st.sort = (st.sort.min(3) as i32 + d).rem_euclid(ST_SORTS.len() as i32) as usize;
            st.page = 1;
            again = true;
        }
        y += 42.0 * s;
        w.text(BOLD, 8.0, x, y, th.accent, crate::i18n::tr("СТРАНИЦА"));
        y += 12.0 * s;
        let label = if st.last_page > 0 { crate::trf!("{} из {}", "{} of {}", st.page, st.last_page) } else { st.page.to_string() };
        let d = w.cycle(Rect::new(x, y, sw, 26.0 * s), &label);
        if d != 0 {
            let max = if st.last_page > 0 { st.last_page } else { 9999 };
            let np = (st.page as i32 + d).clamp(1, max as i32) as u32;
            if np != st.page {
                st.page = np;
                again = true;
            }
        }
        y += 42.0 * s;
        let note = if crate::we::converter_ready() {
            crate::i18n::tr("Нужен запущенный Steam с Wallpaper Engine в библиотеке. Видео играют сразу; сцену после загрузки один раз записываем в ролик — это около минуты.").to_string()
        } else {
            crate::trf!("Нужен запущенный Steam с Wallpaper Engine в библиотеке. Видео играют сразу. Сцены на этом компьютере записать нечем — не хватает: {}.", "Needs Steam running with Wallpaper Engine in your library. Videos play at once. Scenes cannot be recorded on this computer, missing: {}.", crate::we::converter_missing().join(", "))
        };
        w.para(MEDIUM, 9.0, x, y, sw, th.dim, &note);
    } else {
        w.para(MEDIUM, 9.2, x, y, sw, th.dim, crate::i18n::tr("Восемь последних фотографий дня с главной страницы Bing в разрешении 4K."));
    }
    let dir = if st.source == 2 { crate::i18n::tr("Библиотека Steam (подписка в Мастерской)").to_string() } else { config::shorten(&c.cfg.download_dir().to_string_lossy()) };
    w.text(BOLD, 8.0, x, side.bottom() - 44.0 * s, th.accent, crate::i18n::tr("КУДА СОХРАНЯЕТСЯ"));
    let dir = w.d.fit(MEDIUM, 9.0 * s, &dir, sw);
    w.text(MEDIUM, 9.0, x, side.bottom() - 26.0 * s, th.dim, &dir);

    let title = match st.source {
        0 => "Wallhaven",
        2 => crate::i18n::tr("Мастерская Wallpaper Engine"),
        _ => crate::i18n::tr("Фото дня Bing"),
    };
    let sub = if st.loading {
        crate::i18n::tr("Ищем…").to_string()
    } else if let Some(e) = &st.error {
        e.clone()
    } else {
        crate::trf!("Найдено на странице: {}. Наведите на картинку, чтобы скачать.", "{} on this page. Hover a picture to download it.", st.items.len())
    };
    let top = w.heading(body.x, body.y + 34.0 * s, crate::i18n::tr("ИСТОЧНИКИ"), title, &sub, body.w - 60.0 * s) + 8.0 * s;
    if w.close_button(win) {
        *c.close = true;
    }
    let area = Rect::new(body.x, top, body.w, win.bottom() - top - 14.0 * s);
    let cols = ((area.w / (250.0 * s)).floor() as usize).clamp(2, 6);
    let gap = 6.0 * s;
    let cw = (area.w - 14.0 * s - gap * (cols as f32 - 1.0)) / cols as f32;
    let chh = cw * 9.0 / 16.0;
    w.d.clip(Some(area));
    let mut start: Option<(Remote, bool)> = None;
    for (n, it) in st.items.iter().enumerate() {
        let r = Rect::new(area.x + (cw + gap) * (n % cols) as f32, area.y + (chh + gap) * (n / cols) as f32 - st.scroll, cw, chh);
        if r.bottom() < area.y || r.y > area.bottom() {
            continue;
        }
        match st.texs.get(&it.thumb) {
            Some(t) => w.d.shape(&Shape::new(r, [1.0; 4]).cover(t, r)),
            None => {
                w.d.rect(r, th.alt);
                if st.asked.insert(it.thumb.clone()) {
                    let url = it.thumb.clone();
                    c.pool.run(true, move || {
                        let img = jobs::http_get(&url, 20).and_then(|b| {
                            // У анимированных превью первый кадр часто чёрный — берём кадр подальше.
                            if b.starts_with(b"GIF") {
                                use image::AnimationDecoder;
                                let frame = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&b)).ok().and_then(|d| d.into_frames().take(10).filter_map(Result::ok).last());
                                if let Some(f) = frame {
                                    return Some(image::DynamicImage::ImageRgba8(f.into_buffer()));
                                }
                            }
                            // Pictures from the network: refuse anything huge before decoding.
                            let mut r = image::ImageReader::new(std::io::Cursor::new(&b)).with_guessed_format().ok()?;
                            let mut lim = image::Limits::default();
                            lim.max_image_width = Some(4096);
                            lim.max_image_height = Some(4096);
                            lim.max_alloc = Some(64 << 20);
                            r.limits(lim);
                            r.decode().ok()
                        }).map(|i| {
                            let i = i.into_rgba8();
                            library::Rgba { w: i.width(), h: i.height(), data: i.into_raw() }
                        });
                        Some(Done::Remote(url, img))
                    });
                }
            }
        }
        let saved = st.saved.contains(&it.id);
        let saving = st.saving.contains(&it.id);
        if saving {
            // Ход загрузки: полоса и подпись.
            let (have, total) = st.progress.get(&it.id).copied().unwrap_or((0, 1));
            let label = if total == 0 { crate::i18n::tr("ЗАПИСЬ СЦЕНЫ В ВИДЕО…").to_string() } else if have == 0 { crate::i18n::tr("СКАЧИВАЕТСЯ…").to_string() } else { crate::trf!("СКАЧИВАЕТСЯ {} %", "DOWNLOADING {} %", (have * 100 / total.max(1)).min(100)) };
            let tag = Rect::new(r.x + 6.0 * s, r.y + 6.0 * s, w.text_w(BOLD, 8.5, &label) + 16.0 * s, 20.0 * s);
            w.d.rect(tag, alpha(th.backdrop, 0.92));
            w.d.text_c(BOLD, 8.5 * s, tag, th.text, &label);
            let bar = Rect::new(r.x, r.bottom() - 4.0 * s, r.w, 4.0 * s);
            w.d.rect(bar, alpha(th.backdrop, 0.8));
            let k = if total == 0 { 1.0 } else { have as f32 / total.max(1) as f32 };
            w.d.rect(Rect::new(bar.x, bar.y, bar.w * k.clamp(0.02, 1.0), bar.h), th.accent);
        } else if saved {
            let tag = Rect::new(r.x + 6.0 * s, r.y + 6.0 * s, 76.0 * s, 20.0 * s);
            w.d.rect(tag, th.accent);
            w.d.text_c(BOLD, 8.5 * s, tag, th.on_accent, crate::i18n::tr("СКАЧАНО"));
        }
        if w.ui.hover(r) && area.has(w.ui.mouse) {
            w.d.rect(r, alpha(th.backdrop, 0.62));
            let info = w.d.fit(BOLD, 9.0 * s, if it.title.is_empty() { &it.info } else { &it.title }, r.w - 16.0 * s);
            w.text(BOLD, 9.0, r.x + 8.0 * s, r.y + 16.0 * s, th.text, &info);
            if !it.title.is_empty() {
                w.text(MEDIUM, 8.5, r.x + 8.0 * s, r.y + 32.0 * s, th.dim, &it.info);
            }
            if !saving {
                let bw = (r.w - 22.0 * s) / 2.0;
                let by = r.bottom() - 34.0 * s;
                let steam = it.full.starts_with("steam:");
                let (l1, l2) = match (steam, saved) {
                    (true, true) => (crate::i18n::tr("Обновить"), crate::i18n::tr("Поставить")),
                    (true, false) => (crate::i18n::tr("Подписаться"), crate::i18n::tr("Подписаться и поставить")),
                    (false, true) => (crate::i18n::tr("Скачать снова"), crate::i18n::tr("Скачать и поставить")),
                    (false, false) => (crate::i18n::tr("Скачать"), crate::i18n::tr("Скачать и поставить")),
                };
                if w.button(Rect::new(r.x + 8.0 * s, by, bw, 26.0 * s), l1) {
                    start = Some((it.clone(), false));
                }
                if w.button_on(Rect::new(r.x + 14.0 * s + bw, by, bw, 26.0 * s), l2, true) {
                    start = Some((it.clone(), true));
                }
            }
        }
    }
    w.d.clip(None);
    let rows = st.items.len().div_ceil(cols) as f32;
    w.scroll(area, rows * (chh + gap), &mut st.scroll);
    if let Some((it, apply)) = start.clone().filter(|(it, _)| it.full.starts_with("steam:")) {
        st.saving.insert(it.id.clone());
        st.progress.remove(&it.id);
        let (cfg, tx) = (c.cfg.clone(), c.pool.tx.clone());
        c.pool.run(true, move || {
            use std::io::BufRead;
            let key = it.id.clone();
            let fail = |why: String| {
                let _ = tx.send(Done::Note(why));
                Some(Done::Saved(key.clone(), None, apply))
            };
            let Ok(exe) = std::env::current_exe() else { return fail(crate::i18n::tr("не найден исполняемый файл").into()) };
            let Ok(mut child) = std::process::Command::new(exe).args(["steam", "get", &it.full[6..]]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn() else {
                return fail(crate::i18n::tr("не удалось запустить загрузку").into());
            };
            let mut folder = None;
            let mut error = None;
            if let Some(out) = child.stdout.take() {
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(rest) = line.strip_prefix("progress ") {
                        let mut n = rest.split(' ').filter_map(|v| v.parse::<u64>().ok());
                        if let (Some(a), Some(b)) = (n.next(), n.next()) {
                            let _ = tx.send(Done::Progress(key.clone(), a, b.max(1)));
                        }
                    } else if let Some(rest) = line.strip_prefix("done ") {
                        folder = Some(rest.to_string());
                    } else if let Some(rest) = line.strip_prefix("error ") {
                        error = Some(rest.to_string());
                    }
                }
            }
            let _ = child.wait();
            let Some(folder) = folder.filter(|f| !f.is_empty()) else { return fail(error.unwrap_or_else(|| crate::i18n::tr("Steam не скачал обои").into())) };
            let dir = std::path::PathBuf::from(folder);
            if crate::we::project(&dir).is_some_and(|p| p.kind == "scene") {
                let _ = tx.send(Done::Progress(key.clone(), 0, 0));
            }
            match crate::we::playable(&cfg, &dir) {
                Ok(file) => Some(Done::Saved(key, Some(file.to_string_lossy().into_owned()), apply)),
                Err(why) => fail(why),
            }
        });
    } else if let Some((it, apply)) = start {
        st.saving.insert(it.id.clone());
        let dir = c.cfg.download_dir();
        c.pool.run(true, move || {
            let ext = it.full.rsplit('.').next().filter(|e| e.len() <= 4 && e.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or("jpg").to_lowercase();
            let safe: String = it.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            let path = dir.join(format!("{safe}.{ext}"));
            let ok = jobs::http_get(&it.full, 120).filter(|b| b.len() > 1024 && image::guess_format(b).is_ok()).is_some_and(|b| std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, b).is_ok());
            Some(Done::Saved(it.id, ok.then(|| path.to_string_lossy().into_owned()), apply))
        });
    }
    if again {
        search(st, c);
    }
}

/// Ответы сети для панели загрузок.
pub fn done(p: &mut Picker, d: Done, gl: &Gl, _cfg: &Config, out: &mut Vec<Action>) {
    let (_, _, _, _, st, toast) = p.panel_states();
    match d {
        Done::Net(tag, body) => {
            if tag != format!("dl:{}", st.gen) {
                return;
            }
            st.loading = false;
            if st.source == 2 {
                if let Some(text) = body.as_ref().map(|b| String::from_utf8_lossy(b).trim().to_string()) {
                    if let Some(why) = text.strip_prefix("error ") {
                        st.error = Some(why.to_string());
                        return;
                    }
                }
            }
            let Some(v) = body.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) else {
                st.error = Some(crate::i18n::tr("Не удалось получить ответ. Проверьте подключение к интернету.").into());
                return;
            };
            let text = |v: &serde_json::Value| v.as_str().unwrap_or("").to_string();
            if st.source == 2 {
                st.last_page = (v["total"].as_u64().unwrap_or(0) as u32).div_ceil(50);
                for it in v["items"].as_array().into_iter().flatten() {
                    let (wid, thumb) = (text(&it["id"]), text(&it["preview"]));
                    if wid.is_empty() || !thumb.starts_with("https://") {
                        continue;
                    }
                    let tags: Vec<String> = it["tags"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(|s| s.to_string())).collect();
                    let kind = if tags.iter().any(|t| t == "Video") { crate::i18n::tr("Видео") } else if tags.iter().any(|t| t == "Scene") { crate::i18n::tr("Сцена") } else { crate::i18n::tr("Обои") };
                    let res = tags.iter().find(|t| t.contains(" x ") && t.chars().next().is_some_and(|c| c.is_ascii_digit())).map(|t| format!(" · {}", t.replace(" x ", " × "))).unwrap_or_default();
                    let key = format!("steam-{wid}");
                    if it["installed"].as_bool().unwrap_or(false) {
                        st.saved.insert(key.clone());
                    }
                    st.items.push(Remote { id: key, thumb, full: format!("steam:{wid}"), info: format!("{kind}{res}"), title: text(&it["title"]) });
                }
            } else if st.source == 0 {
                st.last_page = v["meta"]["last_page"].as_u64().unwrap_or(0) as u32;
                for it in v["data"].as_array().into_iter().flatten() {
                    let (id, full, thumb) = (text(&it["id"]), text(&it["path"]), text(&it["thumbs"]["small"]));
                    if id.is_empty() || !full.starts_with("https://") || !thumb.starts_with("https://") {
                        continue;
                    }
                    let size = it["file_size"].as_u64().unwrap_or(0);
                    st.items.push(Remote { id: format!("wallhaven-{id}"), thumb, full, info: format!("{} · {}", text(&it["resolution"]).replace('x', " × "), crate::picker::human_size(size)), title: String::new() });
                }
            } else {
                for it in v["images"].as_array().into_iter().flatten() {
                    let base = text(&it["urlbase"]);
                    if !base.starts_with('/') {
                        continue;
                    }
                    let name = base.split("OHR.").nth(1).and_then(|s| s.split('_').next()).unwrap_or("bing").to_string();
                    st.items.push(Remote { id: format!("bing-{}-{}", text(&it["startdate"]), name), thumb: format!("https://www.bing.com{base}_640x360.jpg"), full: format!("https://www.bing.com{base}_UHD.jpg"), info: "3840 × 2160".into(), title: text(&it["title"]) });
                }
            }
            if st.items.is_empty() {
                st.error = Some(crate::i18n::tr("Ничего не нашлось. Попробуйте другой запрос.").into());
            }
        }
        Done::Remote(url, img) => {
            if let Some(img) = img {
                if st.asked.contains(&url) {
                    st.texs.insert(url, Tex::rgba(gl, img.w, img.h, Some(&img.data), true));
                }
            }
        }
        Done::Progress(key, have, total) => {
            st.progress.insert(key, (have, total));
        }
        Done::Note(text) => *toast = Some((text, Instant::now())),
        Done::Saved(key, path, apply) => {
            st.saving.remove(&key);
            st.progress.remove(&key);
            match path {
                Some(path) => {
                    st.saved.insert(key);
                    *toast = Some((if apply { crate::i18n::tr("Скачано и поставлено") } else { crate::i18n::tr("Скачано в библиотеку") }.into(), Instant::now()));
                    out.push(Action::Rescan);
                    if apply {
                        out.push(Action::Commit(path));
                    }
                }
                None => {
                    if toast.as_ref().is_none_or(|t| t.1.elapsed().as_secs() > 1) {
                        *toast = Some((crate::i18n::tr("Не удалось скачать").into(), Instant::now()));
                    }
                }
            }
        }
        _ => {}
    }
}
