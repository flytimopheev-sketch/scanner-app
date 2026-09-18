//! scanner-worker — отдельный процесс, который общается с SANE.
//!
//! Протокол: JSON lines. Запросы приходят в stdin, ответы и события
//! уходят в stdout (по одному JSON-объекту на строку).
//!
//! Разделение GUI и воркера защищает интерфейс от зависаний и падений
//! драйверов (тот же подход, что и в NAPS2).
//!
//! Архитектура потоков:
//! * reader — читает stdin, шлёт `Msg::Request` в общий канал;
//! * scan-threads — шлют `Msg::Response(Event)` в тот же канал;
//! * writer — единственный пишет в stdout, обрабатывает запросы:
//!   Cancel/Ping/Shutdown — сразу, тяжёлые SANE-операции — в фоновых
//!   потоках (сериализуются op_lock), поэтому Cancel не блокируется
//!   даже «зависшим» драйвером.

use scanner_core::scan_job::{run_scan_job, Request, Response, ScanEvent, WorkerRequest};
use scanner_core::{create_backend, ScannerBackend};
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

enum Msg {
    Request(WorkerRequest),
    Response(Response),
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let backend: Arc<dyn ScannerBackend> = create_backend();
    let (tx, rx) = mpsc::channel::<Msg>();

    // Поток чтения stdin.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => break,
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<WorkerRequest>(&line) {
                    Ok(req) => {
                        let done = matches!(req.request, Request::Shutdown);
                        if tx.send(Msg::Request(req)).is_err() || done {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Response(Response::Err {
                            id: 0,
                            code: "bad_request".into(),
                            message: format!("invalid request: {e}"),
                        }));
                    }
                }
            }
        });
    }

    log::info!("scanner-worker started");

    // Единственный писатель в stdout — отдельный поток. Все тяжёлые
    // операции (list/capabilities/test/scan/preview) выполняются в
    // фоновых потоках и сериализуются блокировкой op_lock: главный цикл
    // не блокируется никогда, поэтому Cancel и Ping обрабатываются
    // мгновенно, даже если драйвер SANE «завис» (раньше scanimage -L
    // выполнялся прямо в этом цикле и до 90 секунд блокировал Cancel,
    // из-за чего кнопки «Сканировать»/«Прервать» казались зависшими).
    let writer = std::thread::spawn(move || {
        let op_lock = Arc::new(Mutex::new(()));
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Response(resp) => {
                    if write_response(&mut out, &resp).is_err() {
                        log::warn!("stdout закрыт — воркер завершается");
                        break;
                    }
                }
                Msg::Request(req) => {
                    if !handle_request(&backend, &tx, &op_lock, req) {
                        break;
                    }
                }
            }
        }
    });
    let _ = writer.join();
    Ok(())
}

/// Обработка одного запроса. Возвращает `false`, когда пора завершать
/// воркер (Shutdown). Ответы уходят через канал писателю, тяжёлая работа —
/// в фоновый поток с сериализацией через `op_lock`.
fn handle_request(
    backend: &Arc<dyn ScannerBackend>,
    tx: &mpsc::Sender<Msg>,
    op_lock: &Arc<Mutex<()>>,
    req: WorkerRequest,
) -> bool {
    let id = req.id;
    match req.request {
        Request::Ping => {
            send_ok(tx, id, serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }));
        }
        Request::Shutdown => {
            send_ok(tx, id, serde_json::json!({ "bye": true }));
            return false;
        }
        Request::Cancel => {
            // Отмена — вне очереди и без блокировок: убивает текущий
            // процесс scanimage, застрявшая операция разблокируется.
            backend.cancel();
            send_ok(tx, id, serde_json::json!({ "cancelled": true }));
        }
        Request::ListDevices => {
            let tx = tx.clone();
            let backend = backend.clone();
            let lock = op_lock.clone();
            std::thread::spawn(move || {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                let resp = match backend.list_devices() {
                    Ok(devices) => Response::Ok {
                        id,
                        result: serde_json::to_value(scanner_core::DeviceListResult { devices })
                            .unwrap(),
                    },
                    Err(e) => {
                        Response::Err { id, code: e.code().into(), message: e.to_string() }
                    }
                };
                let _ = tx.send(Msg::Response(resp));
            });
        }
        Request::Capabilities { device } => {
            let tx = tx.clone();
            let backend = backend.clone();
            let lock = op_lock.clone();
            std::thread::spawn(move || {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                let resp = match backend.capabilities(&device) {
                    Ok(capabilities) => Response::Ok {
                        id,
                        result: serde_json::to_value(scanner_core::CapabilitiesResult {
                            capabilities,
                        })
                        .unwrap(),
                    },
                    Err(e) => {
                        Response::Err { id, code: e.code().into(), message: e.to_string() }
                    }
                };
                let _ = tx.send(Msg::Response(resp));
            });
        }
        Request::TestIp { address, protocol } => {
            let tx = tx.clone();
            let backend = backend.clone();
            let lock = op_lock.clone();
            std::thread::spawn(move || {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                let resp = match backend.test_ip(&address, protocol.as_deref().unwrap_or("auto")) {
                    Ok(test) => Response::Ok {
                        id,
                        result: serde_json::to_value(scanner_core::TestIpResult { test }).unwrap(),
                    },
                    Err(e) => {
                        Response::Err { id, code: e.code().into(), message: e.to_string() }
                    }
                };
                let _ = tx.send(Msg::Response(resp));
            });
        }
        Request::Preview { device, dpi, out_dir } => {
            // Предпросмотр — в фоновом потоке (может занять несколько секунд
            // на медленном планшете). Единственный ответ уходит ПО завершении:
            // у запроса нет промежуточного «started», чтобы GUI сопоставил
            // ответ по id без конфликтов.
            let tx = tx.clone();
            let backend = backend.clone();
            let lock = op_lock.clone();
            std::thread::spawn(move || {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                let resp = match backend.preview(&device, dpi, std::path::Path::new(&out_dir)) {
                    Ok(path) => Response::Ok {
                        id,
                        result: serde_json::to_value(scanner_core::PreviewResult {
                            path: path.to_string_lossy().into_owned(),
                        })
                        .unwrap(),
                    },
                    Err(e) => {
                        Response::Err { id, code: e.code().into(), message: e.to_string() }
                    }
                };
                let _ = tx.send(Msg::Response(resp));
            });
        }
        Request::Scan { device, options, out_dir } => {
            // Сканирование уходит в фоновый поток; op_lock гарантирует, что
            // два задания не будут одновременно бороться за один сканер
            // (иначе процессы scanimage «съедают» друг друга и устройство
            // остаётся занятым зависшим процессом).
            let ev_out = tx.clone();
            let backend = backend.clone();
            let lock = op_lock.clone();
            std::thread::spawn(move || {
                let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
                let (ev_tx, ev_rx) = mpsc::channel::<ScanEvent>();
                let handle = run_scan_job(backend, device, options, out_dir.into(), ev_tx);
                for ev in ev_rx {
                    if ev_out.send(Msg::Response(Response::Event { event: ev })).is_err() {
                        break;
                    }
                }
                let _ = handle.join();
            });
            send_ok(tx, id, serde_json::json!({ "started": true }));
        }
    }
    true
}

fn send_ok(tx: &mpsc::Sender<Msg>, id: u64, result: serde_json::Value) {
    let _ = tx.send(Msg::Response(Response::Ok { id, result }));
}

fn write_response<W: Write>(out: &mut W, resp: &Response) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, resp)?;
    out.write_all(b"\n")?;
    out.flush()
}
