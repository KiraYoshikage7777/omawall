//! omawall — обои и меню выбора обоев для Omarchy.
mod app;
mod config;
mod draw;
mod fx;
mod gfx;
mod i18n;
mod icons;
mod jobs;
mod library;
mod mpv;
mod panels;
mod picker;
mod steam;
mod ui;
mod video;
mod wallpaper;
mod we;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const HELP_EN: &str = "omawall — wallpaper engine and wallpaper picker for Omarchy

  omawall daemon                 run the daemon (normally started by systemd)
  omawall toggle | show | hide   open or close the picker
  omawall apply <file|name> [-o MONITOR] [--instant]
  omawall next | prev | random   change wallpaper from the rotation list
  omawall pause | resume         pause or resume video
  omawall current | list | status
  omawall rescan                 re-read wallpaper folders
  omawall reload                 re-read config.toml
  omawall quit                   stop the daemon";

const HELP: &str = "omawall — обои и меню выбора обоев для Omarchy

  omawall daemon                 запустить демон (обычно это делает systemd)
  omawall toggle | show | hide   открыть или закрыть меню
  omawall apply <файл|имя> [-o МОНИТОР] [--instant]
  omawall next | prev | random   сменить обои из списка автосмены
  omawall pause | resume         остановить или продолжить видео
  omawall current | list | status
  omawall rescan                 перечитать папки с обоями
  omawall reload                 перечитать config.toml
  omawall quit                   остановить демон";

fn send(args: &[String]) -> Option<String> {
    let mut s = UnixStream::connect(config::socket_path()).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all((args.join("\x1f") + "\n").as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    Some(line.trim_end().to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("daemon") => {
            if let Err(e) = app::run() {
                eprintln!("omawall: {e}");
                std::process::exit(1);
            }
        }
        Some("steam") => std::process::exit(steam::main(&args[1..])),
        None | Some("help" | "--help" | "-h") => println!("{}", if i18n::en() { HELP_EN } else { HELP }),
        Some("--version" | "version") => println!("omawall {}", env!("CARGO_PKG_VERSION")),
        Some(cmd) => {
            let mut reply = send(&args);
            if reply.is_none() && cmd != "quit" && std::env::var_os("OMAWALL_SOCK").is_none() {
                // Демон не запущен — поднимаем и повторяем.
                let started = std::process::Command::new("systemctl").args(["--user", "start", "omawall.service"]).status().is_ok_and(|s| s.success());
                if !started {
                    if let Ok(exe) = std::env::current_exe() {
                        let _ = std::process::Command::new("setsid").arg("-f").arg(exe).arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
                    }
                }
                for _ in 0..60 {
                    std::thread::sleep(Duration::from_millis(50));
                    reply = send(&args);
                    if reply.is_some() {
                        break;
                    }
                }
            }
            match reply {
                Some(r) => {
                    println!("{r}");
                    if r.starts_with("error") {
                        std::process::exit(1);
                    }
                }
                None => {
                    eprintln!("omawall: демон не отвечает");
                    std::process::exit(3);
                }
            }
        }
    }
}
