//! Клиент воркера: отдельный процесс scanner-worker, JSON-lines поверх
//! stdin/stdout. Ответы доставляются в главный цикл GTK через async-channel.
//!
//! Воркер может погибнуть (падение libsane и т.п.) — тогда `request()`
//! прозрачно перезапускает процесс и повторяет запрос: интерфейс не
//! остаётся «мёртвым» до перезапуска приложения.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

struct WorkerConn {
    req_tx: async_channel::Sender<(u64, Request)>,
    /// Стал false, когда поток чтения обнаружил завершение воркера.
    alive: Arc<AtomicBool>,
}

pub struct WorkerClient {
    conn: RefCell<Option<WorkerConn>>,
    on_msg: Rc<dyn Fn(WorkerIn)>,
    counter: Cell<u64>,
}

impl WorkerClient {
    /// Запустить воркера и мост в главный цикл GTK.
    pub fn spawn(on_msg: impl Fn(WorkerIn) + 'static) -> anyhow::Result<Rc<Self>> {
        let client = Rc::new(Self {
            conn: RefCell::new(None),
            on_msg: Rc::new(on_msg),
            counter: Cell::new(0),
        });
        client.start()?;
        Ok(client)
    }

    /// Запустить (или перезапустить) процесс воркера и его потоки.
    fn start(&self) -> anyhow::Result<()> {
        let (req_tx, req_rx) = async_channel::unbounded::<(u64, Request)>();
        let (in_tx, in_rx) = async_channel::unbounded::<WorkerIn>();
        let alive = Arc::new(AtomicBool::new(true));

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
            let in_tx = in_tx.clone();
            let alive = alive.clone();
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
                alive.store(false, Ordering::SeqCst);
                let _ = in_tx.send_blocking(WorkerIn::Dead("worker exited".into()));
            });
        } else {
            alive.store(false, Ordering::SeqCst);
            let _ = child.wait();
        }

        // Мост в главный цикл GTK.
        let on_msg = self.on_msg.clone();
        glib::spawn_future_local(async move {
            while let Ok(msg) = in_rx.recv().await {
                on_msg(msg);
            }
        });

        *self.conn.borrow_mut() = Some(WorkerConn { req_tx, alive });
        Ok(())
    }

    fn next_id(&self) -> u64 {
        let id = self.counter.get() + 1;
        self.counter.set(id);
        id
    }

    /// Воркер жив?
    pub fn is_alive(&self) -> bool {
        self.conn
            .borrow()
            .as_ref()
            .map(|c| c.alive.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    /// Отправить запрос, вернув id для сопоставления ответа. Если воркер
    /// завершился — прозрачно перезапускаем его и повторяем запрос.
    pub fn request(&self, req: Request) -> u64 {
        let id = self.next_id();

        // Мёртвый (или ещё не запущенный) воркер перезапускаем ДО отправки:
        // так `req` перемещается ровно один раз.
        let need_restart = {
            let conn = self.conn.borrow();
            match conn.as_ref() {
                Some(c) => !c.alive.load(Ordering::SeqCst),
                None => true,
            }
        };
        if need_restart && self.start().is_err() {
            (self.on_msg)(WorkerIn::Err {
                id: 0,
                code: "spawn".into(),
                message: "не удалось перезапустить scanner-worker".into(),
            });
            return id;
        }

        let mut send_failed = false;
        {
            let conn = self.conn.borrow();
            match conn.as_ref() {
                Some(c) => {
                    if c.req_tx.send_blocking((id, req)).is_err() {
                        // Канал закрыт: воркер умер между проверкой и отправкой.
                        c.alive.store(false, Ordering::SeqCst);
                        send_failed = true;
                    }
                }
                None => send_failed = true,
            }
        }
        if send_failed {
            // Сообщаем UI: ответа по этому id не будет, воркер перезапустится
            // при следующем действии пользователя.
            (self.on_msg)(WorkerIn::Dead("канал воркера закрыт".into()));
        }
        id
    }
}
