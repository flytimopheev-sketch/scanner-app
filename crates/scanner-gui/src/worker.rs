//! Клиент воркера: отдельный процесс scanner-worker, JSON-lines поверх
//! stdin/stdout. Ответы доставляются в главный цикл GTK через async-channel.

use std::cell::Cell;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;

use scanner_core::scan_job::{Request, Response, WorkerRequest};
use scanner_core::ScanEvent;
use gtk4::glib;

/// Сообщение от воркера в UI.
#[derive(Debug, Clone)]
pub enum WorkerIn {
    Ok { id: u64, result: serde_json::Value },
    Err { id: u64, code: String, message: String },
    Event(ScanEvent),
    /// Воркер завершился / не отвечает.
    Dead(String),
}

pub struct WorkerClient {
    req_tx: async_channel::Sender<(u64, Request)>,
    counter: Cell<u64>,
}

impl WorkerClient {
    /// Запустить воркера и мост в главный цикл GTK.
    pub fn spawn(on_msg: impl Fn(WorkerIn) + 'static) -> anyhow::Result<Rc<Self>> {
        let (req_tx, req_rx) = async_channel::unbounded::<(u64, Request)>();
        let (in_tx, in_rx) = async_channel::unbounded::<WorkerIn>();

        // Путь к бинарнику воркера: рядом с GUI или в PATH.
        let bin = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("scanner-worker")))
            .filter(|p| p.exists())
            .unwrap_or_else(|| PathBuf::from("scanner-worker"));

        let mut child: Child = Command::new(&bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env("RUST_LOG", "warn")
            .spawn()
            .map_err(|e| anyhow::anyhow!("не удалось запустить {bin:?}: {e}"))?;

        // Поток записи запросов.
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                while let Ok((id, req)) = req_rx.recv_blocking() {
                    let line = match serde_json::to_string(&WorkerRequest { id, request: req }) {
                        Ok(mut s) => {
                            s.push('\n');
                            s
                        }
                        Err(_) => break,
                    };
                    if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                        break;
                    }
                }
            });
        }

        // Поток чтения ответов/событий.
        if let Some(stdout) = child.stdout.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                    let Ok(resp) = serde_json::from_str::<Response>(&line) else { continue };
                    let msg = match resp {
                        Response::Ok { id, result } => WorkerIn::Ok { id, result },
                        Response::Err { id, code, message } => WorkerIn::Err { id, code, message },
                        Response::Event { event } => WorkerIn::Event(event),
                    };
                    if in_tx.send_blocking(msg).is_err() {
                        break;
                    }
                }
                let _ = child.wait();
                let _ = in_tx.send_blocking(WorkerIn::Dead("worker exited".into()));
            });
        } else {
            let _ = child.wait();
        }

        // Мост в главный цикл GTK.
        glib::spawn_future_local(async move {
            while let Ok(msg) = in_rx.recv().await {
                on_msg(msg);
            }
        });

        Ok(Rc::new(WorkerClient { req_tx, counter: Cell::new(0) }))
    }

    /// Отправить запрос, вернув id для сопоставления ответа.
    pub fn request(&self, req: Request) -> u64 {
        let id = self.counter.get() + 1;
        self.counter.set(id);
        let _ = self.req_tx.send_blocking((id, req));
        id
    }
}
