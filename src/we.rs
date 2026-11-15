//! Обои Wallpaper Engine из Мастерской Steam: видео играем как есть, сцены один раз
//! записываем в зацикленный ролик (скрипт assets/we-adapt.py).
use crate::config::{self, Config};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const SCRIPT: &str = include_str!("../assets/we-adapt.py");

fn has(cmd: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
}

/// Есть ли всё нужное для записи сцен.
pub fn converter_ready() -> bool {
    static READY: OnceLock<bool> = OnceLock::new();
    *READY.get_or_init(|| ["python3", "sway", "wf-recorder", "ffmpeg", "skwd-wall-vk"].iter().all(|c| has(c)))
}

pub fn converter_missing() -> Vec<&'static str> {
    ["sway", "wf-recorder", "ffmpeg", "skwd-wall-vk"].into_iter().filter(|c| !has(c)).collect()
}

pub struct Project {
    pub kind: String,
    pub title: String,
    pub file: Option<PathBuf>,
}

pub fn project(dir: &Path) -> Option<Project> {
    let text = std::fs::read_to_string(dir.join("project.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let id = dir.file_name()?.to_string_lossy().into_owned();
    Some(Project {
        kind: v["type"].as_str().unwrap_or("").to_lowercase(),
        title: v["title"].as_str().map(|s| s.to_string()).filter(|s| !s.trim().is_empty()).unwrap_or(id),
        file: v["file"].as_str().map(|f| dir.join(f)),
    })
}

pub fn videos_dir(cfg: &Config) -> PathBuf {
    config::expand(cfg.library.folders.first().map(|s| s.as_str()).unwrap_or("~/Pictures/Wallpapers")).join("Workshop")
}

/// Уже записанный ролик сцены с этим номером.
pub fn recorded(cfg: &Config, id: &str) -> Option<PathBuf> {
    let suffix = format!("[{id}].mp4");
    std::fs::read_dir(videos_dir(cfg)).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&suffix)))
}

pub fn record_target(cfg: &Config, dir: &Path, title: &str) -> PathBuf {
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let clean: String = title.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let clean: String = clean.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    videos_dir(cfg).join(format!("{} [{id}].mp4", if clean.is_empty() { id.clone() } else { clean }))
}

/// Сцены из Мастерской, для которых ролика ещё нет: (папка сцены, куда писать).
pub fn unconverted(cfg: &Config) -> Vec<(PathBuf, PathBuf)> {
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(crate::library::workshop_dir()) else { return out };
    for e in rd.flatten() {
        let dir = e.path();
        let Some(p) = project(&dir) else { continue };
        let id = e.file_name().to_string_lossy().into_owned();
        if p.kind == "scene" && dir.join("scene.pkg").is_file() && recorded(cfg, &id).is_none() {
            out.push((dir.clone(), record_target(cfg, &dir, &p.title)));
        }
    }
    out
}

/// Записать сцену в ролик. Долго (около минуты), вызывать из рабочего потока.
pub fn convert(scene: &Path, target: &Path) -> bool {
    if !converter_ready() {
        return false;
    }
    let script = config::runtime_dir().join("omawall-we-adapt.py");
    if std::fs::write(&script, SCRIPT).is_err() {
        return false;
    }
    Command::new("nice")
        .args(["-n", "10", "python3"])
        .arg(&script)
        .arg(scene)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        && target.is_file()
}

/// Что показывать для скачанных обоев: файл видео, записанный ролик сцены или причина отказа.
pub fn playable(cfg: &Config, dir: &Path) -> Result<PathBuf, String> {
    let p = project(dir).ok_or(crate::i18n::tr("в загруженной папке нет описания обоев"))?;
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match p.kind.as_str() {
        "video" => p.file.filter(|f| f.is_file()).ok_or_else(|| crate::i18n::tr("в обоях нет файла видео").to_string()),
        "scene" => {
            if let Some(done) = recorded(cfg, &id) {
                return Ok(done);
            }
            if !converter_ready() {
                return Err(crate::trf!("сцену нечем записать в видео, не хватает: {}", "nothing to record the scene with, missing: {}", converter_missing().join(", ")));
            }
            let target = record_target(cfg, dir, &p.title);
            if convert(dir, &target) {
                Ok(target)
            } else {
                Err(crate::i18n::tr("не удалось записать сцену в видео").into())
            }
        }
        other => Err(crate::trf!("обои типа «{other}» показать нельзя — поддерживаются видео и сцены", "“{other}” wallpapers cannot be shown; videos and scenes are supported")),
    }
}
