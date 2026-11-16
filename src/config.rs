//! Настройки (~/.config/omawall/config.toml) и пути.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}
fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var(var).ok().filter(|v| v.starts_with('/')).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
}
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("omawall")
}
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("omawall")
}
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("omawall")
}
/// A private runtime directory: XDG_RUNTIME_DIR or /run/user/UID, but only if
/// it is ours and closed to others; otherwise a private folder in the cache.
pub fn runtime_dir() -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::getuid() };
    let candidates = std::env::var("XDG_RUNTIME_DIR").ok().filter(|v| v.starts_with('/')).into_iter().chain([format!("/run/user/{uid}")]);
    for c in candidates {
        let p = PathBuf::from(c);
        if let Ok(m) = std::fs::symlink_metadata(&p) {
            if m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0 {
                return p;
            }
        }
    }
    let p = cache_dir().join("run");
    let _ = std::fs::create_dir_all(&p);
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700));
    p
}
pub fn socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("OMAWALL_SOCK") {
        return PathBuf::from(p);
    }
    runtime_dir().join("omawall.sock")
}
pub fn omarchy_state() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("omarchy/current")
}
pub fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        home().join(rest)
    } else if p == "~" {
        home()
    } else {
        PathBuf::from(p)
    }
}
pub fn shorten(p: &str) -> String {
    let h = home().to_string_lossy().into_owned();
    match p.strip_prefix(&h) {
        Some(rest) if rest.starts_with('/') || rest.is_empty() => format!("~{rest}"),
        _ => p.to_string(),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Library {
    /// Папки с обоями (вложенные папки читаются тоже).
    pub folders: Vec<String>,
    /// Показывать фоны текущей темы Omarchy.
    pub theme_backgrounds: bool,
    /// Показывать видеообои из Steam Workshop (Wallpaper Engine).
    pub workshop: bool,
}
impl Default for Library {
    fn default() -> Self {
        Library { folders: vec!["~/Pictures/Wallpapers".into()], theme_backgrounds: true, workshop: true }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Wallpaper {
    /// cover | contain | stretch
    pub fill: String,
    /// Название перехода или random.
    pub transition: String,
    pub duration_ms: u32,
    /// Ставить обои только на монитор, где открыто меню.
    pub per_monitor: bool,
    /// Подхватывать смену фона командами Omarchy и сообщать Omarchy о своих обоях.
    pub follow_omarchy: bool,
}
impl Default for Wallpaper {
    fn default() -> Self {
        Wallpaper { fill: "cover".into(), transition: "random".into(), duration_ms: 700, per_monitor: false, follow_omarchy: true }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Video {
    pub mute: bool,
    pub volume: u32,
    pub hwdec: bool,
    /// Останавливать видео при работе от батареи.
    pub pause_on_battery: bool,
    /// Экономный режим: auto (на батарее и на слабых машинах), on, off.
    pub saver: String,
}
impl Default for Video {
    fn default() -> Self {
        Video { mute: true, volume: 50, hwdec: true, pause_on_battery: false, saver: "auto".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Picker {
    pub scale: f32,
    pub visible: u32,
    pub selected_width: f32,
    pub height: f32,
    pub skew: f32,
    pub corner: f32,
    pub offset_y: f32,
    pub badges: bool,
    pub bar: bool,
    pub live_preview: bool,
    pub hover_select: bool,
    pub close_on_apply: bool,
    /// Закрытие меню оставляет те обои, на которых остановились (без Enter).
    pub apply_on_close: bool,
    pub wrap: bool,
    pub animations: bool,
}
impl Default for Picker {
    fn default() -> Self {
        Picker {
            scale: 1.35,
            visible: 12,
            selected_width: 685.0,
            height: 385.0,
            skew: 26.0,
            corner: 47.0,
            offset_y: 0.0,
            badges: true,
            bar: true,
            live_preview: true,
            hover_select: false,
            close_on_apply: false,
            apply_on_close: false,
            wrap: true,
            animations: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Rotation {
    pub enabled: bool,
    pub interval_min: u32,
    pub shuffle: bool,
    /// all | favourites | имя плейлиста
    pub source: String,
}
impl Default for Rotation {
    fn default() -> Self {
        Rotation { enabled: false, interval_min: 30, shuffle: true, source: "all".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Sources {
    /// Папка для скачанных обоев (внутри первой папки библиотеки, если путь относительный).
    pub download_dir: String,
    pub wallhaven_general: bool,
    pub wallhaven_anime: bool,
    pub wallhaven_people: bool,
    pub wallhaven_sort: String,
    pub wallhaven_min_res: String,
}
impl Default for Sources {
    fn default() -> Self {
        Sources { download_dir: "Wallhaven".into(), wallhaven_general: true, wallhaven_anime: true, wallhaven_people: false, wallhaven_sort: "toplist".into(), wallhaven_min_res: "1920x1080".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Palette {
    pub name: String,
    pub accent: String,
    pub on_accent: String,
    pub accent2: String,
    pub panel: String,
    pub text: String,
    pub panel_alt: String,
    pub raised: String,
    pub backdrop: String,
    pub outline: String,
}
impl Default for Palette {
    fn default() -> Self {
        Palette::nord()
    }
}
impl Palette {
    fn make(name: &str, c: [&str; 9]) -> Palette {
        Palette {
            name: name.into(),
            accent: c[0].into(),
            on_accent: c[1].into(),
            accent2: c[2].into(),
            panel: c[3].into(),
            text: c[4].into(),
            panel_alt: c[5].into(),
            raised: c[6].into(),
            backdrop: c[7].into(),
            outline: c[8].into(),
        }
    }
    pub fn nord() -> Palette {
        Palette::make("Nord", ["#88c0d0", "#2e3440", "#b48ead", "#3b4252", "#eceff4", "#434c5e", "#4c566a", "#2e3440", "#4c566a"])
    }
    pub fn presets() -> Vec<Palette> {
        vec![
            Palette::nord(),
            Palette::make("Catppuccin", ["#89b4fa", "#1e1e2e", "#f5c2e7", "#313244", "#cdd6f4", "#45475a", "#585b70", "#1e1e2e", "#585b70"]),
            Palette::make("Gruvbox", ["#d79921", "#282828", "#b16286", "#3c3836", "#ebdbb2", "#504945", "#665c54", "#282828", "#665c54"]),
            Palette::make("Tokyo Night", ["#7aa2f7", "#1a1b26", "#bb9af7", "#24283b", "#c0caf5", "#2f3549", "#414868", "#1a1b26", "#414868"]),
            Palette::make("Dracula", ["#bd93f9", "#282a36", "#ff79c6", "#343746", "#f8f8f2", "#44475a", "#565a70", "#282a36", "#565a70"]),
            Palette::make("Everforest", ["#a7c080", "#2d353b", "#d699b6", "#343f44", "#d3c6aa", "#3d484d", "#475258", "#2d353b", "#475258"]),
            Palette::make("Rosé Pine", ["#c4a7e7", "#191724", "#ebbcba", "#26233a", "#e0def4", "#2f2b45", "#403d52", "#191724", "#403d52"]),
            Palette::make("Solarized", ["#268bd2", "#002b36", "#d33682", "#073642", "#eee8d5", "#0d4654", "#1d5766", "#002b36", "#1d5766"]),
            Palette::make(crate::i18n::tr("Графит"), ["#e0e0e0", "#161616", "#9a9a9a", "#262626", "#f2f2f2", "#303030", "#3d3d3d", "#161616", "#3d3d3d"]),
        ]
    }
    pub const ROLES: [(&'static str, &'static str); 9] = [
        ("Акцент", "Рамка выбранных обоев, активные кнопки, ползунки"),
        ("Текст на акценте", "Надписи и значки на акцентном фоне"),
        ("Второй акцент", "Избранное и отметки"),
        ("Панель", "Фон боковых колонок"),
        ("Текст панели", "Основной текст"),
        ("Альтернативная панель", "Фон полей и строк"),
        ("Приподнятая панель", "Панель фильтров и кнопки"),
        ("Подложка", "Фон окон настроек"),
        ("Контур", "Рамки и разделители"),
    ];
    pub fn get(&self, i: usize) -> &str {
        [&self.accent, &self.on_accent, &self.accent2, &self.panel, &self.text, &self.panel_alt, &self.raised, &self.backdrop, &self.outline][i]
    }
    pub fn set(&mut self, i: usize, v: String) {
        *[&mut self.accent, &mut self.on_accent, &mut self.accent2, &mut self.panel, &mut self.text, &mut self.panel_alt, &mut self.raised, &mut self.backdrop, &mut self.outline][i] = v;
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Theme {
    /// palette — своя палитра ниже; omarchy — цвета текущей темы Omarchy.
    pub source: String,
    pub palette: Palette,
    pub saved: Vec<Palette>,
}
impl Default for Theme {
    fn default() -> Self {
        Theme { source: "palette".into(), palette: Palette::nord(), saved: vec![] }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Config {
    pub library: Library,
    pub wallpaper: Wallpaper,
    pub video: Video,
    pub picker: Picker,
    pub rotation: Rotation,
    pub sources: Sources,
    pub theme: Theme,
    /// Включён ли экономный режим прямо сейчас (вычисляется, не сохраняется).
    #[serde(skip)]
    pub saver_now: bool,
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }
    pub fn load() -> Config {
        let mut c: Config = std::fs::read_to_string(Self::path()).ok().and_then(|s| toml::from_str(&s).map_err(|e| eprintln!("omawall: config.toml: {e}")).ok()).unwrap_or_default();
        c.sanitize();
        c
    }
    pub fn sanitize(&mut self) {
        let p = &mut self.picker;
        p.scale = p.scale.clamp(0.6, 3.0);
        p.visible = p.visible.clamp(3, 31);
        p.selected_width = p.selected_width.clamp(200.0, 1600.0);
        p.height = p.height.clamp(120.0, 900.0);
        p.skew = p.skew.clamp(-120.0, 120.0);
        p.corner = p.corner.clamp(0.0, 200.0);
        p.offset_y = p.offset_y.clamp(-400.0, 400.0);
        self.wallpaper.duration_ms = self.wallpaper.duration_ms.clamp(0, 5000);
        self.video.volume = self.video.volume.min(100);
        self.rotation.interval_min = self.rotation.interval_min.clamp(1, 1440);
        if !["auto", "on", "off"].contains(&self.video.saver.as_str()) {
            self.video.saver = "auto".into();
        }
        if !["cover", "contain", "stretch"].contains(&self.wallpaper.fill.as_str()) {
            self.wallpaper.fill = "cover".into();
        }
    }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(config_dir());
        if let Ok(s) = toml::to_string_pretty(self) {
            write_atomic(&Self::path(), s.as_bytes());
        }
    }
    pub fn download_dir(&self) -> PathBuf {
        let d = expand(&self.sources.download_dir);
        if d.is_absolute() {
            d
        } else {
            expand(self.library.folders.first().map(|s| s.as_str()).unwrap_or("~/Pictures/Wallpapers")).join(d)
        }
    }
}

/// Слабая машина: мало ядер или памяти. Для неё экономный режим включается сам.
pub fn weak_machine() -> bool {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mem_kb: u64 = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| m.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok())).unwrap_or(16 << 20);
    cores <= 4 || mem_kb < 7 * 1024 * 1024
}

pub fn write_atomic(path: &std::path::Path, data: &[u8]) {
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}
