//! Мелкие элементы интерфейса в «немедленном» стиле: рисуются и тут же отвечают на ввод.
use crate::config::Config;
use crate::draw::{alpha, hex, mix, parse_hex, Color, Draw, Rect, Shape, BOLD, ICON, MEDIUM};
use crate::icons;

#[derive(Clone, Copy)]
pub struct Th {
    pub accent: Color,
    pub on_accent: Color,
    pub accent2: Color,
    pub panel: Color,
    pub text: Color,
    pub alt: Color,
    pub raised: Color,
    pub backdrop: Color,
    pub outline: Color,
    pub dim: Color,
    pub faint: Color,
}

impl Th {
    pub fn from(cfg: &Config) -> Th {
        let p = &cfg.theme.palette;
        let c = |s: &str, d: u32| parse_hex(s).unwrap_or(hex(d));
        let mut t = Th {
            accent: c(&p.accent, 0x88c0d0),
            on_accent: c(&p.on_accent, 0x2e3440),
            accent2: c(&p.accent2, 0xb48ead),
            panel: c(&p.panel, 0x3b4252),
            text: c(&p.text, 0xeceff4),
            alt: c(&p.panel_alt, 0x434c5e),
            raised: c(&p.raised, 0x4c566a),
            backdrop: c(&p.backdrop, 0x2e3440),
            outline: c(&p.outline, 0x4c566a),
            dim: [0.0; 4],
            faint: [0.0; 4],
        };
        if cfg.theme.source == "omarchy" {
            if let Some(o) = omarchy_colors() {
                t = Th { dim: [0.0; 4], faint: [0.0; 4], ..o };
            }
        }
        t.dim = alpha(t.text, 0.66);
        t.faint = alpha(t.text, 0.40);
        t
    }
}

/// Палитра из цветов текущей темы Omarchy.
pub fn omarchy_colors() -> Option<Th> {
    let text = std::fs::read_to_string(crate::config::omarchy_state().join("theme/colors.toml")).ok()?;
    let get = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key).then(|| parse_hex(v.trim().trim_matches('"'))).flatten()
        })
    };
    let bg = get("background")?;
    let fg = get("foreground")?;
    let accent = get("accent").unwrap_or(fg);
    Some(Th {
        accent,
        on_accent: bg,
        accent2: get("magenta").unwrap_or(accent),
        panel: mix(bg, fg, 0.08),
        text: fg,
        alt: mix(bg, fg, 0.14),
        raised: mix(bg, fg, 0.22),
        backdrop: bg,
        outline: mix(bg, fg, 0.26),
        dim: fg,
        faint: fg,
    })
}

pub struct Key {
    pub sym: u32,
    pub raw: u32,
    pub text: Option<String>,
    pub ctrl: bool,
    pub shift: bool,
}

#[derive(Default)]
pub struct Ui {
    pub mouse: (f32, f32),
    pub down: bool,
    pub press: bool,
    pub release: bool,
    pub rpress: bool,
    pub wheel: f32,
    /// Поле ввода с фокусом (0 — нет).
    pub focus: u64,
    /// Элемент, который тянут мышью.
    pub active: u64,
    pub keys: Vec<Key>,
    pub tip: Option<(String, Rect)>,
    /// Что-то поменялось так, что нужен ещё один кадр.
    pub again: bool,
    pub caret_on: bool,
}

impl Ui {
    pub fn end_frame(&mut self) {
        self.press = false;
        self.release = false;
        self.rpress = false;
        self.wheel = 0.0;
        self.keys.clear();
        if !self.down {
            self.active = 0;
        }
    }
    pub fn hover(&self, r: Rect) -> bool {
        r.has(self.mouse)
    }
    pub fn clicked(&self, r: Rect) -> bool {
        self.press && r.has(self.mouse)
    }
}

pub fn id(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h | 1
}

pub struct W<'a> {
    pub ui: &'a mut Ui,
    pub d: &'a mut Draw,
    pub th: Th,
    /// Масштаб: единица макета в пикселях.
    pub s: f32,
}

impl<'a> W<'a> {
    pub fn text(&mut self, font: usize, size: f32, x: f32, y: f32, c: Color, t: &str) -> f32 {
        self.d.text(font, size * self.s, x, y, c, t)
    }
    pub fn text_w(&mut self, font: usize, size: f32, t: &str) -> f32 {
        self.d.text_w(font, size * self.s, t)
    }
    /// Текст с переносом; возвращает нижнюю границу.
    pub fn para(&mut self, font: usize, size: f32, x: f32, y: f32, max_w: f32, c: Color, t: &str) -> f32 {
        let lh = size * self.s * 1.4;
        let mut cy = y + lh / 2.0;
        for line in self.d.wrap(font, size * self.s, t, max_w) {
            self.d.text(font, size * self.s, x, cy, c, &line);
            cy += lh;
        }
        cy - lh / 2.0
    }

