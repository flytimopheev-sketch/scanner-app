//! События задания и протокол worker <-> GUI (JSON lines через stdin/stdout).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::errors::ScannerError;

/// События, приходящие от воркера во время сканирования.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScanEvent {
    Started,
    PageStarted { page: u32 },
    PageProgress { page: u32, percent: f32 },
    PageDone { page: u32, path: String },
    Finished { pages: u32, files: Vec<String> },
    Failed { code: String, error: String },
    Cancelled,
}

/// Запросы к воркеру.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    ListDevices,
    Capabilities { device: String },
    Scan { device: String, options: crate::options::ScanOptions, out_dir: String },
    Cancel,
    TestIp { address: String, protocol: Option<String> },
    /// Предпросмотр планшета в низком разрешении (страница НЕ попадает в документ).
    Preview { device: String, dpi: u32, out_dir: String },
    Shutdown,
}

/// Запрос к воркеру с идентификатором для сопоставления ответов.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerRequest {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

/// Ответы и события воркера.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    /// Ответ на запрос с совпадающим id.
    Ok { id: u64, result: serde_json::Value },
    Err { id: u64, code: String, message: String },
    /// Незапрашиваемое событие прогресса.
    Event { event: ScanEvent },
}

/// Результаты операций (сериализуются в `Response::Ok.result`).
#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceListResult {
    pub devices: Vec<crate::device::ScannerDevice>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CapabilitiesResult {
    pub capabilities: crate::backend::DeviceCapabilities,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScanResult {
    pub pages: u32,
    pub files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TestIpResult {
    pub test: crate::backend::TestResult,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PreviewResult {
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PingResult {
    pub version: String,
}

/// Запуск задания в отдельном потоке с отправкой событий в канал.
pub fn run_scan_job(
    backend: std::sync::Arc<dyn crate::backend::ScannerBackend>,
    device: String,
    options: crate::options::ScanOptions,
    out_dir: PathBuf,
    tx: std::sync::mpsc::Sender<ScanEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _ = tx.send(ScanEvent::Started);
        let progress = |p: crate::backend::ScanProgress| {
            let ev = match p {
                crate::backend::ScanProgress::PageStarted(n) => ScanEvent::PageStarted { page: n },
                crate::backend::ScanProgress::PageProgress(n, pct) => {
                    ScanEvent::PageProgress { page: n, percent: pct }
                }
                crate::backend::ScanProgress::PageDone(n, path) => ScanEvent::PageDone {
                    page: n,
                    path: path.to_string_lossy().into_owned(),
                },
            };
            let _ = tx.send(ev);
        };

        match backend.scan(&device, &options, &out_dir, &progress) {
            Ok(files) => {
                let n = files.len() as u32;
                let paths = files
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                let _ = tx.send(ScanEvent::Finished { pages: n, files: paths });
            }
            Err(ScannerError::Cancelled) => {
                let _ = tx.send(ScanEvent::Cancelled);
            }
            Err(e) => {
                let _ = tx.send(ScanEvent::Failed {
                    code: e.code().to_string(),
                    error: e.to_string(),
                });
            }
        }
    })
}
