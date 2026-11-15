//! Библиотека обоев: обход папок, миниатюры, разбор цвета, пользовательские отметки.
use crate::config::{self, Config};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const IMAGE_EXT: [&str; 6] = ["jpg", "jpeg", "png", "webp", "bmp", "jfif"];
/// Картинки, которые не умеет встроенный декодер: читаем через ffmpeg.
pub const IMAGE_EXT_FF: [&str; 6] = ["avif", "jxl", "heic", "heif", "tif", "tiff"];
pub const VIDEO_EXT: [&str; 7] = ["mp4", "m4v", "mov", "webm", "mkv", "avi", "gif"];

pub const THUMB_H: u32 = 540;
pub const THUMB_MAX_W: u32 = 1440;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Fx {
    pub id: String,
    pub p: [f32; 4],
    pub c: [[f32; 3]; 2],
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Item {
    /// Что показываем и чем отличаем обои (для Workshop — папка проекта).
    pub path: String,
    /// Файл, который реально открываем.
    pub file: String,
    pub name: String,
    pub folder: String,
    pub video: bool,
    pub w: u32,
    pub h: u32,
    pub size: u64,
    pub mtime: i64,
    pub duration: f32,
    /// Разбор картинки: оттенок 0..360, насыщенность, яркость, цветность, «минимализм».
    pub hue: f32,
    pub sat: f32,
    pub lum: f32,
    pub rich: f32,
    pub minimal: f32,
    pub thumb: String,
    pub analyzed: bool,
    pub broken: bool,
    pub fav: bool,
    pub hidden: bool,
    pub tags: Vec<String>,
    pub applied: u32,
    pub last_applied: i64,
    pub fx: Vec<Fx>,
}

impl Item {
    pub fn thumb_path(&self) -> PathBuf {
        config::cache_dir().join("thumbs").join(&self.thumb)
    }
    pub fn portrait(&self) -> bool {
        self.h > self.w
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Playlist {
    pub name: String,
    pub items: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct ViewState {
    pub kind: String,
    pub orient: String,
    pub folder: String,
    pub sort: String,
    pub reverse: bool,
    pub favourites: bool,
    pub show_hidden: bool,
    pub color: i32,
    pub bar_hidden: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct State {
    /// Обои по выходам; ключ "*" — для всех остальных.
    pub wallpapers: HashMap<String, String>,
    pub items: Vec<Item>,
    pub playlists: Vec<Playlist>,
    pub view: ViewState,
    pub rotation_pos: usize,
    pub history: Vec<String>,
}

impl State {
    pub fn path() -> PathBuf {
        config::state_dir().join("state.json")
    }
    pub fn load() -> State {
        let mut s: State = std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        if s.view.sort.is_empty() {
            s.view.sort = "date".into();
            s.view.color = -1;
        }
        s
    }
    pub fn save(&self) {
        let _ = std::fs::create_dir_all(config::state_dir());
        if let Ok(b) = serde_json::to_vec(self) {
            config::write_atomic(&Self::path(), &b);
        }
    }
    pub fn find(&self, path: &str) -> Option<usize> {
        self.items.iter().position(|i| i.path == path || i.file == path)
    }
    pub fn all_tags(&self) -> Vec<(String, usize)> {
        let mut m: HashMap<&str, usize> = HashMap::new();
        for i in &self.items {
            for t in &i.tags {
                *m.entry(t.as_str()).or_default() += 1;
            }
        }
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
    pub fn folders(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut v = vec![];
        for i in &self.items {
            if !i.folder.is_empty() && seen.insert(i.folder.clone()) {
                v.push(i.folder.clone());
            }
        }
        v.sort_by_key(|s| s.to_lowercase());
        v
    }
}

/// Что нашёл обход диска.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: String,
    pub file: String,
    pub name: String,
    pub folder: String,
    pub video: bool,
    pub size: u64,
    pub mtime: i64,
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}
pub fn is_video_path(p: &Path) -> bool {
    VIDEO_EXT.contains(&ext_of(p).as_str())
}
pub fn is_media(p: &Path) -> Option<bool> {
    let e = ext_of(p);
    if VIDEO_EXT.contains(&e.as_str()) {
        Some(true)
    } else if IMAGE_EXT.contains(&e.as_str()) || IMAGE_EXT_FF.contains(&e.as_str()) {
        Some(false)
    } else {
        None
    }
}

fn stat(p: &Path) -> Option<(u64, i64)> {
    let m = std::fs::metadata(p).ok()?;
    let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((m.len(), mtime))
}

fn walk(dir: &Path, root: &Path, label: &str, depth: u32, seen_dirs: &mut HashSet<PathBuf>, seen_files: &mut HashSet<PathBuf>, out: &mut Vec<Found>) {
    if depth > 6 {
        return;
    }
    let Ok(real) = dir.canonicalize() else { return };
    if !seen_dirs.insert(real) {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let fname = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if fname.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, root, label, depth + 1, seen_dirs, seen_files, out);
            continue;
        }
        let Some(video) = is_media(&p) else { continue };
        let Ok(real) = p.canonicalize() else { continue };
        if !seen_files.insert(real.clone()) {
            continue;
        }
        let Some((size, mtime)) = stat(&real) else { continue };
        if size == 0 {
            continue;
        }
        // Папка в фильтре — первая вложенная папка библиотеки.
        let folder = p.parent().and_then(|d| d.strip_prefix(root).ok()).and_then(|rel| rel.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_else(|| label.to_string());
        let s = real.to_string_lossy().into_owned();
        out.push(Found { path: s.clone(), file: s, name: p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or(fname), folder, video, size, mtime });
    }
}

pub fn workshop_dir() -> PathBuf {
    config::home().join(".local/share/Steam/steamapps/workshop/content/431960")
}

/// Обойти все источники. Выполняется в рабочем потоке.
pub fn scan(cfg: &Config) -> Vec<Found> {
    let mut out = vec![];
    let mut seen_dirs = HashSet::new();
    let mut seen_files = HashSet::new();
    for f in &cfg.library.folders {
        let root = config::expand(f);
        walk(&root, &root, "", 0, &mut seen_dirs, &mut seen_files, &mut out);
    }
    if cfg.library.theme_backgrounds {
        let st = config::omarchy_state();
        let theme = std::fs::read_to_string(st.join("theme.name")).unwrap_or_default().trim().to_string();
        let dirs = [st.join("theme/backgrounds"), config::home().join(".config/omarchy/backgrounds").join(&theme)];
        for d in dirs {
            if !theme.is_empty() || d.exists() {
                let before = out.len();
                walk(&d, &d, "", 0, &mut seen_dirs, &mut seen_files, &mut out);
                for f in &mut out[before..] {
                    f.folder = crate::i18n::tr("Тема Omarchy").into();
                }
            }
        }
    }
    if cfg.library.workshop {
        if let Ok(rd) = std::fs::read_dir(workshop_dir()) {
            for e in rd.flatten() {
                let dir = e.path();
                let Ok(text) = std::fs::read_to_string(dir.join("project.json")) else { continue };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')) else { continue };
                if !v["type"].as_str().unwrap_or("").eq_ignore_ascii_case("video") {
                    continue;
                }
                let Some(file) = v["file"].as_str() else { continue };
                let file = dir.join(file);
                let Ok(real) = file.canonicalize() else { continue };
                if !seen_files.insert(real.clone()) {
                    continue;
                }
                let Some((size, mtime)) = stat(&real) else { continue };
                let name = v["title"].as_str().map(|s| s.to_string()).unwrap_or_else(|| dir.file_name().unwrap().to_string_lossy().into_owned());
                let s = real.to_string_lossy().into_owned();
                out.push(Found { path: s.clone(), file: s, name, folder: "Workshop".into(), video: true, size, mtime });
            }
        }
    }
    out
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn thumb_name(file: &str, size: u64, mtime: i64) -> String {
    format!("{:016x}.jpg", fnv(&format!("{file}|{size}|{mtime}")))
}

/// Свести результат обхода с базой. Возвращает индексы элементов, которым нужна миниатюра.
pub fn merge(state: &mut State, found: Vec<Found>) -> Vec<usize> {
    let mut old: HashMap<String, Item> = state.items.drain(..).map(|i| (i.path.clone(), i)).collect();
    let mut need = vec![];
    for f in found {
        let thumb = thumb_name(&f.file, f.size, f.mtime);
        let mut it = old.remove(&f.path).unwrap_or_default();
        let changed = it.thumb != thumb;
        it.path = f.path;
        it.file = f.file;
        it.name = f.name;
        it.folder = f.folder;
        it.video = f.video;
        it.size = f.size;
        it.mtime = f.mtime;
        if changed {
            it.thumb = thumb;
            it.analyzed = false;
            it.broken = false;
        }
        if !it.analyzed && !it.broken {
            need.push(state.items.len());
        }
        state.items.push(it);
    }
    need
}

pub struct Rgba {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

/// Декодировать картинку целиком (с поворотом по EXIF); при неудаче — через ffmpeg.
pub fn decode_image(path: &str) -> Option<Rgba> {
    use image::ImageDecoder;
    let ext = ext_of(Path::new(path));
    if !IMAGE_EXT_FF.contains(&ext.as_str()) {
        let native = (|| {
            let reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
            let mut dec = reader.into_decoder().ok()?;
            let orient = dec.orientation().ok();
            let mut img = image::DynamicImage::from_decoder(dec).ok()?;
            if let Some(o) = orient {
                img.apply_orientation(o);
            }
            let rgba = img.into_rgba8();
            Some(Rgba { w: rgba.width(), h: rgba.height(), data: rgba.into_raw() })
        })();
        if native.is_some() {
            return native;
        }
    }
    ffmpeg_frame(path, 0.0, None)
}

fn probe(path: &str) -> Option<(u32, u32, f32)> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height:stream_side_data=rotation:format=duration", "-of", "json", path])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let st = &v["streams"][0];
    let (mut w, mut h) = (st["width"].as_u64()? as u32, st["height"].as_u64()? as u32);
    let rot = st["side_data_list"][0]["rotation"].as_i64().unwrap_or(0).abs();
    if rot == 90 || rot == 270 {
        std::mem::swap(&mut w, &mut h);
    }
    let dur = v["format"]["duration"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    Some((w, h, dur))
}

/// Один кадр через ffmpeg в RGBA; `fit` — вписать в этот размер.
pub fn ffmpeg_frame(path: &str, at: f32, fit: Option<(u32, u32)>) -> Option<Rgba> {
    let (w, h, _) = probe(path)?;
    let (tw, th) = match fit {
        Some((mw, mh)) => {
            let s = (mw as f32 / w as f32).min(mh as f32 / h as f32).min(1.0);
            (((w as f32 * s).round() as u32).max(2) & !1, ((h as f32 * s).round() as u32).max(2) & !1)
        }
        None => (w, h),
    };
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if at > 0.0 {
        cmd.args(["-ss", &format!("{at:.2}")]);
    }
    cmd.args(["-i", path, "-frames:v", "1", "-vf", &format!("scale={tw}:{th}:flags=bilinear"), "-f", "rawvideo", "-pix_fmt", "rgba", "-"]);
    let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    (out.stdout.len() == (tw * th * 4) as usize).then(|| Rgba { w: tw, h: th, data: out.stdout })
}

pub fn resize(src: &Rgba, w: u32, h: u32) -> Rgba {
    use fast_image_resize as fr;
    if (src.w, src.h) == (w, h) {
        return Rgba { w, h, data: src.data.clone() };
    }
    let from = fr::images::ImageRef::new(src.w, src.h, &src.data, fr::PixelType::U8x4);
    let mut to = fr::images::Image::new(w, h, fr::PixelType::U8x4);
    let ok = from.is_ok() && {
        let mut r = fr::Resizer::new();
        let opts = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom)).use_alpha(false);
        r.resize(&from.unwrap(), &mut to, &opts).is_ok()
    };
    if !ok {
        // Запасной вариант: ближайший сосед.
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            let sy = (y as u64 * src.h as u64 / h as u64) as u32;
            for x in 0..w {
                let sx = (x as u64 * src.w as u64 / w as u64) as u32;
                let (a, b) = (((sy * src.w + sx) * 4) as usize, ((y * w + x) * 4) as usize);
                data[b..b + 4].copy_from_slice(&src.data[a..a + 4]);
            }
        }
        return Rgba { w, h, data };
    }
    Rgba { w, h, data: to.into_vec() }
}

/// Уменьшить так, чтобы картинка покрывала target, но не была больше необходимого.
pub fn fit_cover(src: Rgba, target: (u32, u32)) -> Rgba {
    let s = (target.0 as f32 / src.w as f32).max(target.1 as f32 / src.h as f32);
    if s >= 0.999 {
        return src;
    }
    let (w, h) = (((src.w as f32 * s).round() as u32).max(1), ((src.h as f32 * s).round() as u32).max(1));
    resize(&src, w, h)
}

pub struct Analysis {
    pub hue: f32,
    pub sat: f32,
    pub lum: f32,
    pub rich: f32,
    pub minimal: f32,
}

pub fn analyze(img: &Rgba) -> Analysis {
    let small = resize(img, 64, 36);
    let mut bins = [0f32; 36];
    let (mut sat_sum, mut lum_sum) = (0f32, 0f32);
    let mut lum = vec![0f32; 64 * 36];
    let (mut rg_sum, mut yb_sum, mut rg_sq, mut yb_sq) = (0f32, 0f32, 0f32, 0f32);
    for (i, px) in small.data.chunks_exact(4).enumerate() {
        let c = [px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0, 1.0];
        let (h, s, v) = crate::draw::to_hsv(c);
        bins[((h / 10.0) as usize).min(35)] += s * v;
        sat_sum += s * v.sqrt();
        let l = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        lum[i] = l;
        lum_sum += l;
        let (rg, yb) = (c[0] - c[1], 0.5 * (c[0] + c[1]) - c[2]);
        rg_sum += rg;
        yb_sum += yb;
        rg_sq += rg * rg;
        yb_sq += yb * yb;
    }
    let n = (64 * 36) as f32;
    // Главный оттенок: самый тяжёлый участок гистограммы со сглаживанием по соседям.
    let mut best = (0usize, -1f32);
    for i in 0..36 {
        let w = bins[i] * 2.0 + bins[(i + 35) % 36] + bins[(i + 1) % 36];
        if w > best.1 {
            best = (i, w);
        }
    }
    let (l, c, r) = (bins[(best.0 + 35) % 36], bins[best.0], bins[(best.0 + 1) % 36]);
    let shift = if l + c + r > 0.0 { (r - l) / (l + c + r) } else { 0.0 };
    let hue = ((best.0 as f32 + 0.5 + shift) * 10.0).rem_euclid(360.0);
    // Цветность по Хаслеру — Зюсструнку.
    let (rg_m, yb_m) = (rg_sum / n, yb_sum / n);
    let std = ((rg_sq / n - rg_m * rg_m).max(0.0) + (yb_sq / n - yb_m * yb_m).max(0.0)).sqrt();
    let rich = std + 0.3 * (rg_m * rg_m + yb_m * yb_m).sqrt();
    // Минимализм: мало перепадов яркости.
    let mut edges = 0f32;
    for y in 0..35 {
        for x in 0..63 {
            let i = y * 64 + x;
            edges += (lum[i] - lum[i + 1]).abs() + (lum[i] - lum[i + 64]).abs();
        }
    }
    Analysis { hue, sat: sat_sum / n, lum: lum_sum / n, rich, minimal: 1.0 - (edges / (35.0 * 63.0 * 2.0) * 8.0).min(1.0) }
}

pub struct ThumbResult {
    pub path: String,
    pub w: u32,
    pub h: u32,
    pub duration: f32,
    pub analysis: Option<Analysis>,
}

/// Сделать миниатюру и разобрать картинку. Выполняется в рабочем потоке.
pub fn make_thumb(file: &str, video: bool, thumb: &Path) -> Option<ThumbResult> {
    let (full, w, h, duration) = if video {
        let (w, h, dur) = probe(file)?;
        let at = if dur > 4.0 { (dur * 0.1).min(8.0) } else { 0.0 };
        let frame = ffmpeg_frame(file, at, Some((THUMB_MAX_W, THUMB_H))).or_else(|| ffmpeg_frame(file, 0.0, Some((THUMB_MAX_W, THUMB_H))))?;
        (frame, w, h, dur)
    } else {
        let img = decode_image(file)?;
        let (w, h) = (img.w, img.h);
        (img, w, h, 0.0)
    };
    let s = (THUMB_H as f32 / full.h as f32).min(THUMB_MAX_W as f32 / full.w as f32).min(1.0);
    let small = resize(&full, ((full.w as f32 * s).round() as u32).max(1), ((full.h as f32 * s).round() as u32).max(1));
    drop(full);
    let analysis = analyze(&small);
    let rgb: Vec<u8> = small.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    let mut buf = Vec::with_capacity(96 * 1024);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 88).encode(&rgb, small.w, small.h, image::ExtendedColorType::Rgb8).ok()?;
    let _ = std::fs::create_dir_all(thumb.parent()?);
    config::write_atomic(thumb, &buf);
    Some(ThumbResult { path: file.to_string(), w, h, duration, analysis: Some(analysis) })
}

pub fn load_thumb(path: &Path) -> Option<Rgba> {
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?.into_rgba8();
    Some(Rgba { w: img.width(), h: img.height(), data: img.into_raw() })
}

/// Удалить миниатюры, которые больше ничему не соответствуют.
pub fn prune_thumbs(state: &State) {
    let keep: HashSet<&str> = state.items.iter().map(|i| i.thumb.as_str()).collect();
    if let Ok(rd) = std::fs::read_dir(config::cache_dir().join("thumbs")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep.contains(name.as_str()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

pub const SWATCH_HUES: [f32; 12] = [0.0, 30.0, 60.0, 90.0, 120.0, 150.0, 180.0, 210.0, 240.0, 270.0, 300.0, 330.0];

/// Отбор и сортировка для ленты. Возвращает индексы в `state.items`.
pub fn view(state: &State, query: &str, tags: &[String], playlist: Option<&str>) -> Vec<usize> {
    let v = &state.view;
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(|s| s.to_string()).collect();
    let in_list: Option<HashSet<&str>> = playlist.and_then(|n| state.playlists.iter().find(|p| p.name == n)).map(|p| p.items.iter().map(|s| s.as_str()).collect());
    let mut out: Vec<usize> = (0..state.items.len())
        .filter(|&i| {
            let it = &state.items[i];
            if it.broken {
                return false;
            }
            if it.hidden != v.show_hidden {
                return false;
            }
            match v.kind.as_str() {
                "static" if it.video => return false,
                "video" if !it.video => return false,
                _ => {}
            }
            match v.orient.as_str() {
                "wide" if it.analyzed && it.portrait() => return false,
                "tall" if it.analyzed && !it.portrait() => return false,
                _ => {}
            }
            if !v.folder.is_empty() && it.folder != v.folder {
                return false;
            }
            if v.favourites && !it.fav {
                return false;
            }
            if v.color >= 0 {
                let target = SWATCH_HUES[(v.color as usize).min(11)];
                let d = (it.hue - target).rem_euclid(360.0);
                if it.sat < 0.10 || d.min(360.0 - d) > 18.0 {
                    return false;
                }
            }
            if let Some(l) = &in_list {
                if !l.contains(it.path.as_str()) {
                    return false;
                }
            }
            if !tags.iter().all(|t| it.tags.contains(t)) {
                return false;
            }
            if !words.is_empty() {
                let hay = format!("{} {} {}", it.name, it.folder, it.tags.join(" ")).to_lowercase();
                if !words.iter().all(|w| hay.contains(w.as_str())) {
                    return false;
                }
            }
            true
        })
        .collect();
    let it = |i: &usize| &state.items[*i];
    let f = |x: f32| (x * 10000.0) as i64;
    match v.sort.as_str() {
        "recent" => out.sort_by_key(|i| (-it(i).last_applied, -it(i).mtime)),
        "color" => out.sort_by_key(|i| (it(i).sat < 0.10, f(it(i).hue), f(it(i).lum))),
        "pop" => out.sort_by_key(|i| (-(it(i).applied as i64), -it(i).mtime)),
        "richness" => out.sort_by_key(|i| -f(it(i).rich)),
        "minimalist" => out.sort_by_key(|i| -f(it(i).minimal)),
        "res" => out.sort_by_key(|i| -((it(i).w as i64) * (it(i).h as i64))),
        "name" => out.sort_by_key(|i| it(i).name.to_lowercase()),
        _ => out.sort_by_key(|i| (-it(i).mtime, it(i).name.to_lowercase())),
    }
    if v.reverse {
        out.reverse();
    }
    out
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