    pub fn tip(&mut self, r: Rect, text: &str) {
        if self.ui.hover(r) {
            self.ui.tip = Some((text.to_string(), r));
        }
    }

    fn button_base(&mut self, r: Rect, label: &str, bg: Color, fg: Color, border: Option<Color>, enabled: bool) -> bool {
        let hot = enabled && self.ui.hover(r);
        let bg = if hot { mix(bg, self.th.text, 0.12) } else { bg };
        let mut sh = Shape::new(r, alpha(bg, if enabled { 1.0 } else { 0.45 }));
        if let Some(b) = border {
            sh = sh.border(self.s.max(1.0), b);
        }
        self.d.shape(&sh);
        let px = 9.6 * self.s;
        let label = self.d.fit(BOLD, px, label, r.w - 8.0 * self.s);
        self.d.text_c(BOLD, px, r, alpha(fg, if enabled { 1.0 } else { 0.4 }), &label);
        hot && self.ui.press
    }
    pub fn button(&mut self, r: Rect, label: &str) -> bool {
        self.button_base(r, label, self.th.raised, self.th.text, None, true)
    }
    pub fn button_on(&mut self, r: Rect, label: &str, on: bool) -> bool {
        if on {
            self.button_base(r, label, self.th.accent, self.th.on_accent, None, true)
        } else {
            self.button_base(r, label, self.th.raised, self.th.text, None, true)
        }
    }
    pub fn button_ghost(&mut self, r: Rect, label: &str, enabled: bool) -> bool {
        self.button_base(r, label, self.th.backdrop, self.th.text, Some(self.th.outline), enabled)
    }
    pub fn button_danger(&mut self, r: Rect, label: &str) -> bool {
        self.button_base(r, label, self.th.backdrop, self.th.text, Some(self.th.accent2), true)
    }
    pub fn icon_button(&mut self, r: Rect, icon: char, on: bool, tip: &str) -> bool {
        let hot = self.ui.hover(r);
        let bg = if on { self.th.accent } else if hot { mix(self.th.raised, self.th.text, 0.12) } else { self.th.raised };
        self.d.rect(r, bg);
        let px = (r.h * 0.55).min(14.0 * self.s);
        let mut b = [0u8; 4];
        self.d.text_c(ICON, px, r, if on { self.th.on_accent } else { self.th.text }, icon.encode_utf8(&mut b));
        if !tip.is_empty() {
            self.tip(r, tip);
        }
        hot && self.ui.press
    }

    /// Переключатель. Возвращает true, если по нему щёлкнули.
    pub fn toggle(&mut self, r: Rect, on: bool) -> bool {
        let track = Rect::new(r.x, r.cy() - 8.0 * self.s, 32.0 * self.s, 16.0 * self.s);
        let hot = self.ui.hover(Rect::new(r.x, r.y, track.w, r.h));
        let bg = if on { self.th.accent } else { self.th.raised };
        self.d.rrect(track, track.h / 2.0, if hot { mix(bg, self.th.text, 0.15) } else { bg });
        let kx = if on { track.right() - 14.0 * self.s } else { track.x + 2.0 * self.s };
        self.d.rrect(Rect::new(kx, track.y + 2.0 * self.s, 12.0 * self.s, 12.0 * self.s), 6.0 * self.s, if on { self.th.on_accent } else { self.th.text });
        hot && self.ui.press
    }

    /// Ползунок. Возвращает true, пока значение меняется.
    pub fn slider(&mut self, r: Rect, key: u64, v: &mut f32, min: f32, max: f32) -> bool {
        let s = self.s;
        let track = Rect::new(r.x, r.cy() - 2.0 * s, r.w, 4.0 * s);
        let grab = Rect::new(r.x - 6.0 * s, r.y, r.w + 12.0 * s, r.h);
        if self.ui.press && grab.has(self.ui.mouse) {
            self.ui.active = key;
        }
        let mut changed = false;
        if self.ui.active == key && self.ui.down {
            let t = ((self.ui.mouse.0 - r.x) / r.w.max(1.0)).clamp(0.0, 1.0);
            let nv = min + (max - min) * t;
            if (nv - *v).abs() > f32::EPSILON {
                *v = nv;
                changed = true;
            }
        }
        let t = ((*v - min) / (max - min).max(1e-6)).clamp(0.0, 1.0);
        self.d.rrect(track, 2.0 * s, self.th.raised);
        self.d.rrect(Rect::new(track.x, track.y, track.w * t, track.h), 2.0 * s, self.th.accent);
        let hot = grab.has(self.ui.mouse) || self.ui.active == key;
        let kr = if hot { 7.0 } else { 6.0 } * s;
        self.d.rrect(Rect::new(r.x + r.w * t - kr, r.cy() - kr, kr * 2.0, kr * 2.0), kr, self.th.text);
        changed
    }

