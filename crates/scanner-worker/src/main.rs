//! scanner-worker — отдельный процесс, который общается с SANE.
//!
//! Протокол: JSON lines. Запросы приходят в stdin, ответы и события
//! уходят в stdout (по одному JSON-объекту на строку).
//!
//! Разделение GUI и воркера защищает интерфейс от зависаний и падений
//! драйверов (тот же подход, что и в NAPS2).
//!
//! Архитектура потоков:
//! * главный поток — читает stdin и обрабатывает запросы: Cancel/Ping/
//!   Shutdown мгновенно (без обращений к SANE), тяжёлые операции (list/
//!   capabilities/test/scan/preview) — в фоновых потоках, сериализуются
//!   op_lock, поэтому Cancel не блокируется даже «зависшим» драйвером;
//! * writer — единственный пишет в stdout; при завершении воркер ждёт,
//!   пока очередь ответов опустеет, поэтому ответы не теряются.

use scanner_core::scan_job::{run_scan_job, Request, Response, ScanEvent, WorkerRequest};
use scanner_core::{create_backend, ScannerBackend};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let backend: Arc<dyn ScannerBackend> = create_backend();
    let (tx, rx) = mpsc::channel::<Response>();
    // Флаг «воркер завершается»: writer, обнаружив пустую очередь, выходит.
    let done = Arc::new(AtomicBool::new(false));

    // Единственный писатель в stdout. Все ответы/события идут через канал,
    // поэтому строки JSON не перемешиваются.
    let writer = {
        let done = done.clone();
        std::thread::spawn(move || {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(resp) => {
                        if write_response(&mut out, &resp).is_err() {
                            log::warn!("stdout закрыт — воркер завершается");
                            done.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                    // Очередь пуста: если запрошено завершение — выходим.
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if done.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
    };

    log::info!("scanner-worker started");

    // Сериализация тяжёлых SANE-операций: два задания не должны
    // одновременно бороться за одно устройство.
    let op_lock = Arc::new(Mutex::new(()));

    // Главный цикл: читаем stdin и обрабатываем запросы. Тяжёлые операции
    // уходят в фоновые потоки, поэтому этот цикл всегда готов принять
    // Cancel/Ping — кнопки «Сканировать»/«Прервать» не «подвисают».
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let req = match serde_json::from_str::<WorkerRequest>(&line) {
            Ok(req) => req,
            Err(e) => {
                let _ = tx.send(Response::Err {
                    id: 0,
                    code: "bad_request".into(),
                    message: format!("invalid request: {e}"),
                });
                continue;
            }
        };
        if !handle_request(&backend, &tx, &op_lock, req) {
            break;
        }
        // stdout закрылся (GUI упал) — завершаемся, не дожидаясь новых команд.
        if done.load(Ordering::SeqCst) {
            break;
        }
    }

    // Даём писателю выгрузить очередь ответов и только затем выходим:
    // иначе ответы на последние запросы (ping/shutdown) терялись бы.
    // Активную операцию SANE прерываем: иначе после закрытия stdin (GUI
    // упал) оставался бы зависший scanimage, держащий сканер занятым.
    backend.cancel();
    done.store(true, Ordering::SeqCst);
    let _ = writer.join();
    Ok(())
}

/// Обработка одного запроса. Возвращает `false`, когда пора завершать
/// воркер (Shutdown). Ответы уходят через канал писателю, тяжёлая работа —
/// в фоновый поток с сериализацией через `op_lock`.
fn handle_request(
    backend: &Arc<dyn ScannerBackend>,
    tx: &mpsc::Sender<Response>,
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
                let _ = tx.send(resp);
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
                let _ = tx.send(resp);
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
                let _ = tx.send(resp);
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
                let _ = tx.send(resp);
            });
        }
        Request::Scan { device, options, out_dir } => {
            // Ответ «started» уходит ДО старта потока, поэтому GUI всегда
            // получает его раньше событий сканирования.
            send_ok(tx, id, serde_json::json!({ "started": true }));
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
                    if ev_out.send(Response::Event { event: ev }).is_err() {
                        break;
                    }
                }
                let _ = handle.join();
            });
        }
    }
    true
}

fn send_ok(tx: &mpsc::Sender<Response>, id: u64, result: serde_json::Value) {
    let _ = tx.send(Response::Ok { id, result });
}

fn write_response<W: Write>(out: &mut W, resp: &Response) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, resp)?;
    out.write_all(b"\n")?;
    out.flush()
}
