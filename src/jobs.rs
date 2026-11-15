//! Фоновые задачи: декодирование, миниатюры, сеть. Результаты возвращаются в главный цикл.
use crate::library::{Found, Rgba, ThumbResult};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

pub enum Done {
    /// Картинка для обоев: (номер запроса, путь, пиксели).
    Wall(u64, String, Option<Rgba>),
    /// Миниатюра для ленты: (путь обоев, пиксели).
    Thumb(String, Option<Rgba>),
    /// Миниатюра создана и картинка разобрана.
    Made(String, Option<ThumbResult>),
    Scanned(Vec<Found>),
    /// Ответ сети: (метка запроса, тело).
    Net(String, Option<Vec<u8>>),
    /// Картинка из сети для сетки загрузок: (адрес, пиксели).
    Remote(String, Option<Rgba>),
    /// Файл скачан: (адрес, путь на диске, применить ли).
    Saved(String, Option<String>, bool),
    /// Ход загрузки: (метка, получено, всего; всего = 0 — идёт запись сцены).
    Progress(String, u64, u64),
    /// Короткое сообщение для пользователя.
    Note(String),
    /// Сцена Wallpaper Engine записана в ролик (или не получилось): (папка сцены, успех).
    Converted(String, bool),
    /// Видео из MPV пора разбудить (новый кадр).
    Frame,
}

type Job = Box<dyn FnOnce() -> Option<Done> + Send>;

struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    wake: Condvar,
}

#[derive(Clone)]
pub struct Pool {
    q: Arc<Queue>,
    pub tx: Sender<Done>,
}

impl Pool {
    pub fn new(tx: Sender<Done>) -> Pool {
        let q = Arc::new(Queue { jobs: Mutex::new(VecDeque::new()), wake: Condvar::new() });
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).saturating_sub(1).clamp(2, 6);
        for i in 0..n {
            let (q, tx) = (q.clone(), tx.clone());
            let _ = std::thread::Builder::new().name(format!("omawall-work-{i}")).stack_size(4 << 20).spawn(move || {
                // Фоновая работа не должна мешать интерактивным программам.
                unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 5) };
                loop {
                    let job = {
                        let mut g = q.jobs.lock().unwrap();
                        loop {
                            if let Some(j) = g.pop_front() {
                                break j;
                            }
                            g = q.wake.wait(g).unwrap();
                        }
                    };
                    if let Ok(Some(done)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)) {
                        let _ = tx.send(done);
                    }
                }
            });
        }
        Pool { q, tx }
    }

    /// `urgent` — в начало очереди (то, чего ждёт экран).
    pub fn run(&self, urgent: bool, job: impl FnOnce() -> Option<Done> + Send + 'static) {
        let mut g = self.q.jobs.lock().unwrap();
        if urgent {
            g.push_front(Box::new(job));
        } else {
            g.push_back(Box::new(job));
        }
        drop(g);
        self.q.wake.notify_one();
    }
}

/// Запрос к сети через curl (он есть в любой установке Omarchy).
pub fn http_get(url: &str, timeout: u32) -> Option<Vec<u8>> {
    let out = std::process::Command::new("curl")
        .args(["-sSL", "--fail", "--max-time", &timeout.to_string(), "-A", "omawall/1.0", "--proto", "=https", "--max-filesize", "104857600", url])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}