    /// Выбор из вариантов стрелками: -1 / +1 по щелчку, 0 иначе.
    pub fn cycle(&mut self, r: Rect, label: &str) -> i32 {
        let s = self.s;
        self.d.shape(&Shape::new(r, self.th.backdrop).border(s.max(1.0), self.th.outline));
        let aw = 22.0 * s;
        let (l, rr) = (Rect::new(r.x, r.y, aw, r.h), Rect::new(r.right() - aw, r.y, aw, r.h));
        let mut out = 0;
        for (rect, icon, dir) in [(l, icons::CHEVRON_LEFT, -1), (rr, icons::CHEVRON_RIGHT, 1)] {
            let hot = self.ui.hover(rect);
            if hot {
                self.d.rect(rect.inset(s), self.th.raised);
            }
            let mut b = [0u8; 4];
            self.d.text_c(ICON, 11.0 * s, rect, if hot { self.th.text } else { self.th.dim }, icon.encode_utf8(&mut b));
            if hot && self.ui.press {
                out = dir;
            }
        }
        let mid = Rect::new(r.x + aw, r.y, r.w - 2.0 * aw, r.h);
        let label = self.d.fit(BOLD, 9.6 * s, label, mid.w - 4.0 * s);
        self.d.text_c(BOLD, 9.6 * s, mid, self.th.accent, &label);
        if out == 0 && self.ui.clicked(mid) {
            out = 1;
        }
        if self.ui.hover(r) && self.ui.wheel != 0.0 {
            out = if self.ui.wheel > 0.0 { 1 } else { -1 };
            self.ui.wheel = 0.0;
        }
        out
    }

    /// Поле ввода. true — нажали Enter.
    pub fn text_field(&mut self, r: Rect, key: u64, val: &mut String, hint: &str) -> bool {
        let s = self.s;
        if self.ui.press {
            if r.has(self.ui.mouse) {
                self.ui.focus = key;
            } else if self.ui.focus == key {
                self.ui.focus = 0;
            }
        }
        let focused = self.ui.focus == key;
        let mut submit = false;
        if focused {
            for k in std::mem::take(&mut self.ui.keys) {
                match k.sym {
                    0xff0d | 0xff8d => submit = true,
                    0xff08 => {
                        if k.ctrl {
                            let cut = val.trim_end().rfind(' ').map(|i| i + 1).unwrap_or(0);
                            val.truncate(cut);
                        } else {
                            val.pop();
                        }
                    }
                    _ if k.ctrl && k.raw == 47 => {
                        if let Ok(o) = std::process::Command::new("wl-paste").arg("-n").output() {
                            let t = String::from_utf8_lossy(&o.stdout);
                            val.extend(t.chars().filter(|c| !c.is_control()).take(512));
                        }
                    }
                    _ if k.ctrl && k.raw == 22 => val.clear(),
                    _ => {
                        if let Some(t) = &k.text {
                            if !k.ctrl && val.chars().count() < 512 {
                                val.extend(t.chars().filter(|c| !c.is_control()));
                            }
                        }
                    }
                }
            }
        }
        self.d.shape(&Shape::new(r, self.th.backdrop).border(s.max(1.0), if focused { self.th.accent } else { self.th.outline }));
        let px = 10.0 * s;
        let pad = 8.0 * s;
        self.d.clip(Some(r.inset(s)));
        if val.is_empty() && !focused {
            self.d.text(BOLD, px, r.x + pad, r.cy(), self.th.faint, hint);
        } else {
            let tw = self.d.text_w(BOLD, px, val);
            let x = r.x + pad - (tw - (r.w - 2.5 * pad)).max(0.0);
            self.d.text(BOLD, px, x, r.cy(), self.th.text, val);
            if focused {
                self.d.rect(Rect::new(x + tw + s, r.cy() - 6.5 * s, 1.5 * s, 13.0 * s), self.th.accent);
            }
        }
        self.d.clip(None);
        submit
    }

