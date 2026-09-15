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
//! * main — единственный пишет в stdout, поэтому блокировки не нужны.

use scanner_core::scan_job::{run_scan_job, Request, Response, ScanEvent, WorkerRequest};
use scanner_core::{create_backend, ScannerBackend};
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::sync::Arc;

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

    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Response(resp) => {
                write_response(&mut out, &resp)?;
            }
            Msg::Request(req) => {
                let id = req.id;
                match req.request {
                    Request::Ping => {
                        write_response(
                            &mut out,
                            &Response::Ok {
                                id,
                                result: serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }),
                            },
                        )?;
                    }
                    Request::Shutdown => {
                        write_response(
                            &mut out,
                            &Response::Ok {
                                id,
                                result: serde_json::json!({ "bye": true }),
                            },
                        )?;
                        break;
                    }
                    Request::Cancel => {
                        backend.cancel();
                        write_response(
                            &mut out,
                            &Response::Ok {
                                id,
                                result: serde_json::json!({ "cancelled": true }),
                            },
                        )?;
                    }
                    Request::ListDevices => {
                        let resp = match backend.list_devices() {
                            Ok(devices) => Response::Ok {
                                id,
                                result: serde_json::to_value(
                                    scanner_core::DeviceListResult { devices },
                                )
                                .unwrap(),
                            },
                            Err(e) => {
                                Response::Err { id, code: e.code().into(), message: e.to_string() }
                            }
                        };
                        write_response(&mut out, &resp)?;
                    }
                    Request::Capabilities { device } => {
                        let resp = match backend.capabilities(&device) {
                            Ok(capabilities) => Response::Ok {
                                id,
                                result: serde_json::to_value(
                                    scanner_core::CapabilitiesResult { capabilities },
                                )
                                .unwrap(),
                            },
                            Err(e) => {
                                Response::Err { id, code: e.code().into(), message: e.to_string() }
                            }
                        };
                        write_response(&mut out, &resp)?;
                    }
                    Request::TestIp { address, protocol } => {
                        let resp = match backend.test_ip(&address, protocol.as_deref().unwrap_or("auto")) {
                            Ok(test) => Response::Ok {
                                id,
                                result: serde_json::to_value(scanner_core::TestIpResult { test })
                                    .unwrap(),
                            },
                            Err(e) => {
                                Response::Err { id, code: e.code().into(), message: e.to_string() }
                            }
                        };
                        write_response(&mut out, &resp)?;
                    }
                    Request::Preview { device, dpi, out_dir } => {
                        // Предпросмотр — в фоновом потоке (может занять
                        // несколько секунд на медленном планшете), чтобы
                        // главный цикл продолжал принимать Cancel.
                        // Единственный ответ уходит ПО завершении: у запроса
                        // нет промежуточного «started», чтобы GUI сопоставил
                        // ответ по id без конфликтов.
                        let tx = tx.clone();
                        let backend = backend.clone();
                        std::thread::spawn(move || {
                            let result = backend.preview(
                                &device,
                                dpi,
                                std::path::Path::new(&out_dir),
                            );
                            let resp = match result {
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
                        // Сканирование уходит в фоновый поток: главный цикл
                        // продолжает принимать Cancel и другие запросы.
                        let tx = tx.clone();
                        let backend = backend.clone();
                        std::thread::spawn(move || {
                            let (ev_tx, ev_rx) = mpsc::channel::<ScanEvent>();
                            let handle =
                                run_scan_job(backend, device, options, out_dir.into(), ev_tx);
                            for ev in ev_rx {
                                if tx.send(Msg::Response(Response::Event { event: ev })).is_err() {
                                    break;
                                }
                            }
                            let _ = handle.join();
                        });
                        write_response(
                            &mut out,
                            &Response::Ok {
                                id,
                                result: serde_json::json!({ "started": true }),
                            },
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn write_response<W: Write>(out: &mut W, resp: &Response) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, resp)?;
    out.write_all(b"\n")?;
    out.flush()
}
