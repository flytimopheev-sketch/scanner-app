//! Трейт бэкенда сканирования и фабрика.

pub use crate::errors::{Result, ScannerError};
pub use crate::options::{ScanOptions, ScanMode, ScanSource};
pub use crate::device::ScannerDevice;

use std::path::{Path, PathBuf};

/// Возможности конкретного устройства (что поддерживает драйвер).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DeviceCapabilities {
    pub resolutions: Vec<u32>,
    pub modes: Vec<ScanMode>,
    pub sources: Vec<ScanSource>,
}

/// Прогресс в рамках одного задания.
#[derive(Debug, Clone)]
pub enum ScanProgress {
    PageStarted(u32),
    PageProgress(u32, f32),
    PageDone(u32, PathBuf),
}

/// Результат проверки устройства по IP.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestResult {
    pub ok: bool,
    pub device_name: Option<String>,
    pub message: String,
}

/// Универсальный интерфейс к сканеру.
///
/// Реализации:
/// * [`crate::scanimage::ScanimageBackend`] — через CLI `scanimage` (MVP, надёжен);
/// * [`crate::sane_ffi::SaneFfiBackend`] — прямая работа с libsane (feature `sane-ffi`);
/// * [`crate::mock::MockBackend`] — без железа, для разработки интерфейса.
pub trait ScannerBackend: Send + Sync {
    /// Поиск устройств (USB + сеть).
    fn list_devices(&self) -> Result<Vec<ScannerDevice>>;

    /// Возможности устройства.
    fn capabilities(&self, device: &str) -> Result<DeviceCapabilities>;

    /// Сканирование. Блокирующий вызов — запускайте в отдельном потоке.
    /// Возвращает пути к файлам страниц (TIFF/PNG).
    fn scan(
        &self,
        device: &str,
        options: &ScanOptions,
        out_dir: &Path,
        progress: &dyn Fn(ScanProgress),
    ) -> Result<Vec<PathBuf>>;

    /// Отмена текущей операции (scan/list).
    fn cancel(&self);

    /// Проверка доступности сканера по IP.
    fn test_ip(&self, address: &str, protocol: &str) -> Result<TestResult>;

    /// Предпросмотр: одиночная страница с планшета в низком разрешении.
    /// Результат показывается в диалоге и НЕ добавляется в документ.
    fn preview(&self, device: &str, dpi: u32, out_dir: &Path) -> Result<PathBuf> {
        let _ = (device, dpi, out_dir);
        Err(ScannerError::Unsupported(
            "предпросмотр не поддерживается этим бэкендом".into(),
        ))
    }
}

/// Фабрика бэкенда. Выбор через переменную окружения `SCANNER_BACKEND`:
/// `scanimage` (по умолчанию) | `sane` (feature sane-ffi) | `mock`.
pub fn create_backend() -> std::sync::Arc<dyn ScannerBackend> {
    let choice = std::env::var("SCANNER_BACKEND").unwrap_or_default();
    match choice.as_str() {
        "mock" => std::sync::Arc::new(crate::mock::MockBackend::new()),
        #[cfg(feature = "sane-ffi")]
        "sane" => std::sync::Arc::new(crate::sane_ffi::SaneFfiBackend::new()),
        other => {
            if !other.is_empty() && other != "scanimage" {
                log::warn!("unknown SCANNER_BACKEND={other:?}, falling back to scanimage");
            }
            std::sync::Arc::new(crate::scanimage::ScanimageBackend::new())
        }
    }
}