    pub fn chip(&mut self, r: Rect, label: &str, on: bool) -> bool {
        let hot = self.ui.hover(r);
        let bg = if on { self.th.accent } else if hot { mix(self.th.alt, self.th.text, 0.12) } else { self.th.alt };
        self.d.shape(&Shape::new(r, bg).border(self.s.max(1.0), if on { self.th.accent } else { self.th.outline }));
        self.d.text_c(BOLD, 9.0 * self.s, r, if on { self.th.on_accent } else { self.th.text }, label);
        hot && self.ui.press
    }

    /// Прокрутка колесом внутри области; возвращает смещение содержимого.
    pub fn scroll(&mut self, area: Rect, content_h: f32, pos: &mut f32) -> f32 {
        let max = (content_h - area.h).max(0.0);
        if self.ui.hover(area) && self.ui.wheel != 0.0 {
            *pos += self.ui.wheel * 48.0 * self.s;
            self.ui.wheel = 0.0;
        }
        *pos = pos.clamp(0.0, max);
        if max > 0.0 {
            let bar_h = (area.h * area.h / content_h).max(20.0 * self.s);
            let y = area.y + (area.h - bar_h) * (*pos / max);
            self.d.rrect(Rect::new(area.right() - 3.0 * self.s, y, 3.0 * self.s, bar_h), 1.5 * self.s, alpha(self.th.text, 0.25));
        }
        *pos
    }

    /// Строка бокового списка: ромбик, название, счётчик справа.
    pub fn nav_row(&mut self, r: Rect, label: &str, right: &str, on: bool) -> bool {
        let s = self.s;
        let hot = self.ui.hover(r);
        if on {
            self.d.rect(r, mix(self.th.panel, self.th.accent, 0.38));
        } else if hot {
            self.d.rect(r, mix(self.th.panel, self.th.text, 0.07));
        }
        let mut b = [0u8; 4];
        let icon = if on { icons::RHOMBUS } else { icons::RHOMBUS_OUTLINE };
        self.d.text(ICON, 8.0 * s, r.x + 10.0 * s, r.cy(), if on { self.th.accent } else { self.th.dim }, icon.encode_utf8(&mut b));
        let rw = if right.is_empty() { 0.0 } else { self.d.text_w(BOLD, 8.5 * s, right) + 10.0 * s };
        let label = self.d.fit(BOLD, 10.5 * s, label, r.w - 40.0 * s - rw);
        self.d.text(BOLD, 10.5 * s, r.x + 27.0 * s, r.cy(), self.th.text, &label);
        if !right.is_empty() {
            self.d.text(BOLD, 8.5 * s, r.right() - rw, r.cy(), self.th.accent, right);
        }
        hot && self.ui.press
    }

    /// Окно панели: затемнение фона, подложка, рамка. Возвращает внутренний прямоугольник.
    pub fn window(&mut self, view: (f32, f32), margin: f32) -> Rect {
        let s = self.s;
        self.d.rect(Rect::new(0.0, 0.0, view.0, view.1), [0.0, 0.0, 0.0, 0.45]);
        let m = margin * s;
        let r = Rect::new(m, m, view.0 - 2.0 * m, view.1 - 2.0 * m);
        self.d.shape(&Shape::new(Rect::new(r.x, r.y + 6.0 * s, r.w, r.h), [0.0, 0.0, 0.0, 0.5]).soft(18.0 * s));
        self.d.shape(&Shape::new(r, alpha(self.th.backdrop, 0.97)).border(s.max(1.0), self.th.outline));
        r
    }

    /// Крестик закрытия в правом верхнем углу.
    pub fn close_button(&mut self, win: Rect) -> bool {
        let s = self.s;
        let r = Rect::new(win.right() - 46.0 * s, win.y + 14.0 * s, 30.0 * s, 30.0 * s);
        self.icon_button(r, icons::CLOSE, false, crate::i18n::tr("Закрыть (Esc)"))
    }

    pub fn heading(&mut self, x: f32, y: f32, kicker: &str, title: &str, sub: &str, max_w: f32) -> f32 {
        let s = self.s;
        let mut cy = y;
        if !kicker.is_empty() {
            self.d.text(BOLD, 8.0 * s, x, cy, self.th.accent, kicker);
            cy += 20.0 * s;
        }
        self.d.text(BOLD, 24.0 * s, x, cy + 6.0 * s, self.th.text, title);
        cy += 30.0 * s;
        if !sub.is_empty() {
            cy = self.para(MEDIUM, 9.6, x, cy, max_w, self.th.dim, sub) + 4.0 * s;
        }
        cy
    }
}
